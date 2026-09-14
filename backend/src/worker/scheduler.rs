//! Scheduler — finds endpoints due for a check and enqueues jobs
//! (docs/pulse-architecture.md #2.1).
//!
//! Claiming works by advancing `last_checked_at` to now at enqueue time (in
//! the same transaction as the Redis push), so the next tick won't enqueue
//! the same endpoint again while its job is queued or running.
//! `FOR UPDATE SKIP LOCKED` makes concurrent schedulers (several worker
//! replicas) claim disjoint sets. `last_checked_at` therefore means "last
//! scheduled", which trails the real `checks.checked_at` by queue latency.

use crate::queue::{CheckJob, CheckQueue};
use chrono::Utc;
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

/// Max endpoints claimed per tick. MVP max is 10 users × 50 endpoints = 500.
pub const CLAIM_BATCH: i64 = 500;
/// Skip scheduling while this many jobs are already waiting — workers are
/// behind, and piling on more only produces stale jobs.
pub const MAX_BACKLOG: u64 = 2_000;

/// Marks due endpoints as scheduled and returns their ids.
///
/// `slack`: endpoints due within this much time from now count as due.
/// Pass half the scheduler tick — otherwise an endpoint that becomes due a
/// few ms after a tick waits a whole extra tick (a 10s interval with a 1s
/// tick would drift to 11s).
pub async fn claim_due(
    conn: &mut PgConnection,
    limit: i64,
    slack: Duration,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "WITH due AS (
             SELECT id FROM endpoints
             WHERE is_active
               AND (last_checked_at IS NULL
                    OR last_checked_at + make_interval(secs => check_interval_seconds)
                       <= now() + make_interval(secs => $2))
             ORDER BY last_checked_at NULLS FIRST
             LIMIT $1
             FOR UPDATE SKIP LOCKED
         )
         UPDATE endpoints e SET last_checked_at = now()
         FROM due WHERE e.id = due.id
         RETURNING e.id",
    )
    .bind(limit)
    .bind(slack.as_secs_f64())
    .fetch_all(conn)
    .await
}

/// One scheduler tick. Returns how many jobs were enqueued. If the Redis
/// push fails, the claim is rolled back so those endpoints stay due.
pub async fn schedule_once(
    pool: &PgPool,
    queue: &CheckQueue,
    slack: Duration,
) -> anyhow::Result<usize> {
    let backlog = queue.len().await?;
    if backlog >= MAX_BACKLOG {
        warn!(
            backlog,
            "check queue backlog too large, skipping scheduling tick"
        );
        return Ok(0);
    }

    let mut tx = pool.begin().await?;
    let ids = claim_due(&mut tx, CLAIM_BATCH, slack).await?;
    if ids.is_empty() {
        return Ok(0);
    }

    let scheduled_at = Utc::now();
    let jobs: Vec<CheckJob> = ids
        .iter()
        .map(|&endpoint_id| CheckJob {
            endpoint_id,
            scheduled_at,
        })
        .collect();
    queue.push(&jobs).await?; // on error `tx` drops → rollback
    tx.commit().await?;

    debug!(count = jobs.len(), "enqueued check jobs");
    Ok(jobs.len())
}
