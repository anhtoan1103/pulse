//! Shared library code for Pulse backend.
//!
//! Both entrypoints (`bin/api.rs` — API server, `bin/worker.rs` — Scheduler +
//! Checker Worker) depend on this crate so config loading, DB pool setup,
//! and (later) domain logic live in one place instead of being duplicated.

pub mod app;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod extract;
pub mod rate_limit;

/// Initializes `tracing` with an env-filter (`RUST_LOG`, default `info`).
/// Call once at the top of each binary's `main()`.
pub fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}
