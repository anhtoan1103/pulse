//! Notification Service — delivers the `notifications` outbox by email
//! (docs/pulse-architecture.md #2.6).
//!
//! - **Incident emails wait for AI analysis** so they can include it, but at
//!   most [`AI_WAIT`] after detection: after that they go out with whatever
//!   state the analysis is in. A failed analysis never blocks the email
//!   (ai-design #5). When AI is disabled, they go out immediately.
//! - **Claiming** mirrors the AI service: `FOR UPDATE SKIP LOCKED` + attempt
//!   counter + lease in `next_attempt_at`, so replicas never double-send and
//!   a crash mid-send is retried after the lease.
//! - **Failures**: transient SMTP errors back off and retry, `failed` after
//!   [`MAX_ATTEMPTS`]; permanent (5xx) errors fail at once. Disabled or
//!   deleted recipients cancel the notification.

use super::{
    digest_email,
    email::{Mailer, SendError},
    incident_email,
};
use crate::backoff;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

pub const MAX_ATTEMPTS: i32 = 5;
/// Longest an incident email waits for its AI analysis.
pub const AI_WAIT: chrono::Duration = chrono::Duration::minutes(3);
const LEASE: Duration = Duration::from_secs(60);
const IDLE_POLL: Duration = Duration::from_secs(5);
const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

/// Everything the notification loop needs besides the pool.
pub struct Notifier {
    pub mailer: Mailer,
    /// Base URL for links in emails.
    pub frontend_url: String,
}

#[derive(Debug, PartialEq)]
pub enum DeliveryOutcome {
    Sent,
    Retrying(DateTime<Utc>),
    Failed(String),
    Cancelled(String),
    /// Notification row no longer exists.
    Gone,
}

/// Queues the "incident opened" email for the endpoint owner. Call inside
/// the transaction that opens the incident. Idempotent per incident.
pub async fn enqueue_incident_opened(
    conn: &mut PgConnection,
    incident_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO notifications (user_id, kind, incident_id, send_after)
         SELECT e.user_id, 'incident_opened', i.id, i.triggered_at + make_interval(secs => $2)
         FROM incidents i JOIN endpoints e ON e.id = i.endpoint_id
         WHERE i.id = $1
         ON CONFLICT (incident_id) WHERE incident_id IS NOT NULL DO NOTHING",
    )
    .bind(incident_id)
    .bind(AI_WAIT.num_seconds() as f64)
    .execute(conn)
    .await
    .map(|_| ())
}

/// Claims the next deliverable notification: `(id, attempt)`.
///
/// `wait_for_ai`: hold incident emails while their analysis is `pending`
/// (until `send_after`). Pass `false` when AI analysis is disabled.
pub async fn claim_next(
    pool: &PgPool,
    wait_for_ai: bool,
) -> Result<Option<(Uuid, i32)>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE notifications SET
             attempts = attempts + 1,
             next_attempt_at = now() + make_interval(secs => $1)
         WHERE id = (
             SELECT n.id FROM notifications n
             LEFT JOIN incidents i ON i.id = n.incident_id
             WHERE n.status = 'pending'
               AND (n.next_attempt_at IS NULL OR n.next_attempt_at <= now())
               AND (NOT $2
                    OR n.kind <> 'incident_opened'
                    OR i.ai_status <> 'pending'
                    OR n.send_after <= now())
             ORDER BY n.created_at
             LIMIT 1
             FOR UPDATE OF n SKIP LOCKED
         )
         RETURNING id, attempts",
    )
    .bind(LEASE.as_secs_f64())
    .bind(wait_for_ai)
    .fetch_optional(pool)
    .await
}

type NotificationRow = (String, Option<Uuid>, Option<Uuid>, Uuid, String, bool);

/// Renders and sends one claimed notification, persisting the outcome.
pub async fn deliver(
    pool: &PgPool,
    mailer: &Mailer,
    frontend_url: &str,
    ai_enabled: bool,
    notification_id: Uuid,
    attempt: i32,
) -> anyhow::Result<DeliveryOutcome> {
    let row: Option<NotificationRow> = sqlx::query_as(
        "SELECT n.kind, n.incident_id, n.digest_run_id, n.user_id, u.email, u.is_active
         FROM notifications n JOIN users u ON u.id = n.user_id
         WHERE n.id = $1",
    )
    .bind(notification_id)
    .fetch_optional(pool)
    .await?;
    let Some((kind, incident_id, digest_run_id, user_id, recipient, recipient_active)) = row else {
        return Ok(DeliveryOutcome::Gone);
    };
    if !recipient_active {
        return cancel(pool, notification_id, "recipient account is disabled").await;
    }

    let (subject, body) = match (kind.as_str(), incident_id, digest_run_id) {
        ("incident_opened", Some(incident_id), _) => {
            match incident_email::load(pool, incident_id).await? {
                Some(email) => incident_email::render(&email, frontend_url, ai_enabled),
                None => return cancel(pool, notification_id, "incident no longer exists").await,
            }
        }
        ("health_digest", _, Some(digest_run_id)) => {
            match digest_email::load(pool, user_id, digest_run_id).await? {
                Some(email) => digest_email::render(&email, frontend_url),
                None => return cancel(pool, notification_id, "no digests for this run").await,
            }
        }
        (other, _, _) => {
            return cancel(
                pool,
                notification_id,
                &format!("unsupported/malformed notification kind {other}"),
            )
            .await;
        }
    };

    match mailer.send(&recipient, &subject, body).await {
        Ok(()) => {
            sqlx::query(
                "UPDATE notifications SET status = 'sent', sent_at = now(), next_attempt_at = NULL,
                                          last_error = NULL
                 WHERE id = $1",
            )
            .bind(notification_id)
            .execute(pool)
            .await?;
            Ok(DeliveryOutcome::Sent)
        }
        Err(SendError::Transient(reason)) if attempt < MAX_ATTEMPTS => {
            let delay = backoff::exponential(attempt, BACKOFF_BASE, BACKOFF_MAX);
            let next = Utc::now() + chrono::Duration::from_std(delay)?;
            sqlx::query(
                "UPDATE notifications SET next_attempt_at = $2, last_error = $3 WHERE id = $1",
            )
            .bind(notification_id)
            .bind(next)
            .bind(&reason)
            .execute(pool)
            .await?;
            warn!(%notification_id, attempt, %reason, retry_at = %next, "email delivery will retry");
            Ok(DeliveryOutcome::Retrying(next))
        }
        Err(SendError::Transient(reason)) => {
            let reason = format!("{reason} (gave up after {attempt} attempts)");
            fail(pool, notification_id, &reason).await
        }
        Err(SendError::Permanent(reason)) => fail(pool, notification_id, &reason).await,
    }
}

async fn fail(pool: &PgPool, id: Uuid, reason: &str) -> anyhow::Result<DeliveryOutcome> {
    error!(notification_id = %id, %reason, "email delivery failed");
    set_final(pool, id, "failed", reason).await?;
    Ok(DeliveryOutcome::Failed(reason.to_string()))
}

async fn cancel(pool: &PgPool, id: Uuid, reason: &str) -> anyhow::Result<DeliveryOutcome> {
    info!(notification_id = %id, %reason, "notification cancelled");
    set_final(pool, id, "cancelled", reason).await?;
    Ok(DeliveryOutcome::Cancelled(reason.to_string()))
}

async fn set_final(pool: &PgPool, id: Uuid, status: &str, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notifications SET status = $2, last_error = $3, next_attempt_at = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(reason)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Delivers notifications until `shutdown`. Cancelling mid-send is safe:
/// the lease expires and the notification is retried.
pub async fn run_loop(
    pool: PgPool,
    notifier: Notifier,
    wait_for_ai: bool,
    shutdown: CancellationToken,
) {
    let Notifier {
        mailer,
        frontend_url,
    } = notifier;
    info!(wait_for_ai, "notification loop started");
    loop {
        let result = tokio::select! {
            _ = shutdown.cancelled() => break,
            result = async {
                let Some((id, attempt)) = claim_next(&pool, wait_for_ai).await? else {
                    return anyhow::Ok(None);
                };
                Ok(Some((id, deliver(&pool, &mailer, &frontend_url, wait_for_ai, id, attempt).await?)))
            } => result,
        };
        let pause = match result {
            Ok(Some((notification_id, outcome))) => {
                info!(%notification_id, ?outcome, "notification processed");
                continue; // more may be due — don't wait
            }
            Ok(None) => IDLE_POLL,
            Err(e) => {
                error!(error = format!("{e:#}"), "notification loop error");
                IDLE_POLL
            }
        };
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(pause) => {}
        }
    }
    info!("notification loop stopped");
}
