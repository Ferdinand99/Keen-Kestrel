//! Releases (docs/design.md §4.1, §4.5, §4.9): created on first sight (an
//! event or a session names one) or by sentry-cli, with their counts, health
//! and what happened in them.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::releases::health::{self, Health};

/// A release as the pages and the API show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub id: i64,
    pub project_id: i64,
    pub version: String,
    pub first_seen: String,
    pub last_seen: String,
    /// When sentry-cli finalized it.
    pub date_released: Option<String>,
    pub first_crash_at: Option<String>,
    /// Every event, stored or not.
    pub events: i64,
    /// Issues first seen in it.
    pub new_issues: i64,
    /// All time, every environment.
    pub health: Health,
    /// Its share of the project's sessions in the last 7 days.
    pub adoption: Option<f64>,
    /// Its share of the app's daily active users in the last 7 days (app
    /// analytics: `bundle@version+build` matched to the signals' app), when
    /// any app user ran it.
    pub adoption_users: Option<f64>,
}

const COLUMNS: &str = "r.id, r.project_id, r.version, r.first_seen, r.last_seen, r.date_released,
    r.first_crash_at, r.events,
    (SELECT COUNT(*) FROM issues i WHERE i.first_release_id = r.id)";

fn row(r: &Row<'_>) -> rusqlite::Result<Release> {
    Ok(Release {
        id: r.get(0)?,
        project_id: r.get(1)?,
        version: r.get(2)?,
        first_seen: r.get(3)?,
        last_seen: r.get(4)?,
        date_released: r.get(5)?,
        first_crash_at: r.get(6)?,
        events: r.get(7)?,
        new_issues: r.get(8)?,
        health: Health::default(),
        adoption: None,
        adoption_users: None,
    })
}

/// The first day of the last 7 at `now` (`YYYY-MM-DD HH:MM:SS`).
fn week_start(c: &Connection, now: &str) -> rusqlite::Result<String> {
    c.query_row("SELECT date(?1, '-6 days')", [now], |r| r.get(0))
}

fn fill(c: &Connection, mut r: Release, since: &str) -> rusqlite::Result<Release> {
    r.health = health::totals(c, r.id, None, None)?;
    r.adoption = health::adoption(c, r.project_id, r.id, since)?;
    let today = since
        .parse::<jiff::civil::Date>()
        .map(|d| crate::analytics::day_of(d) + 6)
        .unwrap_or_default();
    r.adoption_users =
        crate::analytics::app::query::release_adoption(c, r.project_id, &r.version, today)?;
    Ok(r)
}

/// A project's releases, newest first (by first sight), and how many in all.
pub fn list(
    c: &Connection,
    project_id: i64,
    limit: i64,
    offset: i64,
    now: &str,
) -> rusqlite::Result<(Vec<Release>, i64)> {
    let total: i64 = c.query_row(
        "SELECT COUNT(*) FROM releases WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    let since = week_start(c, now)?;
    let rows = c
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM releases r WHERE r.project_id = ?1 ORDER BY r.id DESC LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![project_id, limit, offset], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let rows = rows
        .into_iter()
        .map(|r| fill(c, r, &since))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((rows, total))
}

/// Every release of a project, oldest first (the API pages by id).
pub fn all(c: &Connection, project_id: i64, now: &str) -> rusqlite::Result<Vec<Release>> {
    let since = week_start(c, now)?;
    let rows = c
        .prepare_cached(&format!(
            "SELECT {COLUMNS} FROM releases r WHERE r.project_id = ?1 ORDER BY r.id"
        ))?
        .query_map([project_id], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(|r| fill(c, r, &since)).collect()
}

pub fn get(
    c: &Connection,
    project_id: i64,
    release_id: i64,
    now: &str,
) -> rusqlite::Result<Option<Release>> {
    let r = c
        .query_row(
            &format!("SELECT {COLUMNS} FROM releases r WHERE r.project_id = ?1 AND r.id = ?2"),
            [project_id, release_id],
            row,
        )
        .optional()?;
    match r {
        Some(r) => Ok(Some(fill(c, r, &week_start(c, now)?)?)),
        None => Ok(None),
    }
}

/// The id of a project's release by its version, if it exists.
pub fn id_of(c: &Connection, project_id: i64, version: &str) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT id FROM releases WHERE project_id = ?1 AND version = ?2",
        params![project_id, version],
        |r| r.get(0),
    )
    .optional()
}

/// Creates a release (sentry-cli `releases new`) or finds it; returns its
/// id. `released` (finalize) sets `date_released` when given.
pub fn ensure(
    c: &Connection,
    project_id: i64,
    version: &str,
    now: &str,
    released: Option<&str>,
) -> rusqlite::Result<i64> {
    let id = crate::db::issues::upsert_release(c, project_id, version, now)?;
    if let Some(at) = released {
        c.execute(
            "UPDATE releases SET date_released = ?2 WHERE id = ?1",
            params![id, at],
        )?;
    }
    Ok(id)
}

/// An issue as the release page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseIssue {
    pub id: i64,
    pub title: String,
    pub culprit: String,
    pub level: String,
    pub status: String,
    pub times_seen: i64,
    pub first_seen: String,
    pub last_seen: String,
}

fn issue_row(r: &Row<'_>) -> rusqlite::Result<ReleaseIssue> {
    Ok(ReleaseIssue {
        id: r.get(0)?,
        title: r.get(1)?,
        culprit: r.get(2)?,
        level: r.get(3)?,
        status: r.get(4)?,
        times_seen: r.get(5)?,
        first_seen: r.get(6)?,
        last_seen: r.get(7)?,
    })
}

/// Issues first seen in the release, most events first.
pub fn new_issues(
    c: &Connection,
    release_id: i64,
    limit: i64,
) -> rusqlite::Result<Vec<ReleaseIssue>> {
    c.prepare_cached(
        "SELECT id, title, culprit, level, status, times_seen, first_seen, last_seen FROM issues
         WHERE first_release_id = ?1 ORDER BY times_seen DESC, id LIMIT ?2",
    )?
    .query_map(params![release_id, limit], issue_row)?
    .collect()
}

/// Issues that regressed in the release (an event of it brought them back).
pub fn regressions(
    c: &Connection,
    project_id: i64,
    version: &str,
    limit: i64,
) -> rusqlite::Result<Vec<ReleaseIssue>> {
    c.prepare_cached(
        "SELECT i.id, i.title, i.culprit, i.level, i.status, i.times_seen, i.first_seen, i.last_seen
         FROM issues i WHERE i.project_id = ?1 AND EXISTS (
             SELECT 1 FROM issue_activity a WHERE a.issue_id = i.id AND a.kind = 'regressed'
             AND json_extract(a.data, '$.release') = ?2)
         ORDER BY i.last_seen DESC LIMIT ?3",
    )?
    .query_map(params![project_id, version, limit], issue_row)?
    .collect()
}

/// A debug file a release's events were symbolicated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseDif {
    pub id: i64,
    pub object_name: String,
    pub arch: String,
    pub debug_id: String,
    pub last_used_at: Option<String>,
}

/// The debug files whose newest user is the release.
pub fn debug_files(
    c: &Connection,
    project_id: i64,
    version: &str,
) -> rusqlite::Result<Vec<ReleaseDif>> {
    c.prepare_cached(
        "SELECT id, object_name, arch, debug_id, last_used_at FROM dif_files
         WHERE project_id = ?1 AND release = ?2 ORDER BY object_name, arch",
    )?
    .query_map(params![project_id, version], |r| {
        Ok(ReleaseDif {
            id: r.get(0)?,
            object_name: r.get(1)?,
            arch: r.get(2)?,
            debug_id: r.get(3)?,
            last_used_at: r.get(4)?,
        })
    })?
    .collect()
}

/// The release ids of `versions` in a project (for links), `version → id`.
pub fn ids(
    c: &Connection,
    project_id: i64,
    versions: &[&str],
) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut out = Vec::new();
    for v in versions {
        if v.is_empty() || out.iter().any(|(x, _): &(String, i64)| x == v) {
            continue;
        }
        if let Some(id) = id_of(c, project_id, v)? {
            out.push(((*v).to_string(), id));
        }
    }
    Ok(out)
}
