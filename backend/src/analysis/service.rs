//! AI Analysis Service loop — analyzes `pending` incidents
//! (docs/pulse-architecture.md #2.5, docs/pulse-ai-design.md #5).
//!
//! Runs inside the worker process, one incident at a time (free-tier LLM
//! rate limits are per minute; sequential calls with spacing stay under them).
//!
//! Claiming: the next due pending incident is picked with `FOR UPDATE SKIP
//! LOCKED`, its attempt counter bumped and `ai_next_attempt_at` pushed out by
//! [`LEASE`] — so several workers never analyze the same incident, and one
//! whose worker died mid-call is retried once the lease expires.
//!
//! Outcomes (ai-design #5):
//! - valid output → `completed`
//! - invalid output → retried once immediately with a format reminder;
//!   invalid again → `failed`
//! - rate limit / 5xx / timeout → stays `pending`, retried with exponential
//!   backoff (≥ `Retry-After`), `failed` after [`MAX_ATTEMPTS`]
//! - other provider errors (bad key, unknown model) → `failed`
//!
//! A failed analysis never blocks the incident itself: it stays visible with
//! its raw metrics.

use super::{
    context::{load_raw_incident_data, prepare_ai_context},
    llm::{LlmClient, LlmError},
    output::{AiAnalysis, json_schema, parse_analysis},
    prompt::{build_messages, build_retry_messages},
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

pub const MAX_ATTEMPTS: i32 = 5;
/// Must comfortably exceed two LLM calls (initial + format retry).
pub const LEASE: Duration = Duration::from_secs(120);
const IDLE_POLL: Duration = Duration::from_secs(5);
/// Gap between consecutive analyses (~15 requests/min worst case).
const CALL_SPACING: Duration = Duration::from_secs(4);
const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, PartialEq)]
pub enum AnalysisOutcome {
    Completed,
    /// Transient provider problem; retried at the given time.
    Retrying(DateTime<Utc>),
    /// `ai_status = 'failed'` with this reason.
    Failed(String),
    /// Incident no longer exists (endpoint deleted).
    Gone,
}

/// Claims the next due pending incident. Returns `(incident_id, attempt)`.
pub async fn claim_next(pool: &PgPool) -> Result<Option<(Uuid, i32)>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE incidents SET
             ai_attempts = ai_attempts + 1,
             ai_next_attempt_at = now() + make_interval(secs => $1)
         WHERE id = (
             SELECT id FROM incidents
             WHERE ai_status = 'pending'
               AND (ai_next_attempt_at IS NULL OR ai_next_attempt_at <= now())
             ORDER BY ai_next_attempt_at NULLS FIRST, triggered_at
             LIMIT 1
             FOR UPDATE SKIP LOCKED
         )
         RETURNING id, ai_attempts",
    )
    .bind(LEASE.as_secs_f64())
    .fetch_optional(pool)
    .await
}

/// Analyzes one claimed incident and persists the outcome.
pub async fn analyze_incident(
    pool: &PgPool,
    llm: &LlmClient,
    incident_id: Uuid,
    attempt: i32,
) -> anyhow::Result<AnalysisOutcome> {
    let Some(raw) = load_raw_incident_data(pool, incident_id).await? else {
        return Ok(AnalysisOutcome::Gone);
    };
    let context = prepare_ai_context(&raw);
    let schema = json_schema();

    let reply = match llm.complete(&build_messages(&context), &schema).await {
        Ok(reply) => reply,
        Err(e) => return handle_llm_error(pool, incident_id, attempt, e).await,
    };

    let analysis = match parse_analysis(&reply) {
        Ok(analysis) => analysis,
        Err(first_problem) => {
            warn!(%incident_id, problem = %first_problem, "invalid AI output, retrying with format reminder");
            let retry = llm
                .complete(&build_retry_messages(&context, &reply), &schema)
                .await;
            match retry {
                Ok(second_reply) => match parse_analysis(&second_reply) {
                    Ok(analysis) => analysis,
                    Err(second_problem) => {
                        warn!(%incident_id, problem = %second_problem, "invalid AI output after retry");
                        return mark_failed(pool, incident_id, "AI returned invalid output").await;
                    }
                },
                Err(e) => return handle_llm_error(pool, incident_id, attempt, e).await,
            }
        }
    };

    save_completed(pool, incident_id, &analysis).await?;
    Ok(AnalysisOutcome::Completed)
}

async fn handle_llm_error(
    pool: &PgPool,
    incident_id: Uuid,
    attempt: i32,
    err: LlmError,
) -> anyhow::Result<AnalysisOutcome> {
    match err {
        LlmError::Transient {
            message,
            retry_after,
        } => {
            if attempt >= MAX_ATTEMPTS {
                warn!(%incident_id, attempt, reason = %message, "AI analysis giving up");
                return mark_failed(
                    pool,
                    incident_id,
                    &format!("{message} (gave up after {attempt} attempts)"),
                )
                .await;
            }
            let delay = backoff(attempt).max(retry_after.unwrap_or_default());
            let next = Utc::now() + chrono::Duration::from_std(delay)?;
            sqlx::query(
                "UPDATE incidents SET ai_next_attempt_at = $2, ai_error = $3
                 WHERE id = $1 AND ai_status = 'pending'",
            )
            .bind(incident_id)
            .bind(next)
            .bind(&message)
            .execute(pool)
            .await?;
            info!(%incident_id, attempt, reason = %message, retry_at = %next, "AI analysis will retry");
            Ok(AnalysisOutcome::Retrying(next))
        }
        LlmError::Permanent { message, detail } => {
            // Detail is the provider's own error text (e.g. "model not found")
            // — logged for the operator, not stored.
            error!(%incident_id, reason = %message, provider_detail = %detail, "AI analysis failed");
            mark_failed(pool, incident_id, &message).await
        }
    }
}

async fn save_completed(
    pool: &PgPool,
    incident_id: Uuid,
    a: &AiAnalysis,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE incidents SET
             ai_status = 'completed',
             ai_possible_cause = $2,
             ai_confidence = $3,
             ai_evidence = $4,
             ai_suggested_steps = $5,
             ai_error = NULL,
             ai_next_attempt_at = NULL
         WHERE id = $1 AND ai_status = 'pending'",
    )
    .bind(incident_id)
    .bind(&a.possible_cause)
    .bind(a.confidence.as_str())
    .bind(sqlx::types::Json(&a.evidence))
    .bind(sqlx::types::Json(&a.suggested_steps))
    .execute(pool)
    .await
    .map(|_| ())
}

async fn mark_failed(
    pool: &PgPool,
    incident_id: Uuid,
    reason: &str,
) -> anyhow::Result<AnalysisOutcome> {
    sqlx::query(
        "UPDATE incidents SET ai_status = 'failed', ai_error = $2, ai_next_attempt_at = NULL
         WHERE id = $1 AND ai_status = 'pending'",
    )
    .bind(incident_id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(AnalysisOutcome::Failed(reason.to_string()))
}

/// 1 min, 2 min, 4 min, ... capped at 30 min.
pub fn backoff(attempt: i32) -> Duration {
    let exponent = attempt.saturating_sub(1).clamp(0, 16) as u32;
    BACKOFF_BASE
        .saturating_mul(2u32.saturating_pow(exponent))
        .min(BACKOFF_MAX)
}

/// Claims and analyzes one incident, if any is due.
pub async fn run_once(
    pool: &PgPool,
    llm: &LlmClient,
) -> anyhow::Result<Option<(Uuid, AnalysisOutcome)>> {
    let Some((incident_id, attempt)) = claim_next(pool).await? else {
        return Ok(None);
    };
    let outcome = analyze_incident(pool, llm, incident_id, attempt).await?;
    Ok(Some((incident_id, outcome)))
}

/// Processes pending incidents until `shutdown`. Cancelling mid-analysis is
/// safe: the lease expires and the incident is retried.
pub async fn run_loop(pool: PgPool, llm: LlmClient, shutdown: CancellationToken) {
    info!("AI analysis loop started");
    loop {
        let result = tokio::select! {
            _ = shutdown.cancelled() => break,
            result = run_once(&pool, &llm) => result,
        };
        let pause = match result {
            Ok(Some((incident_id, outcome))) => {
                info!(%incident_id, ?outcome, "AI analysis processed");
                CALL_SPACING
            }
            Ok(None) => IDLE_POLL,
            Err(e) => {
                error!(error = format!("{e:#}"), "AI analysis loop error");
                IDLE_POLL
            }
        };
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(pause) => {}
        }
    }
    info!("AI analysis loop stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(60));
        assert_eq!(backoff(2), Duration::from_secs(120));
        assert_eq!(backoff(4), Duration::from_secs(480));
        assert_eq!(backoff(10), BACKOFF_MAX);
        assert_eq!(backoff(i32::MAX), BACKOFF_MAX);
        assert_eq!(backoff(0), Duration::from_secs(60));
    }
}
