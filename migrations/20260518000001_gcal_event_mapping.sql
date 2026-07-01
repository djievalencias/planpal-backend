-- ────────────────────────────────────────────────────────────────────────────
-- Maps PlanPal meetings to Google Calendar event IDs per user.
-- Used for updating/deleting Google Calendar events when meetings change.
-- ────────────────────────────────────────────────────────────────────────────

CREATE TABLE gcal_event_mappings (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_id      UUID        NOT NULL REFERENCES meeting_requests(id) ON DELETE CASCADE,
    user_id         UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider_id     UUID        NOT NULL REFERENCES calendar_providers(id) ON DELETE CASCADE,
    gcal_event_id   TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (meeting_id, user_id)
);

CREATE INDEX idx_gcal_mappings_meeting ON gcal_event_mappings (meeting_id);
CREATE INDEX idx_gcal_mappings_user ON gcal_event_mappings (user_id);
