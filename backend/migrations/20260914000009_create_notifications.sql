-- Notification outbox — docs/pulse-architecture.md #2.6.
--
-- Rows are written in the same transaction as the event they announce (e.g.
-- the Anomaly Detector opening an incident), then delivered by the worker's
-- notifier. A crash between "incident opened" and "email sent" can therefore
-- never lose the email, and delivery can retry independently.

CREATE TABLE notifications (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Recipient. Emails go to users.email, looked up at send time.
    user_id          UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 'health_digest' is used by the periodic digest (implement-order step 9).
    kind             TEXT        NOT NULL CHECK (kind IN ('incident_opened', 'health_digest')),
    incident_id      UUID        REFERENCES incidents (id) ON DELETE CASCADE,
    health_digest_id UUID        REFERENCES health_digests (id) ON DELETE CASCADE,
    -- pending → sent | failed (gave up) | cancelled (e.g. recipient disabled)
    status           TEXT        NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'sent', 'failed', 'cancelled')),
    -- Incident emails wait for AI analysis to finish, but no longer than this.
    send_after       TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts         INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    -- Retry time after a transient failure; also the claim lease.
    next_attempt_at  TIMESTAMPTZ,
    last_error       TEXT,
    sent_at          TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT notifications_subject_matches_kind CHECK (
        (kind = 'incident_opened') = (incident_id IS NOT NULL)
        AND (kind = 'health_digest') = (health_digest_id IS NOT NULL)
    )
);

-- One "incident opened" email per incident.
CREATE UNIQUE INDEX notifications_one_per_incident_idx
    ON notifications (incident_id) WHERE incident_id IS NOT NULL;
CREATE INDEX notifications_user_id_idx ON notifications (user_id);
CREATE INDEX notifications_pending_idx
    ON notifications (next_attempt_at NULLS FIRST, created_at)
    WHERE status = 'pending';
