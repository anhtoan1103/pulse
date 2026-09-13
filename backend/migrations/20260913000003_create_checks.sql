-- checks — docs/pulse-database-schema.md #2.3
--
-- Highest-volume table (one row per check, forever until retention lands).
-- id is BIGINT identity rather than UUID: smaller rows/indexes and cheaper
-- inserts, and check ids are never exposed via the API (#4 of the schema doc
-- leaves this choice to implementation).

CREATE TABLE checks (
    id            BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    endpoint_id   UUID        NOT NULL REFERENCES endpoints (id) ON DELETE CASCADE,
    checked_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- NULL when the request failed entirely (timeout, DNS, connection refused...).
    status_code   INTEGER,
    -- NULL when no response was received.
    latency_ms    INTEGER     CHECK (latency_ms >= 0),
    -- Derived by the Checker: 2xx status and no timeout.
    success       BOOLEAN     NOT NULL,
    -- NULL on success.
    error_message TEXT
);

-- Required: nearly every query is "latest N checks of endpoint X" or
-- "checks of endpoint X in time range Y".
CREATE INDEX checks_endpoint_id_checked_at_idx ON checks (endpoint_id, checked_at DESC);

-- For the (post-MVP) retention job: `DELETE FROM checks WHERE checked_at < ...`
-- across all endpoints. BRIN suits append-only time-ordered data and costs
-- almost nothing on insert, unlike a second B-tree.
CREATE INDEX checks_checked_at_brin_idx ON checks USING BRIN (checked_at);
