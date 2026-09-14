-- Claim table for periodic Health Digest generation
-- (docs/pulse-architecture.md #2.6, PRD #6 "định kỳ health digest").
--
-- One digest run covers one fixed period (e.g. the last 24h, generated daily
-- at 08:00 UTC). `period_end` is unique, so
-- `INSERT ... ON CONFLICT (period_end) DO NOTHING RETURNING id` atomically
-- claims a period across worker replicas — whichever insert wins generates
-- that run's health_digests rows and notifications; the rest see no row
-- returned and skip. This is simpler than a lease: a run is a short,
-- single-transaction burst of aggregate queries, not a long-held job.

CREATE TABLE digest_runs (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    period_start     TIMESTAMPTZ NOT NULL,
    period_end       TIMESTAMPTZ NOT NULL UNIQUE,
    -- How many endpoints got a health_digests row (some are skipped: no
    -- checks in the period, e.g. newly created or paused throughout).
    digests_created  INTEGER     NOT NULL DEFAULT 0 CHECK (digests_created >= 0),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT digest_runs_period_order CHECK (period_end > period_start)
);
