//! Pulse worker entrypoint — Scheduler + Checker Worker (same codebase as
//! the API server, separate process/container per `docs/pulse-architecture.md`
//! #2.1/#2.2 and #5). Logic lives in `pulse_backend::worker`.
//!
//! Never runs migrations (the API owns them); until they're applied, the
//! scheduler just logs errors and retries on its next tick.

use pulse_backend::{
    analysis::llm::{self, LlmClient},
    config::{AiConfig, Config},
    db,
    queue::{CheckQueue, DEFAULT_QUEUE_KEY},
    worker::{
        self,
        checker::{CheckerSettings, HttpChecker},
    },
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pulse_backend::init_tracing();
    let config = Config::from_env();

    let pool = db::connect(&config.database_url).await?;
    let queue = CheckQueue::connect(&config.redis_url, DEFAULT_QUEUE_KEY).await?;
    let checker = HttpChecker::new(CheckerSettings::default())?;
    let analyzer = match AiConfig::from_env()? {
        Some(ai) => Some(LlmClient::new(&ai, llm::DEFAULT_TIMEOUT)?),
        None => {
            warn!("AI_API_KEY not set: AI analysis disabled, incidents stay pending");
            None
        }
    };

    let shutdown = CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            info!("shutdown signal received");
            shutdown.cancel();
        }
    });

    info!(env = %config.app_env, "pulse-worker started");
    worker::run(pool, queue, checker, analyzer, shutdown).await;
    Ok(())
}

/// Ctrl-C, or SIGTERM on Unix (what `docker compose stop` sends).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
