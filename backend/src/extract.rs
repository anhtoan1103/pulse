//! Custom extractors shared by handlers.

use crate::{app::AppState, error::ApiError};
use axum::{
    extract::{ConnectInfo, FromRequest, FromRequestParts},
    http::request::Parts,
};
use std::net::{IpAddr, SocketAddr};

/// `axum::extract::Path`, but rejections (e.g. a malformed UUID) render in
/// the standard API error format.
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

/// `axum::Json`, but rejections render in the standard API error format.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

/// Client IP for rate limiting: from `AuthConfig::client_ip_header` when
/// configured (behind Cloudflare Tunnel every TCP peer is `cloudflared`
/// itself), otherwise the TCP peer address.
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if let Some(header) = &state.client_ip_header {
            return parts
                .headers
                .get(&**header)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok())
                .map(ClientIp)
                .ok_or_else(|| ApiError::bad_request("could not determine client address"));
        }

        // Via the real extractor (not a raw extension lookup) so axum's
        // `MockConnectInfo` works in tests.
        ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map(|ConnectInfo(addr)| ClientIp(addr.ip()))
            .map_err(|_| {
                tracing::error!(
                    "ConnectInfo missing — serve with into_make_service_with_connect_info"
                );
                ApiError::internal()
            })
    }
}
