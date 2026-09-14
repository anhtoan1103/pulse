//! Health Digest — periodic per-endpoint summary, no AI
//! (docs/pulse-architecture.md #2.6, PRD #6: "định kỳ thì chạy đơn giản").
//!
//! Runs once a day (default 08:00 UTC) over the preceding 24h. For each
//! endpoint with at least one check in that window, aggregates
//! `total_checks`/`success_count`/`avg_latency_ms` into a `health_digests`
//! row and classifies it `healthy`/`degraded` against the endpoint's own
//! thresholds (same "strictly above" rule as the Anomaly Detector). One
//! digest email per user then bundles all of that user's endpoint digests
//! for the period, instead of one email per endpoint.
//!
//! **Claiming a period**: `digest_runs.period_end` is unique, so
//! `INSERT ... ON CONFLICT DO NOTHING RETURNING id` lets exactly one worker
//! replica generate a given period; others see no row and skip. Generation
//! itself (a handful of aggregate queries) runs in one transaction, so it's
//! never left half-done.
//!
//! **Missed periods aren't backfilled**: each tick only ever attempts the
//! most recently passed boundary. If the worker is down across an entire
//! boundary, that day's digest is simply skipped — acceptable for a
//! self-hosted MVP that already accepts occasional downtime
//! (pulse-security.md #7b).

use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

/// UTC hour the digest period ends (and the next one starts) each day.
pub const DIGEST_HOUR_UTC: u32 = 8;
pub const DIGEST_PERIOD: Duration = Duration::hours(24);
/// How often to check whether a new period has closed. Cheap even when
/// nothing is due: one `ON CONFLICT DO NOTHING` insert.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(5 * 60);

#[derive(Debug, PartialEq, Eq)]
pub struct GeneratedRun {
    pub run_id: Uuid,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub digests_created: i32,
    pub users_notified: i64,
}

/// The most recent `DIGEST_HOUR_UTC:00:00` at or before `now`.
pub fn latest_boundary(now: DateTime<Utc>) -> DateTime<Utc> {
    let today = now
        .date_naive()
        .and_hms_opt(DIGEST_HOUR_UTC, 0, 0)
        .expect("valid time");
    let today = Utc.from_utc_datetime(&today);
    if now.hour() >= DIGEST_HOUR_UTC {
        today
    } else {
        today - Duration::days(1)
    }
}

/// Attempts to claim and generate the most recently passed period.
/// `Ok(None)` if that period was already claimed (by this or another
/// replica) or hasn't fully elapsed relative to `now`.
pub async fn run_once(pool: &PgPool) -> anyhow::Result<Option<GeneratedRun>> {
    let period_end = latest_boundary(Utc::now());
    let period_start = period_end - DIGEST_PERIOD;

    let mut tx = pool.begin().await?;
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO digest_runs (period_start, period_end) VALUES ($1, $2)
         ON CONFLICT (period_end) DO NOTHING
         RETURNING id",
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(run_id) = claimed else {
        return Ok(None);
    };

    // One row per endpoint that saw at least one check in the period,
    // classified against its own current thresholds. ON CONFLICT DO NOTHING
    // is defense-in-depth (period_end's uniqueness already prevents this
    // running twice); it never actually needs to reconcile diverging values.
    let digests_created: i32 = sqlx::query_scalar(
        "WITH inserted AS (
             INSERT INTO health_digests
                 (endpoint_id, period_start, period_end, total_checks, success_count,
                  avg_latency_ms, status)
             SELECT
                 e.id, $1, $2, agg.total_checks, agg.success_count, agg.avg_latency_ms,
                 CASE WHEN
                     (agg.total_checks - agg.success_count)::numeric * 100
                         > e.error_rate_threshold_percent * agg.total_checks
                     OR agg.avg_latency_ms > e.latency_threshold_ms
                 THEN 'degraded' ELSE 'healthy' END
             FROM endpoints e
             JOIN LATERAL (
                 SELECT
                     count(*)::integer AS total_checks,
                     count(*) FILTER (WHERE success)::integer AS success_count,
                     round(avg(latency_ms))::integer AS avg_latency_ms
                 FROM checks c
                 WHERE c.endpoint_id = e.id AND c.checked_at >= $1 AND c.checked_at < $2
             ) agg ON true
             WHERE agg.total_checks > 0
             ON CONFLICT (endpoint_id, period_start) DO NOTHING
             RETURNING 1
         )
         SELECT count(*)::integer FROM inserted",
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_one(&mut *tx)
    .await?;

    // One notification per user who has at least one digest this period,
    // bundling all of their endpoints into a single email.
    let users_notified: i64 = sqlx::query_scalar(
        "WITH inserted AS (
             INSERT INTO notifications (user_id, kind, digest_run_id, send_after)
             SELECT DISTINCT e.user_id, 'health_digest', $2, now()
             FROM health_digests hd JOIN endpoints e ON e.id = hd.endpoint_id
             WHERE hd.period_start = $1
             ON CONFLICT (user_id, digest_run_id) WHERE digest_run_id IS NOT NULL DO NOTHING
             RETURNING 1
         )
         SELECT count(*) FROM inserted",
    )
    .bind(period_start)
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query("UPDATE digest_runs SET digests_created = $2 WHERE id = $1")
        .bind(run_id)
        .bind(digests_created)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(Some(GeneratedRun {
        run_id,
        period_start,
        period_end,
        digests_created,
        users_notified,
    }))
}

/// Ticks [`run_once`] every [`TICK`] until `shutdown`.
pub async fn run_loop(pool: PgPool, shutdown: CancellationToken) {
    info!("health digest loop started");
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tick.tick() => {}
        }
        match run_once(&pool).await {
            Ok(Some(run)) => info!(
                run_id = %run.run_id,
                period_start = %run.period_start,
                period_end = %run.period_end,
                digests_created = run.digests_created,
                users_notified = run.users_notified,
                "health digest generated"
            ),
            Ok(None) => {}
            Err(e) => error!(error = format!("{e:#}"), "health digest run failed"),
        }
    }
    info!("health digest loop stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn boundary_is_today_at_or_after_the_digest_hour() {
        assert_eq!(
            latest_boundary(at(2026, 9, 14, 8, 0)),
            at(2026, 9, 14, 8, 0)
        );
        assert_eq!(
            latest_boundary(at(2026, 9, 14, 23, 59)),
            at(2026, 9, 14, 8, 0)
        );
    }

    #[test]
    fn boundary_is_yesterday_before_the_digest_hour() {
        assert_eq!(
            latest_boundary(at(2026, 9, 14, 7, 59)),
            at(2026, 9, 13, 8, 0)
        );
        assert_eq!(
            latest_boundary(at(2026, 9, 14, 0, 0)),
            at(2026, 9, 13, 8, 0)
        );
    }
}
