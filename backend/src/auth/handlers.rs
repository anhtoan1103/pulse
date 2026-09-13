use super::{AuthUser, MAX_USERS, insert_email_user, lock_user_creation, password, validate};
use crate::{
    app::AppState,
    error::ApiError,
    extract::{ApiJson, ClientIp},
};
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Request body for register + login. No `Debug`: holds a plaintext password.
#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    user_id: Uuid,
    email: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    token: String,
    user: LoginUser,
}

#[derive(Serialize)]
pub struct LoginUser {
    id: Uuid,
    email: String,
    role: String,
}

/// `POST /api/v1/auth/register` → 201 `{ user_id, email }`
pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(body): ApiJson<Credentials>,
) -> Result<(StatusCode, Json<RegisterResponse>), ApiError> {
    if !state.auth_rate_limiter.check("auth.register", ip) {
        return Err(ApiError::rate_limited());
    }

    let email = validate::normalize_email(&body.email)?;
    validate::validate_password(&body.password)?;
    // Hash before opening the transaction so the lock isn't held while hashing.
    let password_hash = password::hash(body.password).await?;

    let mut tx = state.pool.begin().await?;
    lock_user_creation(&mut tx).await?;

    let user_count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    if user_count >= MAX_USERS {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "REGISTRATION_CLOSED",
            "user limit reached, registration is closed",
        ));
    }

    let user_id = insert_email_user(&mut tx, &email, &password_hash, "user")
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(|db| db.is_unique_violation())
            {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "EMAIL_TAKEN",
                    "an account with this email already exists",
                )
            } else {
                e.into()
            }
        })?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse { user_id, email }),
    ))
}

/// `POST /api/v1/auth/login` → 200 `{ token, user: { id, email, role } }`
///
/// Unknown email and wrong password are indistinguishable (same status, same
/// message, same hashing work). A disabled account only gets its specific
/// 403 once the correct password was supplied.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(body): ApiJson<Credentials>,
) -> Result<Json<LoginResponse>, ApiError> {
    if !state.auth_rate_limiter.check("auth.login", ip) {
        return Err(ApiError::rate_limited());
    }

    let invalid_credentials = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "INVALID_CREDENTIALS",
            "invalid email or password",
        )
    };

    if body.password.chars().count() > validate::PASSWORD_MAX_LEN {
        return Err(invalid_credentials());
    }

    let user = match validate::normalize_email(&body.email) {
        Ok(email) => sqlx::query_as::<_, (Uuid, String, String, Option<String>, bool)>(
            "SELECT id, email, role, password_hash, is_active FROM users WHERE lower(email) = $1",
        )
        .bind(email)
        .fetch_optional(&state.pool)
        .await?,
        Err(_) => None,
    };

    let stored_hash = user.as_ref().and_then(|u| u.3.clone());
    if !password::verify(body.password, stored_hash).await? {
        return Err(invalid_credentials());
    }
    // verify() only returns true for a real stored hash, so `user` is Some.
    let (id, email, role, _, is_active) = user.ok_or_else(invalid_credentials)?;

    if !is_active {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "ACCOUNT_DISABLED",
            "this account has been disabled",
        ));
    }

    let token = state.jwt.issue(id)?;
    Ok(Json(LoginResponse {
        token,
        user: LoginUser { id, email, role },
    }))
}

/// `GET /api/v1/auth/me` → 200 `{ id, email, role, created_at }`
pub async fn me(user: AuthUser) -> Json<AuthUser> {
    Json(user)
}

/// `POST /api/v1/auth/logout` → 204
///
/// Tokens are stateless JWTs, so there's nothing to revoke server-side: the
/// client discards its token. (A token denylist is post-MVP; disabling a
/// user already invalidates their tokens via `AuthUser`'s `is_active` check.)
pub async fn logout() -> StatusCode {
    StatusCode::NO_CONTENT
}
