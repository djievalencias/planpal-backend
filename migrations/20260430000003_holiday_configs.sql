-- Admin-configured public holidays per country per year.
-- These are used to populate calendar data for users who have that country selected.

CREATE TABLE holiday_configs (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    country    TEXT        NOT NULL,       -- ISO 3166-1 alpha-2 (e.g. "ID", "SG")
    year       SMALLINT    NOT NULL,       -- e.g. 2025
    name       TEXT        NOT NULL,       -- e.g. "Independence Day"
    date       DATE        NOT NULL,       -- the actual date of the holiday
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (country, date)                 -- one entry per country + date
);

CREATE INDEX idx_holiday_configs_country_year ON holiday_configs (country, year);
