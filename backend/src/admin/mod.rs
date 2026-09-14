//! Admin API — `docs/pulse-api-spec.md` #6.
//!
//! Every route here requires [`AdminUser`], a single extractor wrapping
//! [`AuthUser`] with one added check (`role == admin`, 403 otherwise) — the
//! "middleware riêng, áp dụng nhất quán" pulse-security.md #3 asks for:
//! consistency comes from every handler taking `AdminUser` instead of each
//! one re-checking `user.is_admin()` by hand.

mod handlers;

use crate::{app::AppState, auth::AuthUser, error::ApiError};
use axum::{
    Router,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    routing::{get, patch},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(handlers::list_users))
        .route("/users/{id}", patch(handlers::update_user))
        .route("/stats", get(handlers::stats))
}

/// An authenticated caller who is also an admin. `403` (not `404`) for a
/// non-admin — admin routes existing isn't a secret worth hiding, unlike
/// another user's data (contrast with the 404-for-everything-unowned rule
/// elsewhere).
pub struct AdminUser(pub AuthUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if user.is_admin() {
            Ok(AdminUser(user))
        } else {
            Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "admin access required",
            ))
        }
    }
}
