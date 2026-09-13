//! Pulse API server entrypoint.
//!
//! Startup: load config → connect Postgres → apply migrations → seed the
//! first admin (if configured) → serve the router from `pulse_backend::app`.

use pulse_backend::{
    app::{self, AppState},
    auth::seed,
    config::{AuthConfig, Config},
    db,
};
use std::net::SocketAddr;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pulse_backend::init_tracing();
    let config = Config::from_env();
    let auth_config = AuthConfig::from_env()?;

    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    info!("database migrations up to date");

    if let Some(admin_seed) = &auth_config.admin_seed {
        let outcome = seed::seed_admin(&pool, admin_seed).await?;
        info!(?outcome, "admin seed");
    }

    let app = app::router(AppState::new(pool, &auth_config));

    let listener = tokio::net::TcpListener::bind(&config.api_bind_addr).await?;
    info!(addr = %config.api_bind_addr, env = %config.app_env, "pulse-api listening");
    // Connect info = TCP peer address, used by rate limiting when no
    // CLIENT_IP_HEADER is configured.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
