-- User public holiday preferences: array of ISO 3166-1 alpha-2 country codes.
-- Default is empty (no holidays selected).

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS public_holidays TEXT[] NOT NULL DEFAULT '{}';
