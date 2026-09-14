//! Incidents read + resolve API — `docs/pulse-api-spec.md` #4.
//!
//! Ownership: incidents don't carry `user_id` directly, so every query
//! joins through `endpoints` and filters on the caller (pulse-security.md
//! #3) — another user's incident is a 404, same as endpoints.

mod handlers;

use crate::app::AppState;
use axum::{Router, routing::get};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::list))
        .route("/{id}", get(handlers::get_one))
        .route("/{id}/resolve", axum::routing::patch(handlers::resolve))
}
