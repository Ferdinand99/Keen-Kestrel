-- Raw analytics (docs/design.md §5): one row per web pageview or custom event
-- from k.js; app signals (phase 9) will use the same table with source 'app'.
-- Kept 30 days (per project), rolled up every minute into kestrel.sqlite's
-- analytics_* tables, which are what the dashboards read.
--
-- Never an IP address or a user agent: visitor is
-- trunc64(sha256(daily salt ‖ project ‖ IP ‖ user agent ‖ host)), with a salt
-- that lives in memory only and changes at midnight UTC, so it can't be tied
-- to a person or to the same browser on another day. Empty strings mean
-- "none" (no referrer, no UTM, an unknown browser).
CREATE TABLE analytics_events (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    source TEXT NOT NULL DEFAULT 'web',      -- 'web' (k.js); 'app' later
    ts INTEGER NOT NULL,                     -- Unix seconds, UTC
    visitor INTEGER NOT NULL,                -- daily visitor id (see above)
    name TEXT NOT NULL,                      -- 'pageview' or a custom event's name
    hostname TEXT NOT NULL DEFAULT '',
    path TEXT NOT NULL DEFAULT '',           -- without the query; scrubbed
    referrer TEXT NOT NULL DEFAULT '',       -- the referring domain only
    utm_source TEXT NOT NULL DEFAULT '',
    utm_medium TEXT NOT NULL DEFAULT '',
    utm_campaign TEXT NOT NULL DEFAULT '',
    utm_term TEXT NOT NULL DEFAULT '',
    utm_content TEXT NOT NULL DEFAULT '',
    browser TEXT NOT NULL DEFAULT '',        -- from the user agent, which isn't kept
    os TEXT NOT NULL DEFAULT '',
    device TEXT NOT NULL DEFAULT '',         -- desktop, mobile, tablet
    screen TEXT NOT NULL DEFAULT '',         -- a width bucket, never the width
    props TEXT                               -- custom event properties (JSON), scrubbed
);

CREATE INDEX idx_analytics_events_project_ts ON analytics_events (project_id, ts);
CREATE INDEX idx_analytics_events_ts ON analytics_events (ts);

-- How far the rollup has read (one row). It lives next to the rows it
-- points into, so a deleted store starts over with it.
CREATE TABLE analytics_rollup_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_id INTEGER NOT NULL DEFAULT 0
);

INSERT INTO analytics_rollup_state (id, last_id) VALUES (1, 0);

-- Who the rollup already counted on a day that can still get events (today
-- and yesterday, UTC), so daily uniques stay distinct: per project, day
-- (days since 1970), dimension (a small number), value (a 64-bit hash of
-- it) and visitor id. Rows of older days are deleted by the rollup.
CREATE TABLE analytics_seen (
    project_id INTEGER NOT NULL,
    day INTEGER NOT NULL,
    dim INTEGER NOT NULL,
    value INTEGER NOT NULL,
    visitor INTEGER NOT NULL,
    PRIMARY KEY (project_id, day, dim, value, visitor)
) WITHOUT ROWID;
