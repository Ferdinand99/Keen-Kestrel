//! `projects` and `project_keys`.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::project::{Platform, generate_public_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub platform: Platform,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKey {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub public_key: String,
    pub enabled: bool,
    pub rate_limit_per_minute: Option<i64>,
    /// Empty: any origin may send.
    pub allowed_origins: Vec<String>,
    pub created_at: String,
}

/// What an update of a key changes (`None` keeps the field).
#[derive(Debug, Default)]
pub struct KeyChanges {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    /// `Some(None)` removes the limit.
    pub rate_limit_per_minute: Option<Option<i64>>,
    pub allowed_origins: Option<Vec<String>>,
}

const PROJECT_COLUMNS: &str = "id, name, slug, platform, created_at";
const KEY_COLUMNS: &str =
    "id, project_id, name, public_key, enabled, rate_limit_per_minute, allowed_origins, created_at";

fn project(row: &Row<'_>) -> rusqlite::Result<Project> {
    let platform: String = row.get(3)?;
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        platform: Platform::parse(&platform).unwrap_or(Platform::Other),
        created_at: row.get(4)?,
    })
}

fn key(row: &Row<'_>) -> rusqlite::Result<ProjectKey> {
    let origins: String = row.get(6)?;
    Ok(ProjectKey {
        id: row.get(0)?,
        project_id: row.get(1)?,
        name: row.get(2)?,
        public_key: row.get(3)?,
        enabled: row.get(4)?,
        rate_limit_per_minute: row.get(5)?,
        allowed_origins: origins.lines().map(str::to_string).collect(),
        created_at: row.get(7)?,
    })
}

/// Every project, by id.
pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Project>> {
    conn.prepare(&format!(
        "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY id"
    ))?
    .query_map([], project)?
    .collect()
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Project>> {
    conn.query_row(
        &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"),
        [id],
        project,
    )
    .optional()
}

/// The project with `slug`.
pub fn by_slug(conn: &Connection, slug: &str) -> rusqlite::Result<Option<Project>> {
    conn.query_row(
        &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE slug = ?1"),
        [slug],
        project,
    )
    .optional()
}

/// Whether another project (not `except`) has `slug`.
pub fn slug_taken(conn: &Connection, slug: &str, except: Option<i64>) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM projects WHERE slug = ?1 AND id IS NOT ?2)",
        params![slug, except],
        |r| r.get(0),
    )
}

/// `base`, or `base-2`, `base-3` … whichever is free.
pub fn free_slug(conn: &Connection, base: &str) -> rusqlite::Result<String> {
    if !slug_taken(conn, base, None)? {
        return Ok(base.to_string());
    }
    let stem: String = base
        .chars()
        .take(crate::project::MAX_SLUG_LEN - 4)
        .collect();
    let stem = stem.trim_end_matches('-');
    let mut n = 2;
    loop {
        let candidate = format!("{stem}-{n}");
        if !slug_taken(conn, &candidate, None)? {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// Creates a project with its first key ("Default") and returns its id.
pub fn insert(
    conn: &mut Connection,
    name: &str,
    slug: &str,
    platform: Platform,
    first_key_name: &str,
) -> rusqlite::Result<i64> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO projects (name, slug, platform) VALUES (?1, ?2, ?3)",
        params![name, slug, platform.as_str()],
    )?;
    let id = tx.last_insert_rowid();
    insert_key(&tx, id, first_key_name)?;
    tx.commit()?;
    Ok(id)
}

/// Changes name, slug and platform. `false` when there is no such project.
pub fn update(
    conn: &Connection,
    id: i64,
    name: &str,
    slug: &str,
    platform: Platform,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE projects SET name = ?2, slug = ?3, platform = ?4 WHERE id = ?1",
        params![id, name, slug, platform.as_str()],
    )? > 0)
}

/// Deletes a project and its keys. `false` when there is no such project.
pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM projects WHERE id = ?1", [id])? > 0)
}

/// A project's keys, oldest first.
pub fn keys(conn: &Connection, project_id: i64) -> rusqlite::Result<Vec<ProjectKey>> {
    conn.prepare(&format!(
        "SELECT {KEY_COLUMNS} FROM project_keys WHERE project_id = ?1 ORDER BY id"
    ))?
    .query_map([project_id], key)?
    .collect()
}

/// Every key, for the project list: `project id → (enabled keys, all keys)`.
pub fn key_counts(conn: &Connection) -> rusqlite::Result<Vec<(i64, i64, i64)>> {
    conn.prepare("SELECT project_id, SUM(enabled), COUNT(*) FROM project_keys GROUP BY project_id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect()
}

pub fn get_key(
    conn: &Connection,
    project_id: i64,
    key_id: i64,
) -> rusqlite::Result<Option<ProjectKey>> {
    conn.query_row(
        &format!("SELECT {KEY_COLUMNS} FROM project_keys WHERE project_id = ?1 AND id = ?2"),
        [project_id, key_id],
        key,
    )
    .optional()
}

/// Adds an enabled key with a fresh public key and returns its id.
pub fn insert_key(conn: &Connection, project_id: i64, name: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO project_keys (project_id, name, public_key) VALUES (?1, ?2, ?3)",
        params![project_id, name, generate_public_key()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Applies `changes` to one key. `false` when there is no such key.
pub fn update_key(
    conn: &mut Connection,
    project_id: i64,
    key_id: i64,
    changes: &KeyChanges,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM project_keys WHERE project_id = ?1 AND id = ?2)",
        [project_id, key_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(false);
    }
    if let Some(name) = &changes.name {
        tx.execute(
            "UPDATE project_keys SET name = ?2 WHERE id = ?1",
            params![key_id, name],
        )?;
    }
    if let Some(enabled) = changes.enabled {
        tx.execute(
            "UPDATE project_keys SET enabled = ?2 WHERE id = ?1",
            params![key_id, enabled],
        )?;
    }
    if let Some(limit) = changes.rate_limit_per_minute {
        tx.execute(
            "UPDATE project_keys SET rate_limit_per_minute = ?2 WHERE id = ?1",
            params![key_id, limit],
        )?;
    }
    if let Some(origins) = &changes.allowed_origins {
        tx.execute(
            "UPDATE project_keys SET allowed_origins = ?2 WHERE id = ?1",
            params![key_id, origins.join("\n")],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

/// Whether the project takes web analytics (`None`: no such project).
pub fn web_analytics(conn: &Connection, project_id: i64) -> rusqlite::Result<Option<bool>> {
    conn.query_row(
        "SELECT web_analytics FROM projects WHERE id = ?1",
        [project_id],
        |r| r.get(0),
    )
    .optional()
}

/// Turns web analytics on or off. `false` when there is no such project.
pub fn set_web_analytics(conn: &Connection, project_id: i64, on: bool) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE projects SET web_analytics = ?2 WHERE id = ?1",
        params![project_id, on],
    )? > 0)
}

/// Deletes one key. `false` when there is no such key.
pub fn delete_key(conn: &Connection, project_id: i64, key_id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM project_keys WHERE project_id = ?1 AND id = ?2",
        [project_id, key_id],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(vole_kit::db::CONNECTION_PRAGMAS).unwrap();
        c.execute_batch(include_str!("../../migrations/0001_projects.sql"))
            .unwrap();
        c
    }

    #[test]
    fn a_project_starts_with_one_key_and_takes_them_along() {
        let mut c = conn();
        let id = insert(&mut c, "Sylo", "sylo", Platform::Apple, "Default").unwrap();
        let ks = keys(&c, id).unwrap();
        assert_eq!(ks.len(), 1);
        assert!(ks[0].enabled && ks[0].allowed_origins.is_empty());
        assert_eq!(ks[0].public_key.len(), 32);
        assert!(delete(&c, id).unwrap());
        let left: i64 = c
            .query_row("SELECT COUNT(*) FROM project_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut c = conn();
        let a = insert(&mut c, "A", "a", Platform::Other, "Default").unwrap();
        delete(&c, a).unwrap();
        let b = insert(&mut c, "B", "b", Platform::Other, "Default").unwrap();
        assert!(b > a);
    }

    #[test]
    fn free_slugs_get_a_number() {
        let mut c = conn();
        insert(&mut c, "Web", "web", Platform::JavaScript, "Default").unwrap();
        assert_eq!(free_slug(&c, "web").unwrap(), "web-2");
        assert_eq!(free_slug(&c, "app").unwrap(), "app");
        let id = insert(&mut c, "Web 2", "web-2", Platform::JavaScript, "Default").unwrap();
        assert_eq!(free_slug(&c, "web").unwrap(), "web-3");
        assert!(!slug_taken(&c, "web-2", Some(id)).unwrap());
        assert!(slug_taken(&c, "web-2", None).unwrap());
    }

    #[test]
    fn key_changes_apply_only_what_they_name() {
        let mut c = conn();
        let id = insert(&mut c, "Sylo", "sylo", Platform::Apple, "Default").unwrap();
        let k = keys(&c, id).unwrap()[0].id;
        let changes = KeyChanges {
            enabled: Some(false),
            rate_limit_per_minute: Some(Some(600)),
            allowed_origins: Some(vec!["https://sylo.app".into(), "*".into()]),
            ..KeyChanges::default()
        };
        assert!(update_key(&mut c, id, k, &changes).unwrap());
        let got = get_key(&c, id, k).unwrap().unwrap();
        assert_eq!(got.name, "Default");
        assert!(!got.enabled);
        assert_eq!(got.rate_limit_per_minute, Some(600));
        assert_eq!(got.allowed_origins, ["https://sylo.app", "*"]);
        assert!(!update_key(&mut c, id + 1, k, &changes).unwrap());
        assert_eq!(key_counts(&c).unwrap(), [(id, 0, 1)]);
    }
}
