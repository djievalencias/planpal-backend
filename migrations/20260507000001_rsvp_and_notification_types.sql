-- ── RSVP tracking per attendee ────────────────────────────────────────────────
-- Tracks whether each invitee has responded to a meeting invitation.
ALTER TABLE meeting_attendees
    ADD COLUMN rsvp_status  TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (rsvp_status IN ('pending', 'accepted', 'rejected')),
    ADD COLUMN responded_at TIMESTAMPTZ;

CREATE INDEX idx_attendees_rsvp ON meeting_attendees (meeting_request_id, rsvp_status);

-- ── Notification event type ────────────────────────────────────────────────────
-- Distinguishes why a notification was sent so the frontend can render it correctly.
ALTER TABLE notifications
    ADD COLUMN notification_type TEXT NOT NULL DEFAULT 'meeting_confirmed'
                                CHECK (notification_type IN (
                                    'meeting_invitation',
                                    'meeting_confirmed',
                                    'attendee_responded',
                                    'meeting_cancelled'
                                ));

CREATE INDEX idx_notifications_type ON notifications (user_id, notification_type, created_at DESC);
