//! Pulse API server entrypoint.
//!
//! Skeleton stage: just boots Axum with a `/health` endpoint so Docker
//! Compose / CI have something real to build and run against. Real routes
//! (`/api/v1/...` per `docs/pulse-api-spec.md`) get added starting with the
//! auth implementation step.

use axum::{Router, routing::get};
use pulse_backend::config::Config;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pulse_backend::init_tracing();
    let config = Config::from_env();

    let app = Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive()); // TODO: restrict to FRONTEND_URL before prod (see pulse-security.md #6)

    let listener = tokio::net::TcpListener::bind(&config.api_bind_addr).await?;
    info!(addr = %config.api_bind_addr, env = %config.app_env, "pulse-api listening");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}
