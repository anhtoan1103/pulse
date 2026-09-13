//! Router + shared state, built here (not in `bin/api.rs`) so integration
//! tests can drive the exact same app in-process.

use crate::{
    auth::{self, token::JwtKeys},
    config::AuthConfig,
    error::ApiError,
    rate_limit::RateLimiter,
};
use axum::{Router, extract::State, http::StatusCode, routing::get};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::error;

/// api-spec #8: 5 requests/minute/IP on login and register (each counted separately).
const AUTH_RATE_LIMIT: u32 = 5;
const AUTH_RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt: Arc<JwtKeys>,
    pub auth_rate_limiter: Arc<RateLimiter>,
    pub client_ip_header: Option<Arc<str>>,
}

impl AppState {
    pub fn new(pool: PgPool, auth: &AuthConfig) -> Self {
        Self {
            pool,
            jwt: Arc::new(JwtKeys::new(
                auth.jwt_secret.as_bytes(),
                chrono::Duration::hours(auth.jwt_expiry_hours),
            )),
            auth_rate_limiter: Arc::new(RateLimiter::new(AUTH_RATE_LIMIT, AUTH_RATE_WINDOW)),
            client_ip_header: auth.client_ip_header.as_deref().map(Arc::from),
        }
    }

    /// Swap the auth rate limiter (tests use a looser or stricter one).
    pub fn with_auth_rate_limiter(mut self, limiter: RateLimiter) -> Self {
        self.auth_rate_limiter = Arc::new(limiter);
        self
    }
}

pub fn router(state: AppState) -> Router {
    let api_v1 = Router::new().nest("/auth", auth::routes());

    Router::new()
        .route("/health", get(health))
        .nest("/api/v1", api_v1)
        .fallback(|| async { ApiError::not_found() })
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive()) // TODO: restrict to FRONTEND_URL before prod (see pulse-security.md #6)
}

/// Liveness + DB reachability. Returns 503 (no internal details, per
/// pulse-security.md #6) if Postgres can't be reached.
async fn health(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => (StatusCode::OK, "ok"),
        Err(e) => {
            error!(error = %e, "health check: database unreachable");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}
