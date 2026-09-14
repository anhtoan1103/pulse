//! Metrics (Checks) API tests — docs/pulse-api-spec.md #9.2 "Checks/Metrics".

mod common;

use axum::http::{Method, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use common::{TestApp, assert_error};
use pulse_backend::db::MIGRATOR;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn create_endpoint(app: &TestApp, token: &str) -> Value {
    let (status, body) = app
        .request(
            Method::POST,
            "/api/v1/endpoints",
            Some(json!({
                "name": "API",
                "url": "https://api.example.com/",
                "check_interval_seconds": 60,
                "latency_threshold_ms": 2000,
                "error_rate_threshold_percent": 5.0
            })),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn insert_check(
    pool: &sqlx::PgPool,
    endpoint_id: Uuid,
    ago: ChronoDuration,
    status_code: Option<i32>,
    latency_ms: Option<i32>,
) {
    let success = status_code.is_some_and(|s| (200..300).contains(&s));
    sqlx::query(
        "INSERT INTO checks (endpoint_id, checked_at, status_code, latency_ms, success)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(endpoint_id)
    .bind(Utc::now() - ago)
    .bind(status_code)
    .bind(latency_ms)
    .bind(success)
    .execute(pool)
    .await
    .unwrap();
}

// ---------- checks list ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn checks_list_returns_newest_first_and_respects_ownership(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let endpoint = create_endpoint(&app, &alice).await;
    let id = endpoint["id"].as_str().unwrap();

    insert_check(
        app.pool(),
        id.parse().unwrap(),
        ChronoDuration::minutes(10),
        Some(200),
        Some(100),
    )
    .await;
    insert_check(
        app.pool(),
        id.parse().unwrap(),
        ChronoDuration::minutes(5),
        Some(500),
        None,
    )
    .await;

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks"),
            None,
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let checks = body["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0]["status_code"], 500, "newest first");
    assert_eq!(checks[1]["status_code"], 200);
    assert_eq!(checks[1]["success"], true);

    // Bob can't see Alice's endpoint's checks.
    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn checks_list_filters_by_time_range_and_limit(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_endpoint(&app, &token).await;
    let id = endpoint["id"].as_str().unwrap();
    let eid: Uuid = id.parse().unwrap();

    for m in [60, 30, 10, 5] {
        insert_check(
            app.pool(),
            eid,
            ChronoDuration::minutes(m),
            Some(200),
            Some(50),
        )
        .await;
    }

    // 'Z' suffix (not "+00:00") so the raw URL doesn't need percent-encoding
    // ('+' in a query string means space).
    let from = (Utc::now() - ChronoDuration::minutes(40))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks?from={from}"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["checks"].as_array().unwrap().len(),
        3,
        "excludes the 60m-old check"
    );

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks?limit=2"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["checks"].as_array().unwrap().len(), 2);

    for bad in ["limit=0", "limit=1001", "limit=abc"] {
        let (status, body) = app
            .request(
                Method::GET,
                &format!("/api/v1/endpoints/{id}/checks?{bad}"),
                None,
                Some(&token),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad} -> {body}");
    }
}

// ---------- summary ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn summary_computes_success_rate_and_avg_latency(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_endpoint(&app, &token).await;
    let id = endpoint["id"].as_str().unwrap();
    let eid: Uuid = id.parse().unwrap();

    insert_check(
        app.pool(),
        eid,
        ChronoDuration::minutes(10),
        Some(200),
        Some(100),
    )
    .await;
    insert_check(
        app.pool(),
        eid,
        ChronoDuration::minutes(5),
        Some(200),
        Some(300),
    )
    .await;
    insert_check(app.pool(), eid, ChronoDuration::minutes(2), None, None).await;

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks/summary"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total_checks"], 3);
    assert_eq!(body["avg_latency_ms"], 200.0, "average over responses only");
    let rate = body["success_rate_percent"].as_f64().unwrap();
    assert!((rate - 66.67).abs() < 0.01, "{rate}");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn summary_with_no_checks_is_100_percent_not_null(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_endpoint(&app, &token).await;
    let id = endpoint["id"].as_str().unwrap();

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks/summary"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "avg_latency_ms": null, "success_rate_percent": 100.0, "total_checks": 0 })
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn summary_all_failed_is_zero_percent(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_endpoint(&app, &token).await;
    let id = endpoint["id"].as_str().unwrap();
    insert_check(
        app.pool(),
        id.parse().unwrap(),
        ChronoDuration::minutes(1),
        Some(500),
        None,
    )
    .await;

    let (_, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks/summary"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(body["success_rate_percent"], 0.0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn summary_accepts_only_documented_periods(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_endpoint(&app, &token).await;
    let id = endpoint["id"].as_str().unwrap();

    for period in ["24h", "7d", "30d"] {
        let (status, body) = app
            .request(
                Method::GET,
                &format!("/api/v1/endpoints/{id}/checks/summary?period={period}"),
                None,
                Some(&token),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{period} -> {body}");
    }

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks/summary?period=1h"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error(&body, "VALIDATION_ERROR");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn summary_excludes_checks_outside_the_period(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_endpoint(&app, &token).await;
    let id = endpoint["id"].as_str().unwrap();
    let eid: Uuid = id.parse().unwrap();
    insert_check(
        app.pool(),
        eid,
        ChronoDuration::hours(1),
        Some(200),
        Some(50),
    )
    .await;
    insert_check(app.pool(), eid, ChronoDuration::hours(30), Some(500), None).await; // outside 24h

    let (_, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/checks/summary?period=24h"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(body["total_checks"], 1);
    assert_eq!(body["success_rate_percent"], 100.0);
}

// ---------- health digests ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn health_digests_are_listed_newest_first_and_ownership_scoped(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let endpoint = create_endpoint(&app, &alice).await;
    let id: Uuid = endpoint["id"].as_str().unwrap().parse().unwrap();

    for (days_ago, status) in [(2, "healthy"), (1, "degraded")] {
        sqlx::query(
            "INSERT INTO health_digests (endpoint_id, period_start, period_end, total_checks, success_count, avg_latency_ms, status)
             VALUES ($1, now() - ($2 || ' days')::interval - interval '1 day', now() - ($2 || ' days')::interval, 100, 90, 200, $3)",
        )
        .bind(id)
        .bind(days_ago.to_string())
        .bind(status)
        .execute(app.pool())
        .await
        .unwrap();
    }

    let (status, body) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/health-digests"),
            None,
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let digests = body["digests"].as_array().unwrap();
    assert_eq!(digests.len(), 2);
    assert_eq!(digests[0]["status"], "degraded", "newest first");
    assert_eq!(digests[1]["status"], "healthy");

    let (status, _) = app
        .request(
            Method::GET,
            &format!("/api/v1/endpoints/{id}/health-digests"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------- auth required ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn metrics_routes_require_authentication(pool: PgPool) {
    let app = TestApp::new(pool);
    let id = Uuid::new_v4();
    for uri in [
        format!("/api/v1/endpoints/{id}/checks"),
        format!("/api/v1/endpoints/{id}/checks/summary"),
        format!("/api/v1/endpoints/{id}/health-digests"),
    ] {
        let (status, body) = app.request(Method::GET, &uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_error(&body, "UNAUTHORIZED");
    }
}
