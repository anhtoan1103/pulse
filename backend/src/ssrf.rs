//! SSRF protection for user-supplied monitoring URLs — pulse-security.md #1.
//!
//! Pulse calls arbitrary URLs on users' behalf, so a URL must never let a
//! user reach localhost, the Docker network (postgres/redis), the LAN, or
//! cloud metadata endpoints. Enforced here when an endpoint is created or
//! updated; the Checker Worker (step 5) must re-resolve and re-check at
//! request time too, since DNS can change after validation (rebinding).

use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};
use url::{Host, Url};

pub const URL_MAX_LEN: usize = 2048;
const DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// How hostnames get resolved. `Static` exists so tests (and anything else
/// that must not depend on real DNS) can pin host → IP mappings.
pub enum HostResolver {
    System,
    Static(HashMap<String, Vec<IpAddr>>),
}

impl HostResolver {
    pub async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        match self {
            Self::System => {
                let lookup = tokio::net::lookup_host((host, port));
                let addrs = tokio::time::timeout(DNS_TIMEOUT, lookup)
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "DNS lookup timed out")
                    })??;
                Ok(addrs.map(|a| a.ip()).collect())
            }
            Self::Static(map) => map
                .get(&host.to_ascii_lowercase())
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host not found")),
        }
    }
}

/// Why a URL was rejected. Messages are safe to show to the user.
#[derive(Debug, PartialEq, Eq)]
pub enum UrlRejection {
    TooLong,
    Malformed,
    UnsupportedScheme,
    HasCredentials,
    MissingHost,
    Unresolvable,
    ForbiddenAddress,
}

impl UrlRejection {
    pub fn message(&self) -> String {
        match self {
            Self::TooLong => format!("url must be at most {URL_MAX_LEN} characters"),
            Self::Malformed => "url is not a valid URL".into(),
            Self::UnsupportedScheme => "url scheme must be http or https".into(),
            Self::HasCredentials => "url must not contain a username or password".into(),
            Self::MissingHost => "url must include a host".into(),
            Self::Unresolvable => "url host could not be resolved".into(),
            Self::ForbiddenAddress => {
                "url must not point to a private, loopback, link-local or reserved address".into()
            }
        }
    }
}

/// Parses and vets a monitoring target URL. Returns the normalized URL.
///
/// A hostname is rejected if *any* address it resolves to is forbidden —
/// otherwise an attacker could mix one public and one internal record.
pub async fn validate_target_url(raw: &str, resolver: &HostResolver) -> Result<Url, UrlRejection> {
    let raw = raw.trim();
    if raw.len() > URL_MAX_LEN {
        return Err(UrlRejection::TooLong);
    }
    let url = Url::parse(raw).map_err(|_| UrlRejection::Malformed)?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlRejection::UnsupportedScheme);
    }
    // Credentials would be stored and shown in plaintext.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlRejection::HasCredentials);
    }

    let ips = match url.host().ok_or(UrlRejection::MissingHost)? {
        Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
        Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain == "localhost" || domain.ends_with(".localhost") {
                return Err(UrlRejection::ForbiddenAddress);
            }
            let port = url.port_or_known_default().unwrap_or(80);
            let ips = resolver
                .resolve(&domain, port)
                .await
                .map_err(|_| UrlRejection::Unresolvable)?;
            if ips.is_empty() {
                return Err(UrlRejection::Unresolvable);
            }
            ips
        }
    };

    if ips.iter().any(|ip| is_forbidden_ip(*ip)) {
        return Err(UrlRejection::ForbiddenAddress);
    }
    Ok(url)
}

/// True for any address a monitoring check must never reach.
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_forbidden_ipv4(v4),
        IpAddr::V6(v6) => is_forbidden_ipv6(v6),
    }
}

fn is_forbidden_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0                                   // 0.0.0.0/8 "this network"
        || a == 10                           // 10.0.0.0/8 private
        || (a == 100 && (64..128).contains(&b)) // 100.64.0.0/10 CGNAT / Tailscale
        || a == 127                          // loopback
        || (a == 169 && b == 254)            // link-local, cloud metadata
        || (a == 172 && (16..32).contains(&b)) // 172.16.0.0/12 private (Docker default)
        || (a == 192 && b == 0 && c == 0)    // 192.0.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)    // TEST-NET-1
        || (a == 192 && b == 168)            // 192.168.0.0/16 private
        || (a == 198 && (b == 18 || b == 19)) // 198.18.0.0/15 benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113)  // TEST-NET-3
        || a >= 224 // multicast (224/4), reserved (240/4), broadcast
}

fn is_forbidden_ipv6(ip: Ipv6Addr) -> bool {
    // IPv4-mapped (::ffff:a.b.c.d) and NAT64 (64:ff9b::/96) embed an IPv4
    // address that is what actually gets reached — judge that instead.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_forbidden_ipv4(v4);
    }
    let seg = ip.segments();
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        let [.., a, b, c, d] = ip.octets();
        return is_forbidden_ipv4(Ipv4Addr::new(a, b, c, d));
    }
    // 6to4 (2002::/16) embeds an IPv4 address in bits 16–48.
    if seg[0] == 0x2002 {
        let o = ip.octets();
        return is_forbidden_ipv4(Ipv4Addr::new(o[2], o[3], o[4], o[5]));
    }

    ip.is_unspecified()                     // ::
        || ip.is_loopback()                 // ::1
        || (seg[0] & 0xfe00) == 0xfc00      // fc00::/7 unique local
        || (seg[0] & 0xffc0) == 0xfe80      // fe80::/10 link-local
        || (seg[0] & 0xff00) == 0xff00      // ff00::/8 multicast
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // 2001:db8::/32 documentation
        || seg[..6] == [0, 0, 0, 0, 0, 0] // ::a.b.c.d (deprecated IPv4-compatible)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn forbids_internal_ipv4() {
        for s in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
        }
    }

    #[test]
    fn allows_public_ipv4() {
        for s in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.215.14",
            "172.32.0.1",
            "100.128.0.1",
            "172.15.255.255",
        ] {
            assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn forbids_internal_ipv6_including_embedded_ipv4() {
        for s in [
            "::",
            "::1",
            "fd00::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1", // NAT64 → 10.0.0.1
            "2002:7f00:1::",  // 6to4 → 127.0.0.1
            "::127.0.0.1",
        ] {
            assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
        }
    }

    #[test]
    fn allows_public_ipv6() {
        for s in ["2606:4700:4700::1111", "::ffff:8.8.8.8", "2002:0808:0808::"] {
            assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
        }
    }

    fn resolver() -> HostResolver {
        HostResolver::Static(HashMap::from([
            ("public.test".to_string(), vec![ip("93.184.215.14")]),
            ("internal.test".to_string(), vec![ip("10.0.0.5")]),
            (
                "mixed.test".to_string(),
                vec![ip("93.184.215.14"), ip("127.0.0.1")],
            ),
        ]))
    }

    async fn check(url: &str) -> Result<Url, UrlRejection> {
        validate_target_url(url, &resolver()).await
    }

    #[tokio::test]
    async fn accepts_public_targets() {
        assert_eq!(
            check(" https://PUBLIC.test/health?x=1 ")
                .await
                .unwrap()
                .as_str(),
            "https://public.test/health?x=1"
        );
        assert!(check("http://1.1.1.1:8080/").await.is_ok());
    }

    #[tokio::test]
    async fn rejects_bad_urls() {
        use UrlRejection::*;
        let cases = [
            ("not a url", Malformed),
            ("ftp://public.test/", UnsupportedScheme),
            ("file:///etc/passwd", UnsupportedScheme),
            ("https://user:pw@public.test/", HasCredentials),
            ("http://localhost:8080/", ForbiddenAddress),
            ("http://api.localhost/", ForbiddenAddress),
            ("http://127.0.0.1/", ForbiddenAddress),
            ("http://169.254.169.254/latest/meta-data/", ForbiddenAddress),
            ("http://[::1]/", ForbiddenAddress),
            ("http://[::ffff:10.0.0.1]/", ForbiddenAddress),
            ("http://internal.test/", ForbiddenAddress),
            ("http://mixed.test/", ForbiddenAddress),
            ("http://nowhere.test/", Unresolvable),
        ];
        for (url, expected) in cases {
            assert_eq!(check(url).await.unwrap_err(), expected, "{url}");
        }
        let long = format!("https://public.test/{}", "a".repeat(URL_MAX_LEN));
        assert_eq!(check(&long).await.unwrap_err(), TooLong);
    }

    #[tokio::test]
    async fn decimal_and_octal_ip_tricks_are_normalized_by_parser() {
        // WHATWG URL parsing turns these into 127.0.0.1 before we check.
        for url in ["http://2130706433/", "http://0x7f.0.0.1/", "http://127.1/"] {
            assert_eq!(
                check(url).await.unwrap_err(),
                UrlRejection::ForbiddenAddress,
                "{url}"
            );
        }
    }
}
