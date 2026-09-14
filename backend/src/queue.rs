//! Redis-backed check job queue between Scheduler and Checker Workers
//! (docs/pulse-architecture.md #2.1, #4). A plain Redis list: RPUSH to
//! enqueue, BLPOP to consume (FIFO).
//!
//! Delivery is at-most-once: a job popped by a worker that then crashes is
//! lost. That's acceptable here — the endpoint simply gets checked again at
//! its next interval.

use anyhow::Context;
use chrono::{DateTime, Utc};
use redis::{
    Client,
    aio::{ConnectionManager, ConnectionManagerConfig},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

pub const DEFAULT_QUEUE_KEY: &str = "pulse:queue:checks";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckJob {
    pub endpoint_id: Uuid,
    pub scheduled_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct CheckQueue {
    key: String,
    /// Separate connections: BLPOP blocks its connection while waiting, and
    /// ConnectionManager multiplexes commands — sharing one would stall pushes.
    push_conn: ConnectionManager,
    pop_conn: ConnectionManager,
    /// Upper bound for a single blocking pop.
    pop_wait: Duration,
}

impl CheckQueue {
    pub async fn connect(redis_url: &str, key: impl Into<String>) -> anyhow::Result<Self> {
        let client = Client::open(redis_url).context("invalid REDIS_URL")?;
        let pop_wait = Duration::from_secs(5);

        let push_conn = client
            .get_connection_manager_with_config(
                ConnectionManagerConfig::new()
                    .set_connection_timeout(Some(Duration::from_secs(5)))
                    .set_response_timeout(Some(Duration::from_secs(5))),
            )
            .await
            .context("connecting to Redis")?;
        // Response timeout must outlast the BLPOP server-side wait.
        let pop_conn = client
            .get_connection_manager_with_config(
                ConnectionManagerConfig::new()
                    .set_connection_timeout(Some(Duration::from_secs(5)))
                    .set_response_timeout(Some(pop_wait + Duration::from_secs(5))),
            )
            .await
            .context("connecting to Redis")?;

        Ok(Self {
            key: key.into(),
            push_conn,
            pop_conn,
            pop_wait,
        })
    }

    pub async fn push(&self, jobs: &[CheckJob]) -> anyhow::Result<()> {
        if jobs.is_empty() {
            return Ok(());
        }
        let payloads = jobs
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
        redis::cmd("RPUSH")
            .arg(&self.key)
            .arg(payloads)
            .query_async::<i64>(&mut self.push_conn.clone())
            .await
            .context("RPUSH check jobs")?;
        Ok(())
    }

    /// Waits up to ~5s for a job. `Ok(None)` on timeout. Malformed payloads
    /// are logged and skipped rather than wedging the consumer.
    pub async fn pop(&self) -> anyhow::Result<Option<CheckJob>> {
        let popped: Option<(String, String)> = redis::cmd("BLPOP")
            .arg(&self.key)
            .arg(self.pop_wait.as_secs_f64())
            .query_async(&mut self.pop_conn.clone())
            .await
            .context("BLPOP check job")?;

        Ok(
            popped.and_then(|(_, payload)| match serde_json::from_str(&payload) {
                Ok(job) => Some(job),
                Err(e) => {
                    tracing::warn!(error = %e, "discarding malformed check job");
                    None
                }
            }),
        )
    }

    pub async fn len(&self) -> anyhow::Result<u64> {
        redis::cmd("LLEN")
            .arg(&self.key)
            .query_async::<u64>(&mut self.push_conn.clone())
            .await
            .context("LLEN check queue")
    }
}
