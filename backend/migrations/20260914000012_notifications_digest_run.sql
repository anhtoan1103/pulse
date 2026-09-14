-- Repoints 'health_digest' notifications from a single health_digests row
-- to a digest_runs row, since one digest email aggregates ALL of a user's
-- endpoint digests for a period into one message (docs/pulse-architecture.md
-- #2.6) rather than sending one email per endpoint per day.
--
-- Safe to alter rather than add a new migration on top: notifications
-- shipped in the same feature set and no 'health_digest' row has ever been
-- written (step 8 only produced 'incident_opened' rows).

ALTER TABLE notifications DROP CONSTRAINT notifications_subject_matches_kind;
ALTER TABLE notifications DROP COLUMN health_digest_id;
ALTER TABLE notifications ADD COLUMN digest_run_id UUID REFERENCES digest_runs (id) ON DELETE CASCADE;

ALTER TABLE notifications ADD CONSTRAINT notifications_subject_matches_kind CHECK (
    (kind = 'incident_opened') = (incident_id IS NOT NULL)
    AND (kind = 'health_digest') = (digest_run_id IS NOT NULL)
);

-- One digest email per user per run.
CREATE UNIQUE INDEX notifications_one_per_user_per_run_idx
    ON notifications (user_id, digest_run_id) WHERE digest_run_id IS NOT NULL;
