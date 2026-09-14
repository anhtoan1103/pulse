//! Incidents API tests — docs/pulse-api-spec.md #9.2 "Incidents".

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, assert_error};
use pulse_backend::db::MIGRATOR;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn create_endpoint(app: &TestApp, token: &str, name: &str) -> Uuid {
    let (status, body) = app
        .request(
            Method::POST,
            "/api/v1/endpoints",
            Some(json!({
                "name": name,
                "url": "https://api.example.com/",
                "check_interval_seconds": 60,
                "latency_threshold_ms": 2000,
                "error_rate_threshold_percent": 5.0
            })),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().parse().unwrap()
}

/// Inserts an incident row directly, bypassing the real Anomaly Detector.
#[allow(clippy::too_many_arguments)]
async fn insert_incident(
    pool: &sqlx::PgPool,
    endpoint_id: Uuid,
    reason: &str,
    resolved: bool,
    ai_status: &str,
    ai_cause: Option<&str>,
    ai_evidence: Option<Value>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO incidents
             (endpoint_id, trigger_reason, resolved_at, ai_status, ai_possible_cause, ai_confidence, ai_evidence)
         VALUES ($1, $2, CASE WHEN $3 THEN now() ELSE NULL END, $4, $5,
                 CASE WHEN $5 IS NOT NULL THEN 'high' ELSE NULL END, $6)
         RETURNING id",
    )
    .bind(endpoint_id)
    .bind(reason)
    .bind(resolved)
    .bind(ai_status)
    .bind(ai_cause)
    .bind(ai_evidence)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---------- list ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn list_returns_only_callers_own_incidents_newest_first(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let a_ep = create_endpoint(&app, &alice, "Alice API").await;
    let b_ep = create_endpoint(&app, &bob, "Bob API").await;

    insert_incident(
        app.pool(),
        a_ep,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;
    let a2 = insert_incident(
        app.pool(),
        a_ep,
        "error_rate_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;
    insert_incident(
        app.pool(),
        b_ep,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;

    let (status, body) = app
        .request(Method::GET, "/api/v1/incidents", None, Some(&alice))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let incidents = body["incidents"].as_array().unwrap();
    assert_eq!(incidents.len(), 2, "only alice's incidents");
    assert_eq!(incidents[0]["id"], a2.to_string(), "newest first");
    for incident in incidents {
        assert!(
            incident.get("metric_before").is_none(),
            "summary shape has no snapshots"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn list_filters_by_status_and_endpoint_id(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let ep1 = create_endpoint(&app, &alice, "API One").await;
    let ep2 = create_endpoint(&app, &alice, "API Two").await;
    let bob_ep = create_endpoint(&app, &bob, "Bob API").await;

    let open1 = insert_incident(
        app.pool(),
        ep1,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;
    insert_incident(
        app.pool(),
        ep1,
        "error_rate_threshold_exceeded",
        true,
        "completed",
        None,
        None,
    )
    .await;
    let open2 = insert_incident(
        app.pool(),
        ep2,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;

    let (_, body) = app
        .request(
            Method::GET,
            "/api/v1/incidents?status=open",
            None,
            Some(&alice),
        )
        .await;
    let ids: Vec<_> = body["incidents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    assert_eq!(ids, [json!(open2), json!(open1)]);

    let (_, body) = app
        .request(
            Method::GET,
            "/api/v1/incidents?status=resolved",
            None,
            Some(&alice),
        )
        .await;
    assert_eq!(body["incidents"].as_array().unwrap().len(), 1);

    let (_, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/incidents?endpoint_id={ep2}"),
            None,
            Some(&alice),
        )
        .await;
    let ids: Vec<_> = body["incidents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    assert_eq!(ids, [json!(open2)]);

    // Filtering by another user's endpoint_id yields empty, not an error/leak.
    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/incidents?endpoint_id={bob_ep}"),
            None,
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["incidents"].as_array().unwrap().len(), 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn list_requires_authentication(pool: PgPool) {
    let app = TestApp::new(pool);
    let (status, body) = app
        .request(Method::GET, "/api/v1/incidents", None, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_error(&body, "UNAUTHORIZED");
}

// ---------- get one ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_one_returns_full_detail_including_ai_analysis(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let ep = create_endpoint(&app, &token, "API").await;
    let incident = insert_incident(
        app.pool(),
        ep,
        "error_rate_threshold_exceeded",
        false,
        "completed",
        Some("The upstream service is down."),
        Some(json!(["error rate 0% -> 100%"])),
    )
    .await;

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/incidents/{incident}"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], incident.to_string());
    assert_eq!(body["endpoint_id"], ep.to_string());
    assert_eq!(body["endpoint_name"], "API");
    assert_eq!(body["trigger_reason"], "error_rate_threshold_exceeded");
    assert_eq!(body["ai_status"], "completed");
    assert_eq!(body["ai_possible_cause"], "The upstream service is down.");
    assert_eq!(body["ai_confidence"], "high");
    assert_eq!(body["ai_evidence"], json!(["error rate 0% -> 100%"]));
    assert!(body["resolved_at"].is_null());
    assert!(body["created_at"].is_string());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_one_is_ownership_scoped(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let ep = create_endpoint(&app, &alice, "API").await;
    let incident = insert_incident(
        app.pool(),
        ep,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/incidents/{incident}"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");

    let missing = format!("/api/v1/incidents/{}", Uuid::new_v4());
    let (status, missing_body) = app.request(Method::GET, &missing, None, Some(&bob)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing_body, body, "same 404 as a nonexistent id");
}

// ---------- resolve ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_can_resolve_an_open_incident(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let ep = create_endpoint(&app, &token, "API").await;
    let incident = insert_incident(
        app.pool(),
        ep,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/incidents/{incident}/resolve"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["resolved_at"].is_string());

    let (_, fetched) = app
        .request(
            Method::GET,
            &format!("/api/v1/incidents/{incident}"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(
        fetched["resolved_at"], body["resolved_at"],
        "persisted, not just echoed"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn resolving_twice_is_idempotent(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let ep = create_endpoint(&app, &token, "API").await;
    let incident = insert_incident(
        app.pool(),
        ep,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;
    let uri = format!("/api/v1/incidents/{incident}/resolve");

    let (_, first) = app.request(Method::PATCH, &uri, None, Some(&token)).await;
    let (status, second) = app.request(Method::PATCH, &uri, None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        first["resolved_at"], second["resolved_at"],
        "timestamp doesn't move"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn only_owner_can_resolve_an_incident(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let ep = create_endpoint(&app, &alice, "API").await;
    let incident = insert_incident(
        app.pool(),
        ep,
        "latency_threshold_exceeded",
        false,
        "pending",
        None,
        None,
    )
    .await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/incidents/{incident}/resolve"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");

    let is_resolved: bool =
        sqlx::query_scalar("SELECT resolved_at IS NOT NULL FROM incidents WHERE id = $1")
            .bind(incident)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert!(
        !is_resolved,
        "bob's attempt must not resolve alice's incident"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn resolve_requires_authentication(pool: PgPool) {
    let app = TestApp::new(pool);
    let uri = format!("/api/v1/incidents/{}/resolve", Uuid::new_v4());
    let (status, body) = app.request(Method::PATCH, &uri, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_error(&body, "UNAUTHORIZED");
}
