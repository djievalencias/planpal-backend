-- Meeting room configuration on the request itself.
ALTER TABLE meeting_requests
    ADD COLUMN room_type TEXT NOT NULL DEFAULT 'none'
        CHECK (room_type IN ('zoom', 'gmeet', 'teams', 'phone', 'in_person', 'none')),
    ADD COLUMN room_link TEXT,     -- Video link (Zoom/GMeet/Teams URL)
    ADD COLUMN location  TEXT;     -- Physical address (in_person only)

-- Per-attendee conflict metadata set by the scheduler worker.
-- NULL = no conflict found; non-null = first conflicting event title/reason.
ALTER TABLE meeting_attendees
    ADD COLUMN conflict_reason TEXT;
