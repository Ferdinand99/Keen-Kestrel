-- Alerts and release health (docs/design.md §4.7, §4.9): the instance's
-- notification channels (vole-kit's providers), each project's alert rules
-- and what they sent, and the daily release-health rollups of the SDKs'
-- sessions.

-- Where alerts go. kind is one of vole-kit's providers; config is its JSON
-- (secrets included: it never leaves the server unredacted). language is
-- the language the messages are written in ('' = the instance's).
CREATE TABLE notification_channels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    active INTEGER NOT NULL DEFAULT 1,
    language TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- A project's alert rules. kind: new_issue, regression, spike (events in an
-- hour above max(threshold, multiplier × the hourly average of the 7 days
-- before), first_crash (the first crash of a release). environment limits
-- new_issue, regression and first_crash to one environment ('' = any).
-- An issue alerts a rule at most once per cooldown_minutes. With
-- digest_minutes, a channel gets one message with everything that came up
-- in that many minutes instead of one per alert.
CREATE TABLE alert_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('new_issue', 'regression', 'spike', 'first_crash')),
    enabled INTEGER NOT NULL DEFAULT 1,
    environment TEXT NOT NULL DEFAULT '',
    threshold INTEGER NOT NULL DEFAULT 100 CHECK (threshold BETWEEN 1 AND 10000000),
    multiplier REAL NOT NULL DEFAULT 5 CHECK (multiplier BETWEEN 1 AND 1000),
    cooldown_minutes INTEGER NOT NULL DEFAULT 60 CHECK (cooldown_minutes BETWEEN 0 AND 525600),
    digest_minutes INTEGER CHECK (digest_minutes IS NULL OR digest_minutes BETWEEN 1 AND 1440),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_alert_rules_project ON alert_rules (project_id);

CREATE TABLE alert_rule_channels (
    rule_id INTEGER NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES notification_channels(id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, channel_id)
) WITHOUT ROWID;

-- When a rule last alerted for an issue (the cooldown).
CREATE TABLE alert_cooldowns (
    rule_id INTEGER NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    fired_at TEXT NOT NULL,
    PRIMARY KEY (rule_id, issue_id)
) WITHOUT ROWID;

-- Every alert for every channel: the delivery queue and the history.
-- status: pending (to send at next_attempt_at), digest (waits for its
-- channel's digest at due_at), sent, failed (after the last retry or for
-- good), skipped (the channel was paused or deleted). data is JSON with
-- what the message says (issue, project, level, release, counts, link), so
-- it is written in the channel's language when it is sent. Names are copied
-- so the history still reads after a rule or channel is deleted.
CREATE TABLE alert_log (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    rule_id INTEGER REFERENCES alert_rules(id) ON DELETE SET NULL,
    rule_name TEXT NOT NULL,
    channel_id INTEGER REFERENCES notification_channels(id) ON DELETE SET NULL,
    channel_name TEXT NOT NULL,
    issue_id INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'digest', 'sent', 'failed', 'skipped')),
    attempts INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    next_attempt_at TEXT,
    due_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    sent_at TEXT
);

CREATE INDEX idx_alert_log_project ON alert_log (project_id, id DESC);
CREATE INDEX idx_alert_log_queue ON alert_log (status, next_attempt_at);

-- Releases: when sentry-cli finalized one, its first crash (the alert
-- fires once per release), and its events (all of them, stored or not).
ALTER TABLE releases ADD COLUMN date_released TEXT;
ALTER TABLE releases ADD COLUMN first_crash_at TEXT;
ALTER TABLE releases ADD COLUMN events INTEGER NOT NULL DEFAULT 0;

UPDATE releases SET events = coalesce((
    SELECT SUM(t.times_seen) FROM issue_tag_values t JOIN issues i ON i.id = t.issue_id
    WHERE i.project_id = releases.project_id AND t.key = 'release' AND t.value = releases.version
), 0);

-- Release health (design §4.9): the SDKs' sessions per project, release,
-- environment and UTC day (the day each session started). sessions: started
-- sessions; errored: sessions with at least one error (crashed and abnormal
-- ones included); crashed; abnormal (ended without a clean exit, e.g. the
-- app was killed); users and crashed_users: distinct device ids that day.
-- Kept 400 days.
CREATE TABLE release_health (
    release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    day TEXT NOT NULL,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sessions INTEGER NOT NULL DEFAULT 0,
    errored INTEGER NOT NULL DEFAULT 0,
    crashed INTEGER NOT NULL DEFAULT 0,
    abnormal INTEGER NOT NULL DEFAULT 0,
    users INTEGER NOT NULL DEFAULT 0,
    crashed_users INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (release_id, environment, day)
) WITHOUT ROWID;

CREATE INDEX idx_release_health_project ON release_health (project_id, day);

-- Who and what the rollup already counted for a day that can still get
-- sessions (hashes: device ids hashed with that day's key at ingest, session
-- ids hashed here), so counts stay distinct. Deleted once the day is over.
CREATE TABLE release_health_seen (
    release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    day TEXT NOT NULL,
    kind TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (release_id, environment, day, kind, hash)
) WITHOUT ROWID;

CREATE INDEX idx_release_health_seen_day ON release_health_seen (day);
