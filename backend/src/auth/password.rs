//! Argon2id password hashing (pulse-security.md #2).
//!
//! Hashing is intentionally CPU/memory-heavy, so it runs on the blocking
//! thread pool instead of stalling the async runtime.

use anyhow::{Context, anyhow};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use std::sync::LazyLock;

/// Hash of a random throwaway password. Login verifies against it when the
/// email doesn't exist (or has no password), so response time doesn't reveal
/// which emails are registered.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let random = uuid::Uuid::new_v4();
    hash_blocking(random.as_bytes()).expect("hashing dummy password")
});

fn hash_blocking(password: &[u8]) -> anyhow::Result<String> {
    Argon2::default()
        .hash_password(password)
        .map(|h| h.to_string())
        .map_err(|e| anyhow!("argon2 hash failed: {e}"))
}

/// Returns an Argon2id PHC string (`$argon2id$v=19$...`), params + salt included.
pub async fn hash(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || hash_blocking(password.as_bytes()))
        .await
        .context("hash task panicked")?
}

/// Checks `password` against a stored PHC string. `None` (unknown user or
/// OAuth-only account) still burns a full verification and returns `false`.
pub async fn verify(password: String, stored_hash: Option<String>) -> anyhow::Result<bool> {
    tokio::task::spawn_blocking(move || {
        let (phc, is_real) = match &stored_hash {
            Some(h) => (h.as_str(), true),
            None => (DUMMY_HASH.as_str(), false),
        };
        let parsed = PasswordHash::new(phc).map_err(|e| anyhow!("stored hash unparseable: {e}"))?;
        let matches = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(is_real && matches)
    })
    .await
    .context("verify task panicked")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_then_verify() {
        let h = hash("correct horse battery".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(
            verify("correct horse battery".into(), Some(h.clone()))
                .await
                .unwrap()
        );
        assert!(!verify("wrong password".into(), Some(h)).await.unwrap());
    }

    #[tokio::test]
    async fn missing_hash_never_verifies() {
        assert!(!verify("anything".into(), None).await.unwrap());
    }

    #[tokio::test]
    async fn same_password_gets_distinct_salts() {
        let a = hash("same password".into()).await.unwrap();
        let b = hash("same password".into()).await.unwrap();
        assert_ne!(a, b);
    }
}
