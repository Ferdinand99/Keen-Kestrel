-- App signals (docs/design.md §5.1, phase 9) share analytics_events with the
-- web's events: source 'app', the signal's type in `name`, the user hash in
-- `visitor`, the platform (iOS, macOS …) in `os` and the payload (JSON,
-- scrubbed) in `props`. The time is when the signal happened, corrected for
-- the device's clock.
--
-- Never the install id, an IP address or a user agent: `visitor` is
-- trunc64(HMAC(project secret, install id)) (keyed with a daily key in strict
-- mode) and `session` the same of the session id.
ALTER TABLE analytics_events ADD COLUMN bundle_id TEXT NOT NULL DEFAULT '';
ALTER TABLE analytics_events ADD COLUMN app_version TEXT NOT NULL DEFAULT '';
ALTER TABLE analytics_events ADD COLUMN build TEXT NOT NULL DEFAULT '';
ALTER TABLE analytics_events ADD COLUMN os_version TEXT NOT NULL DEFAULT '';
ALTER TABLE analytics_events ADD COLUMN model TEXT NOT NULL DEFAULT '';
ALTER TABLE analytics_events ADD COLUMN locale TEXT NOT NULL DEFAULT '';
-- 1 debug build, 2 TestFlight, 4 simulator: test signals, left out of the
-- rollups (the dashboard can include them from the raw signals).
ALTER TABLE analytics_events ADD COLUMN flags INTEGER NOT NULL DEFAULT 0;
ALTER TABLE analytics_events ADD COLUMN session INTEGER NOT NULL DEFAULT 0;

-- The app users of each day of the last 31 (UTC), per project: the rollup's
-- daily users, and what weekly and monthly active users are counted from
-- (a new user of a day is looked up in the days before). Older days are
-- deleted by the rollup.
CREATE TABLE app_users (
    project_id INTEGER NOT NULL,
    day INTEGER NOT NULL,                    -- days since 1970
    user INTEGER NOT NULL,                   -- the user hash
    PRIMARY KEY (project_id, day, user)
) WITHOUT ROWID;

CREATE INDEX idx_app_users_user ON app_users (project_id, user, day);

-- Current users (the last five minutes) without test signals, covered; and
-- the count of test signals the dashboard mentions.
CREATE INDEX idx_analytics_events_app_live ON analytics_events (project_id, ts, visitor)
    WHERE source = 'app' AND flags = 0;
CREATE INDEX idx_analytics_events_app_test ON analytics_events (project_id, ts)
    WHERE source = 'app' AND flags <> 0;
