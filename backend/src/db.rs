//! Postgres pool setup + embedded migrations (`backend/migrations/`, schema
//! per `docs/pulse-database-schema.md`).

use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use std::time::Duration;

/// Migrations are embedded into the binary at compile time, so the runtime
/// image doesn't need the `migrations/` directory to apply them.
pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Opens a connection pool sized to `max_connections`. api and worker each
/// hold their own pool (separate `connect()` calls), so size each to that
/// process's own concurrency, not the MVP's user-facing scale — the API
/// server's human-triggered load is small, but the worker's is driven by
/// `worker::MAX_CONCURRENT_CHECKS`; a pool much smaller than that would
/// serialize checks behind pool-acquire waits (and risk hitting
/// `acquire_timeout`) despite the semaphore/`JoinSet` design intending them
/// to run in parallel. Callers should derive their value from what they
/// actually need rather than guessing, as `bin/worker.rs` does.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
}

/// Applies pending migrations. Only the API server calls this on startup —
/// the worker never migrates, so the two containers can't race on it.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
