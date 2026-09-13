//! Pulse API server entrypoint.
//!
//! Connects to Postgres and applies pending migrations on startup, then
//! serves `/health`. Real routes (`/api/v1/...` per `docs/pulse-api-spec.md`)
//! get added starting with the auth implementation step.

use axum::{Router, extract::State, http::StatusCode, routing::get};
use pulse_backend::{config::Config, db};
use sqlx::PgPool;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pulse_backend::init_tracing();
    let config = Config::from_env();

    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    info!("database migrations up to date");

    let app = Router::new()
        .route("/health", get(health))
        .with_state(pool)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive()); // TODO: restrict to FRONTEND_URL before prod (see pulse-security.md #6)

    let listener = tokio::net::TcpListener::bind(&config.api_bind_addr).await?;
    info!(addr = %config.api_bind_addr, env = %config.app_env, "pulse-api listening");
    axum::serve(listener, app).await?;

    Ok(())
}

/// Liveness + DB reachability. Returns 503 (no internal details, per
/// pulse-security.md #6) if Postgres can't be reached.
async fn health(State(pool): State<PgPool>) -> (StatusCode, &'static str) {
    match sqlx::query("SELECT 1").execute(&pool).await {
        Ok(_) => (StatusCode::OK, "ok"),
        Err(e) => {
            error!(error = %e, "health check: database unreachable");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}
