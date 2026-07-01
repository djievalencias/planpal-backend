-- ────────────────────────────────────────────────────────────────────────────
-- AI Chat — conversational meeting scheduling via Bedrock
-- ────────────────────────────────────────────────────────────────────────────

CREATE TABLE ai_chat_sessions (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES users(id),
    status      TEXT        NOT NULL DEFAULT 'active'
                            CHECK (status IN ('active', 'completed', 'expired')),
    meeting_id  UUID        REFERENCES meeting_requests(id),
    metadata    JSONB       NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE ai_chat_messages (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id  UUID        NOT NULL REFERENCES ai_chat_sessions(id),
    role        TEXT        NOT NULL CHECK (role IN ('user', 'assistant')),
    content     TEXT        NOT NULL,
    metadata    JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_ai_sessions_user ON ai_chat_sessions (user_id, created_at DESC);
CREATE INDEX idx_ai_messages_session ON ai_chat_messages (session_id, created_at ASC);
