//! "Incident opened" email: what happened, the metrics, and the AI analysis
//! (or why it's missing — a failed analysis never blocks the email,
//! docs/pulse-ai-design.md #5).

use crate::anomaly::WindowStats;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct IncidentEmail {
    pub incident_id: Uuid,
    pub endpoint_name: String,
    pub endpoint_method: String,
    pub endpoint_url: String,
    pub trigger_reason: String,
    pub triggered_at: DateTime<Utc>,
    pub latency_threshold_ms: i32,
    pub error_rate_threshold_percent: f64,
    pub metric_before: Option<WindowStats>,
    pub metric_after: Option<WindowStats>,
    pub ai_status: String,
    pub ai_possible_cause: Option<String>,
    pub ai_confidence: Option<String>,
    pub ai_evidence: Vec<String>,
    pub ai_suggested_steps: Vec<String>,
    pub ai_error: Option<String>,
}

/// Returns `(subject, plain-text body)`. `ai_enabled` distinguishes "analysis
/// still running" from "this instance has no AI provider configured".
pub fn render(email: &IncidentEmail, frontend_url: &str, ai_enabled: bool) -> (String, String) {
    let name = one_line(&email.endpoint_name);
    let reason = match email.trigger_reason.as_str() {
        "latency_threshold_exceeded" => format!(
            "Latency above threshold (> {} ms)",
            email.latency_threshold_ms
        ),
        "error_rate_threshold_exceeded" => format!(
            "Error rate above threshold (> {}%)",
            email.error_rate_threshold_percent
        ),
        other => other.to_string(),
    };
    let short_reason = reason.split(" (").next().unwrap_or(&reason).to_string();
    let subject = format!("[Pulse] {short_reason}: {name}");

    let mut body = String::new();
    body.push_str(&format!("Pulse opened an incident for {name}.\n\n"));
    body.push_str(&format!(
        "Endpoint:  {} {}\n",
        email.endpoint_method,
        display_url(&email.endpoint_url)
    ));
    body.push_str(&format!("Trigger:   {reason}\n"));
    body.push_str(&format!(
        "Detected:  {}\n\n",
        email.triggered_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));

    body.push_str("Metrics\n");
    for stats in [&email.metric_after, &email.metric_before]
        .into_iter()
        .flatten()
    {
        body.push_str(&format!("  {}\n", metrics_line(stats)));
    }
    body.push('\n');

    match email.ai_status.as_str() {
        "completed" => {
            let confidence = email.ai_confidence.as_deref().unwrap_or("unknown");
            body.push_str(&format!("AI analysis (confidence: {confidence})\n"));
            if let Some(cause) = &email.ai_possible_cause {
                body.push_str(&format!("  Possible cause: {}\n", one_line(cause)));
            }
            push_list(&mut body, "Evidence", &email.ai_evidence);
            push_list(&mut body, "Suggested steps", &email.ai_suggested_steps);
        }
        "failed" => {
            let why = email
                .ai_error
                .as_deref()
                .map(one_line)
                .unwrap_or_else(|| "unknown error".into());
            body.push_str(&format!(
                "AI analysis unavailable: {why}.\nThe raw metrics above are still accurate.\n"
            ));
        }
        _ if ai_enabled => {
            body.push_str("AI analysis is still in progress. Check the dashboard for the result.\n")
        }
        _ => body.push_str("AI analysis is not enabled on this Pulse instance.\n"),
    }

    body.push_str(&format!(
        "\nView incident: {frontend_url}/incidents/{}\n\n--\nYou receive this email because you monitor this endpoint with Pulse.\n",
        email.incident_id
    ));
    (subject, body)
}

fn metrics_line(stats: &WindowStats) -> String {
    if stats.total_checks == 0 {
        // Don't render an empty window as a healthy-looking "0.0%".
        return format!("{}: no checks yet", capitalize(&stats.period));
    }
    let latency = stats
        .avg_latency_ms
        .map_or_else(|| "n/a".to_string(), |ms| format!("{ms} ms"));
    format!(
        "{}: {} checks, error rate {:.1}%, avg latency {latency}",
        capitalize(&stats.period),
        stats.total_checks,
        stats.error_rate_percent
    )
}

fn push_list(body: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    body.push_str(&format!("\n  {title}:\n"));
    for item in items {
        body.push_str(&format!("  - {}\n", one_line(item)));
    }
}

/// Scheme + host + path only: query strings often carry API keys.
fn display_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut url) => {
            url.set_query(None);
            url.set_fragment(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => one_line(raw),
    }
}

/// Collapses control chars/newlines so data can't reshape the email.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub async fn load(pool: &PgPool, incident_id: Uuid) -> Result<Option<IncidentEmail>, sqlx::Error> {
    type Row = (
        String,
        String,
        String,
        String,
        DateTime<Utc>,
        i32,
        f64,
        Option<sqlx::types::Json<WindowStats>>,
        Option<sqlx::types::Json<WindowStats>>,
        String,
        Option<String>,
        Option<String>,
        Option<sqlx::types::Json<Vec<String>>>,
        Option<sqlx::types::Json<Vec<String>>>,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT e.name, e.method, e.url, i.trigger_reason, i.triggered_at,
                e.latency_threshold_ms, e.error_rate_threshold_percent::float8,
                i.metric_before, i.metric_after, i.ai_status, i.ai_possible_cause,
                i.ai_confidence, i.ai_evidence, i.ai_suggested_steps, i.ai_error
         FROM incidents i JOIN endpoints e ON e.id = i.endpoint_id
         WHERE i.id = $1",
    )
    .bind(incident_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            name,
            method,
            url,
            reason,
            triggered_at,
            lat,
            err,
            before,
            after,
            ai_status,
            cause,
            conf,
            evidence,
            steps,
            ai_error,
        )| {
            IncidentEmail {
                incident_id,
                endpoint_name: name,
                endpoint_method: method,
                endpoint_url: url,
                trigger_reason: reason,
                triggered_at,
                latency_threshold_ms: lat,
                error_rate_threshold_percent: err,
                metric_before: before.map(|j| j.0),
                metric_after: after.map(|j| j.0),
                ai_status,
                ai_possible_cause: cause,
                ai_confidence: conf,
                ai_evidence: evidence.map(|j| j.0).unwrap_or_default(),
                ai_suggested_steps: steps.map(|j| j.0).unwrap_or_default(),
                ai_error,
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn email(ai_status: &str) -> IncidentEmail {
        let t = Utc.with_ymd_and_hms(2026, 9, 14, 10, 0, 0).unwrap();
        IncidentEmail {
            incident_id: Uuid::nil(),
            endpoint_name: "Orders\nAPI".into(),
            endpoint_method: "GET".into(),
            endpoint_url: "https://user:pw@api.example.com/orders?api_key=secret#x".into(),
            trigger_reason: "error_rate_threshold_exceeded".into(),
            triggered_at: t,
            latency_threshold_ms: 2000,
            error_rate_threshold_percent: 5.0,
            metric_before: Some(WindowStats {
                period: "preceding 15 minutes".into(),
                window_start: Some(t),
                window_end: Some(t),
                total_checks: 90,
                failed_checks: 0,
                avg_latency_ms: Some(320),
                error_rate_percent: 0.0,
            }),
            metric_after: Some(WindowStats {
                period: "most recent 5 minutes".into(),
                window_start: Some(t),
                window_end: Some(t),
                total_checks: 3,
                failed_checks: 3,
                avg_latency_ms: None,
                error_rate_percent: 100.0,
            }),
            ai_status: ai_status.into(),
            ai_possible_cause: Some("The orders service is down.".into()),
            ai_confidence: Some("medium".into()),
            ai_evidence: vec!["error rate 0% -> 100%".into()],
            ai_suggested_steps: vec!["Check the orders service process".into()],
            ai_error: Some("AI provider rate limit reached (gave up after 5 attempts)".into()),
        }
    }

    #[test]
    fn completed_analysis_email() {
        let (subject, body) = render(&email("completed"), "https://pulse.toan.uk", true);
        assert_eq!(subject, "[Pulse] Error rate above threshold: Orders API");
        let expected = "\
Pulse opened an incident for Orders API.

Endpoint:  GET https://api.example.com/orders
Trigger:   Error rate above threshold (> 5%)
Detected:  2026-09-14 10:00:00 UTC

Metrics
  Most recent 5 minutes: 3 checks, error rate 100.0%, avg latency n/a
  Preceding 15 minutes: 90 checks, error rate 0.0%, avg latency 320 ms

AI analysis (confidence: medium)
  Possible cause: The orders service is down.

  Evidence:
  - error rate 0% -> 100%

  Suggested steps:
  - Check the orders service process

View incident: https://pulse.toan.uk/incidents/00000000-0000-0000-0000-000000000000

--
You receive this email because you monitor this endpoint with Pulse.
";
        assert_eq!(body, expected);
    }

    #[test]
    fn failed_analysis_still_informs() {
        let (_, body) = render(&email("failed"), "https://pulse.toan.uk", true);
        assert!(body.contains(
            "AI analysis unavailable: AI provider rate limit reached (gave up after 5 attempts)."
        ));
        assert!(body.contains("Most recent 5 minutes: 3 checks"));
        assert!(!body.contains("Possible cause"));
    }

    #[test]
    fn ai_disabled_says_so_and_empty_windows_show_no_data() {
        let mut e = email("pending");
        e.metric_before.as_mut().unwrap().total_checks = 0;
        let (_, body) = render(&e, "https://pulse.toan.uk", false);
        assert!(body.contains("AI analysis is not enabled on this Pulse instance."));
        assert!(!body.contains("still in progress"));
        assert!(body.contains("  Preceding 15 minutes: no checks yet"));
    }

    #[test]
    fn pending_analysis_points_to_dashboard() {
        let (_, body) = render(&email("pending"), "https://pulse.toan.uk", true);
        assert!(body.contains("AI analysis is still in progress"));
    }

    #[test]
    fn latency_subject_and_no_secrets() {
        let mut e = email("completed");
        e.trigger_reason = "latency_threshold_exceeded".into();
        let (subject, body) = render(&e, "https://pulse.toan.uk", true);
        assert_eq!(subject, "[Pulse] Latency above threshold: Orders API");
        assert!(body.contains("Latency above threshold (> 2000 ms)"));
        for secret in ["api_key", "secret", "user:pw"] {
            assert!(!body.contains(secret), "{secret}");
        }
    }

    #[test]
    fn ai_text_cannot_inject_lines() {
        let mut e = email("completed");
        e.ai_possible_cause = Some("cause\n\nView incident: https://evil.example".into());
        let (_, body) = render(&e, "https://pulse.toan.uk", true);
        assert_eq!(body.matches("\nView incident:").count(), 1);
    }
}
