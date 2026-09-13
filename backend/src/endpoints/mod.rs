//! Monitored endpoints CRUD — `docs/pulse-api-spec.md` #2.
//!
//! Ownership (pulse-security.md #3, IDOR): every query filters on the
//! caller's `user_id` from the token, never on ids alone. Someone else's
//! endpoint is indistinguishable from a nonexistent one (404).

mod handlers;
pub mod validate;

use crate::app::AppState;
use axum::{Router, routing::get};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Per-user cap (project-context #3). Counts all of a user's endpoints,
/// paused ones included — counting only active ones would let a user pause
/// 50, create 50 more, then re-activate everything.
pub const MAX_ENDPOINTS_PER_USER: i64 = 50;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::list).post(handlers::create))
        .route(
            "/{id}",
            get(handlers::get_one)
                .patch(handlers::update)
                .delete(handlers::delete),
        )
}

/// Endpoint as returned by the API.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Endpoint {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub method: String,
    pub check_interval_seconds: i32,
    pub latency_threshold_ms: i32,
    /// NUMERIC(5,2) in the DB, read as float8 (see [`endpoint_columns`]).
    pub error_rate_threshold_percent: f64,
    pub is_active: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Column list matching [`Endpoint`]. A macro (not a const) so queries can
/// be assembled with `concat!` and stay `&'static str`, as sqlx requires.
macro_rules! endpoint_columns {
    () => {
        "id, name, url, method, check_interval_seconds, latency_threshold_ms, \
         error_rate_threshold_percent::float8 AS error_rate_threshold_percent, \
         is_active, last_checked_at, created_at"
    };
}
pub(crate) use endpoint_columns;
