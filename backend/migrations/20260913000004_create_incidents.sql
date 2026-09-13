-- incidents — docs/pulse-database-schema.md #2.4, docs/pulse-ai-design.md

CREATE TABLE incidents (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    endpoint_id        UUID        NOT NULL REFERENCES endpoints (id) ON DELETE CASCADE,
    triggered_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    trigger_reason     TEXT        NOT NULL
        CHECK (trigger_reason IN ('latency_threshold_exceeded', 'error_rate_threshold_exceeded')),
    -- Metric snapshots: baseline before vs. values at detection time.
    metric_before      JSONB,
    metric_after       JSONB,
    -- AI analysis output; NULL until ai_status = 'completed'.
    ai_possible_cause  TEXT,
    ai_evidence        JSONB       CHECK (ai_evidence IS NULL OR jsonb_typeof(ai_evidence) = 'array'),
    ai_suggested_steps JSONB       CHECK (ai_suggested_steps IS NULL OR jsonb_typeof(ai_suggested_steps) = 'array'),
    ai_status          TEXT        NOT NULL DEFAULT 'pending'
        CHECK (ai_status IN ('pending', 'completed', 'failed')),
    -- NULL = open.
    resolved_at        TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX incidents_endpoint_id_triggered_at_idx ON incidents (endpoint_id, triggered_at DESC);
