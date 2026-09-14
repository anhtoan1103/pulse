//! Health Digest generation + delivery tests (docs/pulse-architecture.md
//! #2.6, PRD #6).

use chrono::{Duration as ChronoDuration, TimeZone, Timelike, Utc};
use pulse_backend::{
    config::{EmailConfig, SmtpTls},
    db::MIGRATOR,
    health_digest::{DIGEST_HOUR_UTC, GeneratedRun, latest_boundary, run_once},
    notify::{
        email::Mailer,
        service::{DeliveryOutcome, claim_next, deliver},
    },
};
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

const FRONTEND: &str = "https://pulse.test";

fn boundary() -> chrono::DateTime<Utc> {
    latest_boundary(Utc::now())
}

async fn insert_user(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_endpoint(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    latency_threshold_ms: i32,
    error_rate_threshold_percent: f64,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO endpoints
             (user_id, name, url, check_interval_seconds, latency_threshold_ms, error_rate_threshold_percent)
         VALUES ($1, $2, 'https://api.example.com', 60, $3, $4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(latency_threshold_ms)
    .bind(error_rate_threshold_percent)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A check at `ago` before the current period boundary (so it lands inside
/// the period `run_once` will claim).
async fn check_before_boundary(
    pool: &PgPool,
    endpoint: Uuid,
    ago: ChronoDuration,
    status: Option<i32>,
    latency_ms: Option<i32>,
) {
    let success = status.is_some_and(|s| (200..300).contains(&s));
    sqlx::query(
        "INSERT INTO checks (endpoint_id, checked_at, status_code, latency_ms, success)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(endpoint)
    .bind(boundary() - ago)
    .bind(status)
    .bind(latency_ms)
    .bind(success)
    .execute(pool)
    .await
    .unwrap();
}

type DigestRow = (String, i32, i32, Option<i32>);

async fn digest_for(pool: &PgPool, endpoint: Uuid) -> DigestRow {
    sqlx::query_as(
        "SELECT status, total_checks, success_count, avg_latency_ms
         FROM health_digests WHERE endpoint_id = $1",
    )
    .bind(endpoint)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn notification_for_user(pool: &PgPool, user_id: Uuid) -> Option<(String, Uuid)> {
    sqlx::query_as::<_, (String, Option<Uuid>)>(
        "SELECT status, digest_run_id FROM notifications
         WHERE user_id = $1 AND kind = 'health_digest'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|(status, run_id)| (status, run_id.unwrap()))
}

// ---------- boundary math ----------

#[test]
fn boundary_lands_exactly_on_the_digest_hour() {
    let b = latest_boundary(Utc.with_ymd_and_hms(2026, 9, 14, 15, 30, 0).unwrap());
    assert_eq!((b.hour(), b.minute()), (DIGEST_HOUR_UTC, 0));
}

// ---------- generation ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn generates_healthy_and_degraded_digests(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let healthy = insert_endpoint(&pool, user, "Healthy API", 2000, 5.0).await;
    let slow = insert_endpoint(&pool, user, "Slow API", 2000, 5.0).await;
    let flaky = insert_endpoint(&pool, user, "Flaky API", 2000, 5.0).await;
    let untouched = insert_endpoint(&pool, user, "Untouched API", 2000, 5.0).await;

    for h in 1..24 {
        check_before_boundary(
            &pool,
            healthy,
            ChronoDuration::hours(h),
            Some(200),
            Some(100),
        )
        .await;
        check_before_boundary(&pool, slow, ChronoDuration::hours(h), Some(200), Some(3000)).await;
    }
    check_before_boundary(&pool, flaky, ChronoDuration::hours(1), Some(500), None).await;
    check_before_boundary(&pool, flaky, ChronoDuration::hours(2), Some(200), Some(50)).await;

    let run = run_once(&pool).await.unwrap().expect("period is claimable");
    assert_eq!(run.period_end, boundary());
    assert_eq!(run.period_end - run.period_start, ChronoDuration::hours(24));
    assert_eq!(run.digests_created, 3, "untouched endpoint has no checks");
    assert_eq!(run.users_notified, 1);

    let (status, total, success, avg) = digest_for(&pool, healthy).await;
    assert_eq!(
        (status.as_str(), total, success, avg),
        ("healthy", 23, 23, Some(100))
    );

    let (status, ..) = digest_for(&pool, slow).await;
    assert_eq!(status, "degraded", "avg latency 3000 > threshold 2000");

    let (status, total, success, _) = digest_for(&pool, flaky).await;
    assert_eq!(
        (status.as_str(), total, success),
        ("degraded", 2, 1),
        "50% error rate > 5% threshold"
    );

    let untouched_digests: i64 =
        sqlx::query_scalar("SELECT count(*) FROM health_digests WHERE endpoint_id = $1")
            .bind(untouched)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(untouched_digests, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn error_rate_exactly_at_threshold_is_healthy(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    // 5% threshold, exactly 1 failure in 20 = 5% -> not "above".
    let ep = insert_endpoint(&pool, user, "Borderline API", 2000, 5.0).await;
    check_before_boundary(&pool, ep, ChronoDuration::hours(1), None, None).await;
    for h in 2..=20 {
        check_before_boundary(&pool, ep, ChronoDuration::hours(h), Some(200), Some(100)).await;
    }

    run_once(&pool).await.unwrap();
    assert_eq!(digest_for(&pool, ep).await.0, "healthy");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn checks_outside_the_period_are_excluded(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let ep = insert_endpoint(&pool, user, "API", 2000, 5.0).await;
    check_before_boundary(&pool, ep, ChronoDuration::hours(1), Some(200), Some(100)).await;
    check_before_boundary(&pool, ep, ChronoDuration::hours(25), Some(500), None).await; // before period_start
    check_before_boundary(&pool, ep, ChronoDuration::hours(-1), Some(500), None).await; // after period_end

    run_once(&pool).await.unwrap();
    let (status, total, ..) = digest_for(&pool, ep).await;
    assert_eq!((status.as_str(), total), ("healthy", 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn one_notification_bundles_all_of_a_users_endpoints(pool: PgPool) {
    let alice = insert_user(&pool, "alice@example.com").await;
    let bob = insert_user(&pool, "bob@example.com").await;
    let a1 = insert_endpoint(&pool, alice, "A1", 2000, 5.0).await;
    let a2 = insert_endpoint(&pool, alice, "A2", 2000, 5.0).await;
    let b1 = insert_endpoint(&pool, bob, "B1", 2000, 5.0).await;
    for ep in [a1, a2, b1] {
        check_before_boundary(&pool, ep, ChronoDuration::hours(1), Some(200), Some(100)).await;
    }

    let run = run_once(&pool).await.unwrap().unwrap();
    assert_eq!(run.users_notified, 2);

    let alice_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'health_digest'",
    )
    .bind(alice)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(alice_count, 1, "one email for both of alice's endpoints");

    let (bob_status, _) = notification_for_user(&pool, bob).await.unwrap();
    assert_eq!(bob_status, "pending");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn user_with_no_checks_gets_no_notification(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    insert_endpoint(&pool, user, "Untouched", 2000, 5.0).await;

    run_once(&pool).await.unwrap();
    assert!(notification_for_user(&pool, user).await.is_none());
}

// ---------- claiming ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_period_is_only_generated_once(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let ep = insert_endpoint(&pool, user, "API", 2000, 5.0).await;
    check_before_boundary(&pool, ep, ChronoDuration::hours(1), Some(200), Some(100)).await;

    assert!(run_once(&pool).await.unwrap().is_some());
    assert_eq!(run_once(&pool).await.unwrap(), None, "already claimed");

    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM digest_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_run_once_calls_only_one_wins(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let ep = insert_endpoint(&pool, user, "API", 2000, 5.0).await;
    check_before_boundary(&pool, ep, ChronoDuration::hours(1), Some(200), Some(100)).await;

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let pool = pool.clone();
        tasks.spawn(async move { run_once(&pool).await.unwrap() });
    }
    let results: Vec<Option<GeneratedRun>> = tasks.join_all().await;
    assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);

    let digests: i64 = sqlx::query_scalar("SELECT count(*) FROM health_digests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(digests, 1, "no duplicate digest rows from the race");
}

// ---------- delivery ----------

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

/// A local SMTP stub that accepts everything and just closes; enough to
/// prove the digest email gets built and "sent" without asserting content
/// (tests/notify.rs's fake server covers full SMTP semantics already).
async fn accept_all_smtp() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let _ = write.write_all(b"220 ok\r\n").await;
                while let Ok(Some(line)) = lines.next_line().await {
                    let reply = if line.eq_ignore_ascii_case("data") {
                        let _ = write.write_all(b"354 go\r\n").await;
                        while let Ok(Some(l)) = lines.next_line().await {
                            if l == "." {
                                break;
                            }
                        }
                        "250 queued\r\n"
                    } else if line.eq_ignore_ascii_case("quit") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    } else {
                        "250 OK\r\n"
                    };
                    if write.write_all(reply.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn digest_notification_delivers_through_the_notifier(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let ep1 = insert_endpoint(&pool, user, "API One", 2000, 5.0).await;
    let ep2 = insert_endpoint(&pool, user, "API Two", 2000, 5.0).await;
    check_before_boundary(&pool, ep1, ChronoDuration::hours(1), Some(200), Some(100)).await;
    check_before_boundary(&pool, ep2, ChronoDuration::hours(1), Some(500), None).await;

    run_once(&pool).await.unwrap();
    let (_, run_id) = notification_for_user(&pool, user).await.unwrap();

    let port = accept_all_smtp().await;
    let mailer = mailer_for_port(port);
    let (id, attempt) = claim_next(&pool, false).await.unwrap().unwrap();
    let outcome = deliver(&pool, &mailer, FRONTEND, false, id, attempt)
        .await
        .unwrap();
    assert_eq!(outcome, DeliveryOutcome::Sent);

    let (status, stored_run_id) = notification_for_user(&pool, user).await.unwrap();
    assert_eq!((status.as_str(), stored_run_id), ("sent", run_id));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn digest_notification_cancels_if_digests_vanish(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let ep = insert_endpoint(&pool, user, "API", 2000, 5.0).await;
    check_before_boundary(&pool, ep, ChronoDuration::hours(1), Some(200), Some(100)).await;
    run_once(&pool).await.unwrap();

    // The endpoint (and its digest) disappears before delivery.
    sqlx::query("DELETE FROM endpoints WHERE id = $1")
        .bind(ep)
        .execute(&pool)
        .await
        .unwrap();

    let port = accept_all_smtp().await;
    let mailer = mailer_for_port(port);
    let (id, attempt) = claim_next(&pool, false).await.unwrap().unwrap();
    let outcome = deliver(&pool, &mailer, FRONTEND, false, id, attempt)
        .await
        .unwrap();
    assert!(
        matches!(outcome, DeliveryOutcome::Cancelled(_)),
        "{outcome:?}"
    );
}
