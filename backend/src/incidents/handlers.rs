use crate::{
    app::AppState,
    auth::AuthUser,
    error::ApiError,
    extract::{ApiPath, ApiQuery},
};
use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Summary shape for `GET /incidents` — no metric snapshots (those are only
/// meaningful alongside the endpoint context on the detail page).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct IncidentSummary {
    pub id: Uuid,
    pub endpoint_id: Uuid,
    pub endpoint_name: String,
    pub trigger_reason: String,
    pub triggered_at: DateTime<Utc>,
    pub recovered_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub ai_status: String,
    pub ai_possible_cause: Option<String>,
    pub ai_confidence: Option<String>,
}

/// Full shape for `GET /incidents/{id}` — everything, per api-spec #4.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct IncidentDetail {
    pub id: Uuid,
    pub endpoint_id: Uuid,
    pub endpoint_name: String,
    pub trigger_reason: String,
    pub triggered_at: DateTime<Utc>,
    pub metric_before: Option<Value>,
    pub metric_after: Option<Value>,
    pub recovered_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub ai_status: String,
    pub ai_possible_cause: Option<String>,
    pub ai_confidence: Option<String>,
    pub ai_evidence: Option<Value>,
    pub ai_suggested_steps: Option<Value>,
    pub ai_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    endpoint_id: Option<Uuid>,
    /// "open" | "resolved"; anything else (or omitted) means both.
    status: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
pub struct IncidentList {
    incidents: Vec<IncidentSummary>,
}

/// `GET /api/v1/incidents` → 200 `{ incidents: [...] }`, scoped to the
/// caller's own endpoints; an `endpoint_id` the caller doesn't own yields an
/// empty list rather than an error (never confirms who owns what).
pub async fn list(
    user: AuthUser,
    State(state): State<AppState>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<IncidentList>, ApiError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let resolved_filter: Option<bool> = match q.status.as_deref() {
        Some("open") => Some(false),
        Some("resolved") => Some(true),
        _ => None,
    };

    let incidents = sqlx::query_as::<_, IncidentSummary>(
        "SELECT i.id, i.endpoint_id, e.name AS endpoint_name, i.trigger_reason, i.triggered_at,
                i.recovered_at, i.resolved_at, i.ai_status, i.ai_possible_cause, i.ai_confidence
         FROM incidents i JOIN endpoints e ON e.id = i.endpoint_id
         WHERE e.user_id = $1
           AND ($2::uuid IS NULL OR i.endpoint_id = $2)
           AND ($3::boolean IS NULL OR (i.resolved_at IS NOT NULL) = $3)
         ORDER BY i.triggered_at DESC
         LIMIT $4",
    )
    .bind(user.id)
    .bind(q.endpoint_id)
    .bind(resolved_filter)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(IncidentList { incidents }))
}

/// `GET /api/v1/incidents/{id}` → 200 full incident, 404 if missing/not owned
pub async fn get_one(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<IncidentDetail>, ApiError> {
    sqlx::query_as::<_, IncidentDetail>(
        "SELECT i.id, i.endpoint_id, e.name AS endpoint_name, i.trigger_reason, i.triggered_at,
                i.metric_before, i.metric_after, i.recovered_at, i.resolved_at, i.ai_status,
                i.ai_possible_cause, i.ai_confidence, i.ai_evidence, i.ai_suggested_steps,
                i.ai_error, i.created_at
         FROM incidents i JOIN endpoints e ON e.id = i.endpoint_id
         WHERE i.id = $1 AND e.user_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or_else(ApiError::not_found)
}

/// `PATCH /api/v1/incidents/{id}/resolve` → 200 updated incident.
/// Idempotent: resolving an already-resolved incident just returns it as-is.
pub async fn resolve(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<IncidentDetail>, ApiError> {
    sqlx::query(
        "UPDATE incidents SET resolved_at = now()
         WHERE id = $1 AND resolved_at IS NULL
           AND endpoint_id IN (SELECT id FROM endpoints WHERE user_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .execute(&state.pool)
    .await?;

    get_one(user, State(state), ApiPath(id)).await
}
