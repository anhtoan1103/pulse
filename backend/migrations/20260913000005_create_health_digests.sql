-- health_digests — docs/pulse-database-schema.md #2.5
-- Plain aggregates over `checks`, no AI involved.

CREATE TABLE health_digests (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    endpoint_id    UUID        NOT NULL REFERENCES endpoints (id) ON DELETE CASCADE,
    period_start   TIMESTAMPTZ NOT NULL,
    period_end     TIMESTAMPTZ NOT NULL,
    total_checks   INTEGER     NOT NULL CHECK (total_checks >= 0),
    success_count  INTEGER     NOT NULL CHECK (success_count >= 0),
    -- NULL when the period had no responses to average.
    avg_latency_ms INTEGER     CHECK (avg_latency_ms >= 0),
    status         TEXT        NOT NULL CHECK (status IN ('healthy', 'degraded')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT health_digests_success_count_le_total CHECK (success_count <= total_checks),
    CONSTRAINT health_digests_period_order CHECK (period_end > period_start)
);

-- Not in the schema doc: backs `GET /endpoints/:id/health-digests` (newest
-- first) and the FK cascade on endpoint delete.
CREATE INDEX health_digests_endpoint_id_period_start_idx ON health_digests (endpoint_id, period_start DESC);
