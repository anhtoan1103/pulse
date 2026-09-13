//! JWT access tokens (HS256), pulse-security.md #2.
//!
//! Claims carry only the user id — role and `is_active` are re-read from the
//! DB on every request (see [`super::AuthUser`]), so disabling a user or
//! changing their role takes effect immediately instead of at token expiry.

use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub iat: i64,
    pub exp: i64,
}

pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl: Duration,
}

impl JwtKeys {
    pub fn new(secret: &[u8], ttl: Duration) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["exp", "sub"]);
        Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
            validation,
            ttl,
        }
    }

    pub fn issue(&self, user_id: Uuid) -> anyhow::Result<String> {
        self.issue_at(user_id, Utc::now())
    }

    /// Like [`Self::issue`] with an explicit issue time — lets tests mint
    /// already-expired tokens.
    pub fn issue_at(&self, user_id: Uuid, issued_at: DateTime<Utc>) -> anyhow::Result<String> {
        let claims = Claims {
            sub: user_id,
            iat: issued_at.timestamp(),
            exp: (issued_at + self.ttl).timestamp(),
        };
        jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .context("encoding JWT")
    }

    /// `None` for any invalid token: bad signature, expired, malformed, wrong alg.
    pub fn verify(&self, token: &str) -> Option<Claims> {
        jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation)
            .ok()
            .map(|data| data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-secret-that-is-at-least-32-bytes-long";

    #[test]
    fn roundtrip() {
        let keys = JwtKeys::new(SECRET, Duration::hours(24));
        let id = Uuid::new_v4();
        let claims = keys.verify(&keys.issue(id).unwrap()).unwrap();
        assert_eq!(claims.sub, id);
    }

    #[test]
    fn rejects_expired() {
        let keys = JwtKeys::new(SECRET, Duration::hours(1));
        let token = keys
            .issue_at(Uuid::new_v4(), Utc::now() - Duration::hours(3))
            .unwrap();
        assert!(keys.verify(&token).is_none());
    }

    #[test]
    fn rejects_other_secret_and_garbage() {
        let keys = JwtKeys::new(SECRET, Duration::hours(1));
        let other = JwtKeys::new(
            b"another-secret-that-is-32-bytes-long!!",
            Duration::hours(1),
        );
        assert!(keys.verify(&other.issue(Uuid::new_v4()).unwrap()).is_none());
        assert!(keys.verify("not.a.jwt").is_none());
    }
}
