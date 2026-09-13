use super::{Endpoint, MAX_ENDPOINTS_PER_USER, endpoint_columns, validate};
use crate::{
    app::AppState,
    auth::AuthUser,
    error::ApiError,
    extract::{ApiJson, ApiPath},
    ssrf,
};
use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `deny_unknown_fields`: a typo or an attempt to set e.g. `user_id` /
/// `last_checked_at` is a 422, not silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEndpoint {
    name: String,
    url: String,
    #[serde(default)]
    method: Option<String>,
    check_interval_seconds: i32,
    latency_threshold_ms: i32,
    error_rate_threshold_percent: f64,
}

/// Every field optional; omitted (or `null`) = unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateEndpoint {
    name: Option<String>,
    url: Option<String>,
    method: Option<String>,
    check_interval_seconds: Option<i32>,
    latency_threshold_ms: Option<i32>,
    error_rate_threshold_percent: Option<f64>,
    is_active: Option<bool>,
}

#[derive(Serialize)]
pub struct EndpointList {
    endpoints: Vec<Endpoint>,
}

/// URL validation incl. SSRF checks (DNS resolution), mapped to a 422.
async fn validate_url(state: &AppState, raw: &str) -> Result<String, ApiError> {
    ssrf::validate_target_url(raw, &state.resolver)
        .await
        .map(String::from)
        .map_err(|rejection| ApiError::validation(rejection.message()))
}

/// `POST /api/v1/endpoints` → 201 endpoint
pub async fn create(
    user: AuthUser,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CreateEndpoint>,
) -> Result<(StatusCode, Json<Endpoint>), ApiError> {
    let name = validate::name(&body.name)?;
    let method = validate::method(body.method.as_deref().unwrap_or("GET"))?;
    let interval = validate::check_interval_seconds(body.check_interval_seconds)?;
    let latency = validate::latency_threshold_ms(body.latency_threshold_ms)?;
    let error_rate = validate::error_rate_threshold_percent(body.error_rate_threshold_percent)?;
    // Last: the only check that does network I/O.
    let url = validate_url(&state, &body.url).await?;

    let mut tx = state.pool.begin().await?;
    // Lock the owner's row so concurrent creates by the same user serialize
    // and can't both pass the count check.
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM endpoints WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_ENDPOINTS_PER_USER {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ENDPOINT_LIMIT_REACHED",
            format!("a user can have at most {MAX_ENDPOINTS_PER_USER} endpoints"),
        ));
    }

    let endpoint = sqlx::query_as::<_, Endpoint>(concat!(
        "INSERT INTO endpoints (user_id, name, url, method, check_interval_seconds, \
         latency_threshold_ms, error_rate_threshold_percent) \
         VALUES ($1, $2, $3, $4, $5, $6, $7::numeric) RETURNING ",
        endpoint_columns!()
    ))
    .bind(user.id)
    .bind(name)
    .bind(url)
    .bind(method)
    .bind(interval)
    .bind(latency)
    .bind(error_rate)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(endpoint)))
}

/// `GET /api/v1/endpoints` → 200 `{ endpoints: [...] }` (caller's only)
pub async fn list(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<EndpointList>, ApiError> {
    let endpoints = sqlx::query_as::<_, Endpoint>(concat!(
        "SELECT ",
        endpoint_columns!(),
        " FROM endpoints WHERE user_id = $1 ORDER BY created_at, id"
    ))
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(EndpointList { endpoints }))
}

/// `GET /api/v1/endpoints/{id}` → 200 endpoint, 404 if missing or not owned
pub async fn get_one(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Endpoint>, ApiError> {
    sqlx::query_as::<_, Endpoint>(concat!(
        "SELECT ",
        endpoint_columns!(),
        " FROM endpoints WHERE id = $1 AND user_id = $2"
    ))
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or_else(ApiError::not_found)
}

/// `PATCH /api/v1/endpoints/{id}` → 200 updated endpoint, 404 if missing or
/// not owned
pub async fn update(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateEndpoint>,
) -> Result<Json<Endpoint>, ApiError> {
    let name = body.name.as_deref().map(validate::name).transpose()?;
    let method = body.method.as_deref().map(validate::method).transpose()?;
    let interval = body
        .check_interval_seconds
        .map(validate::check_interval_seconds)
        .transpose()?;
    let latency = body
        .latency_threshold_ms
        .map(validate::latency_threshold_ms)
        .transpose()?;
    let error_rate = body
        .error_rate_threshold_percent
        .map(validate::error_rate_threshold_percent)
        .transpose()?;
    let url = match body.url.as_deref() {
        Some(raw) => Some(validate_url(&state, raw).await?),
        None => None,
    };

    // Single statement: ownership filter and partial update together, so
    // there's no read-then-write window.
    sqlx::query_as::<_, Endpoint>(concat!(
        "UPDATE endpoints SET \
             name = COALESCE($3, name), \
             url = COALESCE($4, url), \
             method = COALESCE($5, method), \
             check_interval_seconds = COALESCE($6, check_interval_seconds), \
             latency_threshold_ms = COALESCE($7, latency_threshold_ms), \
             error_rate_threshold_percent = COALESCE($8::numeric, error_rate_threshold_percent), \
             is_active = COALESCE($9, is_active) \
         WHERE id = $1 AND user_id = $2 RETURNING ",
        endpoint_columns!()
    ))
    .bind(id)
    .bind(user.id)
    .bind(name)
    .bind(url)
    .bind(method)
    .bind(interval)
    .bind(latency)
    .bind(error_rate)
    .bind(body.is_active)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or_else(ApiError::not_found)
}

/// `DELETE /api/v1/endpoints/{id}` → 204, 404 if missing or not owned.
/// Hard delete: checks/incidents/digests cascade (see endpoints migration).
pub async fn delete(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query("DELETE FROM endpoints WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        Err(ApiError::not_found())
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}
