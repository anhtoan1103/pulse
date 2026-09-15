//! Auth API tests — docs/pulse-api-spec.md #9.2 "Auth", plus the security
//! checks from pulse-security.md #2/#6 (hashing, no password_hash exposure,
//! rate limiting, disabled users).

mod common;

use axum::http::{Method, StatusCode};
use common::{PASSWORD, TestApp, assert_error, auth_config};
use pulse_backend::{
    app::AppState,
    auth::{
        MAX_USERS,
        seed::{SeedOutcome, seed_admin},
    },
    config::{AdminSeed, AuthConfig},
    db::MIGRATOR,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------- register ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_creates_user_with_hashed_password_and_email_identity(pool: PgPool) {
    let app = TestApp::new(pool);
    let (status, body) = app.register("  New.User@Example.com ", PASSWORD).await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["email"], "new.user@example.com");
    let user_id: Uuid = body["user_id"].as_str().unwrap().parse().unwrap();
    assert!(body.get("password_hash").is_none());

    let (role, hash): (String, String) =
        sqlx::query_as("SELECT role, password_hash FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(app.pool())
            .await
            .unwrap();
    assert_eq!(role, "user");
    assert!(
        hash.starts_with("$argon2id$"),
        "password must be argon2id-hashed"
    );
    assert!(!hash.contains(PASSWORD));

    let providers: Vec<String> =
        sqlx::query_scalar("SELECT provider FROM auth_identities WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(app.pool())
            .await
            .unwrap();
    assert_eq!(providers, ["email"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_rejects_existing_email_case_insensitively(pool: PgPool) {
    let app = TestApp::new(pool);
    assert_eq!(
        app.register("taken@example.com", PASSWORD).await.0,
        StatusCode::CREATED
    );

    let (status, body) = app.register("TAKEN@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error(&body, "EMAIL_TAKEN");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_rejects_short_password(pool: PgPool) {
    let app = TestApp::new(pool);
    let (status, body) = app.register("a@example.com", "short").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error(&body, "VALIDATION_ERROR");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_rejects_invalid_email(pool: PgPool) {
    let app = TestApp::new(pool);
    let (status, body) = app.register("not-an-email", PASSWORD).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error(&body, "VALIDATION_ERROR");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bad_json_bodies_use_standard_error_format(pool: PgPool) {
    let app = TestApp::new(pool);

    let (status, body) = app
        .request(
            Method::POST,
            "/api/v1/auth/register",
            Some(json!({ "email": "a@example.com" })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error(&body, "VALIDATION_ERROR");

    let req = axum::http::Request::post("/api/v1/auth/register")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    let (status, body) = app.send(req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&body, "BAD_REQUEST");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_closed_once_user_limit_reached(pool: PgPool) {
    let app = TestApp::new(pool);
    for i in 0..MAX_USERS {
        sqlx::query("INSERT INTO users (email) VALUES ($1)")
            .bind(format!("user{i}@example.com"))
            .execute(app.pool())
            .await
            .unwrap();
    }

    let (status, body) = app.register("one-too-many@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_error(&body, "REGISTRATION_CLOSED");
}

/// The count check + insert must be atomic: with one slot left, concurrent
/// registrations must not overshoot the limit.
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_registrations_cannot_exceed_user_limit(pool: PgPool) {
    let app = TestApp::new(pool);
    for i in 0..MAX_USERS - 1 {
        sqlx::query("INSERT INTO users (email) VALUES ($1)")
            .bind(format!("user{i}@example.com"))
            .execute(app.pool())
            .await
            .unwrap();
    }

    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..5 {
        let app = app.clone();
        tasks.spawn(async move {
            app.register(&format!("racer{i}@example.com"), PASSWORD)
                .await
                .0
        });
    }
    let statuses = tasks.join_all().await;

    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(created, 1, "{statuses:?}");
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(app.pool())
        .await
        .unwrap();
    assert_eq!(total, MAX_USERS);
}

// ---------- login ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn login_returns_token_and_user(pool: PgPool) {
    let app = TestApp::new(pool);
    let (_, registered) = app.register("login@example.com", PASSWORD).await;

    let (status, body) = app.login("LOGIN@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(
        body["user"],
        json!({ "id": registered["user_id"], "email": "login@example.com", "role": "user" })
    );
    assert!(!body.to_string().contains("password"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn login_failures_do_not_reveal_whether_email_exists(pool: PgPool) {
    let app = TestApp::new(pool);
    app.register("exists@example.com", PASSWORD).await;

    let (wrong_pw_status, wrong_pw_body) = app.login("exists@example.com", "wrong password").await;
    let (unknown_status, unknown_body) = app.login("nobody@example.com", PASSWORD).await;

    assert_eq!(wrong_pw_status, StatusCode::UNAUTHORIZED);
    assert_error(&wrong_pw_body, "INVALID_CREDENTIALS");
    assert_eq!(
        (unknown_status, unknown_body),
        (wrong_pw_status, wrong_pw_body)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn oauth_only_user_cannot_login_with_password(pool: PgPool) {
    let app = TestApp::new(pool);
    sqlx::query("INSERT INTO users (email, password_hash) VALUES ('oauth@example.com', NULL)")
        .execute(app.pool())
        .await
        .unwrap();

    let (status, body) = app.login("oauth@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_error(&body, "INVALID_CREDENTIALS");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn disabled_user_cannot_login_and_existing_token_stops_working(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("disabled@example.com").await;
    assert_eq!(
        app.request(Method::GET, "/api/v1/auth/me", None, Some(&token))
            .await
            .0,
        StatusCode::OK
    );

    sqlx::query("UPDATE users SET is_active = FALSE WHERE email = 'disabled@example.com'")
        .execute(app.pool())
        .await
        .unwrap();

    let (status, body) = app
        .request(Method::GET, "/api/v1/auth/me", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_error(&body, "UNAUTHORIZED");

    let (status, body) = app.login("disabled@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_error(&body, "ACCOUNT_DISABLED");
}

// ---------- rate limiting ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn login_is_rate_limited_per_ip(pool: PgPool) {
    // Production limits (5/min), not the loose test default.
    let app = TestApp::from_state(AppState::new(
        pool,
        common::TEST_FRONTEND_URL,
        &auth_config(),
    ));

    for _ in 0..5 {
        assert_eq!(
            app.login("nobody@example.com", PASSWORD).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, body) = app.login("nobody@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_error(&body, "RATE_LIMITED");

    // Separate budget for register.
    assert_eq!(
        app.register("fresh@example.com", PASSWORD).await.0,
        StatusCode::CREATED
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rate_limit_uses_client_ip_header_when_configured(pool: PgPool) {
    let config = AuthConfig {
        client_ip_header: Some("CF-Connecting-IP".into()),
        ..auth_config()
    };
    let app = TestApp::from_state(AppState::new(pool, common::TEST_FRONTEND_URL, &config));
    let login = |ip: &'static str| {
        let app = &app;
        async move {
            app.request_with_headers(
                Method::POST,
                "/api/v1/auth/login",
                Some(json!({ "email": "nobody@example.com", "password": PASSWORD })),
                None,
                &[("CF-Connecting-IP", ip)],
            )
            .await
            .0
        }
    };

    for _ in 0..5 {
        login("198.51.100.1").await;
    }
    assert_eq!(login("198.51.100.1").await, StatusCode::TOO_MANY_REQUESTS);
    // Different client behind the same proxy has its own budget.
    assert_eq!(login("198.51.100.2").await, StatusCode::UNAUTHORIZED);

    // Header missing when required → rejected rather than sharing one bucket.
    let (status, body) = app.login("nobody@example.com", PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_error(&body, "BAD_REQUEST");
}

// ---------- me / logout ----------

#[sqlx::test(migrator = "MIGRATOR")]
async fn me_returns_current_user_without_password_hash(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("me@example.com").await;

    let (status, body) = app
        .request(Method::GET, "/api/v1/auth/me", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["email"], "me@example.com");
    assert_eq!(body["role"], "user");
    assert!(body["id"].is_string());
    assert!(body["created_at"].is_string());
    let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["created_at", "email", "id", "role"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn me_rejects_missing_malformed_and_expired_tokens(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("me@example.com").await;
    let user_id = app.state.jwt.verify(&token).unwrap().sub;

    let expired = app
        .state
        .jwt
        .issue_at(user_id, chrono::Utc::now() - chrono::Duration::hours(48))
        .unwrap();
    let forged = pulse_backend::auth::token::JwtKeys::new(
        b"some-other-secret-at-least-32-bytes!!",
        chrono::Duration::hours(24),
    )
    .issue(user_id)
    .unwrap();

    for bad in [
        None,
        Some("garbage"),
        Some(expired.as_str()),
        Some(forged.as_str()),
    ] {
        let (status, body) = app.request(Method::GET, "/api/v1/auth/me", None, bad).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "token {bad:?}");
        assert_error(&body, "UNAUTHORIZED");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logout_returns_no_content(pool: PgPool) {
    let app = TestApp::new(pool);
    let token = app.register_and_login("bye@example.com").await;
    let (status, body) = app
        .request(Method::POST, "/api/v1/auth/logout", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_null());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unknown_route_uses_standard_error_format(pool: PgPool) {
    let app = TestApp::new(pool);
    let (status, body) = app.request(Method::GET, "/api/v1/nope", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error(&body, "NOT_FOUND");
}

// ---------- admin seed ----------

fn seed() -> AdminSeed {
    AdminSeed {
        email: "Admin@Example.com".into(),
        password: "admin seed password".into(),
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_seed_creates_admin_once(pool: PgPool) {
    assert_eq!(
        seed_admin(&pool, &seed()).await.unwrap(),
        SeedOutcome::Created
    );
    assert_eq!(
        seed_admin(&pool, &seed()).await.unwrap(),
        SeedOutcome::AdminExists
    );

    let app = TestApp::new(pool);
    let (status, body) = app.login("admin@example.com", "admin seed password").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["role"], "admin");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admin_seed_never_promotes_existing_account(pool: PgPool) {
    let app = TestApp::new(pool.clone());
    app.register("admin@example.com", PASSWORD).await;

    assert_eq!(
        seed_admin(&pool, &seed()).await.unwrap(),
        SeedOutcome::EmailTakenByNonAdmin
    );
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(admins, 0);
}
