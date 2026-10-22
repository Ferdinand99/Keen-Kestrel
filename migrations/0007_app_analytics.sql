-- App analytics (docs/design.md §5, phase 9): signals from KestrelKit and other
-- app SDKs. Their rollups go into the analytics_* tables of 0006 with source
-- 'app'; the raw signals are in store.sqlite's analytics_events.

-- Whether the project takes signals from apps (with any of its enabled
-- keys), apart from web analytics' switch. On by default: an app only sends
-- once KestrelKit is configured with a key.
ALTER TABLE projects ADD COLUMN app_analytics INTEGER NOT NULL DEFAULT 1;

-- Strict mode: the user hash is keyed with a key that changes every UTC day,
-- so an app user can't be followed from one day to the next (weekly and
-- monthly users then add up the days).
ALTER TABLE projects ADD COLUMN app_strict INTEGER NOT NULL DEFAULT 0;

-- The project's own random secret for HMAC(secret, install_id): never shown,
-- never in the API. Made for every project, existing and new.
ALTER TABLE projects ADD COLUMN app_secret BLOB;

UPDATE projects SET app_secret = randomblob(32) WHERE app_secret IS NULL;

CREATE TRIGGER projects_app_secret AFTER INSERT ON projects
WHEN NEW.app_secret IS NULL
BEGIN
    UPDATE projects SET app_secret = randomblob(32) WHERE id = NEW.id;
END;

-- App rows count signals in `events`, active users in `visitors` and distinct
-- sessions here (web rows keep 0).
ALTER TABLE analytics_hours ADD COLUMN sessions INTEGER NOT NULL DEFAULT 0;
ALTER TABLE analytics_days ADD COLUMN sessions INTEGER NOT NULL DEFAULT 0;

-- Distinct app users in the 7 and 30 days up to and including the day
-- (weekly and monthly active users), kept up to date by the rollup.
ALTER TABLE analytics_days ADD COLUMN wau INTEGER NOT NULL DEFAULT 0;
ALTER TABLE analytics_days ADD COLUMN mau INTEGER NOT NULL DEFAULT 0;
