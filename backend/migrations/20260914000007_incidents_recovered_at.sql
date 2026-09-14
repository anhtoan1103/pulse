-- When an open incident's metrics came back within threshold.
--
-- The Anomaly Detector never resolves incidents itself: resolving is the
-- user's call. It sets recovered_at when the endpoint is healthy again so the
-- dashboard can suggest "metrics back to normal since <recovered_at> —
-- resolve this incident?", and clears it if the endpoint breaches again
-- while the incident is still open. NULL = still breaching (or resolved
-- before recovery was observed).

ALTER TABLE incidents ADD COLUMN recovered_at TIMESTAMPTZ;
