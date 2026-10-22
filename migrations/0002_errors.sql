-- Error tracking (docs/design.md §4.1): environments, releases, issues, the
-- grouping hashes that point at them, a small index row per stored event,
-- tag value counts and full-text search over issue titles and culprits.
-- Event payloads live in store.sqlite (migrations/store/), which can be
-- thrown away without losing anything here.
--
-- Times are UTC text, 'YYYY-MM-DD HH:MM:SS', like the rest of the database.

-- Whether events keep user.email and user.username as sent. Off by default:
-- both are replaced by a keyed hash at ingest (design §4.3).
ALTER TABLE projects ADD COLUMN keep_user_details INTEGER NOT NULL DEFAULT 0;

CREATE TABLE environments (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    UNIQUE (project_id, name)
);

-- Created on first sight (an event names it); sentry-cli can create them too
-- later. Ids grow with first sight, so a larger id is a newer release.
CREATE TABLE releases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    version TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    UNIQUE (project_id, version)
);

CREATE TABLE issues (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    culprit TEXT NOT NULL DEFAULT '',
    level TEXT NOT NULL DEFAULT 'error'
        CHECK (level IN ('fatal', 'error', 'warning', 'info', 'debug')),
    platform TEXT NOT NULL DEFAULT 'other',
    status TEXT NOT NULL DEFAULT 'unresolved'
        CHECK (status IN ('unresolved', 'resolved', 'ignored')),
    substatus TEXT NOT NULL DEFAULT 'new'
        CHECK (substatus IN ('new', 'regressed', 'ongoing')),
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    times_seen INTEGER NOT NULL DEFAULT 0,
    first_release_id INTEGER REFERENCES releases(id) ON DELETE SET NULL,
    last_release_id INTEGER REFERENCES releases(id) ON DELETE SET NULL,
    -- "Resolve in the next release": the version that was current. Events
    -- from a newer release regress the issue, events from this one don't.
    resolved_in_release TEXT,
    resolved_at TEXT,
    -- Ignore conditions: until a time, or until times_seen reaches a count.
    -- Both NULL while ignored means forever.
    ignore_until TEXT,
    ignore_until_count INTEGER,
    -- The grouping configuration that made the issue (e.g. kestrel:v1).
    grouping_config TEXT NOT NULL,
    -- Grouped by image and offset because the frames had no symbols.
    unsymbolicated INTEGER NOT NULL DEFAULT 0,
    -- Payloads stored on payload_day (UTC date); at most 1000 a day.
    payload_day TEXT,
    payloads_today INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_issues_project_last_seen ON issues (project_id, last_seen DESC);
CREATE INDEX idx_issues_project_status ON issues (project_id, status, last_seen DESC);

-- Grouping hash → issue. Several hashes can point at one issue (merges).
CREATE TABLE issue_hashes (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    hash TEXT NOT NULL CHECK (length(hash) = 32),
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    grouping_config TEXT NOT NULL,
    -- JSON: what went into the hash, so the issue page can explain it.
    components TEXT NOT NULL,
    UNIQUE (project_id, hash)
);

CREATE INDEX idx_issue_hashes_issue ON issue_hashes (issue_id);

-- One small row per stored event; the payload is in store.sqlite.
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    event_id TEXT NOT NULL CHECK (length(event_id) = 32),
    timestamp TEXT NOT NULL,                     -- when it happened (SDK clock, clamped)
    received_at TEXT NOT NULL,
    level TEXT NOT NULL,
    release_id INTEGER REFERENCES releases(id) ON DELETE SET NULL,
    environment_id INTEGER REFERENCES environments(id) ON DELETE SET NULL,
    user_hash TEXT,                              -- keyed hash, never the user's own id
    payload_id INTEGER,                          -- store.sqlite payloads.id
    UNIQUE (project_id, event_id)
);

CREATE INDEX idx_events_issue ON events (issue_id, timestamp DESC);

-- How often each tag value was seen per issue: the whitelisted keys plus at
-- most 20 custom keys per issue.
CREATE TABLE issue_tag_values (
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    times_seen INTEGER NOT NULL DEFAULT 0,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    PRIMARY KEY (issue_id, key, value)
) WITHOUT ROWID;

-- Free-text search over titles and culprits, kept in step by triggers.
CREATE VIRTUAL TABLE issues_fts USING fts5 (
    title,
    culprit,
    content = 'issues',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER issues_fts_insert AFTER INSERT ON issues BEGIN
    INSERT INTO issues_fts (rowid, title, culprit) VALUES (new.id, new.title, new.culprit);
END;

CREATE TRIGGER issues_fts_delete AFTER DELETE ON issues BEGIN
    INSERT INTO issues_fts (issues_fts, rowid, title, culprit)
        VALUES ('delete', old.id, old.title, old.culprit);
END;

CREATE TRIGGER issues_fts_update AFTER UPDATE OF title, culprit ON issues BEGIN
    INSERT INTO issues_fts (issues_fts, rowid, title, culprit)
        VALUES ('delete', old.id, old.title, old.culprit);
    INSERT INTO issues_fts (rowid, title, culprit) VALUES (new.id, new.title, new.culprit);
END;
