//! Router + shared state, built here (not in `bin/api.rs`) so integration
//! tests can drive the exact same app in-process.

use crate::{
    admin,
    auth::{self, token::JwtKeys},
    config::AuthConfig,
    endpoints,
    error::ApiError,
    incidents, metrics,
    rate_limit::RateLimiter,
    ssrf::HostResolver,
};
use axum::{
    Router,
    extract::State,
    http::{
        HeaderValue, Method, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    routing::get,
};
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
    /// DNS for SSRF validation of monitored URLs.
    pub resolver: Arc<HostResolver>,
    /// The dashboard's origin — the only one CORS allows (pulse-security.md #6).
    pub frontend_url: Arc<str>,
}

impl AppState {
    pub fn new(pool: PgPool, frontend_url: &str, auth: &AuthConfig) -> Self {
        Self {
            pool,
            jwt: Arc::new(JwtKeys::new(
                auth.jwt_secret.as_bytes(),
                chrono::Duration::hours(auth.jwt_expiry_hours),
            )),
            auth_rate_limiter: Arc::new(RateLimiter::new(AUTH_RATE_LIMIT, AUTH_RATE_WINDOW)),
            client_ip_header: auth.client_ip_header.as_deref().map(Arc::from),
            resolver: Arc::new(HostResolver::System),
            frontend_url: Arc::from(frontend_url),
        }
    }

    /// Swap the auth rate limiter (tests use a looser or stricter one).
    pub fn with_auth_rate_limiter(mut self, limiter: RateLimiter) -> Self {
        self.auth_rate_limiter = Arc::new(limiter);
        self
    }

    /// Swap the DNS resolver (tests pin host → IP mappings).
    pub fn with_resolver(mut self, resolver: HostResolver) -> Self {
        self.resolver = Arc::new(resolver);
        self
    }
}

pub fn router(state: AppState) -> Router {
    let api_v1 = Router::new()
        .nest("/auth", auth::routes())
        .nest("/endpoints", endpoints::routes().merge(metrics::routes()))
        .nest("/incidents", incidents::routes())
        .nest("/admin", admin::routes());

    // Exactly the dashboard's own origin (pulse-security.md #6: never `*` in
    // production) — read before `with_state` moves `state` away. An invalid
    // FRONTEND_URL (must be a bare origin, e.g. no path) fails loudly at
    // startup rather than silently falling back to something permissive.
    let cors_origin = HeaderValue::from_str(&state.frontend_url).unwrap_or_else(|_| {
        panic!(
            "FRONTEND_URL {:?} is not a valid CORS origin",
            state.frontend_url
        )
    });
    let cors = CorsLayer::new()
        .allow_origin(cors_origin)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE]);

    Router::new()
        .route("/health", get(health))
        .nest("/api/v1", api_v1)
        .fallback(|| async { ApiError::not_found() })
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
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
