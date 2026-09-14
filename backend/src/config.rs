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

/// How the SMTP connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// TLS from the first byte (usually port 465).
    Wrapper,
    /// Plaintext, then mandatory STARTTLS (usually port 587). Never falls
    /// back to plaintext.
    StartTls,
    /// No encryption — only for a local dev relay (e.g. Mailpit).
    None,
}

/// SMTP settings for the Notification Service (worker only).
///
/// No `Debug` derive: holds the SMTP password.
#[derive(Clone)]
pub struct EmailConfig {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// `None` for relays without auth (local dev).
    pub credentials: Option<(String, String)>,
    /// e.g. `noreply@toan.uk` or `Pulse <noreply@toan.uk>`.
    pub from: String,
    /// Base URL for links in emails, e.g. `https://pulse.toan.uk`.
    pub frontend_url: String,
}

impl EmailConfig {
    /// `Ok(None)` when `SMTP_HOST` is unset — email is then disabled and
    /// notifications stay `pending` until SMTP is configured.
    ///
    /// `SMTP_TLS`: `tls` | `starttls` | `none`; default `tls` for port 465,
    /// otherwise `starttls`.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(host) = non_empty_var("SMTP_HOST") else {
            return Ok(None);
        };
        let port = match non_empty_var("SMTP_PORT") {
            Some(p) => p
                .trim()
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("SMTP_PORT must be a port number"))?,
            None => 587,
        };
        let tls = match non_empty_var("SMTP_TLS").map(|v| v.trim().to_ascii_lowercase()) {
            None if port == 465 => SmtpTls::Wrapper,
            None => SmtpTls::StartTls,
            Some(v) if v == "tls" => SmtpTls::Wrapper,
            Some(v) if v == "starttls" => SmtpTls::StartTls,
            Some(v) if v == "none" => SmtpTls::None,
            Some(other) => anyhow::bail!("SMTP_TLS must be tls, starttls or none (got {other:?})"),
        };
        let credentials = match (non_empty_var("SMTP_USER"), non_empty_var("SMTP_PASSWORD")) {
            (Some(user), Some(password)) => Some((user.trim().to_string(), password)),
            (None, None) => None,
            _ => anyhow::bail!("SMTP_USER and SMTP_PASSWORD must be set together"),
        };
        let from = non_empty_var("SMTP_FROM")
            .ok_or_else(|| anyhow::anyhow!("SMTP_FROM must be set when SMTP_HOST is set"))?;
        let frontend_url = non_empty_var("FRONTEND_URL")
            .unwrap_or_else(|| "http://localhost:3000".into())
            .trim()
            .trim_end_matches('/')
            .to_string();

        Ok(Some(Self {
            host: host.trim().to_string(),
            port,
            tls,
            credentials,
            from: from.trim().to_string(),
            frontend_url,
        }))
    }
}

/// LLM provider settings for the AI Analysis Service (worker only).
/// Providers are reached through their OpenAI-compatible APIs, so switching
/// provider is just config (project-context #2).
///
/// No `Debug` derive: holds the API key.
#[derive(Clone)]
pub struct AiConfig {
    /// Base URL up to (not including) `/chat/completions`.
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl AiConfig {
    /// `Ok(None)` when `AI_API_KEY` is unset — AI analysis is then disabled
    /// and incidents stay `pending` until a key is configured.
    ///
    /// - `AI_PROVIDER`: `gemini` | `groq` (picks the default base URL)
    /// - `AI_BASE_URL`: optional override (any OpenAI-compatible endpoint)
    /// - `AI_MODEL`: required when a key is set — model ids change often, so
    ///   there's deliberately no built-in default
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(api_key) = non_empty_var("AI_API_KEY") else {
            return Ok(None);
        };
        let provider = non_empty_var("AI_PROVIDER").unwrap_or_else(|| "gemini".into());
        let base_url = match non_empty_var("AI_BASE_URL") {
            Some(url) => url,
            None => match provider.trim().to_ascii_lowercase().as_str() {
                "gemini" => "https://generativelanguage.googleapis.com/v1beta/openai".into(),
                "groq" => "https://api.groq.com/openai/v1".into(),
                other => anyhow::bail!(
                    "unknown AI_PROVIDER {other:?} (expected gemini or groq, or set AI_BASE_URL)"
                ),
            },
        };
        let model = non_empty_var("AI_MODEL")
            .ok_or_else(|| anyhow::anyhow!("AI_MODEL must be set when AI_API_KEY is set"))?;

        Ok(Some(Self {
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
        }))
    }
}
