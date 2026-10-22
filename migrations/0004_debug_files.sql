-- Debug information files (docs/design.md §4.1, §4.8): dSYMs and other
-- object files uploaded with sentry-cli (or as a zip), one row per object,
-- that is per debug id and architecture. The files themselves and the
-- SymCache made from each are in the data directory
-- (difs/<project_id>/<debug_id>/object and …/symcache), not in SQLite.

CREATE TABLE dif_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    debug_id TEXT NOT NULL,                     -- lowercase, as symbolic writes it
    code_id TEXT,                               -- Mach-O UUID / ELF build id, if any
    object_name TEXT NOT NULL,                  -- e.g. KestrelSpike (from the upload)
    arch TEXT NOT NULL,                         -- arm64, x86_64 …
    kind TEXT NOT NULL,                         -- file format: macho, elf, pe, pdb …
    features TEXT NOT NULL DEFAULT '',          -- space separated: debug symtab unwind sources
    size INTEGER NOT NULL,                      -- bytes of the object
    symcache_size INTEGER NOT NULL,
    sha1 TEXT NOT NULL,                         -- of the uploaded file (sentry-cli's checksum)
    release TEXT,                               -- the newest release whose events used it
    uploaded_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT,
    UNIQUE (project_id, debug_id)
);

CREATE INDEX idx_dif_files_sha1 ON dif_files (project_id, sha1);

-- Retention of debug files (design §4.6): a file goes when it was neither
-- uploaded nor used for retention_dif_days, unless one of the project's
-- retention_dif_releases newest releases used it.
ALTER TABLE projects ADD COLUMN retention_dif_days INTEGER NOT NULL DEFAULT 180
    CHECK (retention_dif_days BETWEEN 1 AND 3650);
ALTER TABLE projects ADD COLUMN retention_dif_releases INTEGER NOT NULL DEFAULT 5
    CHECK (retention_dif_releases BETWEEN 0 AND 1000);

-- What Kestrel lets an API token do beyond the kit's read/write scope.
-- difs:write: upload debug files (sentry-cli, the zip endpoint). Such a token
-- is a read-only token everywhere else; its owner must be an editor or admin.
CREATE TABLE token_grants (
    token_id INTEGER NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    scope TEXT NOT NULL CHECK (scope IN ('difs:write')),
    PRIMARY KEY (token_id, scope)
) WITHOUT ROWID;
