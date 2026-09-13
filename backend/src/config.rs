//! App configuration, loaded from environment variables (see `.env.example`
//! at the repo root). Kept intentionally small at skeleton stage — grows as
//! each doc section (auth, AI provider, SMTP, ...) gets implemented.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    pub app_env: String,
    /// Address the API server binds to, e.g. "0.0.0.0:8080".
    pub api_bind_addr: String,
}

impl Config {
    /// Loads config from environment variables. Calls `dotenvy::dotenv()`
    /// first so a local `.env` file (repo root) is picked up in dev; in
    /// containers/production real env vars are used and `.env` is absent,
    /// which is fine — `dotenvy::dotenv()` silently no-ops if the file
    /// doesn't exist.
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();

        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://pulse:pulse@localhost:5432/pulse".into()),
            redis_url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into()),
            app_env: env::var("APP_ENV").unwrap_or_else(|_| "development".into()),
            api_bind_addr: env::var("API_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
        }
    }
}

/// Auth settings — API server only (the worker never needs them, so they're
/// kept out of [`Config`] to avoid making `JWT_SECRET` mandatory there).
///
/// Deliberately no `Debug` derive: it holds secrets that must never end up in
/// logs (pulse-security.md #5).
#[derive(Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub jwt_expiry_hours: i64,
    /// Header carrying the real client IP when running behind a proxy, e.g.
    /// `CF-Connecting-IP` behind Cloudflare Tunnel. Unset = use the TCP peer
    /// address. Only set this when the API is reachable *exclusively* through
    /// that proxy — otherwise clients can spoof the header to dodge rate limits.
    pub client_ip_header: Option<String>,
    /// First-admin seed (schema doc #2.1b). Both or neither must be set.
    pub admin_seed: Option<AdminSeed>,
}

#[derive(Clone)]
pub struct AdminSeed {
    pub email: String,
    pub password: String,
}

/// HS256 secrets shorter than this are too weak (`openssl rand -hex 32`
/// gives 64 chars).
const MIN_JWT_SECRET_LEN: usize = 32;

impl AuthConfig {
    /// Loads auth settings from env (call [`Config::from_env`] first so
    /// `.env` is loaded). Fails loudly on missing/weak secrets rather than
    /// starting with an insecure default.
    pub fn from_env() -> anyhow::Result<Self> {
        let jwt_secret = non_empty_var("JWT_SECRET").ok_or_else(|| {
            anyhow::anyhow!("JWT_SECRET must be set (generate: openssl rand -hex 32)")
        })?;
        anyhow::ensure!(
            jwt_secret.len() >= MIN_JWT_SECRET_LEN,
            "JWT_SECRET must be at least {MIN_JWT_SECRET_LEN} characters"
        );

        let jwt_expiry_hours = match non_empty_var("JWT_EXPIRY_HOURS") {
            Some(v) => v
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|h| *h > 0)
                .ok_or_else(|| anyhow::anyhow!("JWT_EXPIRY_HOURS must be a positive integer"))?,
            None => 24,
        };

        let admin_seed = match (
            non_empty_var("ADMIN_SEED_EMAIL"),
            non_empty_var("ADMIN_SEED_PASSWORD"),
        ) {
            (Some(email), Some(password)) => Some(AdminSeed { email, password }),
            (None, None) => None,
            _ => anyhow::bail!("ADMIN_SEED_EMAIL and ADMIN_SEED_PASSWORD must be set together"),
        };

        Ok(Self {
            jwt_secret,
            jwt_expiry_hours,
            client_ip_header: non_empty_var("CLIENT_IP_HEADER"),
            admin_seed,
        })
    }
}

fn non_empty_var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}
