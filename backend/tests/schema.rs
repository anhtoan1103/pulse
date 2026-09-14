//! Schema tests for `migrations/` (docs/pulse-database-schema.md).
//!
//! `#[sqlx::test]` creates a fresh database per test from `DATABASE_URL`
//! (needs a user allowed to CREATE DATABASE) and applies `MIGRATOR` to it,
//! so tests are isolated from each other and from dev data.

use pulse_backend::db::MIGRATOR;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_user(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_endpoint(pool: &PgPool, user_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO endpoints
             (user_id, name, url, check_interval_seconds, latency_threshold_ms, error_rate_threshold_percent)
         VALUES ($1, 'Orders API', 'https://api.example.com/orders', 60, 2000, 5.0)
         RETURNING id",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    // Audited: table names are test-controlled literals, not user input.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Asserts `result` failed with Postgres `code` (23505 unique, 23514 check).
fn assert_pg_error<T: std::fmt::Debug>(result: Result<T, sqlx::Error>, code: &str) {
    let err = result.expect_err("expected a constraint violation");
    let db_err = err.as_database_error().expect("expected a database error");
    assert_eq!(db_err.code().as_deref(), Some(code), "{db_err}");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn all_tables_exist(pool: PgPool) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations'
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(
        tables,
        [
            "auth_identities",
            "checks",
            "endpoints",
            "health_digests",
            "incidents",
            "notifications",
            "users"
        ]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn user_defaults(pool: PgPool) {
    let id = insert_user(&pool, "a@example.com").await;
    let (role, is_active, password_hash): (String, bool, Option<String>) =
        sqlx::query_as("SELECT role, is_active, password_hash FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(role, "user");
    assert!(is_active);
    assert_eq!(password_hash, None);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn user_email_is_unique_case_insensitively(pool: PgPool) {
    insert_user(&pool, "Toan@Example.com").await;
    let dup = sqlx::query("INSERT INTO users (email) VALUES ('toan@example.com')")
        .execute(&pool)
        .await;
    assert_pg_error(dup, "23505");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn user_role_is_restricted(pool: PgPool) {
    let res = sqlx::query("INSERT INTO users (email, role) VALUES ('a@example.com', 'superadmin')")
        .execute(&pool)
        .await;
    assert_pg_error(res, "23514");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn oauth_account_cannot_link_to_two_users(pool: PgPool) {
    let a = insert_user(&pool, "a@example.com").await;
    let b = insert_user(&pool, "b@example.com").await;
    let insert = "INSERT INTO auth_identities (user_id, provider, provider_user_id)
                  VALUES ($1, 'google', 'google-sub-123')";

    sqlx::query(insert).bind(a).execute(&pool).await.unwrap();
    let res = sqlx::query(insert).bind(b).execute(&pool).await;
    assert_pg_error(res, "23505");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn user_can_link_multiple_providers_but_one_per_provider(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    for (provider, provider_user_id) in [
        ("email", None),
        ("google", Some("g-1")),
        ("github", Some("gh-1")),
    ] {
        sqlx::query(
            "INSERT INTO auth_identities (user_id, provider, provider_user_id) VALUES ($1, $2, $3)",
        )
        .bind(user)
        .bind(provider)
        .bind(provider_user_id)
        .execute(&pool)
        .await
        .unwrap();
    }

    let second_email =
        sqlx::query("INSERT INTO auth_identities (user_id, provider) VALUES ($1, 'email')")
            .bind(user)
            .execute(&pool)
            .await;
    assert_pg_error(second_email, "23505");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn oauth_identity_requires_provider_user_id(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let res = sqlx::query("INSERT INTO auth_identities (user_id, provider) VALUES ($1, 'github')")
        .bind(user)
        .execute(&pool)
        .await;
    assert_pg_error(res, "23514");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn endpoint_defaults(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let id = insert_endpoint(&pool, user).await;
    let (method, is_active, last_checked_at): (
        String,
        bool,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as("SELECT method, is_active, last_checked_at FROM endpoints WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(method, "GET");
    assert!(is_active);
    assert_eq!(
        last_checked_at, None,
        "never-checked endpoints must be due immediately"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn endpoint_rejects_never_valid_values(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let insert = |interval: i32, latency: i32, error_rate: f64, method: &'static str| {
        sqlx::query(
            "INSERT INTO endpoints
                 (user_id, name, url, method, check_interval_seconds, latency_threshold_ms, error_rate_threshold_percent)
             VALUES ($1, 'x', 'https://example.com', $2, $3, $4, $5::numeric)",
        )
        .bind(user)
        .bind(method)
        .bind(interval)
        .bind(latency)
        .bind(error_rate)
        .execute(&pool)
    };

    assert_pg_error(insert(0, 2000, 5.0, "GET").await, "23514");
    assert_pg_error(insert(60, -1, 5.0, "GET").await, "23514");
    assert_pg_error(insert(60, 2000, 100.5, "GET").await, "23514");
    assert_pg_error(insert(60, 2000, 5.0, "CONNECT").await, "23514");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn incident_defaults_and_ai_status_restricted(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let endpoint = insert_endpoint(&pool, user).await;

    let ai_status: String = sqlx::query_scalar(
        "INSERT INTO incidents (endpoint_id, trigger_reason) VALUES ($1, 'latency_threshold_exceeded')
         RETURNING ai_status",
    )
    .bind(endpoint)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ai_status, "pending");

    let res = sqlx::query(
        "INSERT INTO incidents (endpoint_id, trigger_reason, ai_status)
         VALUES ($1, 'latency_threshold_exceeded', 'done')",
    )
    .bind(endpoint)
    .execute(&pool)
    .await;
    assert_pg_error(res, "23514");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn incident_ai_lists_must_be_json_arrays(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let endpoint = insert_endpoint(&pool, user).await;
    let insert = "INSERT INTO incidents (endpoint_id, trigger_reason, ai_evidence)
                  VALUES ($1, 'error_rate_threshold_exceeded', $2)";

    sqlx::query(insert)
        .bind(endpoint)
        .bind(serde_json::json!(["DB query time +640%"]))
        .execute(&pool)
        .await
        .unwrap();

    let res = sqlx::query(insert)
        .bind(endpoint)
        .bind(serde_json::json!({ "not": "an array" }))
        .execute(&pool)
        .await;
    assert_pg_error(res, "23514");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn health_digest_rejects_inconsistent_counts_and_periods(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let endpoint = insert_endpoint(&pool, user).await;
    let insert = "INSERT INTO health_digests
                      (endpoint_id, period_start, period_end, total_checks, success_count, avg_latency_ms, status)
                  VALUES ($1, now() - $2::interval, now(), $3, $4, 240, 'healthy')";

    sqlx::query(insert)
        .bind(endpoint)
        .bind("1 day")
        .bind(100)
        .bind(98)
        .execute(&pool)
        .await
        .unwrap();

    let more_successes_than_checks = sqlx::query(insert)
        .bind(endpoint)
        .bind("1 day")
        .bind(10)
        .bind(11)
        .execute(&pool)
        .await;
    assert_pg_error(more_successes_than_checks, "23514");

    let period_end_before_start = sqlx::query(insert)
        .bind(endpoint)
        .bind("-1 day")
        .bind(10)
        .bind(5)
        .execute(&pool)
        .await;
    assert_pg_error(period_end_before_start, "23514");
}

/// Deleting a user removes everything they own — hard delete per the
/// migration comment in `20260913000002_create_endpoints.sql`.
#[sqlx::test(migrator = "MIGRATOR")]
async fn deleting_user_cascades_to_all_owned_data(pool: PgPool) {
    let user = insert_user(&pool, "a@example.com").await;
    let other_user = insert_user(&pool, "b@example.com").await;
    let endpoint = insert_endpoint(&pool, user).await;
    let other_endpoint = insert_endpoint(&pool, other_user).await;

    for ep in [endpoint, other_endpoint] {
        sqlx::query("INSERT INTO checks (endpoint_id, status_code, latency_ms, success) VALUES ($1, 200, 240, true)")
            .bind(ep)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO incidents (endpoint_id, trigger_reason) VALUES ($1, 'latency_threshold_exceeded')")
            .bind(ep)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO health_digests (endpoint_id, period_start, period_end, total_checks, success_count, status)
             VALUES ($1, now() - interval '1 day', now(), 1, 1, 'healthy')",
        )
        .bind(ep)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO auth_identities (user_id, provider) VALUES ($1, 'email')")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

    // Only the other user's data remains.
    for table in ["endpoints", "checks", "incidents", "health_digests"] {
        assert_eq!(count(&pool, table).await, 1, "{table}");
    }
    assert_eq!(count(&pool, "auth_identities").await, 0);
}
