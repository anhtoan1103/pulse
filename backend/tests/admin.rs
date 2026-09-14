//! Admin API tests — docs/pulse-api-spec.md #9.2 "Admin", pulse-security.md
//! #3 (consistent role check across `/admin/*`).

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, assert_error};
use pulse_backend::db::MIGRATOR;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn make_admin(app: &TestApp, email: &str) -> Uuid {
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(app.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = $1")
        .bind(user_id)
        .execute(app.pool())
        .await
        .unwrap();
    user_id
}

// ---------- non-admin is blocked everywhere ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn non_admin_is_forbidden_from_every_admin_route(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("a@example.com").await;
    let other_id = Uuid::new_v4();

    for (method, uri) in [
        (Method::GET, "/api/v1/admin/users".to_string()),
        (Method::PATCH, format!("/api/v1/admin/users/{other_id}")),
        (Method::GET, "/api/v1/admin/stats".to_string()),
    ] {
        let (status, body) = app
            .request(method.clone(), &uri, Some(json!({})), Some(&token))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        assert_error(&body, "FORBIDDEN");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_routes_require_authentication(pool: PgPool) {
    let app = TestApp::new(pool);
    let (status, body) = app
        .request(Method::GET, "/api/v1/admin/users", None, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_error(&body, "UNAUTHORIZED");
}

// ---------- list users ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_lists_all_users_without_password_hash(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    make_admin(&app, "admin@example.com").await;
    app.register_and_login("regular@example.com").await;

    let (status, body) = app
        .request(Method::GET, "/api/v1/admin/users", None, Some(&admin_token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2, "not limited to the caller's own account");
    let emails: Vec<_> = users.iter().map(|u| u["email"].as_str().unwrap()).collect();
    assert!(emails.contains(&"admin@example.com"));
    assert!(emails.contains(&"regular@example.com"));
    for u in users {
        assert!(u.get("password_hash").is_none(), "{u}");
        assert!(u["id"].is_string() && u["role"].is_string() && u["is_active"].is_boolean());
    }
}

// ---------- update user ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_can_disable_and_reenable_another_user(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    make_admin(&app, "admin@example.com").await;
    let user_token = app.register_and_login("regular@example.com").await;
    let user_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'regular@example.com'")
            .fetch_one(app.pool())
            .await
            .unwrap();

    // The disabled user's existing token stops working (mirrors auth.rs's
    // disabled_user test — enforced by AuthUser re-reading is_active).
    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{user_id}"),
            Some(json!({ "is_active": false })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_active"], false);

    let (status, _) = app
        .request(Method::GET, "/api/v1/auth/me", None, Some(&user_token))
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "disabled user's token must stop working immediately"
    );

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{user_id}"),
            Some(json!({ "is_active": true })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_active"], true);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_can_promote_another_user_to_admin(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    make_admin(&app, "admin@example.com").await;
    app.register_and_login("regular@example.com").await;
    let user_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'regular@example.com'")
            .fetch_one(app.pool())
            .await
            .unwrap();

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{user_id}"),
            Some(json!({ "role": "admin" })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "admin");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn update_rejects_invalid_role_and_unknown_fields(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    make_admin(&app, "admin@example.com").await;
    app.register_and_login("regular@example.com").await;
    let user_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'regular@example.com'")
            .fetch_one(app.pool())
            .await
            .unwrap();

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{user_id}"),
            Some(json!({ "role": "superadmin" })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_error(&body, "VALIDATION_ERROR");

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{user_id}"),
            Some(json!({ "email": "hijacked@example.com" })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown field rejected: {body}"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn update_on_unknown_user_is_not_found(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    make_admin(&app, "admin@example.com").await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{}", Uuid::new_v4()),
            Some(json!({ "is_active": false })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");
}

// ---------- self-lockout guards ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_cannot_disable_their_own_account(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    let admin_id = make_admin(&app, "admin@example.com").await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{admin_id}"),
            Some(json!({ "is_active": false })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_error(&body, "VALIDATION_ERROR");

    let still_active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1")
        .bind(admin_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert!(still_active);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_cannot_demote_themselves(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    let admin_id = make_admin(&app, "admin@example.com").await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{admin_id}"),
            Some(json!({ "role": "user" })),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = $1")
        .bind(admin_id)
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(role, "admin");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_second_admin_can_demote_the_first(pool: PgPool) {
    let app = TestApp::new(pool);
    app.register_and_login("admin-one@example.com").await;
    let admin_one_id = make_admin(&app, "admin-one@example.com").await;
    let admin_two_token = app.register_and_login("admin-two@example.com").await;
    make_admin(&app, "admin-two@example.com").await;

    let (status, body) = app
        .request(
            Method::PATCH,
            &format!("/api/v1/admin/users/{admin_one_id}"),
            Some(json!({ "role": "user" })),
            Some(&admin_two_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "user");
}

// ---------- stats ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn stats_counts_users_active_endpoints_and_recent_incidents(pool: PgPool) {
    let app = TestApp::new(pool);
    let admin_token = app.register_and_login("admin@example.com").await;
    make_admin(&app, "admin@example.com").await;
    let user_token = app.register_and_login("regular@example.com").await;

    let (_, active_ep) = app
        .request(
            Method::POST,
            "/api/v1/endpoints",
            Some(json!({
                "name": "Active", "url": "https://api.example.com/a",
                "check_interval_seconds": 60, "latency_threshold_ms": 2000,
                "error_rate_threshold_percent": 5.0
            })),
            Some(&user_token),
        )
        .await;
    let (_, paused_ep) = app
        .request(
            Method::POST,
            "/api/v1/endpoints",
            Some(json!({
                "name": "Paused", "url": "https://api.example.com/b",
                "check_interval_seconds": 60, "latency_threshold_ms": 2000,
                "error_rate_threshold_percent": 5.0
            })),
            Some(&user_token),
        )
        .await;
    app.request(
        Method::PATCH,
        &format!("/api/v1/endpoints/{}", paused_ep["id"].as_str().unwrap()),
        Some(json!({ "is_active": false })),
        Some(&user_token),
    )
    .await;

    let active_ep_id: Uuid = active_ep["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("INSERT INTO incidents (endpoint_id, trigger_reason) VALUES ($1, 'latency_threshold_exceeded')")
        .bind(active_ep_id)
        .execute(app.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO incidents (endpoint_id, trigger_reason, triggered_at)
         VALUES ($1, 'error_rate_threshold_exceeded', now() - interval '2 days')",
    )
    .bind(active_ep_id)
    .execute(app.pool())
    .await
    .unwrap();

    let (status, body) = app
        .request(Method::GET, "/api/v1/admin/stats", None, Some(&admin_token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total_users"], 2);
    assert_eq!(body["active_endpoints"], 1);
    assert_eq!(
        body["incidents_last_24h"], 1,
        "the 2-day-old incident must not count"
    );
}
