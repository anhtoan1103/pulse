//! Anomaly Detector — compares recent checks against the endpoint's static
//! thresholds and opens/resolves incidents (docs/pulse-architecture.md #2.4,
//! PRD #6: static per-endpoint thresholds, no dynamic baseline in MVP).
//!
//! Runs after every recorded check. Decisions (not fixed by the design docs):
//!
//! - **Windows**: "after" = checks in the last [`AFTER_WINDOW`]; "before" =
//!   the [`BEFORE_WINDOW`] preceding it (matches the example periods in
//!   docs/pulse-ai-design.md #2). A window with fewer than [`MIN_SAMPLES`]
//!   checks is widened to the last `MIN_SAMPLES` checks, so long-interval
//!   endpoints (e.g. hourly) are still evaluated but never on a single check.
//! - **Breach**: window avg latency (over checks that got a response) is
//!   strictly above `latency_threshold_ms`, or its failure rate is strictly
//!   above `error_rate_threshold_percent` ("latency > 2s, error rate > 5%").
//! - **One open incident per endpoint + reason** (also a unique index).
//! - **Auto-resolve** when the window is back within threshold — otherwise a
//!   single never-resolved incident would silence every later outage.
//! - **Re-open cooldown** of [`REOPEN_COOLDOWN`] per endpoint + reason, so a
//!   flapping endpoint can't open (and pay for AI analysis of) an incident
//!   on every check.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

pub const AFTER_WINDOW: Duration = Duration::minutes(5);
pub const BEFORE_WINDOW: Duration = Duration::minutes(15);
pub const MIN_SAMPLES: usize = 3;
pub const REOPEN_COOLDOWN: Duration = Duration::minutes(10);
/// Hard cap on rows pulled per window (10s interval × 15 min = 90).
const WINDOW_ROW_LIMIT: i64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerReason {
    LatencyThresholdExceeded,
    ErrorRateThresholdExceeded,
}

impl TriggerReason {
    pub const ALL: [TriggerReason; 2] = [
        TriggerReason::LatencyThresholdExceeded,
        TriggerReason::ErrorRateThresholdExceeded,
    ];

    /// Value stored in `incidents.trigger_reason` (matches its CHECK constraint).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LatencyThresholdExceeded => "latency_threshold_exceeded",
            Self::ErrorRateThresholdExceeded => "error_rate_threshold_exceeded",
        }
    }
}

/// One check as the detector sees it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CheckSample {
    pub checked_at: DateTime<Utc>,
    pub latency_ms: Option<i32>,
    pub success: bool,
}

/// Aggregated window, stored as `incidents.metric_before` / `metric_after`
/// and fed to AI context preparation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowStats {
    /// Human-readable description of what the window covers.
    pub period: String,
    pub window_start: Option<DateTime<Utc>>,
    pub window_end: Option<DateTime<Utc>>,
    pub total_checks: usize,
    pub failed_checks: usize,
    /// Average over checks that received a response; `None` if none did.
    pub avg_latency_ms: Option<i64>,
    /// Rounded to 1 decimal.
    pub error_rate_percent: f64,
}

impl WindowStats {
    pub fn from_samples(period: impl Into<String>, samples: &[CheckSample]) -> Self {
        let latencies: Vec<i64> = samples
            .iter()
            .filter_map(|s| s.latency_ms.map(i64::from))
            .collect();
        let failed = samples.iter().filter(|s| !s.success).count();
        let error_rate = if samples.is_empty() {
            0.0
        } else {
            failed as f64 * 100.0 / samples.len() as f64
        };

        Self {
            period: period.into(),
            window_start: samples.iter().map(|s| s.checked_at).min(),
            window_end: samples.iter().map(|s| s.checked_at).max(),
            total_checks: samples.len(),
            failed_checks: failed,
            avg_latency_ms: (!latencies.is_empty()).then(|| {
                (latencies.iter().sum::<i64>() as f64 / latencies.len() as f64).round() as i64
            }),
            error_rate_percent: (error_rate * 10.0).round() / 10.0,
        }
    }
}

/// Thresholds breached by `stats`. Empty if there isn't enough data.
pub fn breached_thresholds(
    stats: &WindowStats,
    latency_threshold_ms: i32,
    error_rate_threshold_percent: f64,
) -> Vec<TriggerReason> {
    if stats.total_checks < MIN_SAMPLES {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    if stats
        .avg_latency_ms
        .is_some_and(|avg| avg > i64::from(latency_threshold_ms))
    {
        reasons.push(TriggerReason::LatencyThresholdExceeded);
    }
    // Compare the unrounded rate so e.g. 5.04% vs a 5% threshold isn't
    // rounded down to "not exceeded".
    let exact_rate = if stats.total_checks == 0 {
        0.0
    } else {
        stats.failed_checks as f64 * 100.0 / stats.total_checks as f64
    };
    if exact_rate > error_rate_threshold_percent {
        reasons.push(TriggerReason::ErrorRateThresholdExceeded);
    }
    reasons
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Evaluation {
    /// Newly opened incidents (`ai_status = 'pending'`) — step 7 analyzes these.
    pub opened: Vec<Uuid>,
    pub resolved: Vec<Uuid>,
}

/// Evaluates one endpoint's recent checks and opens/resolves incidents.
pub async fn evaluate_endpoint(pool: &PgPool, endpoint_id: Uuid) -> anyhow::Result<Evaluation> {
    let mut tx = pool.begin().await?;

    // Row lock serializes concurrent evaluations of the same endpoint (e.g.
    // two overlapping checks finishing together).
    let thresholds: Option<(i32, f64)> = sqlx::query_as(
        "SELECT latency_threshold_ms, error_rate_threshold_percent::float8
         FROM endpoints WHERE id = $1 FOR UPDATE",
    )
    .bind(endpoint_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((latency_threshold, error_rate_threshold)) = thresholds else {
        return Ok(Evaluation::default());
    };

    let now = Utc::now();
    let after_samples = window(
        &mut tx,
        endpoint_id,
        now + Duration::milliseconds(1),
        AFTER_WINDOW,
    )
    .await?;
    if after_samples.len() < MIN_SAMPLES {
        return Ok(Evaluation::default()); // not enough data to judge either way
    }
    let after = WindowStats::from_samples(
        period_label(&after_samples, AFTER_WINDOW, "most recent"),
        &after_samples,
    );
    let breached = breached_thresholds(&after, latency_threshold, error_rate_threshold);

    let open: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, trigger_reason FROM incidents WHERE endpoint_id = $1 AND resolved_at IS NULL",
    )
    .bind(endpoint_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut evaluation = Evaluation::default();
    let mut before: Option<WindowStats> = None;

    for reason in TriggerReason::ALL {
        let open_incident = open
            .iter()
            .find(|(_, r)| r == reason.as_str())
            .map(|(id, _)| *id);

        match (breached.contains(&reason), open_incident) {
            (true, None) => {
                if in_cooldown(&mut tx, endpoint_id, reason, now).await? {
                    continue;
                }
                if before.is_none() {
                    // Oldest sample is first: windows are returned newest-first.
                    let after_start = after_samples.last().map(|s| s.checked_at).unwrap_or(now);
                    let before_samples =
                        window(&mut tx, endpoint_id, after_start, BEFORE_WINDOW).await?;
                    before = Some(WindowStats::from_samples(
                        period_label(&before_samples, BEFORE_WINDOW, "preceding"),
                        &before_samples,
                    ));
                }
                let id: Option<Uuid> = sqlx::query_scalar(
                    "INSERT INTO incidents (endpoint_id, triggered_at, trigger_reason, metric_before, metric_after)
                     VALUES ($1, $2, $3, $4, $5)
                     ON CONFLICT (endpoint_id, trigger_reason) WHERE resolved_at IS NULL DO NOTHING
                     RETURNING id",
                )
                .bind(endpoint_id)
                .bind(now)
                .bind(reason.as_str())
                .bind(sqlx::types::Json(&before))
                .bind(sqlx::types::Json(&after))
                .fetch_optional(&mut *tx)
                .await?;
                evaluation.opened.extend(id);
            }
            (false, Some(incident_id)) => {
                sqlx::query(
                    "UPDATE incidents SET resolved_at = $2 WHERE id = $1 AND resolved_at IS NULL",
                )
                .bind(incident_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                evaluation.resolved.push(incident_id);
            }
            // Still breaching with an incident open, or healthy with none.
            _ => {}
        }
    }

    tx.commit().await?;
    Ok(evaluation)
}

/// Checks in `[end - span, end)`, newest first; widened to the last
/// `MIN_SAMPLES` checks before `end` if the span holds fewer.
async fn window(
    conn: &mut PgConnection,
    endpoint_id: Uuid,
    end: DateTime<Utc>,
    span: Duration,
) -> Result<Vec<CheckSample>, sqlx::Error> {
    let samples: Vec<CheckSample> = sqlx::query_as(
        "SELECT checked_at, latency_ms, success FROM checks
         WHERE endpoint_id = $1 AND checked_at < $2 AND checked_at >= $3
         ORDER BY checked_at DESC LIMIT $4",
    )
    .bind(endpoint_id)
    .bind(end)
    .bind(end - span)
    .bind(WINDOW_ROW_LIMIT)
    .fetch_all(&mut *conn)
    .await?;
    if samples.len() >= MIN_SAMPLES {
        return Ok(samples);
    }

    sqlx::query_as(
        "SELECT checked_at, latency_ms, success FROM checks
         WHERE endpoint_id = $1 AND checked_at < $2
         ORDER BY checked_at DESC LIMIT $3",
    )
    .bind(endpoint_id)
    .bind(end)
    .bind(MIN_SAMPLES as i64)
    .fetch_all(conn)
    .await
}

async fn in_cooldown(
    conn: &mut PgConnection,
    endpoint_id: Uuid,
    reason: TriggerReason,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM incidents
                        WHERE endpoint_id = $1 AND trigger_reason = $2 AND triggered_at > $3)",
    )
    .bind(endpoint_id)
    .bind(reason.as_str())
    .bind(now - REOPEN_COOLDOWN)
    .fetch_one(conn)
    .await
}

/// "last 5 minutes" normally; "last 3 checks" when the window was widened.
fn period_label(samples: &[CheckSample], span: Duration, which: &str) -> String {
    let covered = match (samples.first(), samples.last()) {
        (Some(newest), Some(oldest)) => newest.checked_at - oldest.checked_at,
        _ => Duration::zero(),
    };
    if samples.len() <= MIN_SAMPLES && covered > span {
        format!("{which} {} checks", samples.len())
    } else {
        format!("{which} {} minutes", span.num_minutes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(latency_ms: Option<i32>, success: bool) -> CheckSample {
        CheckSample {
            checked_at: Utc::now(),
            latency_ms,
            success,
        }
    }

    #[test]
    fn stats_average_only_responses_and_round() {
        let samples = [
            sample(Some(100), true),
            sample(Some(201), true),
            sample(None, false),
        ];
        let stats = WindowStats::from_samples("p", &samples);
        assert_eq!(stats.total_checks, 3);
        assert_eq!(stats.failed_checks, 1);
        assert_eq!(stats.avg_latency_ms, Some(151)); // 150.5 rounds up
        assert_eq!(stats.error_rate_percent, 33.3);
    }

    #[test]
    fn stats_of_all_failures_have_no_latency() {
        let stats = WindowStats::from_samples("p", &[sample(None, false), sample(None, false)]);
        assert_eq!(stats.avg_latency_ms, None);
        assert_eq!(stats.error_rate_percent, 100.0);
    }

    #[test]
    fn stats_of_empty_window() {
        let stats = WindowStats::from_samples("p", &[]);
        assert_eq!(
            (
                stats.total_checks,
                stats.avg_latency_ms,
                stats.error_rate_percent
            ),
            (0, None, 0.0)
        );
        assert_eq!(stats.window_start, None);
    }

    fn stats(latencies: &[Option<i32>], failures: usize) -> WindowStats {
        let samples: Vec<_> = latencies
            .iter()
            .enumerate()
            .map(|(i, l)| sample(*l, i >= failures))
            .collect();
        WindowStats::from_samples("p", &samples)
    }

    #[test]
    fn latency_breach_is_strictly_above_threshold() {
        let at = stats(&[Some(2000), Some(2000), Some(2000)], 0);
        assert!(breached_thresholds(&at, 2000, 5.0).is_empty());
        let above = stats(&[Some(2000), Some(2000), Some(2003)], 0);
        assert_eq!(
            breached_thresholds(&above, 2000, 5.0),
            [TriggerReason::LatencyThresholdExceeded]
        );
    }

    #[test]
    fn error_rate_breach_uses_exact_rate() {
        // 1 of 20 = exactly 5% → not above a 5% threshold.
        let five = stats(&[Some(100); 20], 1);
        assert!(breached_thresholds(&five, 2000, 5.0).is_empty());
        // 1 of 19 = 5.26% → above.
        let above = stats(&[Some(100); 19], 1);
        assert_eq!(
            breached_thresholds(&above, 2000, 5.0),
            [TriggerReason::ErrorRateThresholdExceeded]
        );
        // 0% threshold: any failure counts.
        let one = stats(&[Some(100); 30], 1);
        assert_eq!(
            breached_thresholds(&one, 2000, 0.0),
            [TriggerReason::ErrorRateThresholdExceeded]
        );
    }

    #[test]
    fn both_reasons_can_breach_together() {
        let bad = stats(&[None, Some(5000), Some(5000)], 1);
        assert_eq!(
            breached_thresholds(&bad, 2000, 5.0),
            [
                TriggerReason::LatencyThresholdExceeded,
                TriggerReason::ErrorRateThresholdExceeded
            ]
        );
    }

    #[test]
    fn too_few_samples_never_breach() {
        let two_bad = stats(&[None, None], 2);
        assert!(breached_thresholds(&two_bad, 2000, 5.0).is_empty());
    }

    #[test]
    fn all_timeouts_breach_error_rate_not_latency() {
        let timeouts = stats(&[None, None, None], 3);
        assert_eq!(
            breached_thresholds(&timeouts, 1, 5.0),
            [TriggerReason::ErrorRateThresholdExceeded]
        );
    }
}
