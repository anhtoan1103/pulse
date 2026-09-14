//! Minimal client for OpenAI-compatible chat completions (Gemini and Groq
//! both expose one), requesting JSON-schema structured output
//! (docs/pulse-ai-design.md #3, #6).
//!
//! Security (pulse-security.md #5): the API key is only ever placed in the
//! Authorization header; neither it nor prompts/replies are logged. Errors
//! carry a short user-safe message plus a log-only detail.

use super::prompt::Message;
use crate::{config::AiConfig, tls};
use reqwest::{StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DETAIL_MAX_LEN: usize = 200;

#[derive(Debug, PartialEq, Eq)]
pub enum LlmError {
    /// Worth retrying later: rate limit, 5xx, timeout, network error.
    Transient {
        message: String,
        retry_after: Option<Duration>,
    },
    /// Retrying won't help: bad key, unknown model, malformed request.
    Permanent { message: String, detail: String },
}

pub struct LlmClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl LlmClient {
    pub fn new(config: &AiConfig, timeout: Duration) -> anyhow::Result<Self> {
        tls::install_crypto_provider();
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            // Never follow redirects with the Authorization header attached.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            endpoint: format!("{}/chat/completions", config.base_url),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        })
    }

    /// Sends `messages` and returns the assistant's text content (possibly
    /// empty, e.g. on a refusal — the caller's validation handles that).
    pub async fn complete(&self, messages: &[Message], schema: &Value) -> Result<String, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            // Low temperature: we want grounded, repeatable analysis.
            "temperature": 0.2,
            "response_format": {
                "type": "json_schema",
                "json_schema": { "name": "incident_analysis", "schema": schema }
            }
        });

        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| LlmError::Transient {
                message: if e.is_timeout() {
                    "AI provider timed out".into()
                } else {
                    "could not reach AI provider".into()
                },
                retry_after: None,
            })?;

        let status = response.status();
        let retry_after = parse_retry_after(response.headers());
        let text = response.text().await.map_err(|_| LlmError::Transient {
            message: "AI provider response was interrupted".into(),
            retry_after: None,
        })?;

        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(LlmError::Transient {
                message: "AI provider rate limit reached".into(),
                retry_after,
            });
        }
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return Err(LlmError::Transient {
                message: format!("AI provider error (HTTP {})", status.as_u16()),
                retry_after,
            });
        }
        if !status.is_success() {
            return Err(LlmError::Permanent {
                message: format!(
                    "AI provider rejected the request (HTTP {})",
                    status.as_u16()
                ),
                detail: provider_error_detail(&text),
            });
        }

        let parsed: Value = serde_json::from_str(&text).map_err(|_| LlmError::Transient {
            message: "AI provider returned a malformed response".into(),
            retry_after: None,
        })?;
        Ok(parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}

/// `Retry-After` in seconds (the HTTP-date form is ignored).
fn parse_retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// The provider's `error.message`, truncated — for logs only.
fn provider_error_detail(body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            // OpenAI shape `{"error": {"message": ...}}`; Gemini sometimes
            // returns a list of those.
            let err = if v.is_array() {
                &v[0]["error"]
            } else {
                &v["error"]
            };
            err["message"].as_str().map(String::from)
        })
        .unwrap_or_else(|| "no error message".into());
    message.chars().take(DETAIL_MAX_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds() {
        let mut h = header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
        h.insert(header::RETRY_AFTER, " 120 ".parse().unwrap());
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(120)));
        h.insert(
            header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn provider_error_detail_shapes() {
        assert_eq!(
            provider_error_detail(r#"{"error":{"message":"model not found"}}"#),
            "model not found"
        );
        assert_eq!(
            provider_error_detail(r#"[{"error":{"message":"API key not valid"}}]"#),
            "API key not valid"
        );
        assert_eq!(provider_error_detail("<html>"), "no error message");
        assert_eq!(
            provider_error_detail(&format!(
                r#"{{"error":{{"message":"{}"}}}}"#,
                "x".repeat(500)
            ))
            .len(),
            DETAIL_MAX_LEN
        );
    }
}
