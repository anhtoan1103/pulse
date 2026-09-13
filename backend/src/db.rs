//! Postgres pool setup + embedded migrations (`backend/migrations/`, schema
//! per `docs/pulse-database-schema.md`).

use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use std::time::Duration;

/// Migrations are embedded into the binary at compile time, so the runtime
/// image doesn't need the `migrations/` directory to apply them.
pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Opens a connection pool. Small pool on purpose: MVP load is ≤10 users /
/// ≤50 endpoints each, and api + worker each hold their own pool.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
}

/// Applies pending migrations. Only the API server calls this on startup —
/// the worker never migrates, so the two containers can't race on it.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}
