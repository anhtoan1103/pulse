//! Worker process: Scheduler loop + Checker Worker pool
//! (docs/pulse-architecture.md #2.1, #2.2). Both run in the `worker` binary;
//! several worker replicas can run side by side (claims don't overlap, the
//! queue is shared).

pub mod checker;
pub mod job;
pub mod scheduler;

use crate::{
    analysis::{llm::LlmClient, service as analysis},
    queue::CheckQueue,
};
use checker::HttpChecker;
use job::JobResult;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

/// How often due endpoints are claimed. Bounds scheduling lateness; the
/// claim query is a cheap indexed scan, so 1s costs nothing at MVP scale.
pub const SCHEDULER_TICK: Duration = Duration::from_secs(1);
/// Concurrent checks per worker process. Checks are I/O-bound; 500 endpoints
/// at a 10s minimum interval average 50 checks/s, each ≤10s.
pub const MAX_CONCURRENT_CHECKS: usize = 100;
/// Pause after a Redis/DB error before retrying the loop, so an outage
/// doesn't turn into a hot error-logging loop.
const ERROR_BACKOFF: Duration = Duration::from_secs(2);

/// Runs until `shutdown` is cancelled, then stops scheduling/consuming and
/// waits for in-flight checks to finish. `analyzer` is `None` when no AI
/// provider is configured: incidents are still opened, but stay `pending`.
pub async fn run(
    pool: PgPool,
    queue: CheckQueue,
    checker: HttpChecker,
    analyzer: Option<LlmClient>,
    shutdown: CancellationToken,
) {
    let analysis =
        analyzer.map(|llm| tokio::spawn(analysis::run_loop(pool.clone(), llm, shutdown.clone())));
    let scheduler = tokio::spawn(scheduler_loop(
        pool.clone(),
        queue.clone(),
        shutdown.clone(),
    ));
    consumer_loop(pool, queue, Arc::new(checker), shutdown).await;
    let _ = scheduler.await;
    if let Some(analysis) = analysis {
        let _ = analysis.await;
    }
    info!("worker stopped");
}

async fn scheduler_loop(pool: PgPool, queue: CheckQueue, shutdown: CancellationToken) {
    let mut tick = tokio::time::interval(SCHEDULER_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tick.tick() => {}
        }
        if let Err(e) = scheduler::schedule_once(&pool, &queue, SCHEDULER_TICK / 2).await {
            error!(error = format!("{e:#}"), "scheduler tick failed");
        }
    }
}

async fn consumer_loop(
    pool: PgPool,
    queue: CheckQueue,
    checker: Arc<HttpChecker>,
    shutdown: CancellationToken,
) {
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_CHECKS));
    let mut in_flight = JoinSet::new();

    loop {
        // Only pop when there's capacity to run the job right away.
        let permit = tokio::select! {
            _ = shutdown.cancelled() => break,
            permit = permits.clone().acquire_owned() => permit.expect("semaphore never closed"),
        };
        let job = tokio::select! {
            _ = shutdown.cancelled() => break,
            job = queue.pop() => job,
        };
        // Reap finished tasks so the set doesn't grow unbounded.
        while in_flight.try_join_next().is_some() {}

        match job {
            Ok(Some(job)) => {
                let (pool, checker) = (pool.clone(), checker.clone());
                in_flight.spawn(async move {
                    let _permit = permit;
                    match job::process_job(&pool, &checker, &job).await {
                        Ok(JobResult::Recorded(outcome)) => debug!(
                            endpoint_id = %job.endpoint_id,
                            success = outcome.success,
                            status = ?outcome.status_code,
                            latency_ms = ?outcome.latency_ms,
                            "check recorded"
                        ),
                        Ok(skipped) => {
                            debug!(endpoint_id = %job.endpoint_id, ?skipped, "check skipped")
                        }
                        Err(e) => error!(
                            endpoint_id = %job.endpoint_id,
                            error = format!("{e:#}"),
                            "check job failed"
                        ),
                    }
                });
            }
            Ok(None) => {} // pop timed out, loop to re-check shutdown
            Err(e) => {
                error!(error = format!("{e:#}"), "consuming check queue failed");
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(ERROR_BACKOFF) => {}
                }
            }
        }
    }

    info!(in_flight = in_flight.len(), "waiting for in-flight checks");
    in_flight.join_all().await;
}
