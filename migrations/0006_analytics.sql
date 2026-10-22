-- Web analytics (docs/design.md §5): the rollups the dashboards, the API
-- and MCP read. The raw events are in store.sqlite (migrations/store/); a job
-- adds them here every minute. Counts only: no visitor ids, IP addresses or
-- user agents. Days and hours are UTC. source is 'web' (k.js); app signals
-- (phase 9) will add 'app'.

-- Whether the project takes pageviews and custom events from k.js (with any
-- of its enabled keys). On by default: a page only sends with the snippet.
ALTER TABLE projects ADD COLUMN web_analytics INTEGER NOT NULL DEFAULT 1;

-- How long raw analytics events (30) and the rollups (400) are kept.
ALTER TABLE projects ADD COLUMN retention_analytics_raw_days INTEGER NOT NULL DEFAULT 30
    CHECK (retention_analytics_raw_days BETWEEN 1 AND 400);
ALTER TABLE projects ADD COLUMN retention_analytics_days INTEGER NOT NULL DEFAULT 400
    CHECK (retention_analytics_days BETWEEN 1 AND 3650);

-- Per project and UTC hour: pageviews, custom events and distinct visitors
-- in that hour.
CREATE TABLE analytics_hours (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    hour TEXT NOT NULL,                          -- 'YYYY-MM-DD HH:00:00'
    pageviews INTEGER NOT NULL DEFAULT 0,
    events INTEGER NOT NULL DEFAULT 0,
    visitors INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, source, hour)
) WITHOUT ROWID;

-- Per project and UTC day: the same. Unique visitors over a range are the
-- sum of the days' visitors (a visitor id changes every day).
CREATE TABLE analytics_days (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    day TEXT NOT NULL,                           -- 'YYYY-MM-DD'
    pageviews INTEGER NOT NULL DEFAULT 0,
    events INTEGER NOT NULL DEFAULT 0,
    visitors INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, source, day)
) WITHOUT ROWID;

-- Per project, UTC day, dimension and value: count (pageviews; entries for
-- 'entry'; events for 'event') and distinct visitors that day. Dimensions:
-- page, entry (a visitor's first page of the day), referrer ('' = direct or
-- none), utm_source, utm_medium, utm_campaign, browser, os, device, screen,
-- event (custom event names).
CREATE TABLE analytics_breakdown (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    dim TEXT NOT NULL,
    day TEXT NOT NULL,
    value TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    visitors INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, source, dim, day, value)
) WITHOUT ROWID;

-- Retention deletes by day across projects.
CREATE INDEX idx_analytics_breakdown_day ON analytics_breakdown (day);
