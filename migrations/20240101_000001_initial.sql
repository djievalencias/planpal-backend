-- PlanPal initial schema
-- All timestamps use TIMESTAMPTZ (always UTC).
-- All PKs are UUID v4 via gen_random_uuid() (requires Postgres 13+).

-- ────────────────────────────────────────────────────────────────────────────
-- Users
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE users (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT        NOT NULL UNIQUE,
    display_name  TEXT        NOT NULL,
    password_hash TEXT,                          -- NULL for OAuth-only accounts
    google_sub    TEXT        UNIQUE,            -- NULL for password-only accounts
    role          TEXT        NOT NULL DEFAULT 'regular'
                                  CHECK (role IN ('regular', 'admin')),
    fcm_token     TEXT,                          -- Firebase push token (latest device)
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_users_email      ON users (email);
CREATE INDEX idx_users_google_sub ON users (google_sub) WHERE google_sub IS NOT NULL;

-- ────────────────────────────────────────────────────────────────────────────
-- Refresh tokens (JWT revocation list)
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE refresh_tokens (
    jti        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT        NOT NULL,     -- SHA-256 hex of the raw refresh token
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_refresh_tokens_user_id ON refresh_tokens (user_id);

-- ────────────────────────────────────────────────────────────────────────────
-- Calendar providers
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE calendar_providers (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind           TEXT        NOT NULL CHECK (kind IN ('google_calendar', 'ical', 'local')),
    display_name   TEXT        NOT NULL,
    -- OAuth tokens stored as application-layer ciphertext
    access_token   TEXT,
    refresh_token  TEXT,
    token_expiry   TIMESTAMPTZ,
    -- iCal feed URL (kind = 'ical')
    ical_url       TEXT,
    last_synced_at TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_cal_providers_user_id ON calendar_providers (user_id);

-- ────────────────────────────────────────────────────────────────────────────
-- Calendar events (denormalised cache from external providers + local events)
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE calendar_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id UUID        NOT NULL REFERENCES calendar_providers(id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    external_id TEXT,                    -- Google event ID / iCal UID
    title       TEXT        NOT NULL,
    start_at    TIMESTAMPTZ NOT NULL,
    end_at      TIMESTAMPTZ NOT NULL,
    is_all_day  BOOLEAN     NOT NULL DEFAULT FALSE,
    status      TEXT        NOT NULL DEFAULT 'confirmed'
                                CHECK (status IN ('confirmed', 'tentative', 'cancelled')),
    is_free     BOOLEAN     NOT NULL DEFAULT FALSE, -- TRANSP:TRANSPARENT in iCal
    raw_json    JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT chk_event_times   CHECK (end_at > start_at),
    UNIQUE (provider_id, external_id)    -- idempotent upsert key
);

CREATE INDEX idx_events_user_time ON calendar_events (user_id, start_at, end_at);
CREATE INDEX idx_events_provider  ON calendar_events (provider_id);
-- GiST range index for efficient overlap queries
CREATE INDEX idx_events_range     ON calendar_events USING GIST (
    tstzrange(start_at, end_at)
);

-- ────────────────────────────────────────────────────────────────────────────
-- Meeting requests
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE meeting_requests (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    requester_id           UUID        NOT NULL REFERENCES users(id),
    title                  TEXT        NOT NULL,
    description            TEXT,
    duration_minutes       INTEGER     NOT NULL CHECK (duration_minutes > 0),
    preferred_window_start TIMESTAMPTZ NOT NULL,
    preferred_window_end   TIMESTAMPTZ NOT NULL,
    status                 TEXT        NOT NULL DEFAULT 'pending'
                               CHECK (status IN ('pending', 'proposal_ready', 'confirmed', 'cancelled')),
    created_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE meeting_attendees (
    meeting_request_id UUID NOT NULL REFERENCES meeting_requests(id) ON DELETE CASCADE,
    user_id            UUID NOT NULL REFERENCES users(id),
    PRIMARY KEY (meeting_request_id, user_id)
);

-- ────────────────────────────────────────────────────────────────────────────
-- Meeting proposals (output of the scheduler)
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE meeting_proposals (
    id                 UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    meeting_request_id UUID             NOT NULL REFERENCES meeting_requests(id) ON DELETE CASCADE,
    proposed_start     TIMESTAMPTZ      NOT NULL,
    proposed_end       TIMESTAMPTZ      NOT NULL,
    score              DOUBLE PRECISION NOT NULL,
    is_selected        BOOLEAN          NOT NULL DEFAULT FALSE,
    created_at         TIMESTAMPTZ      NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_proposals_meeting ON meeting_proposals (meeting_request_id, score DESC);

-- ────────────────────────────────────────────────────────────────────────────
-- Notifications log
-- ────────────────────────────────────────────────────────────────────────────
CREATE TABLE notifications (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID        NOT NULL REFERENCES users(id),
    meeting_id    UUID        REFERENCES meeting_requests(id),
    channel       TEXT        NOT NULL CHECK (channel IN ('email', 'push')),
    status        TEXT        NOT NULL DEFAULT 'pending'
                                  CHECK (status IN ('pending', 'sent', 'failed')),
    payload       JSONB       NOT NULL,
    error_message TEXT,
    sent_at       TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_notifications_user ON notifications (user_id, created_at DESC);
