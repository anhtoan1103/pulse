//! Performs the actual HTTP check against a monitored endpoint
//! (docs/pulse-architecture.md #2.2) with the request-time SSRF guarantees
//! from pulse-security.md #1:
//!
//! - **DNS re-checked at connect time.** The client's DNS resolver refuses
//!   hostnames that resolve to any forbidden address, and the addresses it
//!   returns are the ones actually connected to — so DNS rebinding after
//!   endpoint creation can't slip through (no check-then-connect gap).
//! - **Literal IPs** (which skip DNS) are checked before sending.
//! - **Redirects** are re-validated hop by hop (scheme, literal IP; hostnames
//!   go through the guarded resolver again), max 5 hops.
//! - **Timeouts** on connect and on the whole request; proxies disabled so
//!   env vars can't reroute traffic around the guard.
//! - **No connection reuse**, so every check's latency includes connection
//!   setup consistently.

use crate::ssrf;
use reqwest::{
    Method,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};
use std::{
    error::Error as StdError,
    fmt,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use url::{Host, Url};

const USER_AGENT: &str = concat!("PulseMonitor/", env!("CARGO_PKG_VERSION"));
const ERROR_MESSAGE_MAX_LEN: usize = 300;

pub struct CheckerSettings {
    /// Whole request, from connect to response headers.
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub max_redirects: usize,
    /// Address policy. Always [`ssrf::is_forbidden_ip`] in production; tests
    /// relax it to reach a local test server.
    pub is_forbidden: fn(IpAddr) -> bool,
}

impl Default for CheckerSettings {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(5),
            max_redirects: 5,
            is_forbidden: ssrf::is_forbidden_ip,
        }
    }
}

/// Result of one check, shaped like a `checks` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// `None` when no HTTP response was received.
    pub status_code: Option<i32>,
    /// Time to response headers; `None` when no response was received.
    pub latency_ms: Option<i32>,
    /// 2xx response (after redirects) within the timeout.
    pub success: bool,
    /// Why no response was received. `None` whenever a response arrived —
    /// the status code already describes non-2xx outcomes.
    pub error_message: Option<String>,
}

impl CheckOutcome {
    fn failed(message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > ERROR_MESSAGE_MAX_LEN {
            let cut = (0..=ERROR_MESSAGE_MAX_LEN)
                .rev()
                .find(|i| message.is_char_boundary(*i))
                .unwrap_or(0);
            message.truncate(cut);
        }
        Self {
            status_code: None,
            latency_ms: None,
            success: false,
            error_message: Some(message),
        }
    }
}

pub struct HttpChecker {
    client: reqwest::Client,
    timeout: Duration,
    is_forbidden: fn(IpAddr) -> bool,
}

impl HttpChecker {
    pub fn new(settings: CheckerSettings) -> anyhow::Result<Self> {
        crate::tls::install_crypto_provider();
        let CheckerSettings {
            timeout,
            connect_timeout,
            max_redirects,
            is_forbidden,
        } = settings;

        let redirect_policy = redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= max_redirects {
                return attempt.error(format!("more than {max_redirects} redirects"));
            }
            match literal_target_problem(attempt.url(), is_forbidden) {
                Some(problem) => attempt.error(problem),
                None => attempt.follow(),
            }
        });

        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .connect_timeout(connect_timeout)
            .no_proxy()
            // Fresh connection per check: with keep-alive pooling only the
            // first check pays DNS + TCP + TLS, so latency would jump whenever
            // a pooled connection is dropped — false latency anomalies.
            .pool_max_idle_per_host(0)
            .redirect(redirect_policy)
            .dns_resolver(Arc::new(GuardedResolver { is_forbidden }))
            .build()?;

        Ok(Self {
            client,
            timeout,
            is_forbidden,
        })
    }

    /// Checks `url` with `method`. Never errors: every failure mode is a
    /// failed [`CheckOutcome`]. The response body is never downloaded.
    pub async fn check(&self, method: &str, url: &str) -> CheckOutcome {
        let url = match Url::parse(url) {
            Ok(url) => url,
            Err(_) => return CheckOutcome::failed("invalid url"),
        };
        if let Some(problem) = literal_target_problem(&url, self.is_forbidden) {
            return CheckOutcome::failed(problem.to_string());
        }
        let method = match Method::from_bytes(method.as_bytes()) {
            Ok(m) => m,
            Err(_) => return CheckOutcome::failed("invalid method"),
        };

        let started = Instant::now();
        match self.client.request(method, url).send().await {
            Ok(response) => {
                let latency_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
                let status = response.status();
                // Dropping `response` without reading closes the connection.
                CheckOutcome {
                    status_code: Some(i32::from(status.as_u16())),
                    latency_ms: Some(latency_ms),
                    success: status.is_success(),
                    error_message: None,
                }
            }
            Err(err) => CheckOutcome::failed(self.describe_error(&err)),
        }
    }

    fn describe_error(&self, err: &reqwest::Error) -> String {
        if let Some(blocked) = find_in_chain::<TargetBlocked>(err) {
            return blocked.to_string();
        }
        if err.is_timeout() {
            return format!("timeout after {}s", self.timeout.as_secs_f32());
        }
        let root = root_cause(err);
        if err.is_redirect() {
            format!("redirect failed: {root}")
        } else if err.is_connect() {
            format!("connection failed: {root}")
        } else {
            format!("request failed: {root}")
        }
    }
}

/// Error for targets the SSRF policy refuses. Found again in reqwest's error
/// source chain to produce a clear message.
#[derive(Debug)]
struct TargetBlocked(&'static str);

impl fmt::Display for TargetBlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "blocked: {}", self.0)
    }
}

impl StdError for TargetBlocked {}

const BLOCKED_ADDRESS: TargetBlocked =
    TargetBlocked("target resolves to a private, loopback, link-local or reserved address");

/// Checks that don't need DNS: scheme and literal-IP hosts.
fn literal_target_problem(url: &Url, is_forbidden: fn(IpAddr) -> bool) -> Option<TargetBlocked> {
    if !matches!(url.scheme(), "http" | "https") {
        return Some(TargetBlocked("url scheme must be http or https"));
    }
    let ip = match url.host()? {
        Host::Ipv4(ip) => IpAddr::V4(ip),
        Host::Ipv6(ip) => IpAddr::V6(ip),
        Host::Domain(_) => return None,
    };
    is_forbidden(ip).then_some(BLOCKED_ADDRESS)
}

/// System DNS, but fails the lookup if any resolved address is forbidden.
struct GuardedResolver {
    is_forbidden: fn(IpAddr) -> bool,
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let is_forbidden = self.is_forbidden;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // Port 0: reqwest substitutes the URL's port.
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() {
                return Err(format!("no addresses found for {host}").into());
            }
            if addrs.iter().any(|a| is_forbidden(a.ip())) {
                return Err(Box::new(BLOCKED_ADDRESS) as Box<dyn StdError + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn find_in_chain<'a, T: StdError + 'static>(err: &'a (dyn StdError + 'static)) -> Option<&'a T> {
    let mut current = Some(err);
    while let Some(e) = current {
        if let Some(found) = e.downcast_ref::<T>() {
            return Some(found);
        }
        current = e.source();
    }
    None
}

fn root_cause(err: &(dyn StdError + 'static)) -> String {
    let mut current = err;
    while let Some(source) = current.source() {
        current = source;
    }
    current.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        http::StatusCode,
        response::Redirect,
        routing::{any, get},
    };
    use tokio::net::TcpListener;

    /// Test policy: loopback allowed (so we can reach the local test server),
    /// everything else follows the real policy.
    fn allow_loopback(ip: IpAddr) -> bool {
        !ip.is_loopback() && ssrf::is_forbidden_ip(ip)
    }

    fn checker(is_forbidden: fn(IpAddr) -> bool) -> HttpChecker {
        HttpChecker::new(CheckerSettings {
            timeout: Duration::from_millis(500),
            connect_timeout: Duration::from_millis(500),
            max_redirects: 2,
            is_forbidden,
        })
        .unwrap()
    }

    async fn serve(router: Router) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        addr
    }

    fn test_server() -> Router {
        Router::new()
            .route("/ok", get(|| async { "ok" }))
            .route(
                "/method",
                any(|m: axum::http::Method| async move {
                    if m == axum::http::Method::POST {
                        StatusCode::OK
                    } else {
                        StatusCode::METHOD_NOT_ALLOWED
                    }
                }),
            )
            .route("/down", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    "late"
                }),
            )
            .route("/redirect-ok", get(|| async { Redirect::temporary("/ok") }))
            .route(
                "/redirect-metadata",
                get(|| async { Redirect::temporary("http://169.254.169.254/latest/meta-data/") }),
            )
            .route(
                "/redirect-loop",
                get(|| async { Redirect::temporary("/redirect-loop") }),
            )
    }

    #[tokio::test]
    async fn success_records_status_and_latency() {
        let addr = serve(test_server()).await;
        let outcome = checker(allow_loopback)
            .check("GET", &format!("http://{addr}/ok"))
            .await;
        assert_eq!(outcome.status_code, Some(200));
        assert!(outcome.success);
        assert!(outcome.latency_ms.is_some());
        assert_eq!(outcome.error_message, None);
    }

    #[tokio::test]
    async fn non_2xx_is_a_failed_check_with_status() {
        let addr = serve(test_server()).await;
        let outcome = checker(allow_loopback)
            .check("GET", &format!("http://{addr}/down"))
            .await;
        assert_eq!(outcome.status_code, Some(503));
        assert!(!outcome.success);
        assert!(outcome.latency_ms.is_some());
        assert_eq!(outcome.error_message, None);
    }

    #[tokio::test]
    async fn uses_configured_method() {
        let addr = serve(test_server()).await;
        let c = checker(allow_loopback);
        assert_eq!(
            c.check("POST", &format!("http://{addr}/method"))
                .await
                .status_code,
            Some(200)
        );
        assert_eq!(
            c.check("GET", &format!("http://{addr}/method"))
                .await
                .status_code,
            Some(405)
        );
    }

    #[tokio::test]
    async fn timeout_is_reported() {
        let addr = serve(test_server()).await;
        let outcome = checker(allow_loopback)
            .check("GET", &format!("http://{addr}/slow"))
            .await;
        assert_eq!(outcome.status_code, None);
        assert_eq!(outcome.latency_ms, None);
        assert!(!outcome.success);
        assert_eq!(outcome.error_message.as_deref(), Some("timeout after 0.5s"));
    }

    #[tokio::test]
    async fn connection_refused_is_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        // Longer timeouts than `checker()`: Windows retries refused connects
        // for ~2s before failing, Linux fails immediately.
        let checker = HttpChecker::new(CheckerSettings {
            timeout: Duration::from_secs(8),
            connect_timeout: Duration::from_secs(8),
            max_redirects: 2,
            is_forbidden: allow_loopback,
        })
        .unwrap();
        let outcome = checker.check("GET", &format!("http://{addr}/")).await;
        assert!(!outcome.success);
        assert_eq!(outcome.status_code, None);
        let msg = outcome.error_message.unwrap();
        assert!(msg.starts_with("connection failed"), "{msg}");
    }

    #[tokio::test]
    async fn follows_safe_redirects() {
        let addr = serve(test_server()).await;
        let outcome = checker(allow_loopback)
            .check("GET", &format!("http://{addr}/redirect-ok"))
            .await;
        assert_eq!(outcome.status_code, Some(200));
        assert!(outcome.success);
    }

    #[tokio::test]
    async fn blocks_redirect_to_internal_address() {
        let addr = serve(test_server()).await;
        let outcome = checker(allow_loopback)
            .check("GET", &format!("http://{addr}/redirect-metadata"))
            .await;
        assert!(!outcome.success);
        assert_eq!(outcome.status_code, None);
        let msg = outcome.error_message.unwrap();
        assert!(msg.starts_with("blocked:"), "{msg}");
    }

    #[tokio::test]
    async fn stops_redirect_loops() {
        let addr = serve(test_server()).await;
        let outcome = checker(allow_loopback)
            .check("GET", &format!("http://{addr}/redirect-loop"))
            .await;
        assert!(!outcome.success);
        let msg = outcome.error_message.unwrap();
        assert!(msg.contains("redirect"), "{msg}");
    }

    /// With the production policy, a live server on loopback is unreachable
    /// both by literal IP and by a hostname that resolves to it.
    #[tokio::test]
    async fn production_policy_blocks_loopback_literal_and_hostname() {
        let addr = serve(test_server()).await;
        let c = checker(ssrf::is_forbidden_ip);
        for url in [
            format!("http://{addr}/ok"),
            format!("http://localhost:{}/ok", addr.port()),
        ] {
            let outcome = c.check("GET", &url).await;
            assert!(!outcome.success, "{url}");
            assert_eq!(outcome.status_code, None, "{url}");
            let msg = outcome.error_message.unwrap();
            assert!(msg.starts_with("blocked:"), "{url}: {msg}");
        }
    }

    #[tokio::test]
    async fn rejects_non_http_scheme_and_garbage() {
        let c = checker(ssrf::is_forbidden_ip);
        assert!(
            c.check("GET", "file:///etc/passwd")
                .await
                .error_message
                .unwrap()
                .starts_with("blocked:")
        );
        assert_eq!(
            c.check("GET", "not a url").await.error_message.as_deref(),
            Some("invalid url")
        );
    }

    #[test]
    fn long_error_messages_are_truncated() {
        let outcome = CheckOutcome::failed("é".repeat(400));
        assert!(outcome.error_message.unwrap().len() <= ERROR_MESSAGE_MAX_LEN);
    }
}
