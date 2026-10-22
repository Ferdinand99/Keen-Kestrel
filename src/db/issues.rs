//! Issues and what hangs off them: the writer's upsert of one event
//! (release, environment, issue, grouping hash, tag values, users, hourly
//! counts, activity, event row and payload). The pages' reads and the
//! lifecycle actions are in [`super::triage`].
//!
//! The functions run inside the writer's transaction on a connection that
//! has `store.sqlite` attached as `store`.

use rusqlite::{Connection, OptionalExtension, params};

/// Stored payloads per issue and UTC day unless the project says otherwise
/// (`projects.payloads_per_day`); later events only count.
pub const MAX_PAYLOADS_PER_DAY: i64 = 1000;
/// Custom tag keys per issue (on top of the whitelisted ones).
pub const MAX_CUSTOM_TAG_KEYS: i64 = 20;
/// Distinct values counted per issue and tag key.
pub const MAX_TAG_VALUES: i64 = 1000;

/// Tag keys Kestrel derives from every event; they don't count towards the
/// custom keys.
pub const STANDARD_TAGS: &[&str] = &[
    "level",
    "environment",
    "release",
    "dist",
    "os",
    "os.name",
    "client_os",
    "client_os.name",
    "browser",
    "browser.name",
    "device",
    "device.family",
    "runtime",
    "runtime.name",
    "server_name",
    "transaction",
    "handled",
    "mechanism",
];

/// One normalised, grouped event, ready to be written.
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub project_id: i64,
    pub event_id: String,
    /// When it happened, `YYYY-MM-DD HH:MM:SS` UTC.
    pub timestamp: String,
    pub received_at: String,
    pub level: String,
    pub platform: String,
    pub title: String,
    pub culprit: String,
    pub release: Option<String>,
    pub environment: String,
    pub user_hash: Option<String>,
    /// Standard tags first, then at most 20 custom ones.
    pub tags: Vec<(String, String)>,
    pub grouping_config: &'static str,
    pub hash: String,
    /// The hash of the previous grouping configuration (`kestrel:v1`) when
    /// it differs: an issue it points at gets the event (and `hash`).
    pub legacy_hash: Option<String>,
    /// JSON of the grouping components.
    pub components: String,
    pub unsymbolicated: bool,
    /// A crash (fatal, or unhandled): the first one of a release can alert.
    pub crash: bool,
    /// zstd-compressed JSON.
    pub payload: Vec<u8>,
}

/// What happened to one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new issue was made.
    NewIssue(i64),
    /// An existing issue; `true` when it regressed.
    Existing(i64, bool),
    /// The event id was already stored.
    Duplicate,
}

/// The UTC day of a `YYYY-MM-DD HH:MM:SS` time.
fn day(time: &str) -> &str {
    time.get(..10).unwrap_or(time)
}

pub fn upsert_release(
    c: &Connection,
    project_id: i64,
    version: &str,
    ts: &str,
) -> rusqlite::Result<i64> {
    c.prepare_cached(
        "INSERT INTO releases (project_id, version, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT (project_id, version) DO UPDATE SET
             first_seen = min(first_seen, excluded.first_seen),
             last_seen = max(last_seen, excluded.last_seen)
         RETURNING id",
    )?
    .query_row(params![project_id, version, ts], |r| r.get(0))
}

fn upsert_environment(
    c: &Connection,
    project_id: i64,
    name: &str,
    ts: &str,
) -> rusqlite::Result<i64> {
    c.prepare_cached(
        "INSERT INTO environments (project_id, name, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT (project_id, name) DO UPDATE SET
             first_seen = min(first_seen, excluded.first_seen),
             last_seen = max(last_seen, excluded.last_seen)
         RETURNING id",
    )?
    .query_row(params![project_id, name, ts], |r| r.get(0))
}

struct IssueState {
    id: i64,
    status: String,
    resolved_in_release: Option<String>,
    ignore_until: Option<String>,
    ignore_until_count: Option<i64>,
    times_seen: i64,
    payload_day: Option<String>,
    payloads_today: i64,
}

fn find_issue(c: &Connection, project_id: i64, hash: &str) -> rusqlite::Result<Option<IssueState>> {
    c.prepare_cached(
        "SELECT i.id, i.status, i.resolved_in_release, i.ignore_until, i.ignore_until_count,
                i.times_seen, i.payload_day, i.payloads_today
         FROM issue_hashes h JOIN issues i ON i.id = h.issue_id
         WHERE h.project_id = ?1 AND h.hash = ?2",
    )?
    .query_row(params![project_id, hash], |r| {
        Ok(IssueState {
            id: r.get(0)?,
            status: r.get(1)?,
            resolved_in_release: r.get(2)?,
            ignore_until: r.get(3)?,
            ignore_until_count: r.get(4)?,
            times_seen: r.get(5)?,
            payload_day: r.get(6)?,
            payloads_today: r.get(7)?,
        })
    })
    .optional()
}

/// Whether an event from `release` regresses an issue resolved in the next
/// release after `resolved_in`: only events from a release first seen after
/// that one do.
fn newer_release(
    c: &Connection,
    project_id: i64,
    resolved_in: &str,
    release_id: Option<i64>,
) -> rusqlite::Result<bool> {
    let Some(release_id) = release_id else {
        return Ok(false);
    };
    let resolved_id: Option<i64> = c
        .prepare_cached("SELECT id FROM releases WHERE project_id = ?1 AND version = ?2")?
        .query_row(params![project_id, resolved_in], |r| r.get(0))
        .optional()?;
    Ok(resolved_id.is_none_or(|resolved| release_id > resolved))
}

/// The project's daily cap on stored payloads per issue.
fn payload_cap(c: &Connection, project_id: i64) -> rusqlite::Result<i64> {
    Ok(
        c.prepare_cached("SELECT payloads_per_day FROM projects WHERE id = ?1")?
            .query_row([project_id], |r| r.get(0))
            .optional()?
            .unwrap_or(MAX_PAYLOADS_PER_DAY),
    )
}

/// Notes something that happened to an issue (`actor` `None`: Kestrel).
pub fn log_activity(
    c: &Connection,
    issue_id: i64,
    kind: &str,
    actor: Option<&str>,
    data: &serde_json::Value,
) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO issue_activity (issue_id, kind, actor, data) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![issue_id, kind, actor, data.to_string()])?;
    Ok(())
}

/// `YYYY-MM-DD HH:00:00` of a `YYYY-MM-DD HH:MM:SS` time.
pub fn hour_of(time: &str) -> String {
    match time.get(..13) {
        Some(h) => format!("{h}:00:00"),
        None => time.to_string(),
    }
}

/// What writing one event did, for the alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    pub outcome: Outcome,
    /// The release whose first crash this was.
    pub first_crash: Option<i64>,
}

/// [`write_event_full`] without the details for the alerts.
pub fn write_event(c: &Connection, e: &EventRecord) -> rusqlite::Result<Outcome> {
    write_event_full(c, e).map(|w| w.outcome)
}

/// The issue an event's hash points at; failing that, the one its
/// previous configuration's hash points at, which then gets the new hash
/// too (so the issue keeps its configuration and its events).
fn find_or_adopt(c: &Connection, e: &EventRecord) -> rusqlite::Result<Option<IssueState>> {
    if let Some(issue) = find_issue(c, e.project_id, &e.hash)? {
        return Ok(Some(issue));
    }
    let Some(legacy) = &e.legacy_hash else {
        return Ok(None);
    };
    let Some(issue) = find_issue(c, e.project_id, legacy)? else {
        return Ok(None);
    };
    c.prepare_cached(
        "INSERT OR IGNORE INTO issue_hashes (project_id, hash, issue_id, grouping_config, components)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        e.project_id,
        e.hash,
        issue.id,
        e.grouping_config,
        e.components
    ])?;
    Ok(Some(issue))
}

/// Writes one event: release, environment, issue (new or updated, with
/// regression and ignore conditions, both logged), tag values, the user and
/// the hourly count, and, while the issue has stored fewer than the
/// project's daily cap ([`MAX_PAYLOADS_PER_DAY`] by default) today, the
/// event row and its payload. A crash notes its release's first crash.
pub fn write_event_full(c: &Connection, e: &EventRecord) -> rusqlite::Result<Written> {
    let duplicate: bool = c
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM events WHERE project_id = ?1 AND event_id = ?2)",
        )?
        .query_row(params![e.project_id, e.event_id], |r| r.get(0))?;
    if duplicate {
        return Ok(Written {
            outcome: Outcome::Duplicate,
            first_crash: None,
        });
    }
    let release_id = match &e.release {
        Some(version) => {
            let id = upsert_release(c, e.project_id, version, &e.timestamp)?;
            c.prepare_cached("UPDATE releases SET events = events + 1 WHERE id = ?1")?
                .execute([id])?;
            Some(id)
        }
        None => None,
    };
    let first_crash = match release_id {
        Some(id) if e.crash => {
            let changed = c
                .prepare_cached(
                    "UPDATE releases SET first_crash_at = ?2 WHERE id = ?1 AND first_crash_at IS NULL",
                )?
                .execute(params![id, e.received_at])?;
            (changed > 0).then_some(id)
        }
        _ => None,
    };
    let environment_id = upsert_environment(c, e.project_id, &e.environment, &e.timestamp)?;
    let today = day(&e.received_at);
    let cap = payload_cap(c, e.project_id)?;

    let (issue_id, outcome, store_payload) = match find_or_adopt(c, e)? {
        None => {
            c.prepare_cached(
                "INSERT INTO issues (project_id, title, culprit, level, platform, first_seen, last_seen,
                     times_seen, first_release_id, last_release_id, grouping_config, unsymbolicated,
                     payload_day, payloads_today)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 1, ?7, ?7, ?8, ?9, ?10, 0)",
            )?
            .execute(params![
                e.project_id,
                e.title,
                e.culprit,
                e.level,
                e.platform,
                e.timestamp,
                release_id,
                e.grouping_config,
                e.unsymbolicated,
                today,
            ])?;
            let id = c.last_insert_rowid();
            c.prepare_cached(
                "INSERT INTO issue_hashes (project_id, hash, issue_id, grouping_config, components)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![
                e.project_id,
                e.hash,
                id,
                e.grouping_config,
                e.components
            ])?;
            (id, Outcome::NewIssue(id), true)
        }
        Some(issue) => {
            let regressed = issue.status == "resolved"
                && match &issue.resolved_in_release {
                    None => true,
                    Some(resolved_in) => newer_release(c, e.project_id, resolved_in, release_id)?,
                };
            let unignored = issue.status == "ignored"
                && (issue
                    .ignore_until
                    .as_deref()
                    .is_some_and(|until| e.received_at.as_str() >= until)
                    || issue
                        .ignore_until_count
                        .is_some_and(|count| issue.times_seen + 1 >= count));
            let (status, substatus) = if regressed {
                (Some("unresolved"), Some("regressed"))
            } else if unignored {
                (Some("unresolved"), Some("ongoing"))
            } else {
                (None, None)
            };
            let store_payload =
                issue.payload_day.as_deref() != Some(today) || issue.payloads_today < cap;
            c.prepare_cached(
                "UPDATE issues SET
                     times_seen = times_seen + 1,
                     level = ?2,
                     first_seen = min(first_seen, ?3),
                     last_seen = max(last_seen, ?3),
                     first_release_id = coalesce(first_release_id, ?4),
                     last_release_id = coalesce(?4, last_release_id),
                     status = coalesce(?5, status),
                     substatus = coalesce(?6, substatus),
                     resolved_at = CASE WHEN ?5 IS NULL THEN resolved_at END,
                     resolved_in_release = CASE WHEN ?5 IS NULL THEN resolved_in_release END,
                     ignore_until = CASE WHEN ?5 IS NULL THEN ignore_until END,
                     ignore_until_count = CASE WHEN ?5 IS NULL THEN ignore_until_count END
                 WHERE id = ?1",
            )?
            .execute(params![
                issue.id,
                e.level,
                e.timestamp,
                release_id,
                status,
                substatus
            ])?;
            if issue.payload_day.as_deref() != Some(today) {
                c.prepare_cached(
                    "UPDATE issues SET payload_day = ?2, payloads_today = 0 WHERE id = ?1",
                )?
                .execute(params![issue.id, today])?;
            }
            if regressed {
                log_activity(
                    c,
                    issue.id,
                    "regressed",
                    None,
                    &serde_json::json!({ "release": e.release }),
                )?;
            } else if unignored {
                log_activity(c, issue.id, "unignored", None, &serde_json::json!({}))?;
            }
            (
                issue.id,
                Outcome::Existing(issue.id, regressed),
                store_payload,
            )
        }
    };

    write_tags(c, issue_id, &e.tags, &e.timestamp)?;
    c.prepare_cached(
        "INSERT INTO issue_hours (issue_id, hour, project_id, events) VALUES (?1, ?2, ?3, 1)
         ON CONFLICT (issue_id, hour) DO UPDATE SET events = events + 1",
    )?
    .execute(params![issue_id, hour_of(&e.received_at), e.project_id])?;
    if let Some(user) = &e.user_hash {
        let added = c
            .prepare_cached(
                "INSERT OR IGNORE INTO issue_users (issue_id, user_hash) VALUES (?1, ?2)",
            )?
            .execute(params![issue_id, user])?;
        if added > 0 {
            c.prepare_cached("UPDATE issues SET users_seen = users_seen + 1 WHERE id = ?1")?
                .execute([issue_id])?;
        }
    }

    if store_payload {
        c.prepare_cached(
            "INSERT INTO store.payloads (project_id, event_id, received_at, data) VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![e.project_id, e.event_id, e.received_at, e.payload])?;
        let payload_id = c.last_insert_rowid();
        c.prepare_cached(
            "INSERT INTO events (project_id, issue_id, event_id, timestamp, received_at, level,
                 release_id, environment_id, user_hash, payload_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?
        .execute(params![
            e.project_id,
            issue_id,
            e.event_id,
            e.timestamp,
            e.received_at,
            e.level,
            release_id,
            environment_id,
            e.user_hash,
            payload_id,
        ])?;
        c.prepare_cached("UPDATE issues SET payloads_today = payloads_today + 1 WHERE id = ?1")?
            .execute([issue_id])?;
    }
    Ok(Written {
        outcome,
        first_crash,
    })
}

/// Counts the event's tag values: standard keys always, custom keys while
/// the issue has fewer than [`MAX_CUSTOM_TAG_KEYS`], new values while the key
/// has fewer than [`MAX_TAG_VALUES`].
fn write_tags(
    c: &Connection,
    issue_id: i64,
    tags: &[(String, String)],
    ts: &str,
) -> rusqlite::Result<()> {
    for (key, value) in tags {
        let updated = c
            .prepare_cached(
                "UPDATE issue_tag_values SET times_seen = times_seen + 1,
                     first_seen = min(first_seen, ?4), last_seen = max(last_seen, ?4)
                 WHERE issue_id = ?1 AND key = ?2 AND value = ?3",
            )?
            .execute(params![issue_id, key, value, ts])?;
        if updated > 0 {
            continue;
        }
        let values: i64 = c
            .prepare_cached(
                "SELECT COUNT(*) FROM issue_tag_values WHERE issue_id = ?1 AND key = ?2",
            )?
            .query_row(params![issue_id, key], |r| r.get(0))?;
        if values >= MAX_TAG_VALUES {
            continue;
        }
        if values == 0 && !STANDARD_TAGS.contains(&key.as_str()) {
            let custom: i64 = c
                .prepare_cached(
                    "SELECT key FROM issue_tag_values WHERE issue_id = ?1 GROUP BY key",
                )?
                .query_map([issue_id], |r| r.get::<_, String>(0))?
                .filter(|k| {
                    k.as_ref()
                        .is_ok_and(|k| !STANDARD_TAGS.contains(&k.as_str()))
                })
                .count() as i64;
            if custom >= MAX_CUSTOM_TAG_KEYS {
                continue;
            }
        }
        c.prepare_cached(
            "INSERT INTO issue_tag_values (issue_id, key, value, times_seen, first_seen, last_seen)
             VALUES (?1, ?2, ?3, 1, ?4, ?4)",
        )?
        .execute(params![issue_id, key, value, ts])?;
    }
    Ok(())
}

/// Whether an event row exists (attachments only belong to stored events).
pub fn event_stored(c: &Connection, project_id: i64, event_id: &str) -> rusqlite::Result<bool> {
    c.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM events WHERE project_id = ?1 AND event_id = ?2)",
    )?
    .query_row(params![project_id, event_id], |r| r.get(0))
}

/// An issue as the tests (and later the pages) read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub culprit: String,
    pub level: String,
    pub status: String,
    pub substatus: String,
    pub times_seen: i64,
    pub first_seen: String,
    pub last_seen: String,
    pub grouping_config: String,
    pub unsymbolicated: bool,
}

/// A project's issues, most recently seen first.
pub fn list(c: &Connection, project_id: i64) -> rusqlite::Result<Vec<Issue>> {
    c.prepare(
        "SELECT id, project_id, title, culprit, level, status, substatus, times_seen, first_seen,
                last_seen, grouping_config, unsymbolicated
         FROM issues WHERE project_id = ?1 ORDER BY last_seen DESC, id DESC",
    )?
    .query_map([project_id], |r| {
        Ok(Issue {
            id: r.get(0)?,
            project_id: r.get(1)?,
            title: r.get(2)?,
            culprit: r.get(3)?,
            level: r.get(4)?,
            status: r.get(5)?,
            substatus: r.get(6)?,
            times_seen: r.get(7)?,
            first_seen: r.get(8)?,
            last_seen: r.get(9)?,
            grouping_config: r.get(10)?,
            unsymbolicated: r.get(11)?,
        })
    })?
    .collect()
}

/// Marks an issue resolved (optionally in the next release after `in_release`).
pub fn resolve(c: &Connection, issue_id: i64, in_release: Option<&str>) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE issues SET status = 'resolved', resolved_at = datetime('now'), resolved_in_release = ?2
         WHERE id = ?1",
        params![issue_id, in_release],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// kestrel.sqlite in memory with a store attached from a temporary file.
    fn conn(name: &str) -> (Connection, std::path::PathBuf) {
        crate::product::install();
        let path =
            std::env::temp_dir().join(format!("kk-issues-{}-{name}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = Connection::open(&path).unwrap();
        crate::store::prepare(&store).unwrap();
        vole_kit::db::migrate::apply_migrations_with(&mut store, &crate::store::MIGRATIONS)
            .unwrap();
        drop(store);
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(vole_kit::db::CONNECTION_PRAGMAS).unwrap();
        vole_kit::db::apply_migrations(&mut c).unwrap();
        c.execute("ATTACH DATABASE ?1 AS store", [path.to_string_lossy()])
            .unwrap();
        crate::db::projects::insert(&mut c, "P", "p", crate::project::Platform::Other, "Default")
            .unwrap();
        (c, path)
    }

    fn record(n: usize, hash: &str) -> EventRecord {
        EventRecord {
            project_id: 1,
            event_id: format!("{n:032x}"),
            timestamp: "2026-10-22 12:00:00".into(),
            received_at: "2026-10-22 12:00:01".into(),
            level: "error".into(),
            platform: "python".into(),
            title: "ValueError: bad".into(),
            culprit: "app in f".into(),
            release: Some("app@1".into()),
            environment: "production".into(),
            user_hash: None,
            tags: vec![("level".into(), "error".into())],
            grouping_config: "kestrel:v2",
            hash: hash.repeat(32 / hash.len()),
            legacy_hash: None,
            components: "{}".into(),
            unsymbolicated: false,
            crash: false,
            payload: vec![1, 2, 3],
        }
    }

    #[test]
    fn new_issues_existing_issues_and_duplicates() {
        let (c, path) = conn("basic");
        let first = write_event(&c, &record(1, "a")).unwrap();
        let Outcome::NewIssue(id) = first else {
            panic!("{first:?}")
        };
        assert_eq!(
            write_event(&c, &record(2, "a")).unwrap(),
            Outcome::Existing(id, false)
        );
        assert_eq!(
            write_event(&c, &record(2, "a")).unwrap(),
            Outcome::Duplicate
        );
        assert!(matches!(
            write_event(&c, &record(3, "b")).unwrap(),
            Outcome::NewIssue(_)
        ));
        let issues = list(&c, 1).unwrap();
        assert_eq!(issues.len(), 2);
        let a = issues.iter().find(|i| i.id == id).unwrap();
        assert_eq!(a.times_seen, 2);
        let releases: i64 = c
            .query_row("SELECT COUNT(*) FROM releases", [], |r| r.get(0))
            .unwrap();
        assert_eq!(releases, 1);
        let fts: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM issues_fts WHERE issues_fts MATCH 'ValueError'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 2);
        drop(c);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn payloads_stop_at_1000_a_day_but_counts_go_on() {
        let (mut c, path) = conn("cap");
        let tx = c.transaction().unwrap();
        for n in 0..1005 {
            write_event(&tx, &record(n, "c")).unwrap();
        }
        tx.commit().unwrap();
        let count = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT times_seen FROM issues"), 1005);
        assert_eq!(count("SELECT COUNT(*) FROM events"), MAX_PAYLOADS_PER_DAY);
        assert_eq!(
            count("SELECT COUNT(*) FROM store.payloads"),
            MAX_PAYLOADS_PER_DAY
        );
        assert_eq!(
            count("SELECT times_seen FROM issue_tag_values WHERE key = 'level'"),
            1005
        );
        // A new day starts over.
        let mut next = record(2000, "c");
        next.received_at = "2026-10-23 00:00:01".into();
        write_event(&c, &next).unwrap();
        assert_eq!(
            count("SELECT COUNT(*) FROM events"),
            MAX_PAYLOADS_PER_DAY + 1
        );
        assert_eq!(count("SELECT payloads_today FROM issues"), 1);
        drop(c);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn custom_tag_keys_are_capped() {
        let (c, path) = conn("tags");
        let mut r = record(1, "d");
        r.tags = (0..25)
            .map(|i| (format!("custom{i:02}"), "v".to_string()))
            .collect();
        r.tags.push(("level".into(), "error".into()));
        write_event(&c, &r).unwrap();
        let keys: i64 = c
            .query_row(
                "SELECT COUNT(DISTINCT key) FROM issue_tag_values",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(keys, MAX_CUSTOM_TAG_KEYS + 1, "20 custom keys and level");
        drop(c);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn v1_issues_keep_getting_their_events() {
        let (c, path) = conn("legacy");
        // An issue made by kestrel:v1 ...
        let mut old = record(1, "f");
        old.grouping_config = "kestrel:v1";
        let Outcome::NewIssue(id) = write_event(&c, &old).unwrap() else {
            panic!()
        };
        // ... gets the v2 event whose v1 hash matches, and the v2 hash with it.
        let mut new = record(2, "9");
        new.legacy_hash = Some(old.hash.clone());
        assert_eq!(write_event(&c, &new).unwrap(), Outcome::Existing(id, false));
        let configs: Vec<String> = c
            .prepare("SELECT grouping_config FROM issue_hashes WHERE issue_id = ?1 ORDER BY id")
            .unwrap()
            .query_map([id], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(configs, ["kestrel:v1", "kestrel:v2"]);
        let config: String = c
            .query_row(
                "SELECT grouping_config FROM issues WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(config, "kestrel:v1", "the issue keeps its configuration");
        // Later events find it by the v2 hash directly.
        let mut later = record(3, "9");
        later.legacy_hash = Some("0".repeat(32));
        assert_eq!(
            write_event(&c, &later).unwrap(),
            Outcome::Existing(id, false)
        );
        drop(c);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_release_notes_its_first_crash_once() {
        let (c, path) = conn("crash");
        let mut r = record(1, "a");
        assert_eq!(write_event_full(&c, &r).unwrap().first_crash, None);
        r = record(2, "b");
        r.crash = true;
        let first = write_event_full(&c, &r).unwrap().first_crash;
        assert!(first.is_some());
        r = record(3, "b");
        r.crash = true;
        assert_eq!(write_event_full(&c, &r).unwrap().first_crash, None);
        r = record(4, "b");
        r.crash = true;
        r.release = Some("app@2".into());
        assert!(write_event_full(&c, &r).unwrap().first_crash.is_some());
        let events: i64 = c
            .query_row(
                "SELECT events FROM releases WHERE version = 'app@1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 3);
        drop(c);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ignored_issues_come_back_when_their_condition_is_met() {
        let (c, path) = conn("ignore");
        write_event(&c, &record(1, "e")).unwrap();
        c.execute(
            "UPDATE issues SET status = 'ignored', ignore_until_count = 3",
            [],
        )
        .unwrap();
        write_event(&c, &record(2, "e")).unwrap();
        let status = |c: &Connection| -> (String, String) {
            c.query_row("SELECT status, substatus FROM issues", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
        };
        assert_eq!(status(&c).0, "ignored");
        write_event(&c, &record(3, "e")).unwrap();
        assert_eq!(status(&c), ("unresolved".into(), "ongoing".into()));
        drop(c);
        let _ = std::fs::remove_file(path);
    }
}
