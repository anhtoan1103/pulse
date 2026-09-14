use super::AdminUser;
use crate::{
    app::AppState,
    error::ApiError,
    extract::{ApiJson, ApiPath},
};
use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Never carries `password_hash` (pulse-security.md #6) — not selected at all.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AdminUserView {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct UsersList {
    users: Vec<AdminUserView>,
}

/// `GET /api/v1/admin/users` → 200 `{ users: [...] }`, all users.
pub async fn list_users(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<UsersList>, ApiError> {
    let users = sqlx::query_as::<_, AdminUserView>(
        "SELECT id, email, role, is_active, created_at FROM users ORDER BY created_at",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(UsersList { users }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUser {
    is_active: Option<bool>,
    role: Option<String>,
}

/// `PATCH /api/v1/admin/users/{id}` → 200 updated user.
///
/// An admin can't disable their own account or remove their own admin role:
/// there's no promote-to-admin flow (schema doc #2.1b — a new admin is
/// seeded via `ADMIN_SEED_*`, not granted through the app), so either action
/// on the last admin would be an unrecoverable lockout from the admin panel
/// itself. Get another admin to do it, or re-seed.
pub async fn update_user(
    admin: AdminUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateUser>,
) -> Result<Json<AdminUserView>, ApiError> {
    if let Some(role) = &body.role
        && role != "admin"
        && role != "user"
    {
        return Err(ApiError::validation("role must be admin or user"));
    }
    if id == admin.0.id {
        if body.is_active == Some(false) {
            return Err(ApiError::validation("cannot disable your own account"));
        }
        if body.role.as_deref() == Some("user") {
            return Err(ApiError::validation("cannot remove your own admin access"));
        }
    }

    sqlx::query_as::<_, AdminUserView>(
        "UPDATE users SET is_active = COALESCE($2, is_active), role = COALESCE($3, role)
         WHERE id = $1
         RETURNING id, email, role, is_active, created_at",
    )
    .bind(id)
    .bind(body.is_active)
    .bind(body.role)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or_else(ApiError::not_found)
}

#[derive(Serialize)]
pub struct Stats {
    total_users: i64,
    active_endpoints: i64,
    incidents_last_24h: i64,
}

/// `GET /api/v1/admin/stats` → 200 overview counts.
pub async fn stats(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Stats>, ApiError> {
    let total_users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    let active_endpoints: i64 =
        sqlx::query_scalar("SELECT count(*) FROM endpoints WHERE is_active")
            .fetch_one(&state.pool)
            .await?;
    let incidents_last_24h: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incidents WHERE triggered_at >= now() - interval '24 hours'",
    )
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(Stats {
        total_users,
        active_endpoints,
        incidents_last_24h,
    }))
}
