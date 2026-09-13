//! Email/password authentication — `docs/pulse-api-spec.md` #1.1, #1.3.
//! OAuth (#1.2) comes later, once the Cloudflare Tunnel domain exists for
//! callback URLs (project-context #6 step 3).

mod handlers;
pub mod password;
pub mod seed;
pub mod token;
pub mod validate;

use crate::{app::AppState, error::ApiError};
use axum::{
    Router,
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

/// System-wide user cap (PRD / project-context #3). Enforced on register.
pub const MAX_USERS: i64 = 10;

/// Advisory lock key serializing user creation, so concurrent registrations
/// can't both pass the `MAX_USERS` count check. Arbitrary constant, unique
/// within this app.
pub(crate) const USER_CREATION_LOCK_KEY: i64 = 0x5055_4c53_4555_5352; // "PULSEUSR"

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/register", post(handlers::register))
        .route("/login", post(handlers::login))
        .route("/me", get(handlers::me))
        .route("/logout", post(handlers::logout))
}

/// The authenticated caller. Add as a handler argument to require a valid
/// `Authorization: Bearer <jwt>`; responds 401 otherwise.
///
/// Loaded fresh from the DB on every request, so a disabled user's
/// still-unexpired token stops working immediately. Never carries
/// `password_hash` (pulse-security.md #6).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;

        let claims = state
            .jwt
            .verify(token.trim())
            .ok_or_else(ApiError::unauthorized)?;

        sqlx::query_as::<_, AuthUser>(
            "SELECT id, email, role, created_at FROM users WHERE id = $1 AND is_active",
        )
        .bind(claims.sub)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::unauthorized)
    }
}

/// Inserts a user plus its `email` auth identity. Callers own the
/// transaction (and the `USER_CREATION_LOCK_KEY` lock). `email` must already
/// be normalized.
pub(crate) async fn insert_email_user(
    conn: &mut PgConnection,
    email: &str,
    password_hash: &str,
    role: &str,
) -> Result<Uuid, sqlx::Error> {
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, role) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(email)
    .bind(password_hash)
    .bind(role)
    .fetch_one(&mut *conn)
    .await?;

    sqlx::query("INSERT INTO auth_identities (user_id, provider) VALUES ($1, 'email')")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;

    Ok(user_id)
}

/// Takes the user-creation advisory lock for the rest of the transaction.
pub(crate) async fn lock_user_creation(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(USER_CREATION_LOCK_KEY)
        .execute(conn)
        .await
        .map(|_| ())
}
