//! Daily health digest email: one message per user bundling every endpoint's
//! digest for the period (docs/pulse-architecture.md #2.6). No AI involved.

use crate::notify::one_line;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct EndpointDigest {
    pub name: String,
    pub status: String,
    pub total_checks: i32,
    pub success_count: i32,
    pub avg_latency_ms: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct DigestEmail {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    /// Alphabetical by endpoint name.
    pub endpoints: Vec<EndpointDigest>,
}

/// Returns `(subject, plain-text body)`.
pub fn render(email: &DigestEmail, frontend_url: &str) -> (String, String) {
    let degraded = email
        .endpoints
        .iter()
        .filter(|e| e.status == "degraded")
        .count();
    let healthy = email.endpoints.len() - degraded;

    let subject = if degraded == 0 {
        format!("[Pulse] Daily health digest — all {healthy} endpoints healthy")
    } else {
        format!("[Pulse] Daily health digest — {degraded} degraded, {healthy} healthy")
    };

    let mut body = String::new();
    body.push_str(&format!(
        "Health digest for {} → {} UTC\n\n",
        email.period_start.format("%Y-%m-%d %H:%M"),
        email.period_end.format("%Y-%m-%d %H:%M")
    ));
    body.push_str(&format!(
        "{} of {} endpoints healthy.\n\n",
        healthy,
        email.endpoints.len()
    ));

    for e in &email.endpoints {
        let success_rate = if e.total_checks > 0 {
            e.success_count as f64 * 100.0 / e.total_checks as f64
        } else {
            0.0
        };
        let latency = e
            .avg_latency_ms
            .map_or_else(|| "n/a".to_string(), |ms| format!("{ms} ms"));
        let marker = if e.status == "degraded" {
            "DEGRADED"
        } else {
            "healthy"
        };
        body.push_str(&format!(
            "  [{marker}] {}: {} checks, {success_rate:.1}% success, avg latency {latency}\n",
            one_line(&e.name),
            e.total_checks
        ));
    }

    body.push_str(&format!(
        "\nView dashboard: {frontend_url}/dashboard\n\n--\nYou receive this email because you monitor these endpoints with Pulse.\n"
    ));
    (subject, body)
}

/// `None` if the user has no digests for this run (shouldn't happen if a
/// notification was enqueued — but endpoints/digests could have been deleted
/// since).
pub async fn load(
    pool: &PgPool,
    user_id: Uuid,
    digest_run_id: Uuid,
) -> Result<Option<DigestEmail>, sqlx::Error> {
    let period: Option<(DateTime<Utc>, DateTime<Utc>)> =
        sqlx::query_as("SELECT period_start, period_end FROM digest_runs WHERE id = $1")
            .bind(digest_run_id)
            .fetch_optional(pool)
            .await?;
    let Some((period_start, period_end)) = period else {
        return Ok(None);
    };

    let rows: Vec<(String, String, i32, i32, Option<i32>)> = sqlx::query_as(
        "SELECT e.name, hd.status, hd.total_checks, hd.success_count, hd.avg_latency_ms
         FROM health_digests hd JOIN endpoints e ON e.id = hd.endpoint_id
         WHERE e.user_id = $1 AND hd.period_start = $2
         ORDER BY e.name",
    )
    .bind(user_id)
    .bind(period_start)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DigestEmail {
        period_start,
        period_end,
        endpoints: rows
            .into_iter()
            .map(
                |(name, status, total_checks, success_count, avg_latency_ms)| EndpointDigest {
                    name,
                    status,
                    total_checks,
                    success_count,
                    avg_latency_ms,
                },
            )
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn email(endpoints: Vec<EndpointDigest>) -> DigestEmail {
        DigestEmail {
            period_start: Utc.with_ymd_and_hms(2026, 9, 13, 8, 0, 0).unwrap(),
            period_end: Utc.with_ymd_and_hms(2026, 9, 14, 8, 0, 0).unwrap(),
            endpoints,
        }
    }

    fn digest(
        name: &str,
        status: &str,
        total: i32,
        success: i32,
        latency: Option<i32>,
    ) -> EndpointDigest {
        EndpointDigest {
            name: name.into(),
            status: status.into(),
            total_checks: total,
            success_count: success,
            avg_latency_ms: latency,
        }
    }

    #[test]
    fn all_healthy_subject_and_body() {
        let e = email(vec![
            digest("Orders API", "healthy", 144, 144, Some(210)),
            digest("Status page", "healthy", 144, 144, Some(80)),
        ]);
        let (subject, body) = render(&e, "https://pulse.test");
        assert_eq!(
            subject,
            "[Pulse] Daily health digest — all 2 endpoints healthy"
        );
        assert!(body.contains("2 of 2 endpoints healthy."));
        assert!(
            body.contains("[healthy] Orders API: 144 checks, 100.0% success, avg latency 210 ms")
        );
        assert!(body.contains("View dashboard: https://pulse.test/dashboard"));
    }

    #[test]
    fn degraded_endpoint_is_flagged_and_counted() {
        let e = email(vec![
            digest("Orders API", "degraded", 144, 100, Some(3200)),
            digest("Status page", "healthy", 144, 144, Some(80)),
        ]);
        let (subject, body) = render(&e, "https://pulse.test");
        assert_eq!(
            subject,
            "[Pulse] Daily health digest — 1 degraded, 1 healthy"
        );
        assert!(body.contains("1 of 2 endpoints healthy."));
        assert!(
            body.contains("[DEGRADED] Orders API: 144 checks, 69.4% success, avg latency 3200 ms")
        );
    }

    #[test]
    fn missing_latency_shown_as_na() {
        let e = email(vec![digest("Down API", "degraded", 10, 0, None)]);
        let (_, body) = render(&e, "https://pulse.test");
        assert!(body.contains("avg latency n/a"));
    }

    #[test]
    fn endpoint_name_cannot_inject_lines() {
        let e = email(vec![digest(
            "Evil\n\nView dashboard: https://evil.example",
            "healthy",
            1,
            1,
            Some(1),
        )]);
        let (_, body) = render(&e, "https://pulse.test");
        // The injected text is neutered to a single line (still visible as
        // literal text in the endpoint's bullet), so it can't masquerade as
        // its own line — only the real footer line starts with this prefix.
        let real_footer_lines = body
            .lines()
            .filter(|l| l.starts_with("View dashboard:"))
            .count();
        assert_eq!(real_footer_lines, 1);
    }
}
