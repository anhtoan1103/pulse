-- One health_digests row per endpoint per period — lets digest generation
-- use `ON CONFLICT DO NOTHING` as a safe no-op if a period is (re)claimed
-- more than once (defense in depth alongside digest_runs.period_end being
-- unique).

CREATE UNIQUE INDEX health_digests_endpoint_id_period_start_key
    ON health_digests (endpoint_id, period_start);
