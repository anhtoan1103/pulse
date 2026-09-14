-- At most one open (unresolved) incident per endpoint + trigger reason.
-- The Anomaly Detector checks for an open incident before opening a new one;
-- this index makes that guarantee hold even under concurrent detection runs
-- (INSERT ... ON CONFLICT DO NOTHING). It also serves the detector's
-- "open incidents for this endpoint" lookup.

CREATE UNIQUE INDEX incidents_one_open_per_reason_idx
    ON incidents (endpoint_id, trigger_reason)
    WHERE resolved_at IS NULL;
