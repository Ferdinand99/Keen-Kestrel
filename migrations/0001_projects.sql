-- Keen Kestrel's first tables, after vole-kit's core set (users, api_tokens,
-- settings): projects and their DSN keys. Issues and events come later.

-- One project is one app or site. The id is part of every DSN, so it must stay
-- an integer and is never reused (AUTOINCREMENT): an old DSN can't reach a new
-- project that happens to get a deleted one's id.
CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,                  -- for sentry-cli's --project
    platform TEXT NOT NULL DEFAULT 'other'
        CHECK (platform IN ('apple', 'javascript', 'rust', 'python', 'other')),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- DSN keys. The public key is public by design and can only send events.
CREATE TABLE project_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    public_key TEXT NOT NULL UNIQUE CHECK (length(public_key) = 32),  -- 32 lowercase hex characters
    enabled INTEGER NOT NULL DEFAULT 1,
    rate_limit_per_minute INTEGER CHECK (rate_limit_per_minute IS NULL OR rate_limit_per_minute > 0),
    allowed_origins TEXT NOT NULL DEFAULT '',   -- one origin per line; empty allows any
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_project_keys_project ON project_keys (project_id);
