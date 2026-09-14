use crate::{
    app::AppState,
    auth::AuthUser,
    error::ApiError,
    extract::{ApiPath, ApiQuery},
    pagination::clamp_limit,
};
use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Check {
    pub checked_at: DateTime<Utc>,
    pub status_code: Option<i32>,
    pub latency_ms: Option<i32>,
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(Serialize)]
pub struct ChecksList {
    checks: Vec<Check>,
}

#[derive(Deserialize)]
pub struct ChecksQuery {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

/// Confirms `id` is an endpoint owned by `user`; 404 otherwise (never
/// distinguishable from a non-existent id — pulse-security.md #3).
async fn require_owned_endpoint(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM endpoints WHERE id = $1 AND user_id = $2)",
    )
    .bind(id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    if owned {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// `GET /api/v1/endpoints/{id}/checks` → 200 `{ checks: [...] }`
pub async fn list_checks(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<ChecksQuery>,
) -> Result<Json<ChecksList>, ApiError> {
    require_owned_endpoint(&state.pool, id, user.id).await?;
    let limit = clamp_limit(q.limit)?;
    if let (Some(from), Some(to)) = (q.from, q.to)
        && from > to
    {
        return Err(ApiError::validation("from must not be after to"));
    }

    let checks = sqlx::query_as::<_, Check>(
        "SELECT checked_at, status_code, latency_ms, success, error_message
         FROM checks
         WHERE endpoint_id = $1
           AND ($2::timestamptz IS NULL OR checked_at >= $2)
           AND ($3::timestamptz IS NULL OR checked_at <= $3)
         ORDER BY checked_at DESC
         LIMIT $4",
    )
    .bind(id)
    .bind(q.from)
    .bind(q.to)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(ChecksList { checks }))
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    #[serde(default = "default_period")]
    period: String,
}
fn default_period() -> String {
    "24h".into()
}

#[derive(Serialize)]
pub struct Summary {
    avg_latency_ms: Option<f64>,
    success_rate_percent: f64,
    total_checks: i64,
}

fn period_to_interval(period: &str) -> Result<&'static str, ApiError> {
    match period {
        "24h" => Ok("24 hours"),
        "7d" => Ok("7 days"),
        "30d" => Ok("30 days"),
        _ => Err(ApiError::validation("period must be one of 24h, 7d, 30d")),
    }
}

/// `GET /api/v1/endpoints/{id}/checks/summary` → 200
/// `{ avg_latency_ms, success_rate_percent, total_checks }`
///
/// `success_rate_percent` is `100.0` (not `null`/`NaN`) for an empty window,
/// matching api-spec #9.2's "no checks at all" test case — an endpoint with
/// no data yet isn't reported as failing.
pub async fn summary(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<SummaryQuery>,
) -> Result<Json<Summary>, ApiError> {
    require_owned_endpoint(&state.pool, id, user.id).await?;
    let interval = period_to_interval(&q.period)?;

    let (total_checks, success_count, avg_latency_ms): (i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE success), avg(latency_ms)::float8
         FROM checks
         WHERE endpoint_id = $1 AND checked_at >= now() - $2::interval",
    )
    .bind(id)
    .bind(interval)
    .fetch_one(&state.pool)
    .await?;

    let success_rate_percent = if total_checks > 0 {
        (success_count as f64 * 100.0 / total_checks as f64 * 100.0).round() / 100.0
    } else {
        100.0
    };

    Ok(Json(Summary {
        avg_latency_ms: avg_latency_ms.map(|v| (v * 100.0).round() / 100.0),
        success_rate_percent,
        total_checks,
    }))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct HealthDigest {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub total_checks: i32,
    pub success_count: i32,
    pub avg_latency_ms: Option<i32>,
    pub status: String,
}

#[derive(Deserialize)]
pub struct DigestsQuery {
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct DigestsList {
    digests: Vec<HealthDigest>,
}

/// `GET /api/v1/endpoints/{id}/health-digests` → 200 `{ digests: [...] }`
pub async fn list_health_digests(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<DigestsQuery>,
) -> Result<Json<DigestsList>, ApiError> {
    require_owned_endpoint(&state.pool, id, user.id).await?;
    let limit = clamp_limit(q.limit)?;

    let digests = sqlx::query_as::<_, HealthDigest>(
        "SELECT period_start, period_end, total_checks, success_count, avg_latency_ms, status
         FROM health_digests
         WHERE endpoint_id = $1
         ORDER BY period_start DESC
         LIMIT $2",
    )
    .bind(id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(DigestsList { digests }))
}
