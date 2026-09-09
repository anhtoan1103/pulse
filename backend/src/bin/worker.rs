//! Pulse worker entrypoint — Scheduler + Checker Worker (same codebase as
//! the API server, separate process/container per `docs/pulse-architecture.md`
//! #2.1/#2.2 and #5).
//!
//! Skeleton stage: just a heartbeat loop. Real logic (poll `endpoints` table
//! for due checks, push jobs to Redis, consume + HTTP-check + write `checks`)
//! lands in implement-order step 5.

use pulse_backend::config::Config;
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pulse_backend::init_tracing();
    let config = Config::from_env();

    info!(env = %config.app_env, "pulse-worker started");

    loop {
        // TODO: Scheduler — query `endpoints` where due (last_checked_at + interval),
        // push check jobs to Redis queue (docs/pulse-architecture.md #2.1).
        // TODO: Checker Worker — consume queue, HTTP-check with SSRF validation
        // + timeout (docs/pulse-security.md #1), write results to `checks`.
        info!("worker tick (no-op skeleton)");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
