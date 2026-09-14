-- AI Analysis Service state (docs/pulse-ai-design.md #3, #5).

ALTER TABLE incidents
    -- Output field from the AI schema (#3) that the original table lacked.
    ADD COLUMN ai_confidence TEXT CHECK (ai_confidence IN ('high', 'medium', 'low')),
    -- How many times analysis was attempted (rate limits / provider errors
    -- are retried later, #5).
    ADD COLUMN ai_attempts INTEGER NOT NULL DEFAULT 0 CHECK (ai_attempts >= 0),
    -- Pending incidents are picked up once this is NULL or in the past. Also
    -- used as a lease while a worker is analyzing, so a crashed worker's
    -- incident is retried instead of stuck.
    ADD COLUMN ai_next_attempt_at TIMESTAMPTZ,
    -- Short, user-safe reason when ai_status = 'failed' (never raw provider
    -- responses or secrets).
    ADD COLUMN ai_error TEXT;

CREATE INDEX incidents_ai_pending_idx
    ON incidents (ai_next_attempt_at NULLS FIRST, triggered_at)
    WHERE ai_status = 'pending';
