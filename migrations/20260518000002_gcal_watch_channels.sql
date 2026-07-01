-- Watch channel state for Google Calendar push notifications.
ALTER TABLE calendar_providers
    ADD COLUMN watch_channel_id TEXT,
    ADD COLUMN watch_resource_id TEXT,
    ADD COLUMN watch_expiry TIMESTAMPTZ;
