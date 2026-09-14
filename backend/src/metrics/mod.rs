//! Metrics (Checks) read API — `docs/pulse-api-spec.md` #3.
//!
//! Nested under `/api/v1/endpoints/{id}` alongside the endpoints CRUD
//! router, so it shares the same ownership rule: every query is scoped to
//! an endpoint owned by the caller, otherwise 404 (pulse-security.md #3).

mod handlers;

use crate::app::AppState;
use axum::{Router, routing::get};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{id}/checks", get(handlers::list_checks))
        .route("/{id}/checks/summary", get(handlers::summary))
        .route("/{id}/health-digests", get(handlers::list_health_digests))
}
