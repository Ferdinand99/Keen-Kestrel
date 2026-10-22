//! The analytics rollup (docs/design.md §5.3): every minute, on the
//! writer's connection, raw events from `store.analytics_events` are added
//! to the hourly and daily rollups in `kestrel.sqlite`:
//!
//! * `analytics_hours` and `analytics_days`: pageviews, custom events and
//!   distinct visitors per hour and per day;
//! * `analytics_breakdown`: per day, dimension ([`Dim`]) and value, the
//!   pageviews (entries, events) and distinct visitors.
//!
//! It reads from where it stopped (`store.analytics_rollup_state`) in
//! batches of [`BATCH`]. Counts are added; visitors are counted once per day
//! and value through a seen-set (`store.analytics_seen`) kept for the days
//! that can still get events (today and yesterday, UTC). An event for an
//! older day (the rollup was stopped for a while, or a client clock) is
//! *late*: that day is rolled up again from its raw events, which are kept
//! long enough (the project's raw retention) for the result to be exact.
//! Older than that, it isn't counted.
//!
//! App signals (`source = 'app'`) are read in the same batches and rolled
//! up by [`app::rollup`].
//!
//! Raw events stay after the rollup; the daily retention job removes them
//! after `retention_analytics_raw_days` (30) and the rollups after
//! `retention_analytics_days` (400).

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::app::{self, APP, AppSignal};
use super::{
    Dim, PAGEVIEW, WebEvent, WebEventRow, day_number, day_string, hour_string, value_hash,
};
use crate::writer::{WebChange, Writer, WriterError};

/// Raw events read per transaction.
pub const BATCH: i64 = 5_000;
/// Seen-set code of a day's visitors.
const SEEN_DAY: i64 = 0;
/// Seen-set code of an hour's visitors (the value is the hour of the day).
const SEEN_HOUR: i64 = 12;

/// What one rollup did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Raw events read.
    pub events: usize,
    /// Days rolled up again from their raw events.
    pub late_days: usize,
    /// Projects whose rollups changed.
    pub projects: Vec<i64>,
}

/// Stores one raw event (the writer's job).
pub fn insert_event(c: &Connection, e: &WebEvent) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO store.analytics_events (project_id, source, ts, visitor, name, hostname, path,
             referrer, utm_source, utm_medium, utm_campaign, utm_term, utm_content, browser, os,
             device, screen, props)
         VALUES (?1, 'web', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?
    .execute(params![
        e.project_id,
        e.ts,
        e.visitor,
        e.name,
        e.page.hostname,
        e.page.path,
        e.referrer,
        e.page.utm_source,
        e.page.utm_medium,
        e.page.utm_campaign,
        e.page.utm_term,
        e.page.utm_content,
        e.browser,
        e.os,
        e.device,
        e.screen,
        e.props,
    ])?;
    Ok(())
}

/// Counts and the distinct visitors behind them.
#[derive(Default)]
struct Agg {
    pageviews: i64,
    events: i64,
    visitors: HashSet<i64>,
}

type DayKey = (i64, String, i64);
type DimKey = (i64, String, i64, Dim, String);

/// Adds the distinct `visitors` to the seen-set of `(project, day, dim,
/// value)`; returns how many weren't there yet.
pub(crate) fn new_visitors(
    tx: &Transaction<'_>,
    project_id: i64,
    day: i64,
    dim: i64,
    value: i64,
    visitors: &HashSet<i64>,
) -> rusqlite::Result<i64> {
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO store.analytics_seen (project_id, day, dim, value, visitor)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut n = 0;
    for v in visitors {
        n += stmt.execute(params![project_id, day, dim, value, v])? as i64;
    }
    Ok(n)
}

fn add_day(
    tx: &Transaction<'_>,
    (project_id, source, day): &DayKey,
    pageviews: i64,
    events: i64,
    visitors: i64,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO analytics_days (project_id, source, day, pageviews, events, visitors)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (project_id, source, day) DO UPDATE SET
             pageviews = pageviews + excluded.pageviews, events = events + excluded.events,
             visitors = visitors + excluded.visitors",
    )?
    .execute(params![
        project_id,
        source,
        day_string(*day),
        pageviews,
        events,
        visitors
    ])?;
    Ok(())
}

fn add_hour(
    tx: &Transaction<'_>,
    project_id: i64,
    source: &str,
    hour: &str,
    a: (i64, i64, i64),
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO analytics_hours (project_id, source, hour, pageviews, events, visitors)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (project_id, source, hour) DO UPDATE SET
             pageviews = pageviews + excluded.pageviews, events = events + excluded.events,
             visitors = visitors + excluded.visitors",
    )?
    .execute(params![project_id, source, hour, a.0, a.1, a.2])?;
    Ok(())
}

fn add_breakdown(
    tx: &Transaction<'_>,
    (project_id, source, day, dim, value): &DimKey,
    count: i64,
    visitors: i64,
) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO analytics_breakdown (project_id, source, dim, day, value, count, visitors)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (project_id, source, dim, day, value) DO UPDATE SET
             count = count + excluded.count, visitors = visitors + excluded.visitors",
    )?
    .execute(params![
        project_id,
        source,
        dim.code(),
        day_string(*day),
        value,
        count,
        visitors
    ])?;
    Ok(())
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WebEventRow> {
    Ok(WebEventRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        source: r.get(2)?,
        ts: r.get(3)?,
        visitor: r.get(4)?,
        name: r.get(5)?,
        path: r.get(6)?,
        referrer: r.get(7)?,
        utm_source: r.get(8)?,
        utm_medium: r.get(9)?,
        utm_campaign: r.get(10)?,
        browser: r.get(11)?,
        os: r.get(12)?,
        device: r.get(13)?,
        screen: r.get(14)?,
    })
}

/// The columns [`row`] reads.
pub const ROW_COLUMNS: &str = "id, project_id, source, ts, visitor, name, path, referrer, \
    utm_source, utm_medium, utm_campaign, browser, os, device, screen";

/// Rolls up to [`BATCH`] raw events up. `now` is the Unix time (UTC).
pub fn rollup(c: &mut Connection, now: i64) -> rusqlite::Result<Report> {
    let tx = c.transaction()?;
    let last: i64 = tx
        .query_row(
            "SELECT last_id FROM store.analytics_rollup_state WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    // The batch: the next BATCH raw events of either source.
    let (read, max): (i64, Option<i64>) = tx
        .prepare_cached(
            "SELECT COUNT(*), MAX(id) FROM (
                 SELECT id FROM store.analytics_events WHERE id > ?1 ORDER BY id LIMIT ?2)",
        )?
        .query_row(params![last, BATCH], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let max = max.unwrap_or(last);
    let rows: Vec<WebEventRow> = tx
        .prepare_cached(&format!(
            "SELECT {ROW_COLUMNS} FROM store.analytics_events
             WHERE id > ?1 AND id <= ?2 AND source = 'web' ORDER BY id"
        ))?
        .query_map(params![last, max], row)?
        .collect::<rusqlite::Result<_>>()?;
    let app_rows: Vec<AppSignal> = tx
        .prepare_cached(&format!(
            "SELECT {} FROM store.analytics_events
             WHERE id > ?1 AND id <= ?2 AND source = 'app' ORDER BY id",
            app::rollup::COLUMNS
        ))?
        .query_map(params![last, max], app::rollup::row)?
        .collect::<rusqlite::Result<_>>()?;
    // Projects that exist, with the first day their raw events are complete.
    let today = day_number(now);
    let projects: HashMap<i64, i64> = tx
        .prepare_cached("SELECT id, retention_analytics_raw_days FROM projects")?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .map(|r| r.map(|(id, days)| (id, today - days.max(1) + 1)))
        .collect::<rusqlite::Result<_>>()?;

    let mut days: HashMap<DayKey, Agg> = HashMap::new();
    let mut hours: HashMap<(i64, String, i64), Agg> = HashMap::new();
    let mut dims: HashMap<DimKey, Agg> = HashMap::new();
    let mut entries: Vec<(i64, String, i64, i64, String)> = Vec::new();
    let mut late: BTreeSet<DayKey> = BTreeSet::new();
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    for e in &rows {
        let Some(&complete_from) = projects.get(&e.project_id) else {
            continue;
        };
        let day = day_number(e.ts);
        if day < today - 1 {
            if day >= complete_from {
                late.insert((e.project_id, e.source.clone(), day));
            }
            continue;
        }
        touched.insert(e.project_id);
        let pageview = e.name == PAGEVIEW;
        let bump = |a: &mut Agg| {
            if pageview {
                a.pageviews += 1;
            } else {
                a.events += 1;
            }
            a.visitors.insert(e.visitor);
        };
        bump(
            days.entry((e.project_id, e.source.clone(), day))
                .or_default(),
        );
        bump(
            hours
                .entry((e.project_id, e.source.clone(), e.ts.div_euclid(3600)))
                .or_default(),
        );
        for dim in Dim::ALL {
            if dim == Dim::Entry {
                continue;
            }
            if let Some(value) = dim.value_of(e) {
                bump(
                    dims.entry((e.project_id, e.source.clone(), day, dim, value.to_string()))
                        .or_default(),
                );
            }
        }
        if pageview {
            entries.push((
                e.project_id,
                e.source.clone(),
                day,
                e.visitor,
                e.path.clone(),
            ));
        }
    }

    for (key, a) in &days {
        let n = new_visitors(&tx, key.0, key.2, SEEN_DAY, 0, &a.visitors)?;
        add_day(&tx, key, a.pageviews, a.events, n)?;
    }
    for ((project_id, source, hour), a) in &hours {
        let day = hour.div_euclid(24);
        let n = new_visitors(
            &tx,
            *project_id,
            day,
            SEEN_HOUR,
            hour.rem_euclid(24),
            &a.visitors,
        )?;
        add_hour(
            &tx,
            *project_id,
            source,
            &hour_string(hour * 3600),
            (a.pageviews, a.events, n),
        )?;
    }
    for (key, a) in &dims {
        let n = new_visitors(
            &tx,
            key.0,
            key.2,
            key.3.seen_code(),
            value_hash(&key.4),
            &a.visitors,
        )?;
        add_breakdown(&tx, key, a.pageviews + a.events, n)?;
    }
    // A visitor's first pageview of the day is their entry page.
    let entry = Dim::Entry.seen_code();
    for (project_id, source, day, visitor, path) in entries {
        let first = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO store.analytics_seen (project_id, day, dim, value, visitor)
                 VALUES (?1, ?2, ?3, 0, ?4)",
            )?
            .execute(params![project_id, day, entry, visitor])?
            > 0;
        if first {
            add_breakdown(&tx, &(project_id, source, day, Dim::Entry, path), 1, 1)?;
        }
    }
    app::rollup::add(&tx, &app_rows, today, &projects, &mut late, &mut touched)?;
    for (project_id, source, day) in &late {
        if source == APP {
            app::rollup::recompute_day(&tx, *project_id, *day, today)?;
        } else {
            recompute_day(&tx, *project_id, source, *day)?;
        }
        touched.insert(*project_id);
    }

    if max > last {
        tx.execute(
            "UPDATE store.analytics_rollup_state SET last_id = ?1 WHERE id = 1",
            [max],
        )?;
    }
    tx.execute(
        "DELETE FROM store.analytics_seen WHERE day < ?1",
        [today - 1],
    )?;
    tx.commit()?;
    Ok(Report {
        events: read as usize,
        late_days: late.len(),
        projects: touched.into_iter().collect(),
    })
}

impl Report {
    fn add(&mut self, r: Report) {
        self.events += r.events;
        self.late_days += r.late_days;
        for p in r.projects {
            if !self.projects.contains(&p) {
                self.projects.push(p);
            }
        }
    }
}

/// Rolls everything waiting up on one connection (tests and tooling).
pub fn rollup_all(c: &mut Connection, now: i64) -> rusqlite::Result<Report> {
    let mut total = Report::default();
    loop {
        let r = rollup(c, now)?;
        let done = (r.events as i64) < BATCH;
        total.add(r);
        if done {
            return Ok(total);
        }
    }
}

/// Rolls everything waiting up through the writer, one batch per turn so
/// ingest can write in between, then tells the Web tabs what changed.
pub async fn run(writer: &Writer) -> Result<Report, WriterError> {
    let mut total = Report::default();
    loop {
        let now = jiff::Timestamp::now().as_second();
        let r = writer.run(move |c| rollup(c, now)).await?;
        let done = (r.events as i64) < BATCH;
        total.add(r);
        if done {
            break;
        }
    }
    for p in &total.projects {
        writer.notify_web(WebChange::Rollup(*p));
    }
    Ok(total)
}

/// Replaces one day's rollups of a project with what its raw events say.
pub fn recompute_day(
    tx: &Connection,
    project_id: i64,
    source: &str,
    day: i64,
) -> rusqlite::Result<()> {
    let (start, end) = (day * 86_400, (day + 1) * 86_400);
    let date = day_string(day);
    tx.execute(
        "DELETE FROM analytics_days WHERE project_id = ?1 AND source = ?2 AND day = ?3",
        params![project_id, source, date],
    )?;
    tx.execute(
        "DELETE FROM analytics_hours WHERE project_id = ?1 AND source = ?2 AND hour >= ?3 AND hour < ?4",
        params![project_id, source, date, day_string(day + 1)],
    )?;
    tx.execute(
        "DELETE FROM analytics_breakdown WHERE project_id = ?1 AND source = ?2 AND day = ?3",
        params![project_id, source, date],
    )?;
    let range = "FROM store.analytics_events WHERE project_id = ?1 AND source = ?2 AND ts >= ?3 AND ts < ?4";
    tx.execute(
        &format!(
            "INSERT INTO analytics_days (project_id, source, day, pageviews, events, visitors)
             SELECT ?1, ?2, ?5, SUM(name = 'pageview'), SUM(name <> 'pageview'), COUNT(DISTINCT visitor)
             {range} HAVING COUNT(*) > 0"
        ),
        params![project_id, source, start, end, date],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO analytics_hours (project_id, source, hour, pageviews, events, visitors)
             SELECT ?1, ?2, strftime('%Y-%m-%d %H:00:00', (ts / 3600) * 3600, 'unixepoch'),
                    SUM(name = 'pageview'), SUM(name <> 'pageview'), COUNT(DISTINCT visitor)
             {range} GROUP BY ts / 3600"
        ),
        params![project_id, source, start, end],
    )?;
    for dim in Dim::ALL {
        let col = dim.column();
        let sql = match dim {
            Dim::Entry => format!(
                "INSERT INTO analytics_breakdown (project_id, source, dim, day, value, count, visitors)
                 SELECT ?1, ?2, ?5, ?6, path, COUNT(*), COUNT(*) FROM (
                     SELECT visitor, path, MIN(id) {range} AND name = 'pageview' GROUP BY visitor
                 ) GROUP BY path"
            ),
            Dim::Event => format!(
                "INSERT INTO analytics_breakdown (project_id, source, dim, day, value, count, visitors)
                 SELECT ?1, ?2, ?5, ?6, name, COUNT(*), COUNT(DISTINCT visitor)
                 {range} AND name <> 'pageview' GROUP BY name"
            ),
            _ => format!(
                "INSERT INTO analytics_breakdown (project_id, source, dim, day, value, count, visitors)
                 SELECT ?1, ?2, ?5, ?6, {col}, COUNT(*), COUNT(DISTINCT visitor)
                 {range} AND name = 'pageview' {empty} GROUP BY {col}",
                empty = if dim.counts_empty() {
                    String::new()
                } else {
                    format!("AND {col} <> ''")
                }
            ),
        };
        tx.execute(
            &sql,
            params![project_id, source, start, end, dim.code(), date],
        )?;
    }
    Ok(())
}

/// Deletes up to `batch` raw events of `project_id` older than `cutoff`
/// (Unix seconds). Returns how many.
pub fn prune_raw(
    c: &Connection,
    project_id: i64,
    cutoff: i64,
    batch: i64,
) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM store.analytics_events WHERE id IN (
             SELECT id FROM store.analytics_events WHERE project_id = ?1 AND ts < ?2 LIMIT ?3)",
        params![project_id, cutoff, batch],
    )
}

/// Deletes a project's rollups of days before `cutoff` (`YYYY-MM-DD`).
pub fn prune_rollups(c: &Connection, project_id: i64, cutoff: &str) -> rusqlite::Result<usize> {
    let mut n = c.execute(
        "DELETE FROM analytics_days WHERE project_id = ?1 AND day < ?2",
        params![project_id, cutoff],
    )?;
    n += c.execute(
        "DELETE FROM analytics_hours WHERE project_id = ?1 AND hour < ?2",
        params![project_id, cutoff],
    )?;
    n += c.execute(
        "DELETE FROM analytics_breakdown WHERE day < ?2 AND project_id = ?1",
        params![project_id, cutoff],
    )?;
    Ok(n)
}

/// Every project's analytics retention: `(id, raw days, rollup days)`.
pub fn all_retention(c: &Connection) -> rusqlite::Result<Vec<(i64, i64, i64)>> {
    c.prepare(
        "SELECT id, retention_analytics_raw_days, retention_analytics_days FROM projects ORDER BY id",
    )?
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
    .collect()
}
