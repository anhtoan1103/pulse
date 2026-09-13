-- endpoints — docs/pulse-database-schema.md #2.2
--
-- Range limits (interval 10–3600s, sane thresholds, max 50 endpoints/user)
-- are enforced in the application layer per the schema doc; the DB only
-- guards against values that are never valid.

CREATE TABLE endpoints (
    id                           UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    -- ON DELETE CASCADE: deleting a user removes their endpoints (and, via
    -- the tables below, all checks/incidents/digests). Hard delete chosen
    -- over soft-delete for MVP (api-spec #2 leaves this open).
    user_id                      UUID         NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name                         TEXT         NOT NULL,
    url                          TEXT         NOT NULL,
    method                       TEXT         NOT NULL DEFAULT 'GET'
        CHECK (method IN ('GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS')),
    check_interval_seconds       INTEGER      NOT NULL CHECK (check_interval_seconds > 0),
    latency_threshold_ms         INTEGER      NOT NULL CHECK (latency_threshold_ms > 0),
    error_rate_threshold_percent NUMERIC(5,2) NOT NULL
        CHECK (error_rate_threshold_percent >= 0 AND error_rate_threshold_percent <= 100),
    is_active                    BOOLEAN      NOT NULL DEFAULT TRUE,
    -- NULL = never checked yet, so the Scheduler treats it as due immediately.
    last_checked_at              TIMESTAMPTZ,
    created_at                   TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX endpoints_user_id_idx ON endpoints (user_id);
-- Scheduler scan: active endpoints ordered by how long since last check.
CREATE INDEX endpoints_is_active_last_checked_at_idx ON endpoints (is_active, last_checked_at);
