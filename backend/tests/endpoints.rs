//! Endpoints CRUD API tests — docs/pulse-api-spec.md #9.2 "Endpoints (CRUD)",
//! plus SSRF validation (pulse-security.md #1) and ownership / IDOR (#3).

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, assert_error};
use pulse_backend::{db::MIGRATOR, endpoints::MAX_ENDPOINTS_PER_USER};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const BASE: &str = "/api/v1/endpoints";

fn valid_body() -> Value {
    json!({
        "name": "Orders API",
        "url": "https://api.example.com/orders",
        "method": "GET",
        "check_interval_seconds": 60,
        "latency_threshold_ms": 2000,
        "error_rate_threshold_percent": 5.0
    })
}

/// `valid_body()` with `overrides` merged in (a `null` removes the key).
fn body_with(overrides: Value) -> Value {
    let mut body = valid_body();
    for (k, v) in overrides.as_object().unwrap() {
        if v.is_null() {
            body.as_object_mut().unwrap().remove(k);
        } else {
            body[k] = v.clone();
        }
    }
    body
}

async fn create(app: &TestApp, token: &str, body: Value) -> (StatusCode, Value) {
    app.request(Method::POST, BASE, Some(body), Some(token))
        .await
}

async fn create_ok(app: &TestApp, token: &str) -> Value {
    let (status, body) = create(app, token, valid_body()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

fn item_uri(id: &Value) -> String {
    format!("{BASE}/{}", id.as_str().unwrap())
}

async fn user_id_of(app: &TestApp, token: &str) -> Uuid {
    app.state.jwt.verify(token).unwrap().sub
}

async fn insert_endpoints_directly(app: &TestApp, user_id: Uuid, n: i64) {
    sqlx::query(
        "INSERT INTO endpoints (user_id, name, url, check_interval_seconds, latency_threshold_ms, error_rate_threshold_percent)
         SELECT $1, 'bulk ' || g, 'https://api.example.com/' || g, 60, 2000, 5
         FROM generate_series(1, $2) AS g",
    )
    .bind(user_id)
    .bind(n)
    .execute(app.pool())
    .await
    .unwrap();
}

// ---------- auth required ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn all_routes_require_authentication(pool: PgPool) {
    let app = TestApp::new(pool);
    let item = format!("{BASE}/{}", Uuid::new_v4());
    let cases = [
        (Method::GET, BASE.to_string(), None),
        (Method::POST, BASE.to_string(), Some(valid_body())),
        (Method::GET, item.clone(), None),
        (Method::PATCH, item.clone(), Some(json!({ "name": "x" }))),
        (Method::DELETE, item, None),
    ];
    for (method, uri, body) in cases {
        let (status, resp) = app.request(method.clone(), &uri, body, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert_error(&resp, "UNAUTHORIZED");
    }
}

// ---------- create ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn create_returns_endpoint(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;

    let (status, body) = create(
        &app,
        &token,
        body_with(json!({ "name": "  Orders API ", "method": "post", "url": "https://API.example.com/orders" })),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body["id"].as_str().unwrap().parse::<Uuid>().is_ok());
    assert_eq!(body["name"], "Orders API");
    assert_eq!(body["url"], "https://api.example.com/orders");
    assert_eq!(body["method"], "POST");
    assert_eq!(body["check_interval_seconds"], 60);
    assert_eq!(body["latency_threshold_ms"], 2000);
    assert_eq!(body["error_rate_threshold_percent"], 5.0);
    assert_eq!(body["is_active"], true);
    assert!(body["last_checked_at"].is_null());
    assert!(body["created_at"].is_string());
    assert!(body.get("user_id").is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn create_defaults_method_to_get(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let (status, body) = create(&app, &token, body_with(json!({ "method": null }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["method"], "GET");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn create_rejects_out_of_range_values(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;

    let cases = [
        json!({ "check_interval_seconds": 9 }),
        json!({ "check_interval_seconds": 3601 }),
        json!({ "latency_threshold_ms": 0 }),
        json!({ "latency_threshold_ms": 10_001 }),
        json!({ "error_rate_threshold_percent": -1 }),
        json!({ "error_rate_threshold_percent": 100.5 }),
        json!({ "name": "   " }),
        json!({ "name": "x".repeat(101) }),
        json!({ "method": "CONNECT" }),
        // Wrong type / missing required / unknown field → still 422 in standard format.
        json!({ "check_interval_seconds": "60" }),
        json!({ "latency_threshold_ms": null }),
        json!({ "user_id": Uuid::new_v4() }),
    ];
    for overrides in cases {
        let (status, body) = create(&app, &token, body_with(overrides.clone())).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{overrides} → {body}"
        );
        assert_error(&body, "VALIDATION_ERROR");
    }

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM endpoints")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn create_rejects_ssrf_targets(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;

    for url in [
        "http://localhost:5432/",
        "http://127.0.0.1:8080/health",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
        "http://192.168.1.1/",
        "http://172.18.0.2:6379/",
        "http://[::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://2130706433/",
        "http://internal.example.com/",
        "http://rebind.example.com/",
        "http://postgres:5432/",
        "http://does-not-resolve.example.net/",
        "ftp://api.example.com/",
        "file:///etc/passwd",
        "https://user:secret@api.example.com/",
        "not a url",
    ] {
        let (status, body) = create(&app, &token, body_with(json!({ "url": url }))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{url} → {body}");
        assert_error(&body, "VALIDATION_ERROR");
    }

    // Public literal IPs and IPv6-resolving hosts are fine.
    for url in ["http://93.184.215.14/", "https://status.example.org/ping"] {
        let (status, body) = create(&app, &token, body_with(json!({ "url": url }))).await;
        assert_eq!(status, StatusCode::CREATED, "{url} → {body}");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn create_rejects_over_per_user_limit(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let user_id = user_id_of(&app, &token).await;
    insert_endpoints_directly(&app, user_id, MAX_ENDPOINTS_PER_USER).await;

    let (status, body) = create(&app, &token, valid_body()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error(&body, "ENDPOINT_LIMIT_REACHED");

    // The limit is per user: someone else can still create.
    let other = app.register_and_login("b@example.com").await;
    assert_eq!(
        create(&app, &other, valid_body()).await.0,
        StatusCode::CREATED
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_creates_cannot_exceed_limit(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let user_id = user_id_of(&app, &token).await;
    insert_endpoints_directly(&app, user_id, MAX_ENDPOINTS_PER_USER - 1).await;

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..5 {
        let (app, token) = (app.clone(), token.clone());
        tasks.spawn(async move { create(&app, &token, valid_body()).await.0 });
    }
    let statuses = tasks.join_all().await;

    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(created, 1, "{statuses:?}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM endpoints WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(count, MAX_ENDPOINTS_PER_USER);
}

// ---------- list / get: ownership ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn list_returns_only_callers_endpoints(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;

    let a1 = create_ok(&app, &alice).await;
    let a2 = create_ok(&app, &alice).await;
    let b1 = create_ok(&app, &bob).await;

    let (status, body) = app.request(Method::GET, BASE, None, Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<_> = body["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].clone())
        .collect();
    assert_eq!(ids, [a1["id"].clone(), a2["id"].clone()]);

    let (_, body) = app.request(Method::GET, BASE, None, Some(&bob)).await;
    assert_eq!(body["endpoints"], json!([b1]));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn list_is_empty_for_new_user(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let (status, body) = app.request(Method::GET, BASE, None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "endpoints": [] }));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_returns_own_endpoint_and_hides_others(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let endpoint = create_ok(&app, &alice).await;
    let uri = item_uri(&endpoint["id"]);

    let (status, body) = app.request(Method::GET, &uri, None, Some(&alice)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, endpoint);

    // Bob gets the same 404 as for an id that doesn't exist at all.
    let (status, bob_body) = app.request(Method::GET, &uri, None, Some(&bob)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&bob_body, "NOT_FOUND");
    let missing = format!("{BASE}/{}", Uuid::new_v4());
    let (_, missing_body) = app.request(Method::GET, &missing, None, Some(&bob)).await;
    assert_eq!(bob_body, missing_body);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_id_is_bad_request(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let (status, body) = app
        .request(
            Method::GET,
            &format!("{BASE}/not-a-uuid"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&body, "BAD_REQUEST");
}

// ---------- update ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn update_changes_only_given_fields(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_ok(&app, &token).await;
    let uri = item_uri(&endpoint["id"]);

    let (status, body) = app
        .request(
            Method::PATCH,
            &uri,
            Some(json!({
                "latency_threshold_ms": 1500,
                "error_rate_threshold_percent": 2.5,
                "is_active": false
            })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut expected = endpoint.clone();
    expected["latency_threshold_ms"] = json!(1500);
    expected["error_rate_threshold_percent"] = json!(2.5);
    expected["is_active"] = json!(false);
    assert_eq!(body, expected);

    // Persisted, not just echoed.
    let (_, fetched) = app.request(Method::GET, &uri, None, Some(&token)).await;
    assert_eq!(fetched, expected);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn update_all_fields(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_ok(&app, &token).await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &item_uri(&endpoint["id"]),
            Some(json!({
                "name": "Status page",
                "url": "https://status.example.org/ping",
                "method": "head",
                "check_interval_seconds": 300,
                "latency_threshold_ms": 800,
                "error_rate_threshold_percent": 0,
                "is_active": false
            })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Status page");
    assert_eq!(body["url"], "https://status.example.org/ping");
    assert_eq!(body["method"], "HEAD");
    assert_eq!(body["check_interval_seconds"], 300);
    assert_eq!(body["latency_threshold_ms"], 800);
    assert_eq!(body["error_rate_threshold_percent"], 0.0);
    assert_eq!(body["is_active"], false);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn update_validates_fields_including_ssrf(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_ok(&app, &token).await;
    let uri = item_uri(&endpoint["id"]);

    for patch in [
        json!({ "check_interval_seconds": 1 }),
        json!({ "latency_threshold_ms": 20_000 }),
        json!({ "error_rate_threshold_percent": 150 }),
        json!({ "name": "" }),
        json!({ "method": "TRACE" }),
        json!({ "url": "http://169.254.169.254/" }),
        json!({ "url": "http://internal.example.com/" }),
        json!({ "last_checked_at": "2026-01-01T00:00:00Z" }),
        json!({ "user_id": Uuid::new_v4() }),
    ] {
        let (status, body) = app
            .request(Method::PATCH, &uri, Some(patch.clone()), Some(&token))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{patch} → {body}");
        assert_error(&body, "VALIDATION_ERROR");
    }

    let (_, unchanged) = app.request(Method::GET, &uri, None, Some(&token)).await;
    assert_eq!(unchanged, endpoint);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cannot_update_someone_elses_endpoint(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let endpoint = create_ok(&app, &alice).await;
    let uri = item_uri(&endpoint["id"]);

    let (status, body) = app
        .request(
            Method::PATCH,
            &uri,
            Some(json!({ "url": "https://status.example.org/", "is_active": false })),
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");

    let (_, unchanged) = app.request(Method::GET, &uri, None, Some(&alice)).await;
    assert_eq!(unchanged, endpoint);
}

// ---------- delete ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn delete_removes_endpoint_and_cascades(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let endpoint = create_ok(&app, &token).await;
    let keep = create_ok(&app, &token).await;
    let id: Uuid = endpoint["id"].as_str().unwrap().parse().unwrap();

    sqlx::query("INSERT INTO checks (endpoint_id, status_code, latency_ms, success) VALUES ($1, 200, 100, true)")
        .bind(id)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO incidents (endpoint_id, trigger_reason) VALUES ($1, 'latency_threshold_exceeded')")
        .bind(id)
        .execute(app.pool())
        .await
        .unwrap();

    let uri = item_uri(&endpoint["id"]);
    let (status, body) = app.request(Method::DELETE, &uri, None, Some(&token)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_null());

    assert_eq!(
        app.request(Method::GET, &uri, None, Some(&token)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.request(Method::DELETE, &uri, None, Some(&token))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    for table in ["checks", "incidents"] {
        let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {table} WHERE endpoint_id = $1"
        )))
        .bind(id)
        .fetch_one(app.pool())
        .await
        .unwrap();
        assert_eq!(n, 0, "{table} should cascade");
    }

    let (_, list) = app.request(Method::GET, BASE, None, Some(&token)).await;
    assert_eq!(list["endpoints"], json!([keep]));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cannot_delete_someone_elses_endpoint(pool: PgPool) {
    let app = TestApp::new(pool);
    let alice = app.register_and_login("alice@example.com").await;
    let bob = app.register_and_login("bob@example.com").await;
    let endpoint = create_ok(&app, &alice).await;
    let uri = item_uri(&endpoint["id"]);

    let (status, body) = app.request(Method::DELETE, &uri, None, Some(&bob)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");

    assert_eq!(
        app.request(Method::GET, &uri, None, Some(&alice)).await.0,
        StatusCode::OK
    );
}
