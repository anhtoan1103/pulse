-- Narrows "one incident_opened notification per incident, ever" to "...per
-- incident, while one is still pending" — needed so a relapsed incident (see
-- anomaly/mod.rs: a recovered-but-unresolved incident that breaches again)
-- can get a fresh notification. The old index made that structurally
-- impossible: once the first notification existed, no second row for that
-- incident_id could ever be inserted, even after the first was long since
-- sent and the incident had since relapsed.
--
-- A partial unique index's membership is dynamic: a row drops out once it
-- no longer satisfies the predicate (e.g. status flips from 'pending' to
-- 'sent'), which is exactly what frees up the incident_id for the next
-- relapse's notification.

DROP INDEX notifications_one_per_incident_idx;
CREATE UNIQUE INDEX notifications_one_pending_per_incident_idx
    ON notifications (incident_id) WHERE incident_id IS NOT NULL AND status = 'pending';
