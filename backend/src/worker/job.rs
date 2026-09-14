//! Processing of a single check job: load endpoint → HTTP check → write a
//! `checks` row (docs/pulse-architecture.md #2.2, #3 step 2).

use super::checker::{CheckOutcome, HttpChecker};
use crate::{anomaly, queue::CheckJob};
use chrono::Utc;
use sqlx::PgPool;
use tracing::{info, warn};

#[derive(Debug, PartialEq)]
pub enum JobResult {
    Recorded(CheckOutcome),
    /// Endpoint deleted since the job was queued (or during the check).
    EndpointGone,
    /// Endpoint paused since the job was queued.
    Inactive,
    /// Job waited longer than the endpoint's interval; a newer job has been
    /// (or will be) scheduled, so checking now would just double up.
    Stale,
}

pub async fn process_job(
    pool: &PgPool,
    checker: &HttpChecker,
    job: &CheckJob,
) -> anyhow::Result<JobResult> {
    // Re-read the endpoint: url/method/is_active may have changed since
    // scheduling.
    let endpoint: Option<(String, String, bool, i32)> = sqlx::query_as(
        "SELECT url, method, is_active, check_interval_seconds FROM endpoints WHERE id = $1",
    )
    .bind(job.endpoint_id)
    .fetch_optional(pool)
    .await?;

    let Some((url, method, is_active, interval_seconds)) = endpoint else {
        return Ok(JobResult::EndpointGone);
    };
    if !is_active {
        return Ok(JobResult::Inactive);
    }
    let checked_at = Utc::now();
    if checked_at - job.scheduled_at > chrono::Duration::seconds(i64::from(interval_seconds)) {
        return Ok(JobResult::Stale);
    }

    let outcome = checker.check(&method, &url).await;

    let inserted = sqlx::query(
        "INSERT INTO checks (endpoint_id, checked_at, status_code, latency_ms, success, error_message)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(job.endpoint_id)
    .bind(checked_at)
    .bind(outcome.status_code)
    .bind(outcome.latency_ms)
    .bind(outcome.success)
    .bind(&outcome.error_message)
    .execute(pool)
    .await;

    match inserted {
        Ok(_) => {
            // Detection runs right after each new metric (architecture #2.4).
            // A detector failure is logged but doesn't fail the job: the check
            // itself is recorded, and the next check re-evaluates anyway.
            match anomaly::evaluate_endpoint(pool, job.endpoint_id).await {
                Ok(eval) if eval != anomaly::Evaluation::default() => info!(
                    endpoint_id = %job.endpoint_id,
                    opened = ?eval.opened,
                    recovered = ?eval.recovered,
                    relapsed = ?eval.relapsed,
                    "incidents updated"
                ),
                Ok(_) => {}
                Err(e) => warn!(
                    endpoint_id = %job.endpoint_id,
                    error = format!("{e:#}"),
                    "anomaly detection failed"
                ),
            }
            Ok(JobResult::Recorded(outcome))
        }
        // FK violation: endpoint deleted while the request was in flight.
        Err(e)
            if e.as_database_error()
                .is_some_and(|db| db.is_foreign_key_violation()) =>
        {
            Ok(JobResult::EndpointGone)
        }
        Err(e) => Err(e.into()),
    }
}
