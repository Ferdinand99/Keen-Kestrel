//! The app signals' part of the analytics rollup (docs/design.md §5.3):
//! [`crate::analytics::rollup::rollup`] reads raw events in id order and
//! hands the app's (`source = 'app'`) to [`add`], in the same transaction
//! on the writer's connection. Test signals (debug, TestFlight, simulator)
//! stay raw: the dashboard includes them on request from the raw signals.
//!
//! * `analytics_days` (`source = 'app'`): signals (`events`), daily active
//!   users (`visitors`), sessions, and weekly and monthly active users
//!   (`wau`, `mau`: distinct users in the 7 and 30 days up to the day);
//! * `analytics_hours`: signals, users and sessions per hour;
//! * `analytics_breakdown`: per day, [`AppDim`] and value, signals and
//!   distinct users; and `payload`: signals per type, payload key and value
//!   (at most [`MAX_PAYLOAD_VALUES`] values per type and key and day, the
//!   rest under [`OTHER`]).
//!
//! **Weekly and monthly users** are exact and cheap: `store.app_users` keeps
//! each day's users for 31 days. When a day's row is made, its weekly and
//! monthly counts start from the distinct users of the 6 and 29 days before
//! (two `COUNT(DISTINCT)` per project and day); after that, each user new to
//! the day is looked up once (`idx_app_users_user`): with no day in the 29
//! before, they are new to the month and the week; with none in the 6
//! before, new to the week. A user who arrives late for yesterday is added
//! to today's counts too when today has no other day of theirs in its
//! windows. Older days that get signals late are rolled up again from the
//! raw signals, windows included. Today's row is made at the first rollup
//! of the day for every project with users in the month before, so quiet
//! days have their weekly and monthly users too.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{APP, AppDim, AppSignal};
use crate::analytics::rollup::new_visitors;
use crate::analytics::{day_number, day_string, hour_string, value_hash};

/// Seen-set code of a day's sessions (the value is 0, the "visitor" the
/// session hash).
const SEEN_SESSIONS: i64 = 100;
/// Seen-set code of an hour's users (the value is the hour of the day).
const SEEN_HOUR_USERS: i64 = 101;
/// Seen-set code of an hour's sessions.
const SEEN_HOUR_SESSIONS: i64 = 102;

/// The breakdown dimension of payload values.
pub const PAYLOAD: &str = "payload";
/// Separates type, key and value in a payload breakdown's value.
pub const SEP: char = '\u{1f}';
/// The value that stands for the values over [`MAX_PAYLOAD_VALUES`].
pub const OTHER: &str = "\u{1}";
/// Values kept per signal type, payload key and day.
pub const MAX_PAYLOAD_VALUES: i64 = 50;
/// Days of `store.app_users` kept (the month's window and today).
pub const USER_DAYS: i64 = 31;

/// The columns [`row`] reads.
pub const COLUMNS: &str = "project_id, ts, visitor, session, name, bundle_id, app_version, \
    build, os, os_version, model, locale, flags, props";

/// A raw app signal read back.
pub fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppSignal> {
    Ok(AppSignal {
        project_id: r.get(0)?,
        ts: r.get(1)?,
        user: r.get(2)?,
        session: r.get(3)?,
        kind: r.get(4)?,
        bundle_id: r.get(5)?,
        version: r.get(6)?,
        build: r.get(7)?,
        platform: r.get(8)?,
        os_version: r.get(9)?,
        model: r.get(10)?,
        locale: r.get(11)?,
        flags: r.get(12)?,
        payload: r.get(13)?,
    })
}

/// The breakdown value of a payload key's value.
pub fn payload_value(kind: &str, key: &str, value: &str) -> String {
    format!("{kind}{SEP}{key}{SEP}{value}")
}

/// The range of a type's (and key's) payload values: `[lower, upper)`.
pub fn payload_range(kind: &str, key: Option<&str>) -> (String, String) {
    match key {
        Some(k) => (
            format!("{kind}{SEP}{k}{SEP}"),
            format!("{kind}{SEP}{k}\u{20}"),
        ),
        None => (format!("{kind}{SEP}"), format!("{kind}\u{20}")),
    }
}

/// `(key, value)` pairs of a stored payload.
pub fn payload_pairs(payload: Option<&str>) -> Vec<(String, String)> {
    let Some(Ok(serde_json::Value::Object(map))) =
        payload.map(serde_json::from_str::<serde_json::Value>)
    else {
        return Vec::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
        .collect()
}

#[derive(Default)]
struct Agg {
    signals: i64,
    users: HashSet<i64>,
    sessions: HashSet<i64>,
}

impl Agg {
    fn add(&mut self, s: &AppSignal) {
        self.signals += 1;
        self.users.insert(s.user);
        if s.session != 0 {
            self.sessions.insert(s.session);
        }
    }
}

/// Changes to a day's weekly and monthly users.
#[derive(Default, Clone, Copy)]
struct Window {
    wau: i64,
    mau: i64,
}

/// The latest day of `user` in `from..=to` other than `except`.
fn latest_day(
    tx: &Connection,
    project_id: i64,
    user: i64,
    from: i64,
    to: i64,
    except: i64,
) -> rusqlite::Result<Option<i64>> {
    tx.prepare_cached(
        "SELECT MAX(day) FROM store.app_users
         WHERE project_id = ?1 AND user = ?2 AND day >= ?3 AND day <= ?4 AND day <> ?5",
    )?
    .query_row(params![project_id, user, from, to, except], |r| r.get(0))
}

/// What a user present on day `d` (and in no other day of `day`'s windows
/// than those before `latest`) adds to `day`'s weekly and monthly users.
fn window_add(latest: Option<i64>, day: i64) -> Window {
    match latest {
        None => Window { wau: 1, mau: 1 },
        Some(prev) if prev < day - 6 => Window { wau: 1, mau: 0 },
        Some(_) => Window::default(),
    }
}

fn distinct_users(tx: &Connection, project_id: i64, from: i64, to: i64) -> rusqlite::Result<i64> {
    tx.prepare_cached(
        "SELECT COUNT(DISTINCT user) FROM store.app_users
         WHERE project_id = ?1 AND day >= ?2 AND day <= ?3",
    )?
    .query_row(params![project_id, from, to], |r| r.get(0))
}

fn day_exists(tx: &Connection, project_id: i64, day: i64) -> rusqlite::Result<bool> {
    Ok(tx
        .prepare_cached(
            "SELECT 1 FROM analytics_days WHERE project_id = ?1 AND source = 'app' AND day = ?2",
        )?
        .query_row(params![project_id, day_string(day)], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Makes the day's row when it isn't there, with the weekly and monthly
/// users of the days before.
fn ensure_day(tx: &Connection, project_id: i64, day: i64) -> rusqlite::Result<()> {
    if day_exists(tx, project_id, day)? {
        return Ok(());
    }
    let wau = distinct_users(tx, project_id, day - 6, day - 1)?;
    let mau = distinct_users(tx, project_id, day - 29, day - 1)?;
    tx.prepare_cached(
        "INSERT INTO analytics_days (project_id, source, day, wau, mau) VALUES (?1, 'app', ?2, ?3, ?4)",
    )?
    .execute(params![project_id, day_string(day), wau, mau])?;
    Ok(())
}

fn add_to_day(
    tx: &Connection,
    project_id: i64,
    day: i64,
    (signals, users, sessions): (i64, i64, i64),
    w: Window,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "UPDATE analytics_days SET events = events + ?3, visitors = visitors + ?4,
             sessions = sessions + ?5, wau = wau + ?6, mau = mau + ?7
         WHERE project_id = ?1 AND source = 'app' AND day = ?2",
    )?
    .execute(params![
        project_id,
        day_string(day),
        signals,
        users,
        sessions,
        w.wau,
        w.mau
    ])?;
    Ok(())
}

fn add_breakdown(
    tx: &Connection,
    project_id: i64,
    dim: &str,
    day: i64,
    value: &str,
    count: i64,
    users: i64,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO analytics_breakdown (project_id, source, dim, day, value, count, visitors)
         VALUES (?1, 'app', ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (project_id, source, dim, day, value) DO UPDATE SET
             count = count + excluded.count, visitors = visitors + excluded.visitors",
    )?
    .execute(params![
        project_id,
        dim,
        day_string(day),
        value,
        count,
        users
    ])?;
    Ok(())
}

/// Distinct values a type's payload key already has on a day.
fn payload_values(
    tx: &Connection,
    project_id: i64,
    day: i64,
    kind: &str,
    key: &str,
) -> rusqlite::Result<i64> {
    let (lower, upper) = payload_range(kind, Some(key));
    tx.prepare_cached(
        "SELECT COUNT(*) FROM analytics_breakdown WHERE project_id = ?1 AND source = 'app'
         AND dim = 'payload' AND day = ?2 AND value >= ?3 AND value < ?4 AND value <> ?5",
    )?
    .query_row(
        params![
            project_id,
            day_string(day),
            lower,
            upper,
            payload_value(kind, key, OTHER)
        ],
        |r| r.get(0),
    )
}

fn breakdown_exists(
    tx: &Connection,
    project_id: i64,
    dim: &str,
    day: i64,
    value: &str,
) -> rusqlite::Result<bool> {
    Ok(tx
        .prepare_cached(
            "SELECT 1 FROM analytics_breakdown
             WHERE project_id = ?1 AND source = 'app' AND dim = ?2 AND day = ?3 AND value = ?4",
        )?
        .query_row(params![project_id, dim, day_string(day), value], |_| Ok(()))
        .optional()?
        .is_some())
}

type PayloadKey = (i64, i64, String, String);
/// Per project, day, dimension and value: signals and their users.
type DimAgg = HashMap<(i64, i64, AppDim, String), (i64, HashSet<i64>)>;

/// Adds app signals to the rollups. Signals of days before yesterday are
/// noted in `late` (when the project's raw retention still has their whole
/// day) instead; `touched` gets the projects whose rollups changed.
pub fn add(
    tx: &Transaction<'_>,
    rows: &[AppSignal],
    today: i64,
    complete_from: &HashMap<i64, i64>,
    late: &mut BTreeSet<(i64, String, i64)>,
    touched: &mut BTreeSet<i64>,
) -> rusqlite::Result<()> {
    // (day, project): oldest day first, so yesterday's users are in place
    // before today's row starts from them.
    let mut days: BTreeMap<(i64, i64), Agg> = BTreeMap::new();
    let mut hours: HashMap<(i64, i64), Agg> = HashMap::new();
    let mut dims: DimAgg = HashMap::new();
    let mut payloads: BTreeMap<(PayloadKey, String), i64> = BTreeMap::new();
    for s in rows {
        let Some(&complete) = complete_from.get(&s.project_id) else {
            continue;
        };
        if s.flags != 0 {
            continue;
        }
        let day = day_number(s.ts);
        if day < today - 1 {
            if day >= complete {
                late.insert((s.project_id, APP.to_string(), day));
            }
            continue;
        }
        touched.insert(s.project_id);
        days.entry((day, s.project_id)).or_default().add(s);
        hours
            .entry((s.project_id, s.ts.div_euclid(3600)))
            .or_default()
            .add(s);
        for dim in AppDim::ALL {
            if let Some(value) = dim.value_of(s) {
                let e = dims.entry((s.project_id, day, dim, value)).or_default();
                e.0 += 1;
                e.1.insert(s.user);
            }
        }
        for (key, value) in payload_pairs(s.payload.as_deref()) {
            *payloads
                .entry(((s.project_id, day, s.kind.clone(), key), value))
                .or_default() += 1;
        }
    }

    let mut today_changes: HashMap<i64, Window> = HashMap::new();
    for (&(day, project_id), a) in &days {
        ensure_day(tx, project_id, day)?;
        let today_known = day == today - 1 && day_exists(tx, project_id, today)?;
        let mut w = Window::default();
        let mut new_users = 0;
        for &user in &a.users {
            let inserted = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO store.app_users (project_id, day, user) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![project_id, day, user])?
                > 0;
            if !inserted {
                continue;
            }
            new_users += 1;
            let latest = latest_day(tx, project_id, user, day - 29, day - 1, day)?;
            let add = window_add(latest, day);
            w.wau += add.wau;
            w.mau += add.mau;
            if today_known {
                let latest = latest_day(tx, project_id, user, today - 29, today, day)?;
                let add = window_add(latest, today);
                let t = today_changes.entry(project_id).or_default();
                t.wau += add.wau;
                t.mau += add.mau;
            }
        }
        let sessions = new_visitors(tx, project_id, day, SEEN_SESSIONS, 0, &a.sessions)?;
        add_to_day(tx, project_id, day, (a.signals, new_users, sessions), w)?;
    }
    for (project_id, w) in today_changes {
        add_to_day(tx, project_id, today, (0, 0, 0), w)?;
    }

    for (&(project_id, hour), a) in &hours {
        let day = hour.div_euclid(24);
        let of_day = hour.rem_euclid(24);
        let users = new_visitors(tx, project_id, day, SEEN_HOUR_USERS, of_day, &a.users)?;
        let sessions = new_visitors(tx, project_id, day, SEEN_HOUR_SESSIONS, of_day, &a.sessions)?;
        tx.prepare_cached(
            "INSERT INTO analytics_hours (project_id, source, hour, events, visitors, sessions)
             VALUES (?1, 'app', ?2, ?3, ?4, ?5)
             ON CONFLICT (project_id, source, hour) DO UPDATE SET
                 events = events + excluded.events, visitors = visitors + excluded.visitors,
                 sessions = sessions + excluded.sessions",
        )?
        .execute(params![
            project_id,
            hour_string(hour * 3600),
            a.signals,
            users,
            sessions
        ])?;
    }

    for ((project_id, day, dim, value), (count, users)) in &dims {
        let n = new_visitors(
            tx,
            *project_id,
            *day,
            dim.seen_code(),
            value_hash(value),
            users,
        )?;
        add_breakdown(tx, *project_id, dim.code(), *day, value, *count, n)?;
    }

    // Payload values, bounded per type, key and day.
    let mut known: HashMap<PayloadKey, i64> = HashMap::new();
    for ((key, value), count) in payloads {
        let (project_id, day, kind, k) = &key;
        let stored = payload_value(kind, k, &value);
        let target = if breakdown_exists(tx, *project_id, PAYLOAD, *day, &stored)? {
            stored
        } else {
            let n = match known.get(&key) {
                Some(n) => *n,
                None => payload_values(tx, *project_id, *day, kind, k)?,
            };
            if n < MAX_PAYLOAD_VALUES {
                known.insert(key.clone(), n + 1);
                stored
            } else {
                known.insert(key.clone(), n);
                payload_value(kind, k, OTHER)
            }
        };
        add_breakdown(tx, *project_id, PAYLOAD, *day, &target, count, 0)?;
    }

    for project_id in touched.iter() {
        tx.prepare_cached("DELETE FROM store.app_users WHERE project_id = ?1 AND day < ?2")?
            .execute(params![project_id, today - USER_DAYS + 1])?;
    }
    // Today gets its row as soon as anyone used the app in the month before,
    // so a quiet day still has its weekly and monthly users.
    for &project_id in complete_from.keys() {
        let recent = tx
            .prepare_cached(
                "SELECT 1 FROM store.app_users WHERE project_id = ?1 AND day >= ?2 LIMIT 1",
            )?
            .query_row(params![project_id, today - 29], |_| Ok(()))
            .optional()?
            .is_some();
        if recent {
            ensure_day(tx, project_id, today)?;
        }
    }
    Ok(())
}

/// The raw production signals of a project from day `from` to day `to`
/// (both included): the `FROM … WHERE` and its first parameters.
const RAW: &str = "FROM store.analytics_events WHERE project_id = ?1 AND source = 'app' \
    AND flags = 0 AND ts >= ?2 AND ts < ?3";

/// Distinct users of the raw signals in `day - len + 1 ..= day`.
fn raw_window(tx: &Connection, project_id: i64, day: i64, len: i64) -> rusqlite::Result<i64> {
    tx.query_row(
        &format!("SELECT COUNT(DISTINCT visitor) {RAW}"),
        params![project_id, (day - len + 1) * 86_400, (day + 1) * 86_400],
        |r| r.get(0),
    )
}

/// Replaces one day's app rollups of a project with what its raw signals
/// say, and the weekly and monthly users of the days whose windows hold it.
pub fn recompute_day(
    tx: &Connection,
    project_id: i64,
    day: i64,
    today: i64,
) -> rusqlite::Result<()> {
    let (start, end) = (day * 86_400, (day + 1) * 86_400);
    let date = day_string(day);
    tx.execute(
        "DELETE FROM analytics_days WHERE project_id = ?1 AND source = 'app' AND day = ?2",
        params![project_id, date],
    )?;
    tx.execute(
        "DELETE FROM analytics_hours WHERE project_id = ?1 AND source = 'app' AND hour >= ?2 AND hour < ?3",
        params![project_id, date, day_string(day + 1)],
    )?;
    tx.execute(
        "DELETE FROM analytics_breakdown WHERE project_id = ?1 AND source = 'app' AND day = ?2",
        params![project_id, date],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO analytics_days (project_id, source, day, events, visitors, sessions)
             SELECT ?1, 'app', ?4, COUNT(*), COUNT(DISTINCT visitor), COUNT(DISTINCT NULLIF(session, 0))
             {RAW} HAVING COUNT(*) > 0"
        ),
        params![project_id, start, end, date],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO analytics_hours (project_id, source, hour, events, visitors, sessions)
             SELECT ?1, 'app', strftime('%Y-%m-%d %H:00:00', (ts / 3600) * 3600, 'unixepoch'),
                    COUNT(*), COUNT(DISTINCT visitor), COUNT(DISTINCT NULLIF(session, 0))
             {RAW} GROUP BY ts / 3600"
        ),
        params![project_id, start, end],
    )?;
    for dim in AppDim::ALL {
        tx.execute(
            &format!(
                "INSERT INTO analytics_breakdown (project_id, source, dim, day, value, count, visitors)
                 SELECT ?1, 'app', ?4, ?5, v, COUNT(*), COUNT(DISTINCT visitor) FROM (
                     SELECT {expr} AS v, visitor {RAW}
                 ) WHERE ?4 <> 'release' OR v <> '' GROUP BY v",
                expr = dim.sql()
            ),
            params![project_id, start, end, dim.code(), date],
        )?;
    }
    // Payload values: the most frequent per type and key, the rest as one.
    let counts: Vec<(String, String, String, i64)> = tx
        .prepare(&format!(
            "SELECT name, j.key, j.value, COUNT(*) FROM (SELECT name, props {RAW}) e, json_each(e.props) j
             WHERE j.type = 'text' GROUP BY 1, 2, 3 ORDER BY 4 DESC, 3"
        ))?
        .query_map(params![project_id, start, end], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut per_key: HashMap<(String, String), i64> = HashMap::new();
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for (kind, key, value, count) in counts {
        let n = per_key.entry((kind.clone(), key.clone())).or_default();
        let stored = if *n < MAX_PAYLOAD_VALUES {
            *n += 1;
            payload_value(&kind, &key, &value)
        } else {
            payload_value(&kind, &key, OTHER)
        };
        *out.entry(stored).or_default() += count;
    }
    for (value, count) in out {
        add_breakdown(tx, project_id, PAYLOAD, day, &value, count, 0)?;
    }
    // The day's users, for the windows of the days after it.
    if day > today - USER_DAYS {
        tx.execute(
            "DELETE FROM store.app_users WHERE project_id = ?1 AND day = ?2",
            params![project_id, day],
        )?;
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO store.app_users (project_id, day, user)
                 SELECT DISTINCT ?1, ?4, visitor {RAW}"
            ),
            params![project_id, start, end, day],
        )?;
    }
    for d in day..=(day + 29).min(today) {
        if !day_exists(tx, project_id, d)? {
            continue;
        }
        let (wau, mau) = (
            raw_window(tx, project_id, d, 7)?,
            raw_window(tx, project_id, d, 30)?,
        );
        tx.execute(
            "UPDATE analytics_days SET wau = ?3, mau = ?4
             WHERE project_id = ?1 AND source = 'app' AND day = ?2",
            params![project_id, day_string(d), wau, mau],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_ranges_hold_only_their_key() {
        let (lo, hi) = payload_range("T", Some("a"));
        for inside in [payload_value("T", "a", "x"), payload_value("T", "a", OTHER)] {
            assert!(inside >= lo && inside < hi, "{inside:?}");
        }
        for outside in [
            payload_value("T", "ab", "x"),
            payload_value("T", "", "x"),
            payload_value("T.x", "a", "x"),
            payload_value("U", "a", "x"),
        ] {
            assert!(!(outside >= lo && outside < hi), "{outside:?}");
        }
        let (lo, hi) = payload_range("T", None);
        assert!(payload_value("T", "zz", "v") >= lo && payload_value("T", "zz", "v") < hi);
        assert!(payload_value("T.x", "a", "v") >= hi);
    }

    #[test]
    fn windows() {
        let day = 100;
        assert_eq!(window_add(None, day).wau, 1);
        assert_eq!(window_add(Some(93), day).wau, 1);
        assert_eq!(window_add(Some(93), day).mau, 0);
        assert_eq!(window_add(Some(94), day).wau, 0);
        assert_eq!(
            payload_pairs(Some(r#"{"plan":"pro","n":"1"}"#)),
            [
                ("n".to_string(), "1".to_string()),
                ("plan".to_string(), "pro".to_string())
            ]
        );
        assert!(payload_pairs(None).is_empty());
    }
}
