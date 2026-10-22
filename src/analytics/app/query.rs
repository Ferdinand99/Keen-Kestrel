//! What the App tab, the signal explorer, the release pages, the REST API
//! and MCP read of app analytics (docs/design.md §5.3).
//!
//! Without filters everything comes from the rollups in `kestrel.sqlite`
//! (`source = 'app'`, at most a minute behind, kept 400 days): totals and
//! the daily series from `analytics_days`, a single day's series from
//! `analytics_hours`, breakdowns, version adoption and payload values from
//! `analytics_breakdown`. Daily, weekly and monthly active users are the
//! day's `visitors`, `wau` and `mau`.
//!
//! Filters (a version, a platform, a signal type) and test signals (debug,
//! TestFlight, simulator, which the rollups leave out) need the raw signals
//! in `store.sqlite`, kept 30 days: the rollups count each dimension on its
//! own. Weekly and monthly users are then counted from the raw signals too.
//! Users in breakdowns are counted per day and added up, as for the web.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::types::Value as V;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use super::rollup::{OTHER, PAYLOAD, SEP, payload_range};
use super::{APP, AppDim};
use crate::analytics::query::{LIVE_SECONDS, Period, Row};
use crate::analytics::{day_number, day_string, hour_string};

/// Filters of the App tab, the explorer, the API and MCP.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppFilters {
    pub version: Option<String>,
    pub platform: Option<String>,
    /// A signal type.
    pub kind: Option<String>,
    /// Count test signals (debug builds, TestFlight, the simulator) too.
    pub include_test: bool,
}

impl AppFilters {
    /// The filters in query pairs: `version`, `platform`, `type`, `test`
    /// (`1`, `true` or `on`). Other keys are ignored.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> AppFilters {
        let mut f = AppFilters::default();
        let val = |v: &str| {
            let v: String = v.trim().chars().take(200).collect();
            (!v.is_empty()).then_some(v)
        };
        for (k, v) in pairs {
            match k {
                "version" if f.version.is_none() => f.version = val(v),
                "platform" if f.platform.is_none() => f.platform = val(v),
                "type" if f.kind.is_none() => f.kind = val(v),
                "test" => f.include_test = matches!(v.trim(), "1" | "true" | "on"),
                _ => {}
            }
        }
        f
    }

    /// Whether the rollups can answer (no filter, no test signals).
    pub fn is_empty(&self) -> bool {
        self.version.is_none()
            && self.platform.is_none()
            && self.kind.is_none()
            && !self.include_test
    }

    /// As query pairs.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(v) = &self.version {
            out.push(("version", v.clone()));
        }
        if let Some(v) = &self.platform {
            out.push(("platform", v.clone()));
        }
        if let Some(v) = &self.kind {
            out.push(("type", v.clone()));
        }
        if self.include_test {
            out.push(("test", "1".into()));
        }
        out
    }

    /// SQL conditions on the raw signals and their parameters.
    fn sql(&self) -> (String, Vec<V>) {
        let mut out = String::new();
        let mut args = Vec::new();
        if !self.include_test {
            out.push_str(" AND flags = 0");
        }
        for (col, v) in [
            ("app_version", &self.version),
            ("os", &self.platform),
            ("name", &self.kind),
        ] {
            if let Some(v) = v {
                out.push_str(&format!(" AND {col} = ?"));
                args.push(V::Text(v.clone()));
            }
        }
        (out, args)
    }
}

/// Totals over a period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppTotals {
    pub signals: i64,
    pub sessions: i64,
    /// Daily active users added up over the period.
    pub user_days: i64,
    /// Active users on the period's last day, and in the 7 and 30 days up
    /// to it.
    pub dau: i64,
    pub wau: i64,
    pub mau: i64,
}

/// One point of a time series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPoint {
    /// `YYYY-MM-DD`, or `YYYY-MM-DD HH:00:00` for hours.
    pub start: String,
    pub users: i64,
    /// Days only; `None` for a day the rollups have no row for.
    pub wau: Option<i64>,
    pub mau: Option<i64>,
    pub sessions: i64,
    pub signals: i64,
}

/// Whether the project has app rollups.
pub fn has_data(main: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    Ok(main
        .prepare_cached(
            "SELECT 1 FROM analytics_days WHERE project_id = ?1 AND source = 'app' LIMIT 1",
        )?
        .query_row([project_id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Whether the project has raw app signals (test signals included).
pub fn has_raw(store: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    Ok(store
        .prepare_cached(
            "SELECT 1 FROM analytics_events WHERE project_id = ?1 AND source = 'app' LIMIT 1",
        )?
        .query_row([project_id], |_| Ok(()))
        .optional()?
        .is_some())
}

// ---------------------------------------------------------------------------
// Rollups
// ---------------------------------------------------------------------------

/// Totals over the period from `analytics_days`.
pub fn totals(main: &Connection, project_id: i64, p: Period) -> rusqlite::Result<AppTotals> {
    let (signals, sessions, user_days): (i64, i64, i64) = main
        .prepare_cached(
            "SELECT coalesce(SUM(events), 0), coalesce(SUM(sessions), 0), coalesce(SUM(visitors), 0)
             FROM analytics_days WHERE project_id = ?1 AND source = 'app' AND day >= ?2 AND day <= ?3",
        )?
        .query_row(params![project_id, p.from_date(), p.to_date()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    let (dau, wau, mau) = main
        .prepare_cached(
            "SELECT visitors, wau, mau FROM analytics_days
             WHERE project_id = ?1 AND source = 'app' AND day = ?2",
        )?
        .query_row(params![project_id, p.to_date()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?
        .unwrap_or((0, 0, 0));
    Ok(AppTotals {
        signals,
        sessions,
        user_days,
        dau,
        wau,
        mau,
    })
}

/// Every hour (one-day periods) or day of the period, oldest first.
pub fn series(main: &Connection, project_id: i64, p: Period) -> rusqlite::Result<Vec<AppPoint>> {
    let rows: Vec<AppPoint> = if p.hourly() {
        main.prepare_cached(
            "SELECT hour, visitors, NULL, NULL, sessions, events FROM analytics_hours
             WHERE project_id = ?1 AND source = 'app' AND hour >= ?2 AND hour < ?3 ORDER BY hour",
        )?
        .query_map(
            params![project_id, p.from_date(), day_string(p.to + 1)],
            point,
        )?
        .collect::<rusqlite::Result<_>>()?
    } else {
        main.prepare_cached(
            "SELECT day, visitors, wau, mau, sessions, events FROM analytics_days
             WHERE project_id = ?1 AND source = 'app' AND day >= ?2 AND day <= ?3 ORDER BY day",
        )?
        .query_map(params![project_id, p.from_date(), p.to_date()], point)?
        .collect::<rusqlite::Result<_>>()?
    };
    Ok(fill(p, rows))
}

fn point(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppPoint> {
    Ok(AppPoint {
        start: r.get(0)?,
        users: r.get(1)?,
        wau: r.get(2)?,
        mau: r.get(3)?,
        sessions: r.get(4)?,
        signals: r.get(5)?,
    })
}

/// The period's hours or days in order, `rows` where they exist.
fn fill(p: Period, rows: Vec<AppPoint>) -> Vec<AppPoint> {
    let keys: Vec<String> = if p.hourly() {
        (0..24)
            .map(|h| hour_string(p.from * 86_400 + h * 3600))
            .collect()
    } else {
        (p.from..=p.to).map(day_string).collect()
    };
    let mut by: HashMap<String, AppPoint> =
        rows.into_iter().map(|r| (r.start.clone(), r)).collect();
    keys.into_iter()
        .map(|start| {
            by.remove(&start).unwrap_or(AppPoint {
                start,
                users: 0,
                wau: None,
                mau: None,
                sessions: 0,
                signals: 0,
            })
        })
        .collect()
}

/// The top `limit` values of `dim` over the period, most users first.
pub fn breakdown(
    main: &Connection,
    project_id: i64,
    p: Period,
    dim: AppDim,
    limit: i64,
) -> rusqlite::Result<Vec<Row>> {
    main.prepare_cached(
        "SELECT value, SUM(count), SUM(visitors) FROM analytics_breakdown
         WHERE project_id = ?1 AND source = 'app' AND dim = ?2 AND day >= ?3 AND day <= ?4
         GROUP BY value ORDER BY 3 DESC, 2 DESC, value LIMIT ?5",
    )?
    .query_map(
        params![project_id, dim.code(), p.from_date(), p.to_date(), limit],
        |r| {
            Ok(Row {
                value: r.get(0)?,
                count: r.get(1)?,
                visitors: r.get(2)?,
            })
        },
    )?
    .collect()
}

/// Version adoption: each of the top versions' share of the daily active
/// users per day.
#[derive(Debug, Clone, PartialEq)]
pub struct Adoption {
    /// The days (`YYYY-MM-DD`).
    pub days: Vec<String>,
    /// Version, its share (percent) per day, and its users over the period
    /// (counted per day).
    pub versions: Vec<(String, Vec<f64>, i64)>,
}

/// The `top` versions with the most users over the period and their daily
/// share of the users.
pub fn adoption(
    main: &Connection,
    project_id: i64,
    p: Period,
    top: i64,
) -> rusqlite::Result<Adoption> {
    let versions: Vec<(String, i64)> = breakdown(main, project_id, p, AppDim::Version, top)?
        .into_iter()
        .map(|r| (r.value, r.visitors))
        .collect();
    let days: Vec<String> = (p.from..=p.to).map(day_string).collect();
    let dau: HashMap<String, i64> = main
        .prepare_cached(
            "SELECT day, visitors FROM analytics_days
             WHERE project_id = ?1 AND source = 'app' AND day >= ?2 AND day <= ?3",
        )?
        .query_map(params![project_id, p.from_date(), p.to_date()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut per: HashMap<(String, String), i64> = HashMap::new();
    let mut stmt = main.prepare_cached(
        "SELECT day, visitors FROM analytics_breakdown
         WHERE project_id = ?1 AND source = 'app' AND dim = 'version' AND value = ?2
         AND day >= ?3 AND day <= ?4",
    )?;
    for (v, _) in &versions {
        for r in stmt.query_map(params![project_id, v, p.from_date(), p.to_date()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (day, n) = r?;
            per.insert((v.clone(), day), n);
        }
    }
    let versions = versions
        .into_iter()
        .map(|(v, users)| {
            let shares = days
                .iter()
                .map(|d| {
                    let all = dau.get(d).copied().unwrap_or(0);
                    let mine = per.get(&(v.clone(), d.clone())).copied().unwrap_or(0);
                    if all > 0 {
                        100.0 * mine as f64 / all as f64
                    } else {
                        0.0
                    }
                })
                .collect();
            (v, shares, users)
        })
        .collect();
    Ok(Adoption { days, versions })
}

/// A signal type's payload: per key (most signals first), its top values
/// with their signal counts. [`OTHER`] stands for the values the rollup
/// didn't keep apart.
pub type Payload = Vec<(String, Vec<Row>)>;

/// Groups `(key, value, count)` rows into [`Payload`], `limit` values per
/// key, the rest added to [`OTHER`].
fn group_payload(rows: Vec<(String, String, i64)>, limit: usize) -> Payload {
    let mut keys: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();
    for (k, v, n) in rows {
        keys.entry(k).or_default().push((v, n));
    }
    let mut out: Vec<(String, Vec<Row>, i64)> = keys
        .into_iter()
        .map(|(k, mut values)| {
            let total: i64 = values.iter().map(|(_, n)| n).sum();
            values.sort_by(|a, b| {
                (a.0 == OTHER)
                    .cmp(&(b.0 == OTHER))
                    .then(b.1.cmp(&a.1))
                    .then(a.0.cmp(&b.0))
            });
            let mut rows: Vec<Row> = Vec::new();
            let mut other = 0;
            for (v, n) in values {
                if v == OTHER || rows.len() >= limit {
                    other += n;
                } else {
                    rows.push(Row {
                        value: v,
                        count: n,
                        visitors: 0,
                    });
                }
            }
            if other > 0 {
                rows.push(Row {
                    value: OTHER.into(),
                    count: other,
                    visitors: 0,
                });
            }
            (k, rows, total)
        })
        .collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    out.into_iter().map(|(k, rows, _)| (k, rows)).collect()
}

/// A signal type's payload keys and values over the period (rollups).
pub fn payload(
    main: &Connection,
    project_id: i64,
    p: Period,
    kind: &str,
    limit: usize,
) -> rusqlite::Result<Payload> {
    let (lower, upper) = payload_range(kind, None);
    let rows: Vec<(String, String, i64)> = main
        .prepare_cached(
            "SELECT value, SUM(count) FROM analytics_breakdown
             WHERE project_id = ?1 AND source = 'app' AND dim = ?2 AND day >= ?3 AND day <= ?4
             AND value >= ?5 AND value < ?6 GROUP BY value",
        )?
        .query_map(
            params![
                project_id,
                PAYLOAD,
                p.from_date(),
                p.to_date(),
                lower,
                upper
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )?
        .filter_map(|r| {
            r.map(|(v, n)| {
                let mut parts = v.splitn(3, SEP).skip(1);
                Some((parts.next()?.to_string(), parts.next()?.to_string(), n))
            })
            .transpose()
        })
        .collect::<rusqlite::Result<_>>()?;
    Ok(group_payload(rows, limit))
}

/// Signals and users of one type per day (rollups; days only).
pub fn type_series(
    main: &Connection,
    project_id: i64,
    p: Period,
    kind: &str,
) -> rusqlite::Result<Vec<AppPoint>> {
    let rows: Vec<AppPoint> = main
        .prepare_cached(
            "SELECT day, visitors, NULL, NULL, 0, count FROM analytics_breakdown
             WHERE project_id = ?1 AND source = 'app' AND dim = 'type' AND value = ?2
             AND day >= ?3 AND day <= ?4 ORDER BY day",
        )?
        .query_map(params![project_id, kind, p.from_date(), p.to_date()], point)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(fill(p, rows))
}

/// The share (percent) of the app's daily active users over the 7 days up
/// to `today` that ran `release` (a Sentry release name,
/// `bundle@version+build`; without `+build`, every build of the version).
/// `None` when no app user ran it then.
pub fn release_adoption(
    main: &Connection,
    project_id: i64,
    release: &str,
    today: i64,
) -> rusqlite::Result<Option<f64>> {
    let (from, to) = (day_string(today - 6), day_string(today));
    let mut mine: i64 = main
        .prepare_cached(
            "SELECT coalesce(SUM(visitors), 0) FROM analytics_breakdown
             WHERE project_id = ?1 AND source = 'app' AND dim = 'release' AND value = ?2
             AND day >= ?3 AND day <= ?4",
        )?
        .query_row(params![project_id, release, from, to], |r| r.get(0))?;
    if mine == 0 && !release.contains('+') {
        let (lower, upper) = (format!("{release}+"), format!("{release},"));
        mine = main
            .prepare_cached(
                "SELECT coalesce(SUM(visitors), 0) FROM analytics_breakdown
                 WHERE project_id = ?1 AND source = 'app' AND dim = 'release'
                 AND value >= ?2 AND value < ?3 AND day >= ?4 AND day <= ?5",
            )?
            .query_row(params![project_id, lower, upper, from, to], |r| r.get(0))?;
    }
    if mine == 0 {
        return Ok(None);
    }
    let all: i64 = main
        .prepare_cached(
            "SELECT coalesce(SUM(visitors), 0) FROM analytics_days
             WHERE project_id = ?1 AND source = 'app' AND day >= ?2 AND day <= ?3",
        )?
        .query_row(params![project_id, from, to], |r| r.get(0))?;
    Ok((all > 0).then(|| (100.0 * mine as f64 / all as f64).min(100.0)))
}

// ---------------------------------------------------------------------------
// Raw signals (filters, test signals, current users)
// ---------------------------------------------------------------------------

/// The raw signals' `FROM … WHERE` for a project, the Unix-second range
/// `[start, end)` and the filters, and its parameters.
fn raw_where(project_id: i64, (start, end): (i64, i64), f: &AppFilters) -> (String, Vec<V>) {
    let (cond, args) = f.sql();
    let mut params = vec![
        V::Integer(project_id),
        V::Text(APP.into()),
        V::Integer(start),
        V::Integer(end),
    ];
    params.extend(args);
    (
        format!(
            "FROM analytics_events WHERE project_id = ? AND source = ? AND ts >= ? AND ts < ?{cond}"
        ),
        params,
    )
}

/// What the dashboard shows, from the raw signals: one pass over the
/// filtered signals of the period and the 29 days before (for weekly and
/// monthly users).
#[derive(Debug, Clone, PartialEq)]
pub struct RawSummary {
    pub totals: AppTotals,
    pub series: Vec<AppPoint>,
    /// With `lists`: the top values of every dimension.
    pub lists: Vec<(AppDim, Vec<Row>)>,
    /// With `lists`: version adoption.
    pub adoption: Option<Adoption>,
}

#[derive(Default)]
struct Bucket {
    signals: i64,
    users: HashSet<i64>,
    sessions: HashSet<i64>,
}

#[derive(Default)]
struct ValueAgg {
    signals: i64,
    /// `(day, user)`: users counted per day, as in the rollups.
    user_days: HashSet<(i64, i64)>,
}

/// Reads the filtered raw signals once and counts everything the dashboard
/// shows (`lists`: the breakdowns too, `limit` values each, and the
/// adoption of the `adoption_top` versions).
pub fn raw_summary(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &AppFilters,
    lists: bool,
    limit: usize,
    adoption_top: usize,
) -> rusqlite::Result<RawSummary> {
    let (start, end) = p.bounds();
    let (from, args) = raw_where(project_id, ((p.from - 29) * 86_400, end), f);
    let mut stmt = store.prepare(&format!(
        "SELECT ts, visitor, session, name, app_version, build, bundle_id, os, os_version, model, locale {from}"
    ))?;
    let mut rows = stmt.query(params_from_iter(args.iter()))?;
    // Users per day (the windows), and the period's buckets (hours or days).
    let mut day_users: BTreeMap<i64, HashSet<i64>> = BTreeMap::new();
    let mut buckets: BTreeMap<i64, Bucket> = BTreeMap::new();
    let mut day_sessions: HashSet<(i64, i64)> = HashSet::new();
    let mut signals = 0;
    let mut dims: Vec<HashMap<String, ValueAgg>> =
        AppDim::ALL.iter().map(|_| HashMap::new()).collect();
    let mut version_days: HashMap<(i64, String), HashSet<i64>> = HashMap::new();
    while let Some(r) = rows.next()? {
        let ts: i64 = r.get(0)?;
        let user: i64 = r.get(1)?;
        let day = day_number(ts);
        day_users.entry(day).or_default().insert(user);
        if ts < start {
            continue;
        }
        let session: i64 = r.get(2)?;
        signals += 1;
        let b = buckets
            .entry(if p.hourly() { ts.div_euclid(3600) } else { day })
            .or_default();
        b.signals += 1;
        b.users.insert(user);
        if session != 0 {
            b.sessions.insert(session);
            day_sessions.insert((day, session));
        }
        if !lists {
            continue;
        }
        let s = super::AppSignal {
            kind: r.get(3)?,
            version: r.get(4)?,
            build: r.get(5)?,
            bundle_id: r.get(6)?,
            platform: r.get(7)?,
            os_version: r.get(8)?,
            model: r.get(9)?,
            locale: r.get(10)?,
            ..super::AppSignal::default()
        };
        for (i, dim) in AppDim::ALL.into_iter().enumerate() {
            if let Some(value) = dim.value_of(&s) {
                let a = dims[i].entry(value).or_default();
                a.signals += 1;
                a.user_days.insert((day, user));
            }
        }
        version_days
            .entry((day, s.version))
            .or_default()
            .insert(user);
    }
    let window = |day: i64, len: i64| -> i64 {
        let mut users: HashSet<i64> = HashSet::new();
        for (_, u) in day_users.range(day - len + 1..=day) {
            users.extend(u.iter().copied());
        }
        users.len() as i64
    };
    let in_period = |d: &i64| (p.from..=p.to).contains(d);
    let totals = AppTotals {
        signals,
        sessions: day_sessions.len() as i64,
        user_days: day_users
            .iter()
            .filter(|(d, _)| in_period(d))
            .map(|(_, u)| u.len() as i64)
            .sum(),
        dau: window(p.to, 1),
        wau: window(p.to, 7),
        mau: window(p.to, 30),
    };
    let points: Vec<AppPoint> = buckets
        .into_iter()
        .map(|(k, b)| {
            let daily = !p.hourly();
            AppPoint {
                start: if daily {
                    day_string(k)
                } else {
                    hour_string(k * 3600)
                },
                users: b.users.len() as i64,
                wau: None,
                mau: None,
                sessions: b.sessions.len() as i64,
                signals: b.signals,
            }
        })
        .collect();
    let mut series = fill(p, points);
    if !p.hourly() {
        for (pt, day) in series.iter_mut().zip(p.from..=p.to) {
            pt.wau = Some(window(day, 7));
            pt.mau = Some(window(day, 30));
        }
    }
    let mut out_lists = Vec::new();
    let mut adoption = None;
    if lists {
        let top = |map: &HashMap<String, ValueAgg>, n: usize| -> Vec<Row> {
            let mut rows: Vec<Row> = map
                .iter()
                .map(|(v, a)| Row {
                    value: v.clone(),
                    count: a.signals,
                    visitors: a.user_days.len() as i64,
                })
                .collect();
            rows.sort_by(|a, b| {
                b.visitors
                    .cmp(&a.visitors)
                    .then(b.count.cmp(&a.count))
                    .then(a.value.cmp(&b.value))
            });
            rows.truncate(n);
            rows
        };
        for (i, dim) in AppDim::ALL.into_iter().enumerate() {
            out_lists.push((dim, top(&dims[i], limit)));
        }
        let version_index = AppDim::ALL
            .iter()
            .position(|d| *d == AppDim::Version)
            .unwrap_or(0);
        let versions = top(&dims[version_index], adoption_top);
        let days: Vec<i64> = (p.from..=p.to).collect();
        adoption = Some(Adoption {
            days: days.iter().map(|d| day_string(*d)).collect(),
            versions: versions
                .into_iter()
                .map(|r| {
                    let shares = days
                        .iter()
                        .map(|d| {
                            let all = day_users.get(d).map_or(0, HashSet::len);
                            let mine = version_days
                                .get(&(*d, r.value.clone()))
                                .map_or(0, HashSet::len);
                            if all > 0 {
                                100.0 * mine as f64 / all as f64
                            } else {
                                0.0
                            }
                        })
                        .collect();
                    (r.value, shares, r.visitors)
                })
                .collect(),
        });
    }
    Ok(RawSummary {
        totals,
        series,
        lists: out_lists,
        adoption,
    })
}

/// [`totals`] from the raw signals, filtered.
pub fn raw_totals(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &AppFilters,
) -> rusqlite::Result<AppTotals> {
    Ok(raw_summary(store, project_id, p, f, false, 0, 0)?.totals)
}

/// [`series`] from the raw signals, filtered. Weekly and monthly users per
/// day are counted from the raw signals of the 29 days before as well.
pub fn raw_series(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &AppFilters,
) -> rusqlite::Result<Vec<AppPoint>> {
    Ok(raw_summary(store, project_id, p, f, false, 0, 0)?.series)
}

/// [`breakdown`] from the raw signals, filtered.
pub fn raw_breakdown(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &AppFilters,
    dim: AppDim,
    limit: i64,
) -> rusqlite::Result<Vec<Row>> {
    let (from, mut args) = raw_where(project_id, p.bounds(), f);
    args.push(V::Integer(limit));
    let sql = format!(
        "SELECT v, SUM(c), COUNT(*) FROM (
             SELECT {expr} AS v, ts / 86400 AS d, visitor, COUNT(*) AS c {from}
             GROUP BY v, d, visitor
         ) {release} GROUP BY v ORDER BY 3 DESC, 2 DESC, v LIMIT ?",
        expr = dim.sql(),
        release = if dim == AppDim::Release {
            "WHERE v <> ''"
        } else {
            ""
        }
    );
    store
        .prepare(&sql)?
        .query_map(params_from_iter(args.iter()), |r| {
            Ok(Row {
                value: r.get(0)?,
                count: r.get(1)?,
                visitors: r.get(2)?,
            })
        })?
        .collect()
}

/// [`payload`] from the raw signals, filtered.
pub fn raw_payload(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &AppFilters,
    kind: &str,
    limit: usize,
) -> rusqlite::Result<Payload> {
    let (from, mut args) = raw_where(project_id, p.bounds(), f);
    args.push(V::Text(kind.to_string()));
    let rows: Vec<(String, String, i64)> = store
        .prepare(&format!(
            "SELECT j.key, j.value, COUNT(*) FROM (SELECT props {from} AND name = ?) e, json_each(e.props) j
             WHERE j.type = 'text' GROUP BY 1, 2"
        ))?
        .query_map(params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(group_payload(rows, limit))
}

/// [`adoption`] from the raw signals, filtered.
pub fn raw_adoption(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &AppFilters,
    top: i64,
) -> rusqlite::Result<Adoption> {
    let s = raw_summary(store, project_id, p, f, true, 1, top.max(1) as usize)?;
    Ok(s.adoption.unwrap_or(Adoption {
        days: Vec::new(),
        versions: Vec::new(),
    }))
}

/// The first day with raw app signals (filters can't reach before it).
pub fn raw_first_day(store: &Connection, project_id: i64) -> rusqlite::Result<Option<i64>> {
    let ts: Option<i64> = store
        .prepare_cached(
            "SELECT MIN(ts) FROM analytics_events WHERE project_id = ?1 AND source = 'app'",
        )?
        .query_row([project_id], |r| r.get(0))?;
    Ok(ts.map(day_number))
}

/// Test signals (debug, TestFlight, simulator) in the period: the ones the
/// rollups leave out.
pub fn test_signals(store: &Connection, project_id: i64, p: Period) -> rusqlite::Result<i64> {
    let (start, end) = p.bounds();
    store
        .prepare_cached(
            "SELECT COUNT(*) FROM analytics_events
             WHERE project_id = ?1 AND source = 'app' AND ts >= ?2 AND ts < ?3 AND flags <> 0",
        )?
        .query_row(params![project_id, start, end], |r| r.get(0))
}

/// Distinct users in the last [`LIVE_SECONDS`] before `now` (test signals
/// only with `include_test`).
pub fn current_users(
    store: &Connection,
    project_id: i64,
    now: i64,
    include_test: bool,
) -> rusqlite::Result<i64> {
    // Without test signals the query matches idx_analytics_events_app_live,
    // which covers it.
    let sql = if include_test {
        "SELECT COUNT(DISTINCT visitor) FROM analytics_events
         WHERE project_id = ?1 AND source = 'app' AND ts > ?2 AND ts <= ?3"
    } else {
        "SELECT COUNT(DISTINCT visitor) FROM analytics_events
         WHERE project_id = ?1 AND source = 'app' AND flags = 0 AND ts > ?2 AND ts <= ?3"
    };
    store
        .prepare_cached(sql)?
        .query_row(params![project_id, now - LIVE_SECONDS, now + 60], |r| {
            r.get(0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        let f = AppFilters::from_pairs([
            ("version", "1.4.0"),
            ("period", "7d"),
            ("type", "Paywall.shown"),
            ("version", "2.0"),
            ("platform", " "),
            ("test", "1"),
        ]);
        assert_eq!(f.version.as_deref(), Some("1.4.0"));
        assert_eq!(f.platform, None);
        assert!(f.include_test && !f.is_empty());
        assert_eq!(
            f.pairs(),
            [
                ("version", "1.4.0".to_string()),
                ("type", "Paywall.shown".to_string()),
                ("test", "1".to_string())
            ]
        );
        assert!(AppFilters::default().is_empty());
        let (sql, args) = AppFilters::default().sql();
        assert_eq!(sql, " AND flags = 0");
        assert!(args.is_empty());
    }

    #[test]
    fn payloads_group_by_key() {
        let rows = vec![
            ("plan".to_string(), "pro".to_string(), 5),
            ("plan".to_string(), "free".to_string(), 7),
            ("plan".to_string(), OTHER.to_string(), 2),
            ("plan".to_string(), "team".to_string(), 1),
            ("source".to_string(), "onboarding".to_string(), 20),
        ];
        let p = group_payload(rows, 2);
        assert_eq!(p[0].0, "source");
        let plan = &p[1].1;
        assert_eq!(
            plan.iter()
                .map(|r| (r.value.as_str(), r.count))
                .collect::<Vec<_>>(),
            [("free", 7), ("pro", 5), (OTHER, 3)]
        );
    }
}
