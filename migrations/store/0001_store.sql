-- store.sqlite: the disposable store (docs/design.md §4.1). Deleting the file
-- loses event details, but never issues, counts, releases or settings.
-- Ids here are referenced from kestrel.sqlite without foreign keys.

-- Normalised event payloads: JSON, zstd-compressed.
CREATE TABLE payloads (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    received_at TEXT NOT NULL,
    data BLOB NOT NULL
);

CREATE INDEX idx_payloads_received ON payloads (received_at);

-- Attachments of stored events (screenshots, view hierarchies, logs),
-- zstd-compressed.
CREATE TABLE attachments (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    received_at TEXT NOT NULL,
    filename TEXT NOT NULL,
    content_type TEXT,
    attachment_type TEXT NOT NULL DEFAULT 'event.attachment',
    size INTEGER NOT NULL,
    data BLOB NOT NULL
);

CREATE INDEX idx_attachments_event ON attachments (project_id, event_id);

-- Session updates and aggregates, kept for the release-health rollups
-- (design §4.9). The SDK's device id is replaced by a hash that changes
-- every day; the IP address and user agent are dropped.
CREATE TABLE session_items (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    received_at TEXT NOT NULL,
    release TEXT NOT NULL,
    environment TEXT NOT NULL,
    data TEXT NOT NULL
);

CREATE INDEX idx_session_items_received ON session_items (received_at);

-- User feedback (user_report and feedback items); the email is hashed unless
-- the project keeps user details.
CREATE TABLE feedback (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    event_id TEXT,
    received_at TEXT NOT NULL,
    data TEXT NOT NULL
);

CREATE INDEX idx_feedback_project ON feedback (project_id, received_at);
