//! AI Analysis Service tests against a mock OpenAI-compatible provider
//! (docs/pulse-ai-design.md #3–#5, pulse-security.md #5/#7).

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use chrono::{Duration as ChronoDuration, Utc};
use pulse_backend::{
    analysis::{
        llm::LlmClient,
        prompt::FORMAT_REMINDER,
        service::{AnalysisOutcome, MAX_ATTEMPTS, analyze_incident, claim_next, run_loop},
    },
    anomaly::evaluate_endpoint,
    config::AiConfig,
    db::MIGRATOR,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ---------- mock provider ----------

#[derive(Clone)]
struct Reply {
    status: u16,
    body: String,
    retry_after: Option<&'static str>,
    delay: Duration,
}

fn ok(content: &str) -> Reply {
    Reply {
        status: 200,
        body: json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] })
            .to_string(),
        retry_after: None,
        delay: Duration::ZERO,
    }
}

fn status(code: u16, body: Value, retry_after: Option<&'static str>) -> Reply {
    Reply {
        status: code,
        body: body.to_string(),
        retry_after,
        delay: Duration::ZERO,
    }
}

fn valid_analysis() -> String {
    json!({
        "possible_cause": "The endpoint stopped responding, most likely the upstream service is down.",
        "confidence": "medium",
        "evidence": ["error rate 0% → 100%", "3 of 3 recent checks: timeout after 10s"],
        "suggested_steps": ["Check whether the orders service process is running", "Inspect the load balancer health checks"]
    })
    .to_string()
}

#[derive(Default)]
struct Mock {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<(Option<String>, Value)>>,
}

impl Mock {
    fn requests(&self) -> Vec<(Option<String>, Value)> {
        self.requests.lock().unwrap().clone()
    }
}

async fn chat_completions(
    State(mock): State<Arc<Mock>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    mock.requests.lock().unwrap().push((auth, body));
    let reply = mock.replies.lock().unwrap().pop_front().unwrap_or(Reply {
        status: 500,
        body: "no scripted reply".into(),
        retry_after: None,
        delay: Duration::ZERO,
    });
    tokio::time::sleep(reply.delay).await;

    let mut response_headers = HeaderMap::new();
    if let Some(ra) = reply.retry_after {
        response_headers.insert("retry-after", ra.parse().unwrap());
    }
    (
        StatusCode::from_u16(reply.status).unwrap(),
        response_headers,
        reply.body,
    )
}

/// Starts a mock provider scripted with `replies` (served in order).
async fn mock_provider(replies: Vec<Reply>) -> (LlmClient, Arc<Mock>) {
    let mock = Arc::new(Mock {
        replies: Mutex::new(replies.into()),
        ..Default::default()
    });
    let router = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let config = AiConfig {
        base_url: format!("http://{addr}/v1"),
        api_key: "test-key-123".into(),
        model: "test-model".into(),
    };
    (
        LlmClient::new(&config, Duration::from_secs(1)).unwrap(),
        mock,
    )
}

// ---------- fixtures ----------

/// Endpoint with an open error-rate incident, via the real detector. Its
/// checks carry a leaked DB password to prove it never reaches the provider.
async fn pending_incident(pool: &PgPool) -> Uuid {
    let user: Uuid = sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
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
    for (ago, message) in [
        (30, "timeout after 10s"),
        (20, "connection failed: postgres://admin:hunter2@db:5432"),
        (10, "timeout after 10s"),
    ] {
        sqlx::query(
            "INSERT INTO checks (endpoint_id, checked_at, success, error_message) VALUES ($1, $2, false, $3)",
        )
        .bind(endpoint)
        .bind(Utc::now() - ChronoDuration::seconds(ago))
        .bind(message)
        .execute(pool)
        .await
        .unwrap();
    }
    evaluate_endpoint(pool, endpoint).await.unwrap().opened[0]
}

type AiRow = (
    String,
    Option<String>,
    Option<String>,
    Option<Value>,
    Option<Value>,
    i32,
    Option<chrono::DateTime<Utc>>,
    Option<String>,
);

async fn ai_state(pool: &PgPool, incident: Uuid) -> AiRow {
    sqlx::query_as(
        "SELECT ai_status, ai_possible_cause, ai_confidence, ai_evidence, ai_suggested_steps,
                ai_attempts, ai_next_attempt_at, ai_error
         FROM incidents WHERE id = $1",
    )
    .bind(incident)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Claims and analyzes the (only) due incident.
async fn claim_and_analyze(pool: &PgPool, llm: &LlmClient) -> (Uuid, AnalysisOutcome) {
    let (id, attempt) = claim_next(pool).await.unwrap().expect("a due incident");
    (id, analyze_incident(pool, llm, id, attempt).await.unwrap())
}

// ---------- success ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn valid_output_completes_and_is_stored(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let (llm, _) = mock_provider(vec![ok(&valid_analysis())]).await;

    let (id, outcome) = claim_and_analyze(&pool, &llm).await;
    assert_eq!((id, outcome), (incident, AnalysisOutcome::Completed));

    let (status, cause, confidence, evidence, steps, attempts, next, error) =
        ai_state(&pool, incident).await;
    assert_eq!(status, "completed");
    assert!(cause.unwrap().contains("upstream service is down"));
    assert_eq!(confidence.as_deref(), Some("medium"));
    assert_eq!(evidence.unwrap().as_array().unwrap().len(), 2);
    assert_eq!(steps.unwrap().as_array().unwrap().len(), 2);
    assert_eq!((attempts, next, error), (1, None, None));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn request_is_well_formed_and_carries_no_secrets(pool: PgPool) {
    pending_incident(&pool).await;
    let (llm, mock) = mock_provider(vec![ok(&valid_analysis())]).await;
    claim_and_analyze(&pool, &llm).await;

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    let (auth, body) = &requests[0];
    assert_eq!(auth.as_deref(), Some("Bearer test-key-123"));
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["schema"]["required"],
        json!([
            "possible_cause",
            "confidence",
            "evidence",
            "suggested_steps"
        ])
    );

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert!(
        messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("data, not instructions")
    );
    let user = messages[1]["content"].as_str().unwrap();
    assert!(user.contains("<incident_context>"));
    assert!(user.contains("error_rate_threshold_exceeded"));
    assert!(user.contains("timeout after 10s (x2)"));

    let whole_request = body.to_string();
    for secret in ["hunter2", "topsecret", "test-key-123"] {
        assert!(
            !whole_request.contains(secret),
            "{secret} leaked into request body"
        );
    }
}

// ---------- invalid output ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_output_is_retried_once_with_reminder(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let (llm, mock) = mock_provider(vec![
        ok("I think the database is down."),
        ok(&valid_analysis()),
    ])
    .await;

    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    assert_eq!(outcome, AnalysisOutcome::Completed);
    assert_eq!(ai_state(&pool, incident).await.0, "completed");

    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    let retry_messages = requests[1].1["messages"].as_array().unwrap();
    assert_eq!(retry_messages.len(), 4);
    assert_eq!(retry_messages[2]["role"], "assistant");
    assert_eq!(
        retry_messages[2]["content"],
        "I think the database is down."
    );
    assert_eq!(retry_messages[3]["content"], FORMAT_REMINDER);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_output_twice_fails(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let bad_confidence = json!({
        "possible_cause": "x", "confidence": "certain", "evidence": [], "suggested_steps": []
    })
    .to_string();
    let (llm, mock) = mock_provider(vec![ok("not json"), ok(&bad_confidence)]).await;

    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    assert_eq!(
        outcome,
        AnalysisOutcome::Failed("AI returned invalid output".into())
    );
    let (status, cause, .., next, error) = ai_state(&pool, incident).await;
    assert_eq!(status, "failed");
    assert_eq!(cause, None, "nothing half-stored");
    assert_eq!(next, None);
    assert_eq!(error.as_deref(), Some("AI returned invalid output"));
    assert_eq!(mock.requests().len(), 2, "exactly one retry");
}

// ---------- provider errors ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn rate_limit_reschedules_honoring_retry_after(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let (llm, _) = mock_provider(vec![status(429, json!({}), Some("300"))]).await;

    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    let AnalysisOutcome::Retrying(next) = outcome else {
        panic!("expected retry, got {outcome:?}");
    };
    assert!(
        next > Utc::now() + ChronoDuration::seconds(290),
        "Retry-After (300s) beats 60s backoff"
    );

    let (status, .., attempts, stored_next, error) = ai_state(&pool, incident).await;
    assert_eq!(status, "pending");
    assert_eq!(attempts, 1);
    assert_eq!(stored_next.map(|t| t.timestamp()), Some(next.timestamp()));
    assert_eq!(error.as_deref(), Some("AI provider rate limit reached"));
    assert!(claim_next(&pool).await.unwrap().is_none(), "not due yet");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn server_errors_back_off_then_give_up(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let (llm, _) = mock_provider(vec![
        status(503, json!({}), None),
        status(503, json!({}), None),
    ])
    .await;

    // First attempt: exponential backoff starts at 60s.
    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    let AnalysisOutcome::Retrying(next) = outcome else {
        panic!("expected retry, got {outcome:?}");
    };
    let delay = next - Utc::now();
    assert!(
        delay > ChronoDuration::seconds(55) && delay <= ChronoDuration::seconds(60),
        "{delay}"
    );

    // Last allowed attempt: gives up.
    sqlx::query("UPDATE incidents SET ai_attempts = $2, ai_next_attempt_at = now() WHERE id = $1")
        .bind(incident)
        .bind(MAX_ATTEMPTS - 1)
        .execute(&pool)
        .await
        .unwrap();
    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    let expected = format!("AI provider error (HTTP 503) (gave up after {MAX_ATTEMPTS} attempts)");
    assert_eq!(outcome, AnalysisOutcome::Failed(expected.clone()));
    let (status, .., error) = ai_state(&pool, incident).await;
    assert_eq!((status.as_str(), error), ("failed", Some(expected)));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn auth_or_config_errors_fail_immediately(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let (llm, _) = mock_provider(vec![status(
        401,
        json!({ "error": { "message": "API key not valid" } }),
        None,
    )])
    .await;

    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    assert_eq!(
        outcome,
        AnalysisOutcome::Failed("AI provider rejected the request (HTTP 401)".into())
    );
    assert_eq!(ai_state(&pool, incident).await.0, "failed");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn timeouts_and_malformed_responses_are_transient(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let slow = Reply {
        delay: Duration::from_secs(3), // client timeout is 1s
        ..ok(&valid_analysis())
    };
    let garbage = Reply {
        body: "<html>bad gateway</html>".into(),
        ..ok("")
    };
    let (llm, _) = mock_provider(vec![slow, garbage]).await;

    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    assert!(
        matches!(outcome, AnalysisOutcome::Retrying(_)),
        "{outcome:?}"
    );
    assert_eq!(
        ai_state(&pool, incident).await.7.as_deref(),
        Some("AI provider timed out")
    );

    sqlx::query("UPDATE incidents SET ai_next_attempt_at = now() WHERE id = $1")
        .bind(incident)
        .execute(&pool)
        .await
        .unwrap();
    let (_, outcome) = claim_and_analyze(&pool, &llm).await;
    assert!(
        matches!(outcome, AnalysisOutcome::Retrying(_)),
        "{outcome:?}"
    );
    assert_eq!(
        ai_state(&pool, incident).await.7.as_deref(),
        Some("AI provider returned a malformed response")
    );
}

// ---------- claiming ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn claims_are_leased_and_never_shared(pool: PgPool) {
    let first = pending_incident(&pool).await;
    let second = pending_incident(&pool).await;

    let (a, attempt_a) = claim_next(&pool).await.unwrap().unwrap();
    let (b, attempt_b) = claim_next(&pool).await.unwrap().unwrap();
    assert_ne!(a, b);
    assert!([first, second].contains(&a) && [first, second].contains(&b));
    assert_eq!((attempt_a, attempt_b), (1, 1));
    assert!(claim_next(&pool).await.unwrap().is_none(), "both leased");

    // A worker died mid-analysis: once the lease expires, it's claimable again.
    sqlx::query(
        "UPDATE incidents SET ai_next_attempt_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(a)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(claim_next(&pool).await.unwrap(), Some((a, 2)));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_claims_are_disjoint(pool: PgPool) {
    for _ in 0..3 {
        pending_incident(&pool).await;
    }
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let pool = pool.clone();
        tasks.spawn(async move { claim_next(&pool).await.unwrap().map(|(id, _)| id) });
    }
    let mut claimed: Vec<Uuid> = tasks.join_all().await.into_iter().flatten().collect();
    claimed.sort();
    let before = claimed.len();
    claimed.dedup();
    assert_eq!((before, claimed.len()), (3, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn completed_and_failed_incidents_are_not_claimed(pool: PgPool) {
    let done = pending_incident(&pool).await;
    let failed = pending_incident(&pool).await;
    sqlx::query("UPDATE incidents SET ai_status = 'completed' WHERE id = $1")
        .bind(done)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE incidents SET ai_status = 'failed' WHERE id = $1")
        .bind(failed)
        .execute(&pool)
        .await
        .unwrap();
    assert!(claim_next(&pool).await.unwrap().is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deleted_incident_is_gone(pool: PgPool) {
    pending_incident(&pool).await;
    let (llm, mock) = mock_provider(vec![]).await;
    let (id, attempt) = claim_next(&pool).await.unwrap().unwrap();
    sqlx::query("DELETE FROM endpoints")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        analyze_incident(&pool, &llm, id, attempt).await.unwrap(),
        AnalysisOutcome::Gone
    );
    assert!(
        mock.requests().is_empty(),
        "no provider call for a deleted incident"
    );
}

// ---------- loop ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn run_loop_analyzes_pending_incidents_and_stops(pool: PgPool) {
    let incident = pending_incident(&pool).await;
    let (llm, _) = mock_provider(vec![ok(&valid_analysis())]).await;
    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(run_loop(pool.clone(), llm, shutdown.clone()));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while ai_state(&pool, incident).await.0 != "completed" {
        assert!(
            tokio::time::Instant::now() < deadline,
            "loop didn't analyze in time"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("loop stops promptly")
        .unwrap();
}
