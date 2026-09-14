//! Notification Service tests against a fake SMTP server
//! (docs/pulse-architecture.md #2.6, docs/pulse-ai-design.md #5).

use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use pulse_backend::{
    anomaly::evaluate_endpoint,
    config::{EmailConfig, SmtpTls},
    db::MIGRATOR,
    notify::{
        email::Mailer,
        service::{
            DeliveryOutcome, MAX_ATTEMPTS, Notifier, claim_next, deliver, enqueue_incident_opened,
            run_loop,
        },
    },
};
use sqlx::PgPool;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const FRONTEND: &str = "https://pulse.test";

// ---------- fake SMTP server ----------

#[derive(Debug, Clone)]
struct Captured {
    from: String,
    to: Vec<String>,
    subject: String,
    body: String,
}

#[derive(Default)]
struct FakeSmtp {
    messages: Mutex<Vec<Captured>>,
    /// Scripted replies to RCPT TO, one per message; default "250 OK".
    rcpt_replies: Mutex<VecDeque<&'static str>>,
}

impl FakeSmtp {
    fn messages(&self) -> Vec<Captured> {
        self.messages.lock().unwrap().clone()
    }
}

async fn start_smtp(rcpt_replies: Vec<&'static str>) -> (Mailer, Arc<FakeSmtp>) {
    let state = Arc::new(FakeSmtp {
        rcpt_replies: Mutex::new(rcpt_replies.into()),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_state = state.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(smtp_session(stream, server_state.clone()));
        }
    });
    (mailer_for_port(port), state)
}

fn mailer_for_port(port: u16) -> Mailer {
    Mailer::new(&EmailConfig {
        host: "127.0.0.1".into(),
        port,
        tls: SmtpTls::None,
        credentials: None,
        from: "Pulse <noreply@pulse.test>".into(),
        frontend_url: FRONTEND.into(),
    })
    .unwrap()
}

async fn smtp_session(stream: TcpStream, state: Arc<FakeSmtp>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let _ = write.write_all(b"220 fake.smtp ESMTP\r\n").await;
    let (mut from, mut to) = (String::new(), Vec::new());

    while let Ok(Some(line)) = lines.next_line().await {
        let upper = line.to_ascii_uppercase();
        let reply: String = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            "250 fake.smtp\r\n".into()
        } else if upper.starts_with("MAIL FROM:") {
            from = line[10..].trim().to_string();
            "250 OK\r\n".into()
        } else if upper.starts_with("RCPT TO:") {
            let scripted = state
                .rcpt_replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or("250 OK");
            if scripted.starts_with('2') {
                to.push(line[8..].trim().to_string());
            }
            format!("{scripted}\r\n")
        } else if upper == "DATA" {
            let _ = write
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await;
            let mut data = Vec::new();
            while let Ok(Some(l)) = lines.next_line().await {
                if l == "." {
                    break;
                }
                data.push(l.strip_prefix('.').map(String::from).unwrap_or(l));
            }
            let (subject, body) = parse_message(&data);
            state.messages.lock().unwrap().push(Captured {
                from: std::mem::take(&mut from),
                to: std::mem::take(&mut to),
                subject,
                body,
            });
            "250 queued\r\n".into()
        } else if upper == "QUIT" {
            let _ = write.write_all(b"221 bye\r\n").await;
            break;
        } else if upper == "RSET" || upper == "NOOP" {
            "250 OK\r\n".into()
        } else {
            "502 not implemented\r\n".into()
        };
        if write.write_all(reply.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// Extracts Subject and the decoded body from raw message lines.
fn parse_message(lines: &[String]) -> (String, String) {
    let split = lines
        .iter()
        .position(|l| l.is_empty())
        .unwrap_or(lines.len());
    let (headers, body) = (&lines[..split], &lines[(split + 1).min(lines.len())..]);
    let header = |name: &str| {
        headers
            .iter()
            .find(|h| h.to_ascii_lowercase().starts_with(&format!("{name}:")))
            .map(|h| h[name.len() + 1..].trim().to_string())
            .unwrap_or_default()
    };
    let body = match header("content-transfer-encoding")
        .to_ascii_lowercase()
        .as_str()
    {
        "base64" => String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(body.concat())
                .unwrap(),
        )
        .unwrap(),
        "quoted-printable" => decode_quoted_printable(&body.join("\r\n")),
        _ => body.join("\n"),
    };
    (header("subject"), body.replace("\r\n", "\n"))
}

fn decode_quoted_printable(text: &str) -> String {
    let joined = text.replace("=\r\n", "");
    let bytes = joined.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'='
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&joined[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

// ---------- fixtures ----------

/// Opens an incident through the real detector (which enqueues the email).
/// Returns `(incident_id, owner_email)`.
async fn open_incident(pool: &PgPool) -> (Uuid, String) {
    let email = format!("owner-{}@example.com", &Uuid::new_v4().to_string()[..8]);
    let user: Uuid = sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(&email)
        .fetch_one(pool)
        .await
        .unwrap();
    let endpoint: Uuid = sqlx::query_scalar(
        "INSERT INTO endpoints (user_id, name, url, check_interval_seconds, latency_threshold_ms, error_rate_threshold_percent)
         VALUES ($1, 'Orders API', 'https://api.example.com/orders?api_key=topsecret', 10, 2000, 5) RETURNING id",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap();
    for ago in [30, 20, 10] {
        sqlx::query(
            "INSERT INTO checks (endpoint_id, checked_at, success, error_message)
             VALUES ($1, $2, false, 'timeout after 10s')",
        )
        .bind(endpoint)
        .bind(Utc::now() - ChronoDuration::seconds(ago))
        .execute(pool)
        .await
        .unwrap();
    }
    let incident = evaluate_endpoint(pool, endpoint).await.unwrap().opened[0];
    (incident, email)
}

async fn complete_analysis(pool: &PgPool, incident: Uuid) {
    sqlx::query(
        r#"UPDATE incidents SET ai_status = 'completed', ai_confidence = 'high',
               ai_possible_cause = 'The orders service is down.',
               ai_evidence = '["3 of 3 checks timed out"]', ai_suggested_steps = '["Restart the orders service"]'
           WHERE id = $1"#,
    )
    .bind(incident)
    .execute(pool)
    .await
    .unwrap();
}

type NotificationRow = (String, i32, Option<String>, Option<chrono::DateTime<Utc>>);

async fn notification_for(pool: &PgPool, incident: Uuid) -> NotificationRow {
    sqlx::query_as(
        "SELECT status, attempts, last_error, sent_at FROM notifications WHERE incident_id = $1",
    )
    .bind(incident)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn claim_and_deliver(pool: &PgPool, mailer: &Mailer, wait_for_ai: bool) -> DeliveryOutcome {
    let (id, attempt) = claim_next(pool, wait_for_ai)
        .await
        .unwrap()
        .expect("a deliverable notification");
    deliver(pool, mailer, FRONTEND, wait_for_ai, id, attempt)
        .await
        .unwrap()
}

// ---------- enqueueing ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn opening_an_incident_enqueues_one_email(pool: PgPool) {
    let (incident, _) = open_incident(&pool).await;
    let (status, attempts, ..) = notification_for(&pool, incident).await;
    assert_eq!((status.as_str(), attempts), ("pending", 0));

    // Enqueueing again is a no-op.
    let mut conn = pool.acquire().await.unwrap();
    enqueue_incident_opened(&mut conn, incident).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

// ---------- waiting for AI ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn email_waits_for_ai_then_includes_analysis(pool: PgPool) {
    let (incident, owner) = open_incident(&pool).await;
    let (mailer, smtp) = start_smtp(vec![]).await;

    assert!(
        claim_next(&pool, true).await.unwrap().is_none(),
        "held while AI is pending"
    );

    complete_analysis(&pool, incident).await;
    assert_eq!(
        claim_and_deliver(&pool, &mailer, true).await,
        DeliveryOutcome::Sent
    );

    let messages = smtp.messages();
    assert_eq!(messages.len(), 1);
    let m = &messages[0];
    assert_eq!(m.from, "<noreply@pulse.test>");
    assert_eq!(m.to, [format!("<{owner}>")]);
    assert_eq!(m.subject, "[Pulse] Error rate above threshold: Orders API");
    assert!(
        m.body.contains("AI analysis (confidence: high)"),
        "{}",
        m.body
    );
    assert!(
        m.body
            .contains("Possible cause: The orders service is down.")
    );
    assert!(m.body.contains("- Restart the orders service"));
    assert!(
        m.body
            .contains(&format!("View incident: {FRONTEND}/incidents/{incident}"))
    );
    assert!(
        !m.body.contains("topsecret"),
        "query string must not be emailed"
    );

    let (status, attempts, error, sent_at) = notification_for(&pool, incident).await;
    assert_eq!((status.as_str(), attempts, error), ("sent", 1, None));
    assert!(sent_at.is_some());
    assert!(
        claim_next(&pool, true).await.unwrap().is_none(),
        "never sent twice"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn email_goes_out_without_analysis_after_the_wait(pool: PgPool) {
    let (incident, _) = open_incident(&pool).await;
    let (mailer, smtp) = start_smtp(vec![]).await;
    sqlx::query("UPDATE notifications SET send_after = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        claim_and_deliver(&pool, &mailer, true).await,
        DeliveryOutcome::Sent
    );
    assert!(
        smtp.messages()[0]
            .body
            .contains("AI analysis is still in progress")
    );
    assert_eq!(notification_for(&pool, incident).await.0, "sent");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_analysis_does_not_block_email(pool: PgPool) {
    let (incident, _) = open_incident(&pool).await;
    let (mailer, smtp) = start_smtp(vec![]).await;
    sqlx::query("UPDATE incidents SET ai_status = 'failed', ai_error = 'AI provider rate limit reached' WHERE id = $1")
        .bind(incident)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        claim_and_deliver(&pool, &mailer, true).await,
        DeliveryOutcome::Sent
    );
    assert!(
        smtp.messages()[0]
            .body
            .contains("AI analysis unavailable: AI provider rate limit reached.")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn without_ai_emails_go_out_immediately(pool: PgPool) {
    open_incident(&pool).await;
    let (mailer, smtp) = start_smtp(vec![]).await;
    assert_eq!(
        claim_and_deliver(&pool, &mailer, false).await,
        DeliveryOutcome::Sent
    );
    assert_eq!(smtp.messages().len(), 1);
    assert!(
        smtp.messages()[0]
            .body
            .contains("AI analysis is not enabled")
    );
}

// ---------- failures ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn transient_smtp_errors_retry_then_give_up(pool: PgPool) {
    let (incident, _) = open_incident(&pool).await;
    let (mailer, smtp) = start_smtp(vec!["451 try again later", "451 try again later"]).await;

    let outcome = claim_and_deliver(&pool, &mailer, false).await;
    let DeliveryOutcome::Retrying(next) = outcome else {
        panic!("expected retry, got {outcome:?}");
    };
    let delay = next - Utc::now();
    assert!(
        delay > ChronoDuration::seconds(55) && delay <= ChronoDuration::seconds(60),
        "{delay}"
    );
    let (status, attempts, error, _) = notification_for(&pool, incident).await;
    assert_eq!((status.as_str(), attempts), ("pending", 1));
    assert!(error.unwrap().starts_with("SMTP delivery failed"));
    assert!(
        claim_next(&pool, false).await.unwrap().is_none(),
        "not due yet"
    );

    sqlx::query("UPDATE notifications SET attempts = $1, next_attempt_at = now()")
        .bind(MAX_ATTEMPTS - 1)
        .execute(&pool)
        .await
        .unwrap();
    let outcome = claim_and_deliver(&pool, &mailer, false).await;
    assert!(
        matches!(&outcome, DeliveryOutcome::Failed(r) if r.ends_with(&format!("(gave up after {MAX_ATTEMPTS} attempts)"))),
        "{outcome:?}"
    );
    assert_eq!(notification_for(&pool, incident).await.0, "failed");
    assert!(smtp.messages().is_empty());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn permanent_smtp_errors_fail_immediately(pool: PgPool) {
    let (incident, _) = open_incident(&pool).await;
    let (mailer, _) = start_smtp(vec!["550 no such user"]).await;

    let outcome = claim_and_deliver(&pool, &mailer, false).await;
    assert!(
        matches!(&outcome, DeliveryOutcome::Failed(r) if r.starts_with("SMTP server rejected")),
        "{outcome:?}"
    );
    let (status, attempts, ..) = notification_for(&pool, incident).await;
    assert_eq!((status.as_str(), attempts), ("failed", 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unreachable_smtp_server_is_transient(pool: PgPool) {
    open_incident(&pool).await;
    let dead_port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let outcome = claim_and_deliver(&pool, &mailer_for_port(dead_port), false).await;
    assert!(
        matches!(outcome, DeliveryOutcome::Retrying(_)),
        "{outcome:?}"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn disabled_recipient_is_cancelled(pool: PgPool) {
    let (incident, owner) = open_incident(&pool).await;
    let (mailer, smtp) = start_smtp(vec![]).await;
    sqlx::query("UPDATE users SET is_active = FALSE WHERE email = $1")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        claim_and_deliver(&pool, &mailer, false).await,
        DeliveryOutcome::Cancelled("recipient account is disabled".into())
    );
    assert_eq!(notification_for(&pool, incident).await.0, "cancelled");
    assert!(smtp.messages().is_empty());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deleting_the_endpoint_removes_pending_notifications(pool: PgPool) {
    open_incident(&pool).await;
    sqlx::query("DELETE FROM endpoints")
        .execute(&pool)
        .await
        .unwrap();
    assert!(claim_next(&pool, false).await.unwrap().is_none());
}

// ---------- claiming ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_claims_never_double_send(pool: PgPool) {
    for _ in 0..3 {
        open_incident(&pool).await;
    }
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let pool = pool.clone();
        tasks.spawn(async move { claim_next(&pool, false).await.unwrap().map(|(id, _)| id) });
    }
    let mut claimed: Vec<Uuid> = tasks.join_all().await.into_iter().flatten().collect();
    claimed.sort();
    let total = claimed.len();
    claimed.dedup();
    assert_eq!((total, claimed.len()), (3, 3));
    assert!(
        claim_next(&pool, false).await.unwrap().is_none(),
        "all leased"
    );
}

// ---------- loop ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn run_loop_delivers_and_stops(pool: PgPool) {
    let (incident, _) = open_incident(&pool).await;
    complete_analysis(&pool, incident).await;
    let (mailer, smtp) = start_smtp(vec![]).await;
    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(run_loop(
        pool.clone(),
        Notifier {
            mailer,
            frontend_url: FRONTEND.into(),
        },
        true,
        shutdown.clone(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while smtp.messages().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "email not delivered in time"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("loop stops promptly")
        .unwrap();
    assert_eq!(notification_for(&pool, incident).await.0, "sent");
}
