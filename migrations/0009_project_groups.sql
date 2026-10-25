-- Project groups: sections of the project list ("Sylo", "Websites" …).
--
-- A project is in at most one group (`projects.group_id`); groups don't
-- nest. Deleting a group never deletes its projects: they move to "Not in
-- a group" (ON DELETE SET NULL). Names are unique regardless of case.
CREATE TABLE project_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    sort_order INTEGER NOT NULL DEFAULT 0,      -- the list shows groups by sort_order, then name
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

ALTER TABLE projects ADD COLUMN group_id INTEGER REFERENCES project_groups(id) ON DELETE SET NULL;

CREATE INDEX idx_projects_group ON projects (group_id);
