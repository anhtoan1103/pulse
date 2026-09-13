//! First-admin seed from `ADMIN_SEED_EMAIL` / `ADMIN_SEED_PASSWORD`
//! (schema doc #2.1b: seed one admin at deploy instead of a promotion flow).

use super::{insert_email_user, lock_user_creation, password, validate};
use crate::config::AdminSeed;
use anyhow::anyhow;
use sqlx::PgPool;
use tracing::{info, warn};

/// What [`seed_admin`] did — returned so tests can assert on it.
#[derive(Debug, PartialEq, Eq)]
pub enum SeedOutcome {
    Created,
    /// An admin already exists; nothing to do. The seed password is never
    /// re-applied, so changing it after first login sticks.
    AdminExists,
    /// A non-admin account already uses the seed email. Promoting it would
    /// hand admin to whoever registered that address, so we refuse.
    EmailTakenByNonAdmin,
}

/// Creates the seed admin iff no admin exists yet. Safe to call on every
/// startup. Doesn't count against `MAX_USERS` (the operator configured it).
pub async fn seed_admin(pool: &PgPool, seed: &AdminSeed) -> anyhow::Result<SeedOutcome> {
    let email = validate::normalize_email(&seed.email)
        .map_err(|e| anyhow!("ADMIN_SEED_EMAIL: {}", e.message))?;
    validate::validate_password(&seed.password)
        .map_err(|e| anyhow!("ADMIN_SEED_PASSWORD: {}", e.message))?;

    let mut tx = pool.begin().await?;
    lock_user_creation(&mut tx).await?;

    let admin_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'admin')")
            .fetch_one(&mut *tx)
            .await?;
    if admin_exists {
        return Ok(SeedOutcome::AdminExists);
    }

    let email_taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE lower(email) = $1)")
            .bind(&email)
            .fetch_one(&mut *tx)
            .await?;
    if email_taken {
        warn!(
            "ADMIN_SEED_EMAIL belongs to an existing non-admin account; not promoting it — no admin seeded"
        );
        return Ok(SeedOutcome::EmailTakenByNonAdmin);
    }

    let password_hash = password::hash(seed.password.clone()).await?;
    insert_email_user(&mut tx, &email, &password_hash, "admin").await?;
    tx.commit().await?;

    info!(email = %email, "seeded admin account — change its password after first login");
    Ok(SeedOutcome::Created)
}
