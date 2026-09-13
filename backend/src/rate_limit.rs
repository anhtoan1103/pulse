//! Minimal in-memory fixed-window rate limiter for auth endpoints
//! (api-spec #8: ~5 requests/minute/IP on `/auth/login` and `/auth/register`).
//!
//! In-memory is enough for MVP: there's a single API instance. If the API is
//! ever scaled out, move this to Redis so instances share counters.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Expired entries are pruned once the map grows past this, bounding memory
/// under a flood of distinct IPs.
const PRUNE_THRESHOLD: usize = 10_000;

pub struct RateLimiter {
    limit: u32,
    window: Duration,
    buckets: Mutex<HashMap<(&'static str, IpAddr), Bucket>>,
}

struct Bucket {
    window_start: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Records a hit for (`scope`, `ip`); returns `false` if it exceeds the
    /// limit for the current window. Scopes keep counters independent, e.g.
    /// failed logins don't eat into the register budget.
    pub fn check(&self, scope: &'static str, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());

        if buckets.len() > PRUNE_THRESHOLD {
            buckets.retain(|_, b| now.duration_since(b.window_start) < self.window);
        }

        let bucket = buckets.entry((scope, ip)).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= self.window {
            bucket.window_start = now;
            bucket.count = 0;
        }
        bucket.count = bucket.count.saturating_add(1);
        bucket.count <= self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const IP_A: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    const IP_B: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

    #[test]
    fn allows_up_to_limit_then_blocks() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!((0..3).all(|_| rl.check("login", IP_A)));
        assert!(!rl.check("login", IP_A));
    }

    #[test]
    fn counters_are_per_ip_and_per_scope() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check("login", IP_A));
        assert!(!rl.check("login", IP_A));
        assert!(rl.check("login", IP_B));
        assert!(rl.check("register", IP_A));
    }

    #[test]
    fn window_resets() {
        let rl = RateLimiter::new(1, Duration::from_millis(20));
        assert!(rl.check("login", IP_A));
        assert!(!rl.check("login", IP_A));
        std::thread::sleep(Duration::from_millis(30));
        assert!(rl.check("login", IP_A));
    }
}
