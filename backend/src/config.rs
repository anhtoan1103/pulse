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
