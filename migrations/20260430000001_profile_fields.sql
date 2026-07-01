-- Profile enrichment fields added to users table.
-- All new columns are nullable to preserve existing rows without defaults.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS timezone     TEXT,
    ADD COLUMN IF NOT EXISTS department   TEXT,
    ADD COLUMN IF NOT EXISTS job_title    TEXT,
    ADD COLUMN IF NOT EXISTS work_start   TEXT,  -- "HH:MM" 24-h local time
    ADD COLUMN IF NOT EXISTS work_end     TEXT,  -- "HH:MM" 24-h local time
    ADD COLUMN IF NOT EXISTS manager_name TEXT;
