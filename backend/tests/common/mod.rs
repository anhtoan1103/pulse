//! Shared helpers for API integration tests: drives the real router
//! in-process (no network) against a `#[sqlx::test]` database.

#![allow(dead_code)] // each test binary uses a different subset

use axum::{
    Router,
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use pulse_backend::{
    app::{self, AppState},
    config::AuthConfig,
    rate_limit::RateLimiter,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{net::SocketAddr, time::Duration};
use tower::ServiceExt;

pub const TEST_JWT_SECRET: &str = "test-secret-that-is-at-least-32-bytes-long";
pub const PASSWORD: &str = "correct horse battery";

pub fn auth_config() -> AuthConfig {
    AuthConfig {
        jwt_secret: TEST_JWT_SECRET.into(),
        jwt_expiry_hours: 24,
        client_ip_header: None,
        admin_seed: None,
    }
}

#[derive(Clone)]
pub struct TestApp {
    pub state: AppState,
    router: Router,
}

impl TestApp {
    /// App with a rate limiter loose enough that ordinary tests never hit it.
    pub fn new(pool: PgPool) -> Self {
        let state = AppState::new(pool, &auth_config())
            .with_auth_rate_limiter(RateLimiter::new(1_000, Duration::from_secs(60)));
        Self::from_state(state)
    }

    /// App with a caller-built state (e.g. production rate limits).
    pub fn from_state(state: AppState) -> Self {
        let router = app::router(state.clone())
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 7], 40000))));
        Self { state, router }
    }

    pub fn pool(&self) -> &PgPool {
        &self.state.pool
    }

    /// Sends a request; returns status + body parsed as JSON (`Null` when
    /// empty, a JSON string when the body isn't JSON).
    pub async fn request(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        self.request_with_headers(method, uri, body, token, &[])
            .await
    }

    pub async fn request_with_headers(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        token: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let req = match body {
            Some(body) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();

        self.send(req).await
    }

    pub async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        (status, body)
    }

    pub async fn register(&self, email: &str, password: &str) -> (StatusCode, Value) {
        self.request(
            Method::POST,
            "/api/v1/auth/register",
            Some(json!({ "email": email, "password": password })),
            None,
        )
        .await
    }

    pub async fn login(&self, email: &str, password: &str) -> (StatusCode, Value) {
        self.request(
            Method::POST,
            "/api/v1/auth/login",
            Some(json!({ "email": email, "password": password })),
            None,
        )
        .await
    }

    /// Registers `email` with [`PASSWORD`] and returns a valid token.
    pub async fn register_and_login(&self, email: &str) -> String {
        let (status, body) = self.register(email, PASSWORD).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let (status, body) = self.login(email, PASSWORD).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().to_owned()
    }
}

/// Asserts the standard error body (api-spec #7) with the given code.
#[track_caller]
pub fn assert_error(body: &Value, code: &str) {
    assert_eq!(body["error"]["code"], code, "{body}");
    assert!(body["error"]["message"].is_string(), "{body}");
}
