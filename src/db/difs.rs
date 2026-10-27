//! `dif_files` (debug information files), and the API tokens that may
//! upload them (the kit's `difs:write` token scope).

use rusqlite::{Connection, OptionalExtension, Row, params};
use vole_kit::db::api_tokens::{self, ApiToken};

/// One stored object: a debug id and architecture of an uploaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DifFile {
    pub id: i64,
    pub project_id: i64,
    pub debug_id: String,
    pub code_id: Option<String>,
    pub object_name: String,
    pub arch: String,
    pub kind: String,
    /// `debug`, `symtab`, `unwind`, `sources`.
    pub features: Vec<String>,
    pub size: i64,
    pub symcache_size: i64,
    pub sha1: String,
    pub release: Option<String>,
    pub uploaded_at: String,
    pub last_used_at: Option<String>,
}

impl DifFile {
    pub fn has_debug_info(&self) -> bool {
        self.features.iter().any(|f| f == "debug")
    }
}

/// What a new upload stores.
#[derive(Debug, Clone)]
pub struct NewDif {
    pub project_id: i64,
    pub debug_id: String,
    pub code_id: Option<String>,
    pub object_name: String,
    pub arch: String,
    pub kind: String,
    pub features: Vec<String>,
    pub size: i64,
    pub symcache_size: i64,
    pub sha1: String,
}

const COLUMNS: &str = "id, project_id, debug_id, code_id, object_name, arch, kind, features, size,
     symcache_size, sha1, release, uploaded_at, last_used_at";

fn row(r: &Row<'_>) -> rusqlite::Result<DifFile> {
    let features: String = r.get(7)?;
    Ok(DifFile {
        id: r.get(0)?,
        project_id: r.get(1)?,
        debug_id: r.get(2)?,
        code_id: r.get(3)?,
        object_name: r.get(4)?,
        arch: r.get(5)?,
        kind: r.get(6)?,
        features: features.split_whitespace().map(str::to_string).collect(),
        size: r.get(8)?,
        symcache_size: r.get(9)?,
        sha1: r.get(10)?,
        release: r.get(11)?,
        uploaded_at: r.get(12)?,
        last_used_at: r.get(13)?,
    })
}

/// A project's debug files, newest upload first.
pub fn list(c: &Connection, project_id: i64) -> rusqlite::Result<Vec<DifFile>> {
    c.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM dif_files WHERE project_id = ?1 ORDER BY uploaded_at DESC, id DESC"
    ))?
    .query_map([project_id], row)?
    .collect()
}

/// Every `(project_id, debug_id)` (for the in-memory index at start).
pub fn all_ids(c: &Connection) -> rusqlite::Result<Vec<(i64, String)>> {
    c.prepare("SELECT project_id, debug_id FROM dif_files")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect()
}

pub fn get(c: &Connection, project_id: i64, id: i64) -> rusqlite::Result<Option<DifFile>> {
    c.query_row(
        &format!("SELECT {COLUMNS} FROM dif_files WHERE project_id = ?1 AND id = ?2"),
        params![project_id, id],
        row,
    )
    .optional()
}

pub fn by_debug_id(
    c: &Connection,
    project_id: i64,
    debug_id: &str,
) -> rusqlite::Result<Option<DifFile>> {
    c.query_row(
        &format!("SELECT {COLUMNS} FROM dif_files WHERE project_id = ?1 AND debug_id = ?2"),
        params![project_id, debug_id],
        row,
    )
    .optional()
}

/// The objects an uploaded file (by its SHA-1) became, if it was seen.
pub fn by_sha1(c: &Connection, project_id: i64, sha1: &str) -> rusqlite::Result<Vec<DifFile>> {
    c.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM dif_files WHERE project_id = ?1 AND sha1 = ?2 ORDER BY id"
    ))?
    .query_map(params![project_id, sha1], row)?
    .collect()
}

/// Stores `d`, replacing the row of the same debug id (a new upload of the
/// same build). Returns the row id.
pub fn upsert(c: &Connection, d: &NewDif) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO dif_files (project_id, debug_id, code_id, object_name, arch, kind, features,
             size, symcache_size, sha1)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (project_id, debug_id) DO UPDATE SET
             code_id = excluded.code_id, object_name = excluded.object_name,
             arch = excluded.arch, kind = excluded.kind, features = excluded.features,
             size = excluded.size, symcache_size = excluded.symcache_size,
             sha1 = excluded.sha1, uploaded_at = datetime('now')",
        params![
            d.project_id,
            d.debug_id,
            d.code_id,
            d.object_name,
            d.arch,
            d.kind,
            d.features.join(" "),
            d.size,
            d.symcache_size,
            d.sha1
        ],
    )?;
    c.query_row(
        "SELECT id FROM dif_files WHERE project_id = ?1 AND debug_id = ?2",
        params![d.project_id, d.debug_id],
        |r| r.get(0),
    )
}

/// Deletes one row; `false` when there was none.
pub fn delete(c: &Connection, project_id: i64, id: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "DELETE FROM dif_files WHERE project_id = ?1 AND id = ?2",
        params![project_id, id],
    )? > 0)
}

/// Notes that events used `debug_id` at `at` (and from which release).
pub fn mark_used(
    c: &Connection,
    project_id: i64,
    debug_id: &str,
    release: Option<&str>,
    at: &str,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "UPDATE dif_files SET last_used_at = ?3, release = COALESCE(?4, release)
         WHERE project_id = ?1 AND debug_id = ?2",
    )?
    .execute(params![project_id, debug_id, at, release])?;
    Ok(())
}

/// A project's debug-file retention: days, and how many of the newest
/// releases keep theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DifRetention {
    pub days: i64,
    pub releases: i64,
}

impl Default for DifRetention {
    fn default() -> Self {
        DifRetention {
            days: 180,
            releases: 5,
        }
    }
}

/// Every project's id and debug-file retention.
pub fn all_retention(c: &Connection) -> rusqlite::Result<Vec<(i64, DifRetention)>> {
    c.prepare("SELECT id, retention_dif_days, retention_dif_releases FROM projects ORDER BY id")?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                DifRetention {
                    days: r.get(1)?,
                    releases: r.get(2)?,
                },
            ))
        })?
        .collect()
}

/// The debug files of `project_id` that retention removes at `now`: neither
/// uploaded nor used within `r.days`, and not used by one of the project's
/// `r.releases` newest releases. Returns `(id, debug_id)`.
pub fn expired(
    c: &Connection,
    project_id: i64,
    r: DifRetention,
    now: &str,
) -> rusqlite::Result<Vec<(i64, String)>> {
    c.prepare(
        "SELECT d.id, d.debug_id FROM dif_files d
         WHERE d.project_id = ?1
           AND max(d.uploaded_at, coalesce(d.last_used_at, '')) < datetime(?2, ?3)
           AND (d.release IS NULL OR d.release NOT IN (
                SELECT version FROM releases WHERE project_id = ?1 ORDER BY id DESC LIMIT ?4))",
    )?
    .query_map(
        params![
            project_id,
            now,
            format!("-{} days", r.days.max(1)),
            r.releases
        ],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?
    .collect()
}

/// Which projects a `difs:write` token may upload to (migration 0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// No binding on record: the token is refused. A binding is never
    /// guessed, so this never widens access.
    Unbound,
    /// Every project (tokens from before the binding existed, and the ones
    /// an administrator made so on purpose).
    AllProjects,
    /// This project only.
    Project(i64),
}

/// What `token_id` is bound to.
pub fn binding(c: &Connection, token_id: i64) -> rusqlite::Result<Binding> {
    let row = c
        .query_row(
            "SELECT project_id FROM difs_token_projects WHERE token_id = ?1",
            params![token_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?;
    Ok(match row {
        None => Binding::Unbound,
        Some(None) => Binding::AllProjects,
        Some(Some(id)) => Binding::Project(id),
    })
}

/// Binds `token_id` to `project_id` (`None`: every project). Call it in the
/// transaction that makes the token.
pub fn bind(c: &Connection, token_id: i64, project_id: Option<i64>) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO difs_token_projects (token_id, project_id) VALUES (?1, ?2)",
        params![token_id, project_id],
    )?;
    Ok(())
}

/// The tokens with the `difs:write` scope and what each is bound to, newest
/// first: `user_id`'s, or everyone's.
pub fn upload_tokens(
    c: &Connection,
    user_id: Option<i64>,
) -> rusqlite::Result<Vec<(ApiToken, Binding)>> {
    let mut tokens = Vec::new();
    for id in api_tokens::with_scope(c, crate::product::DIFS_WRITE)? {
        if let Some(token) = api_tokens::get(c, id)?
            && user_id.is_none_or(|u| u == token.user_id)
        {
            tokens.push((token, binding(c, id)?));
        }
    }
    Ok(tokens)
}
