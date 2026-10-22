-- Triage and retention (docs/design.md §4.5–4.6): what happened to an issue
-- and who did it, distinct users and events per hour for the lists and the
-- overview, and how long each project keeps its data.

-- Retention per project (design §4.6). Event details (payloads and
-- attachments in store.sqlite) are removed after retention_event_days;
-- issues, with their events, retention_issue_days after they were last
-- seen. At most payloads_per_day events per issue and UTC day are stored;
-- the counts go on.
ALTER TABLE projects ADD COLUMN retention_event_days INTEGER NOT NULL DEFAULT 30
    CHECK (retention_event_days BETWEEN 1 AND 3650);
ALTER TABLE projects ADD COLUMN retention_issue_days INTEGER NOT NULL DEFAULT 90
    CHECK (retention_issue_days BETWEEN 1 AND 3650);
ALTER TABLE projects ADD COLUMN payloads_per_day INTEGER NOT NULL DEFAULT 1000
    CHECK (payloads_per_day BETWEEN 1 AND 100000);

-- Distinct users per issue (keyed hashes, never the user's own id), counted
-- for every event, stored or not. issues.users_seen is their number.
CREATE TABLE issue_users (
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    user_hash TEXT NOT NULL,
    PRIMARY KEY (issue_id, user_hash)
) WITHOUT ROWID;

ALTER TABLE issues ADD COLUMN users_seen INTEGER NOT NULL DEFAULT 0;

INSERT INTO issue_users (issue_id, user_hash)
    SELECT DISTINCT issue_id, user_hash FROM events WHERE user_hash IS NOT NULL;
UPDATE issues SET users_seen = (SELECT COUNT(*) FROM issue_users u WHERE u.issue_id = issues.id);

-- Events per issue and hour (UTC, by arrival), for every event, stored or
-- not: the overview's counts and the sparklines. Kept for 30 days.
CREATE TABLE issue_hours (
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    hour TEXT NOT NULL,                          -- 'YYYY-MM-DD HH:00:00'
    project_id INTEGER NOT NULL,
    events INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (issue_id, hour)
) WITHOUT ROWID;

CREATE INDEX idx_issue_hours_project ON issue_hours (project_id, hour);

-- Events written before this migration: the stored ones are all there is.
INSERT INTO issue_hours (issue_id, hour, project_id, events)
    SELECT issue_id, strftime('%Y-%m-%d %H:00:00', received_at), project_id, COUNT(*)
    FROM events GROUP BY issue_id, strftime('%Y-%m-%d %H:00:00', received_at);

-- What happened to an issue: regressed, resolved (now or in the next
-- release), unresolved, ignored (with its condition), ignoring ended, merged.
-- actor is the user's name at the time; NULL means Kestrel itself. data is
-- JSON with the details (release, until, count, the merged issues).
CREATE TABLE issue_activity (
    id INTEGER PRIMARY KEY,
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    actor TEXT,
    data TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_issue_activity_issue ON issue_activity (issue_id, id);

-- Retention removes event details by arrival time.
CREATE INDEX idx_events_project_received ON events (project_id, received_at);
-- Issue retention and the lists sort by first seen, events and users too.
CREATE INDEX idx_issues_project_first_seen ON issues (project_id, first_seen DESC);
