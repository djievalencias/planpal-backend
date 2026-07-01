-- Add read_at to notifications so the client can track which notifications
-- the user has already seen in the in-app notification centre.
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS read_at TIMESTAMPTZ;

-- Index to efficiently count unread notifications per user.
CREATE INDEX IF NOT EXISTS idx_notifications_unread
    ON notifications (user_id, read_at)
    WHERE read_at IS NULL;
