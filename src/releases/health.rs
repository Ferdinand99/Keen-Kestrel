//! Release health (docs/design.md §4.9): the SDKs' `session` updates and
//! `sessions` aggregates, which ingest keeps in `store.sqlite`, rolled up
//! per project, release, environment and UTC day into `release_health`.
//!
//! A session counts on the day it started (clamped to the day before it
//! arrived at the earliest, so a wrong device clock can't reach back):
//!
//! * `sessions`: updates with `init: true`, plus every session an aggregate
//!   reports;
//! * `errored`: sessions with at least one error (crashed and abnormal ones
//!   included), `crashed`, `abnormal`: each session once, by its id;
//! * `users`, `crashed_users`: distinct device ids (`did`). Ingest replaced
//!   each `did` by a hash keyed with the day, so the same device has another
//!   hash tomorrow; the rollup keeps the hashes of the days that can still
//!   get sessions (`release_health_seen`) and deletes them after.
//!
//! The rolled-up items are deleted from the store. Crash-free sessions are
//! `1 − crashed / sessions`, crash-free users `1 − crashed_users / users`;
//! over several days the daily user counts add up (a device seen on two
//! days counts twice, as in the analytics' unique visitors).
//!
//! It runs on the writer's connection, every minute with the retention
//! job's tick.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Session items rolled up per transaction.
pub const BATCH: i64 = 2000;

/// What one rollup did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Session items read (and deleted).
    pub items: usize,
}

/// A day's or a release's counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Health {
    pub sessions: i64,
    pub errored: i64,
    pub crashed: i64,
    pub abnormal: i64,
    pub users: i64,
    pub crashed_users: i64,
}

impl Health {
    /// Crash-free sessions in percent, `None` without sessions.
    pub fn crash_free_sessions(&self) -> Option<f64> {
        rate(self.crashed, self.sessions)
    }

    /// Crash-free users in percent, `None` without users.
    pub fn crash_free_users(&self) -> Option<f64> {
        rate(self.crashed_users, self.users)
    }

    pub fn add(&mut self, other: &Health) {
        self.sessions += other.sessions;
        self.errored += other.errored;
        self.crashed += other.crashed;
        self.abnormal += other.abnormal;
        self.users += other.users;
        self.crashed_users += other.crashed_users;
    }
}

fn rate(bad: i64, all: i64) -> Option<f64> {
    (all > 0).then(|| (100.0 * (1.0 - bad.min(all) as f64 / all as f64)).clamp(0.0, 100.0))
}

/// The UTC day a session counts on: the day it `started`, but not before
/// the day before it arrived nor after the day it arrived.
pub fn bucket(started: Option<&str>, received_at: &str) -> String {
    let received_day = received_at.get(..10).unwrap_or(received_at).to_string();
    let Ok(received) = received_day.parse::<jiff::civil::Date>() else {
        return received_day;
    };
    let earliest = received.yesterday().unwrap_or(received);
    let day = started
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .map(|t| t.to_zoned(jiff::tz::TimeZone::UTC).date())
        .unwrap_or(received);
    day.clamp(earliest, received).to_string()
}

fn short_hash(text: &str) -> String {
    hex::encode(&Sha256::digest(text.as_bytes())[..16])
}

/// Where one item's counts go.
struct Bucket<'a> {
    project_id: i64,
    release_id: i64,
    environment: &'a str,
    day: String,
}

#[derive(Clone, Copy)]
enum Column {
    Sessions,
    Errored,
    Crashed,
    Abnormal,
    Users,
    CrashedUsers,
}

impl Column {
    fn name(self) -> &'static str {
        match self {
            Column::Sessions => "sessions",
            Column::Errored => "errored",
            Column::Crashed => "crashed",
            Column::Abnormal => "abnormal",
            Column::Users => "users",
            Column::CrashedUsers => "crashed_users",
        }
    }
}

fn add(c: &Connection, b: &Bucket<'_>, col: Column, n: i64) -> rusqlite::Result<()> {
    if n <= 0 {
        return Ok(());
    }
    let name = col.name();
    c.prepare_cached(&format!(
        "INSERT INTO release_health (release_id, environment, day, project_id, {name})
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (release_id, environment, day) DO UPDATE SET {name} = {name} + excluded.{name}"
    ))?
    .execute(params![b.release_id, b.environment, b.day, b.project_id, n])?;
    Ok(())
}

/// Notes `hash` as counted for `kind` on the bucket's day; `true` the first
/// time.
fn first_time(c: &Connection, b: &Bucket<'_>, kind: &str, hash: &str) -> rusqlite::Result<bool> {
    Ok(c.prepare_cached(
        "INSERT OR IGNORE INTO release_health_seen (release_id, environment, day, kind, hash)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![b.release_id, b.environment, b.day, kind, hash])?
        > 0)
}

/// Counts `col` once per distinct `hash` (`kind` names the set).
fn add_once(
    c: &Connection,
    b: &Bucket<'_>,
    col: Column,
    kind: &str,
    hash: Option<&str>,
) -> rusqlite::Result<()> {
    match hash {
        Some(h) if !h.is_empty() => {
            if first_time(c, b, kind, h)? {
                add(c, b, col, 1)?;
            }
            Ok(())
        }
        // Without an id it can only be counted as it comes.
        _ => add(c, b, col, 1),
    }
}

fn int(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0).max(0)
}

/// One `session` update.
fn session(
    c: &Connection,
    s: &Value,
    project_id: i64,
    release_id: i64,
    environment: &str,
    received_at: &str,
) -> rusqlite::Result<()> {
    let started = s
        .get("started")
        .or_else(|| s.get("timestamp"))
        .and_then(Value::as_str);
    let b = Bucket {
        project_id,
        release_id,
        environment,
        day: bucket(started, received_at),
    };
    let sid = s
        .get("sid")
        .and_then(Value::as_str)
        .map(|sid| short_hash(&sid.to_ascii_lowercase()));
    let did = s.get("did").and_then(Value::as_str);
    let status = s.get("status").and_then(Value::as_str).unwrap_or("ok");
    if s.get("init").and_then(Value::as_bool) == Some(true) {
        add(c, &b, Column::Sessions, 1)?;
    }
    if let Some(did) = did {
        add_once(c, &b, Column::Users, "u", Some(did))?;
    }
    let crashed = status == "crashed";
    let abnormal = status == "abnormal";
    if int(s, "errors") > 0 || crashed || abnormal || status == "errored" {
        add_once(c, &b, Column::Errored, "e", sid.as_deref())?;
    }
    if crashed {
        add_once(c, &b, Column::Crashed, "c", sid.as_deref())?;
        if let Some(did) = did {
            add_once(c, &b, Column::CrashedUsers, "cu", Some(did))?;
        }
    }
    if abnormal {
        add_once(c, &b, Column::Abnormal, "a", sid.as_deref())?;
    }
    Ok(())
}

/// One `sessions` aggregate (request-mode SDKs: counts per started minute).
fn aggregates(
    c: &Connection,
    s: &Value,
    project_id: i64,
    release_id: i64,
    environment: &str,
    received_at: &str,
) -> rusqlite::Result<()> {
    let Some(rows) = s.get("aggregates").and_then(Value::as_array) else {
        return Ok(());
    };
    for row in rows {
        let b = Bucket {
            project_id,
            release_id,
            environment,
            day: bucket(row.get("started").and_then(Value::as_str), received_at),
        };
        let (exited, errored, abnormal, crashed) = (
            int(row, "exited"),
            int(row, "errored"),
            int(row, "abnormal"),
            int(row, "crashed"),
        );
        add(
            c,
            &b,
            Column::Sessions,
            exited + errored + abnormal + crashed,
        )?;
        add(c, &b, Column::Errored, errored + abnormal + crashed)?;
        add(c, &b, Column::Crashed, crashed)?;
        add(c, &b, Column::Abnormal, abnormal)?;
        if let Some(did) = row.get("did").and_then(Value::as_str) {
            add_once(c, &b, Column::Users, "u", Some(did))?;
            if crashed > 0 {
                add_once(c, &b, Column::CrashedUsers, "cu", Some(did))?;
            }
        }
    }
    Ok(())
}

/// Rolls up to [`BATCH`] session items into `release_health` and deletes
/// them, and forgets the counted hashes of days that are over. Runs on the
/// writer's connection (`store.sqlite` attached); `now` is
/// `YYYY-MM-DD HH:MM:SS` UTC.
pub fn rollup(c: &mut Connection, now: &str) -> rusqlite::Result<Report> {
    let tx = c.transaction()?;
    let items: Vec<(i64, i64, String, String, String, String)> = tx
        .prepare_cached(
            "SELECT id, project_id, received_at, release, environment, data
             FROM store.session_items ORDER BY id LIMIT ?1",
        )?
        .query_map([BATCH], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut last = 0;
    for (id, project_id, received_at, release, environment, data) in &items {
        last = *id;
        let exists: bool = tx
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)")?
            .query_row([project_id], |r| r.get(0))?;
        if !exists {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let release_id = crate::db::issues::upsert_release(&tx, *project_id, release, received_at)?;
        if value.get("aggregates").is_some() {
            aggregates(
                &tx,
                &value,
                *project_id,
                release_id,
                environment,
                received_at,
            )?;
        } else {
            session(
                &tx,
                &value,
                *project_id,
                release_id,
                environment,
                received_at,
            )?;
        }
    }
    if last > 0 {
        tx.execute("DELETE FROM store.session_items WHERE id <= ?1", [last])?;
    }
    tx.execute(
        "DELETE FROM release_health_seen WHERE day < date(?1, '-2 days')",
        [now],
    )?;
    tx.commit()?;
    Ok(Report { items: items.len() })
}

/// Rolls up everything waiting (in batches).
pub fn rollup_all(c: &mut Connection, now: &str) -> rusqlite::Result<Report> {
    let mut total = Report::default();
    loop {
        let r = rollup(c, now)?;
        total.items += r.items;
        if (r.items as i64) < BATCH {
            return Ok(total);
        }
    }
}

/// Deletes rollups older than `days` (400 by the design) at `now`.
pub fn prune(c: &Connection, now: &str, days: i64) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM release_health WHERE day < date(?1, ?2)",
        params![now, format!("-{days} days")],
    )
}

/// The counts of `release_id` from `from` (a `YYYY-MM-DD` day) on, all
/// environments or one.
pub fn totals(
    c: &Connection,
    release_id: i64,
    from: Option<&str>,
    environment: Option<&str>,
) -> rusqlite::Result<Health> {
    c.prepare_cached(
        "SELECT coalesce(SUM(sessions), 0), coalesce(SUM(errored), 0), coalesce(SUM(crashed), 0),
                coalesce(SUM(abnormal), 0), coalesce(SUM(users), 0), coalesce(SUM(crashed_users), 0)
         FROM release_health
         WHERE release_id = ?1 AND (?2 IS NULL OR day >= ?2) AND (?3 IS NULL OR environment = ?3)",
    )?
    .query_row(params![release_id, from, environment], |r| {
        Ok(Health {
            sessions: r.get(0)?,
            errored: r.get(1)?,
            crashed: r.get(2)?,
            abnormal: r.get(3)?,
            users: r.get(4)?,
            crashed_users: r.get(5)?,
        })
    })
}

/// A release's counts per day from `from` to `to` (both `YYYY-MM-DD`,
/// every day listed, oldest first).
pub fn daily(
    c: &Connection,
    release_id: i64,
    from: &str,
    to: &str,
) -> rusqlite::Result<Vec<(String, Health)>> {
    let rows: Vec<(String, Health)> = c
        .prepare_cached(
            "SELECT day, SUM(sessions), SUM(errored), SUM(crashed), SUM(abnormal), SUM(users),
                    SUM(crashed_users)
             FROM release_health WHERE release_id = ?1 AND day >= ?2 AND day <= ?3
             GROUP BY day ORDER BY day",
        )?
        .query_map(params![release_id, from, to], |r| {
            Ok((
                r.get(0)?,
                Health {
                    sessions: r.get(1)?,
                    errored: r.get(2)?,
                    crashed: r.get(3)?,
                    abnormal: r.get(4)?,
                    users: r.get(5)?,
                    crashed_users: r.get(6)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let (Ok(mut day), Ok(last)) = (
        from.parse::<jiff::civil::Date>(),
        to.parse::<jiff::civil::Date>(),
    ) else {
        return Ok(rows);
    };
    let mut out = Vec::new();
    while day <= last {
        let key = day.to_string();
        let h = rows
            .iter()
            .find(|(d, _)| *d == key)
            .map(|(_, h)| *h)
            .unwrap_or_default();
        out.push((key, h));
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    Ok(out)
}

/// The share (percent) of the project's sessions since `from` that ran
/// `release_id`, `None` without sessions.
pub fn adoption(
    c: &Connection,
    project_id: i64,
    release_id: i64,
    from: &str,
) -> rusqlite::Result<Option<f64>> {
    let (mine, all): (i64, i64) = c
        .prepare_cached(
            "SELECT coalesce(SUM(CASE WHEN release_id = ?2 THEN sessions END), 0),
                    coalesce(SUM(sessions), 0)
             FROM release_health WHERE project_id = ?1 AND day >= ?3",
        )?
        .query_row(params![project_id, release_id, from], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok((all > 0).then(|| 100.0 * mine as f64 / all as f64))
}

/// Whether the project has any release-health data.
pub fn any(c: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    Ok(
        c.prepare_cached("SELECT 1 FROM release_health WHERE project_id = ?1 LIMIT 1")?
            .query_row([project_id], |_| Ok(()))
            .optional()?
            .is_some(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_count_on_the_day_they_started_within_reason() {
        assert_eq!(
            bucket(Some("2026-10-21T23:59:00Z"), "2026-10-22 00:01:00"),
            "2026-10-21"
        );
        assert_eq!(
            bucket(Some("2026-10-22T08:00:00.000000Z"), "2026-10-22 09:00:00"),
            "2026-10-22"
        );
        // A clock far off: the day before arrival at the earliest, the day of
        // arrival at the latest.
        assert_eq!(
            bucket(Some("2020-01-01T00:00:00Z"), "2026-10-22 09:00:00"),
            "2026-10-21"
        );
        assert_eq!(
            bucket(Some("2030-01-01T00:00:00Z"), "2026-10-22 09:00:00"),
            "2026-10-22"
        );
        assert_eq!(bucket(None, "2026-10-22 09:00:00"), "2026-10-22");
        assert_eq!(
            bucket(Some("nonsense"), "2026-10-22 09:00:00"),
            "2026-10-22"
        );
    }

    #[test]
    fn crash_free_rates() {
        let h = Health {
            sessions: 200,
            crashed: 3,
            users: 50,
            crashed_users: 2,
            ..Health::default()
        };
        assert_eq!(h.crash_free_sessions(), Some(98.5));
        assert_eq!(h.crash_free_users(), Some(96.0));
        assert_eq!(Health::default().crash_free_sessions(), None);
        let odd = Health {
            sessions: 1,
            crashed: 3,
            ..Health::default()
        };
        assert_eq!(odd.crash_free_sessions(), Some(0.0));
    }
}
