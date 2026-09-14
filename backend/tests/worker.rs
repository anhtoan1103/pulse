//! Scheduler + Checker Worker tests (docs/pulse-architecture.md #2.1–#2.3,
//! pulse-security.md #1 request-time checks).
//!
//! Needs Postgres (`DATABASE_URL`) and Redis (`REDIS_URL`, default
//! `redis://localhost:6379`). Each test uses its own Redis key.

use axum::{Router, http::StatusCode, routing::get};
use chrono::{Duration as ChronoDuration, Utc};
use pulse_backend::{
    db::MIGRATOR,
    queue::{CheckJob, CheckQueue},
    ssrf,
    worker::{
        self,
        checker::{CheckerSettings, HttpChecker},
        job::{JobResult, process_job},
        scheduler::{MAX_BACKLOG, claim_due, schedule_once},
    },
};
use sqlx::PgPool;
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ---------- helpers ----------

const SLACK: Duration = Duration::from_millis(500);

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into())
}

/// Queue on a unique Redis key; tests call [`cleanup`] with it when done.
async fn test_queue() -> (CheckQueue, String) {
    let key = format!("pulse:test:{}", Uuid::new_v4());
    (
        CheckQueue::connect(&redis_url(), key.clone())
            .await
            .unwrap(),
        key,
    )
}

async fn cleanup(key: &str) {
    let client = redis::Client::open(redis_url()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let _: () = redis::cmd("DEL")
        .arg(key)
        .query_async(&mut conn)
        .await
        .unwrap();
}

/// Loopback allowed so checks can reach the local test server; everything
/// else follows the production SSRF policy.
fn allow_loopback(ip: IpAddr) -> bool {
    !ip.is_loopback() && ssrf::is_forbidden_ip(ip)
}

fn test_checker() -> HttpChecker {
    HttpChecker::new(CheckerSettings {
        timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(2),
        max_redirects: 2,
        is_forbidden: allow_loopback,
    })
    .unwrap()
}

async fn serve_test_target() -> SocketAddr {
    let router = Router::new()
        .route("/ok", get(|| async { "ok" }))
        .route("/down", get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

async fn insert_user(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Inserts directly (bypassing API validation) so tests can point at
/// loopback and set arbitrary `last_checked_at`.
async fn insert_endpoint(
    pool: &PgPool,
    user_id: Uuid,
    url: &str,
    interval_seconds: i32,
    is_active: bool,
    last_checked_ago: Option<ChronoDuration>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO endpoints (user_id, name, url, check_interval_seconds, latency_threshold_ms,
                                error_rate_threshold_percent, is_active, last_checked_at)
         VALUES ($1, 'test', $2, $3, 2000, 5, $4, $5) RETURNING id",
    )
    .bind(user_id)
    .bind(url)
    .bind(interval_seconds)
    .bind(is_active)
    .bind(last_checked_ago.map(|ago| Utc::now() - ago))
    .fetch_one(pool)
    .await
    .unwrap()
}

type CheckRow = (Option<i32>, Option<i32>, bool, Option<String>);

async fn checks_for(pool: &PgPool, endpoint_id: Uuid) -> Vec<CheckRow> {
    sqlx::query_as(
        "SELECT status_code, latency_ms, success, error_message FROM checks
         WHERE endpoint_id = $1 ORDER BY checked_at",
    )
    .bind(endpoint_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn job_now(endpoint_id: Uuid) -> CheckJob {
    CheckJob {
        endpoint_id,
        scheduled_at: Utc::now(),
    }
}

// ---------- scheduler: claiming ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn claims_only_due_active_endpoints_once(pool: PgPool) {
    let user = insert_user(&pool).await;
    let url = "https://api.example.com/";
    let never_checked = insert_endpoint(&pool, user, url, 60, true, None).await;
    let overdue = insert_endpoint(
        &pool,
        user,
        url,
        60,
        true,
        Some(ChronoDuration::seconds(120)),
    )
    .await;
    let _recent = insert_endpoint(
        &pool,
        user,
        url,
        60,
        true,
        Some(ChronoDuration::seconds(10)),
    )
    .await;
    let _paused = insert_endpoint(&pool, user, url, 60, false, None).await;

    let mut conn = pool.acquire().await.unwrap();
    let claimed: HashSet<Uuid> = claim_due(&mut conn, 100, SLACK)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(claimed, HashSet::from([never_checked, overdue]));

    // Claiming advanced last_checked_at, so nothing is due right after.
    assert!(claim_due(&mut conn, 100, SLACK).await.unwrap().is_empty());
    let recently_scheduled: bool = sqlx::query_scalar(
        "SELECT last_checked_at > now() - interval '5 seconds' FROM endpoints WHERE id = $1",
    )
    .bind(never_checked)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(recently_scheduled);
}

/// Due within the slack window counts as due, so ticks don't add drift.
#[sqlx::test(migrator = "MIGRATOR")]
async fn claim_includes_endpoints_due_within_slack(pool: PgPool) {
    let user = insert_user(&pool).await;
    let url = "https://api.example.com/";
    let almost_due = insert_endpoint(
        &pool,
        user,
        url,
        60,
        true,
        Some(ChronoDuration::milliseconds(59_800)),
    )
    .await;
    let _not_yet = insert_endpoint(
        &pool,
        user,
        url,
        60,
        true,
        Some(ChronoDuration::seconds(58)),
    )
    .await;

    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        claim_due(&mut conn, 100, SLACK).await.unwrap(),
        [almost_due]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn claim_respects_batch_limit(pool: PgPool) {
    let user = insert_user(&pool).await;
    for _ in 0..5 {
        insert_endpoint(&pool, user, "https://api.example.com/", 60, true, None).await;
    }
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(claim_due(&mut conn, 3, SLACK).await.unwrap().len(), 3);
    assert_eq!(claim_due(&mut conn, 3, SLACK).await.unwrap().len(), 2);
}

/// Two schedulers (e.g. two worker replicas) claiming at the same time must
/// never both get the same endpoint.
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_claims_are_disjoint(pool: PgPool) {
    let user = insert_user(&pool).await;
    for _ in 0..4 {
        insert_endpoint(&pool, user, "https://api.example.com/", 60, true, None).await;
    }

    let mut tx1 = pool.begin().await.unwrap();
    let first = claim_due(&mut tx1, 2, SLACK).await.unwrap();
    // tx1 still holds its row locks: tx2 must skip those rows.
    let mut tx2 = pool.begin().await.unwrap();
    let second = claim_due(&mut tx2, 100, SLACK).await.unwrap();

    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    assert!(first.iter().all(|id| !second.contains(id)));
    tx1.commit().await.unwrap();
    tx2.commit().await.unwrap();
}

// ---------- scheduler + queue ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn schedule_once_enqueues_due_endpoints(pool: PgPool) {
    let (queue, key) = test_queue().await;
    let user = insert_user(&pool).await;
    let a = insert_endpoint(&pool, user, "https://api.example.com/a", 60, true, None).await;
    let b = insert_endpoint(&pool, user, "https://api.example.com/b", 60, true, None).await;

    assert_eq!(schedule_once(&pool, &queue, SLACK).await.unwrap(), 2);
    assert_eq!(
        schedule_once(&pool, &queue, SLACK).await.unwrap(),
        0,
        "already claimed"
    );
    assert_eq!(queue.len().await.unwrap(), 2);

    let mut popped = HashSet::new();
    for _ in 0..2 {
        let job = queue.pop().await.unwrap().expect("job queued");
        assert!(Utc::now() - job.scheduled_at < ChronoDuration::seconds(10));
        popped.insert(job.endpoint_id);
    }
    assert_eq!(popped, HashSet::from([a, b]));
    cleanup(&key).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn schedule_once_backs_off_when_queue_is_backlogged(pool: PgPool) {
    let (queue, key) = test_queue().await;
    let backlog: Vec<CheckJob> = (0..MAX_BACKLOG).map(|_| job_now(Uuid::new_v4())).collect();
    queue.push(&backlog).await.unwrap();

    let user = insert_user(&pool).await;
    let endpoint = insert_endpoint(&pool, user, "https://api.example.com/", 60, true, None).await;

    assert_eq!(schedule_once(&pool, &queue, SLACK).await.unwrap(), 0);
    let last_checked: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT last_checked_at FROM endpoints WHERE id = $1")
            .bind(endpoint)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(last_checked, None, "endpoint must stay due when skipped");
    cleanup(&key).await;
}

#[tokio::test]
async fn queue_is_fifo_and_skips_malformed_payloads() {
    let (queue, key) = test_queue().await;
    let jobs: Vec<CheckJob> = (0..3).map(|_| job_now(Uuid::new_v4())).collect();

    // A garbage entry first: must be discarded, not wedge the consumer.
    let client = redis::Client::open(redis_url()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let _: i64 = redis::cmd("RPUSH")
        .arg(&key)
        .arg("not json")
        .query_async(&mut conn)
        .await
        .unwrap();
    queue.push(&jobs).await.unwrap();

    assert_eq!(queue.pop().await.unwrap(), None);
    for expected in &jobs {
        assert_eq!(queue.pop().await.unwrap().as_ref(), Some(expected));
    }
    cleanup(&key).await;
}

// ---------- job processing ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn records_successful_and_failed_checks(pool: PgPool) {
    let target = serve_test_target().await;
    let checker = test_checker();
    let user = insert_user(&pool).await;
    let ok = insert_endpoint(&pool, user, &format!("http://{target}/ok"), 60, true, None).await;
    let down = insert_endpoint(
        &pool,
        user,
        &format!("http://{target}/down"),
        60,
        true,
        None,
    )
    .await;

    assert!(matches!(
        process_job(&pool, &checker, &job_now(ok)).await.unwrap(),
        JobResult::Recorded(o) if o.success
    ));
    assert!(matches!(
        process_job(&pool, &checker, &job_now(down)).await.unwrap(),
        JobResult::Recorded(o) if !o.success
    ));

    let ok_rows = checks_for(&pool, ok).await;
    assert_eq!(ok_rows.len(), 1);
    let (status, latency, success, error) = &ok_rows[0];
    assert_eq!(
        (*status, *success, error.as_deref()),
        (Some(200), true, None)
    );
    assert!(latency.is_some());

    let down_rows = checks_for(&pool, down).await;
    assert_eq!(down_rows.len(), 1);
    assert_eq!((down_rows[0].0, down_rows[0].2), (Some(503), false));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn records_connection_failure_without_status(pool: PgPool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = listener.local_addr().unwrap();
    drop(listener);
    let user = insert_user(&pool).await;
    let endpoint = insert_endpoint(&pool, user, &format!("http://{dead}/"), 60, true, None).await;

    process_job(&pool, &test_checker(), &job_now(endpoint))
        .await
        .unwrap();

    let rows = checks_for(&pool, endpoint).await;
    assert_eq!(rows.len(), 1);
    let (status, latency, success, error) = &rows[0];
    assert_eq!((*status, *latency, *success), (None, None, false));
    let error = error.as_deref().unwrap();
    assert!(
        error.starts_with("connection failed") || error.starts_with("timeout"),
        "{error}"
    );
}

/// An endpoint whose stored URL now points somewhere internal (DNS changed,
/// or a row written around the API) is blocked at request time and recorded
/// as a failed check.
#[sqlx::test(migrator = "MIGRATOR")]
async fn production_policy_blocks_internal_targets_at_request_time(pool: PgPool) {
    let target = serve_test_target().await;
    let checker = HttpChecker::new(CheckerSettings::default()).unwrap();
    let user = insert_user(&pool).await;
    let literal =
        insert_endpoint(&pool, user, &format!("http://{target}/ok"), 60, true, None).await;
    let hostname = insert_endpoint(
        &pool,
        user,
        &format!("http://localhost:{}/ok", target.port()),
        60,
        true,
        None,
    )
    .await;

    for endpoint in [literal, hostname] {
        process_job(&pool, &checker, &job_now(endpoint))
            .await
            .unwrap();
        let rows = checks_for(&pool, endpoint).await;
        assert_eq!(rows.len(), 1);
        let (status, _, success, error) = &rows[0];
        assert_eq!((*status, *success), (None, false));
        assert!(
            error.as_deref().unwrap().starts_with("blocked:"),
            "{error:?}"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn skips_deleted_paused_and_stale_jobs(pool: PgPool) {
    let target = serve_test_target().await;
    let checker = test_checker();
    let user = insert_user(&pool).await;
    let url = format!("http://{target}/ok");
    let paused = insert_endpoint(&pool, user, &url, 60, false, None).await;
    let active = insert_endpoint(&pool, user, &url, 60, true, None).await;

    assert_eq!(
        process_job(&pool, &checker, &job_now(Uuid::new_v4()))
            .await
            .unwrap(),
        JobResult::EndpointGone
    );
    assert_eq!(
        process_job(&pool, &checker, &job_now(paused))
            .await
            .unwrap(),
        JobResult::Inactive
    );
    let stale = CheckJob {
        endpoint_id: active,
        scheduled_at: Utc::now() - ChronoDuration::seconds(120),
    };
    assert_eq!(
        process_job(&pool, &checker, &stale).await.unwrap(),
        JobResult::Stale
    );

    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM checks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 0);
}

// ---------- end to end ----------

/// Full loop: scheduler claims → Redis → consumer → HTTP → `checks` row,
/// then clean shutdown.
#[sqlx::test(migrator = "MIGRATOR")]
async fn worker_run_checks_due_endpoints_end_to_end(pool: PgPool) {
    let target = serve_test_target().await;
    let (queue, key) = test_queue().await;
    let user = insert_user(&pool).await;
    let ok = insert_endpoint(&pool, user, &format!("http://{target}/ok"), 60, true, None).await;
    let down = insert_endpoint(
        &pool,
        user,
        &format!("http://{target}/down"),
        60,
        true,
        None,
    )
    .await;

    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(worker::run(
        pool.clone(),
        queue,
        test_checker(),
        None,
        None,
        shutdown.clone(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checks")
            .fetch_one(&pool)
            .await
            .unwrap();
        if n >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker didn't record checks in time"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(15), handle)
        .await
        .expect("worker shuts down promptly")
        .unwrap();

    assert!(checks_for(&pool, ok).await[0].2);
    assert_eq!(checks_for(&pool, down).await[0].0, Some(503));
    // Checked once, not re-enqueued every tick (interval is 60s).
    assert_eq!(checks_for(&pool, ok).await.len(), 1);
    cleanup(&key).await;
}
