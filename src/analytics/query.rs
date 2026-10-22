//! What the Web tab, the REST API and MCP read (docs/design.md §5.3).
//!
//! Without filters everything comes from the rollups in `kestrel.sqlite`
//! (at most a minute behind, kept 400 days): totals and the daily series
//! from `analytics_days`, a single day's series from `analytics_hours`,
//! breakdowns from `analytics_breakdown`. Unique visitors over several days
//! are the sum of the days' visitors.
//!
//! With filters (a page, a referrer, a browser …) the counts come from the
//! raw events in `store.sqlite`, which are kept 30 days: the rollups count
//! each dimension on its own and can't be combined. Visitors are again
//! counted per day and added up. Current visitors (the last five minutes)
//! always come from the raw events.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Dim, WEB, day_number, day_of, day_string};

/// Longest range a query covers (the rollups' default retention).
pub const MAX_DAYS: i64 = 400;
/// "Current visitors": seen in this many seconds.
pub const LIVE_SECONDS: i64 = 300;

/// A range of whole UTC days, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    /// Day numbers ([`day_number`]).
    pub from: i64,
    pub to: i64,
}

impl Period {
    /// The last `days` days up to `today` (a day number).
    pub fn last(days: i64, today: i64) -> Period {
        Period {
            from: today - days.clamp(1, MAX_DAYS) + 1,
            to: today,
        }
    }

    /// One day (or less) is shown by the hour.
    pub fn hourly(&self) -> bool {
        self.from == self.to
    }

    pub fn days(&self) -> i64 {
        self.to - self.from + 1
    }

    pub fn from_date(&self) -> String {
        day_string(self.from)
    }

    pub fn to_date(&self) -> String {
        day_string(self.to)
    }

    /// Unix seconds: the start of `from` and the end of `to`.
    pub fn bounds(&self) -> (i64, i64) {
        (self.from * 86_400, (self.to + 1) * 86_400)
    }

    /// Reads a period: `today`, `yesterday`, `<n>d` (1–400), or `custom`
    /// with `from` and `to` (`YYYY-MM-DD`, at most 400 days, not after
    /// today). `Err` names the problem.
    pub fn parse(
        period: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        today: i64,
    ) -> Result<(Period, String), &'static str> {
        let date = |s: Option<&str>| -> Result<Option<i64>, &'static str> {
            match s.map(str::trim).filter(|s| !s.is_empty()) {
                None => Ok(None),
                Some(s) => s
                    .parse::<jiff::civil::Date>()
                    .map(|d| Some(day_of(d)))
                    .map_err(|_| "invalid date"),
            }
        };
        let (from, to) = (date(from)?, date(to)?);
        let name = period
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(if from.is_some() { "custom" } else { "7d" })
            .to_ascii_lowercase();
        let p = match name.as_str() {
            "today" | "day" => Period::last(1, today),
            "yesterday" => Period {
                from: today - 1,
                to: today - 1,
            },
            "custom" => {
                let from = from.ok_or("custom periods need from and to")?;
                let to = to.unwrap_or(today).min(today);
                if from > to {
                    return Err("from is after to");
                }
                if to - from + 1 > MAX_DAYS {
                    return Err("at most 400 days");
                }
                Period { from, to }
            }
            other => {
                let n = other
                    .strip_suffix('d')
                    .and_then(|n| n.parse::<i64>().ok())
                    .filter(|n| (1..=MAX_DAYS).contains(n))
                    .ok_or("unknown period")?;
                Period::last(n, today)
            }
        };
        Ok((p, name))
    }
}

/// Filters: values of dimensions the counts must have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub values: Vec<(Dim, String)>,
}

impl Filters {
    /// The filters in query pairs (`page=/pricing&browser=Firefox`); other
    /// keys are ignored.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Filters {
        let mut values: Vec<(Dim, String)> = Vec::new();
        for (k, v) in pairs {
            if let Some(dim) = Dim::parse(k).filter(|d| d.is_filter())
                && !values.iter().any(|(d, _)| *d == dim)
            {
                values.push((dim, v.chars().take(512).collect()));
            }
        }
        values.sort_by_key(|(d, _)| d.seen_code());
        Filters { values }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, dim: Dim) -> Option<&str> {
        self.values
            .iter()
            .find(|(d, _)| *d == dim)
            .map(|(_, v)| v.as_str())
    }

    /// These filters with `dim` set to `value`.
    pub fn with(&self, dim: Dim, value: &str) -> Filters {
        let mut f = self.without(dim);
        f.values.push((dim, value.to_string()));
        f.values.sort_by_key(|(d, _)| d.seen_code());
        f
    }

    pub fn without(&self, dim: Dim) -> Filters {
        Filters {
            values: self
                .values
                .iter()
                .filter(|(d, _)| *d != dim)
                .cloned()
                .collect(),
        }
    }

    /// As query pairs.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        self.values
            .iter()
            .map(|(d, v)| (d.code(), v.clone()))
            .collect()
    }

    /// SQL conditions on the raw events and their parameters.
    fn sql(&self) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut args = Vec::new();
        for (dim, value) in &self.values {
            out.push_str(&format!(" AND {} = ?", dim.column()));
            args.push(value.clone());
        }
        (out, args)
    }
}

/// Pageviews, custom events and unique visitors (summed per day).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub visitors: i64,
    pub pageviews: i64,
    pub events: i64,
}

/// One point of a time series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    /// `YYYY-MM-DD`, or `YYYY-MM-DD HH:00:00` for hours.
    pub start: String,
    pub visitors: i64,
    pub pageviews: i64,
    pub events: i64,
}

/// One value of a breakdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub value: String,
    /// Pageviews (entries for entry pages, events for custom events).
    pub count: i64,
    /// Unique visitors, summed per day.
    pub visitors: i64,
}

/// Whether the project has any web analytics: rollups or raw events.
pub fn has_data(main: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    Ok(main
        .prepare_cached(
            "SELECT 1 FROM analytics_days WHERE project_id = ?1 AND source = ?2 LIMIT 1",
        )?
        .query_row(params![project_id, WEB], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Whether the project has raw events (the store's side of [`has_data`],
/// for the minute before the first rollup).
pub fn has_raw(store: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    Ok(store
        .prepare_cached(
            "SELECT 1 FROM analytics_events WHERE project_id = ?1 AND source = 'web' LIMIT 1",
        )?
        .query_row([project_id], |_| Ok(()))
        .optional()?
        .is_some())
}

// ---------------------------------------------------------------------------
// Rollups (no filters)
// ---------------------------------------------------------------------------

/// Totals over the period from `analytics_days`.
pub fn totals(main: &Connection, project_id: i64, p: Period) -> rusqlite::Result<Totals> {
    main.prepare_cached(
        "SELECT coalesce(SUM(visitors), 0), coalesce(SUM(pageviews), 0), coalesce(SUM(events), 0)
         FROM analytics_days WHERE project_id = ?1 AND source = ?2 AND day >= ?3 AND day <= ?4",
    )?
    .query_row(params![project_id, WEB, p.from_date(), p.to_date()], |r| {
        Ok(Totals {
            visitors: r.get(0)?,
            pageviews: r.get(1)?,
            events: r.get(2)?,
        })
    })
}

/// Every hour (one-day periods) or day of the period, oldest first, with
/// zeros where nothing happened.
pub fn series(main: &Connection, project_id: i64, p: Period) -> rusqlite::Result<Vec<Point>> {
    let rows: Vec<Point> = if p.hourly() {
        main.prepare_cached(
            "SELECT hour, visitors, pageviews, events FROM analytics_hours
             WHERE project_id = ?1 AND source = ?2 AND hour >= ?3 AND hour < ?4 ORDER BY hour",
        )?
        .query_map(
            params![project_id, WEB, p.from_date(), day_string(p.to + 1)],
            point,
        )?
        .collect::<rusqlite::Result<_>>()?
    } else {
        main.prepare_cached(
            "SELECT day, visitors, pageviews, events FROM analytics_days
             WHERE project_id = ?1 AND source = ?2 AND day >= ?3 AND day <= ?4 ORDER BY day",
        )?
        .query_map(params![project_id, WEB, p.from_date(), p.to_date()], point)?
        .collect::<rusqlite::Result<_>>()?
    };
    Ok(fill(p, rows))
}

fn point(r: &rusqlite::Row<'_>) -> rusqlite::Result<Point> {
    Ok(Point {
        start: r.get(0)?,
        visitors: r.get(1)?,
        pageviews: r.get(2)?,
        events: r.get(3)?,
    })
}

/// The period's hours or days in order, `rows` where they exist.
fn fill(p: Period, rows: Vec<Point>) -> Vec<Point> {
    let keys: Vec<String> = if p.hourly() {
        (0..24)
            .map(|h| super::hour_string(p.from * 86_400 + h * 3600))
            .collect()
    } else {
        (p.from..=p.to).map(day_string).collect()
    };
    keys.into_iter()
        .map(|start| {
            rows.iter()
                .find(|r| r.start == start)
                .cloned()
                .unwrap_or(Point {
                    start,
                    visitors: 0,
                    pageviews: 0,
                    events: 0,
                })
        })
        .collect()
}

/// The top `limit` values of `dim` over the period, most visitors first.
pub fn breakdown(
    main: &Connection,
    project_id: i64,
    p: Period,
    dim: Dim,
    limit: i64,
) -> rusqlite::Result<Vec<Row>> {
    main.prepare_cached(
        "SELECT value, SUM(count), SUM(visitors) FROM analytics_breakdown
         WHERE project_id = ?1 AND source = ?2 AND dim = ?3 AND day >= ?4 AND day <= ?5
         GROUP BY value ORDER BY 3 DESC, 2 DESC, value LIMIT ?6",
    )?
    .query_map(
        params![
            project_id,
            WEB,
            dim.code(),
            p.from_date(),
            p.to_date(),
            limit
        ],
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

// ---------------------------------------------------------------------------
// Raw events (with filters; current visitors)
// ---------------------------------------------------------------------------

/// The raw events' `WHERE` for a project, period and filters, and its
/// parameters.
fn raw_where(project_id: i64, p: Period, f: &Filters) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value as V;
    let (start, end) = p.bounds();
    let (cond, args) = f.sql();
    let mut params = vec![
        V::Integer(project_id),
        V::Text(WEB.into()),
        V::Integer(start),
        V::Integer(end),
    ];
    params.extend(args.into_iter().map(V::Text));
    (
        format!(
            "FROM analytics_events WHERE project_id = ? AND source = ? AND ts >= ? AND ts < ?{cond}"
        ),
        params,
    )
}

/// [`totals`] from the raw events, filtered.
pub fn raw_totals(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &Filters,
) -> rusqlite::Result<Totals> {
    let (from, args) = raw_where(project_id, p, f);
    let (pageviews, events): (i64, i64) = store.query_row(
        &format!(
            "SELECT coalesce(SUM(name = 'pageview'), 0), coalesce(SUM(name <> 'pageview'), 0) {from}"
        ),
        rusqlite::params_from_iter(args.iter()),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let visitors: i64 = store.query_row(
        &format!("SELECT COUNT(*) FROM (SELECT DISTINCT ts / 86400, visitor {from})"),
        rusqlite::params_from_iter(args.iter()),
        |r| r.get(0),
    )?;
    Ok(Totals {
        visitors,
        pageviews,
        events,
    })
}

/// [`series`] from the raw events, filtered.
pub fn raw_series(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &Filters,
) -> rusqlite::Result<Vec<Point>> {
    let (from, args) = raw_where(project_id, p, f);
    let (bucket, label) = if p.hourly() {
        (
            "ts / 3600",
            "strftime('%Y-%m-%d %H:00:00', (ts / 3600) * 3600, 'unixepoch')",
        )
    } else {
        (
            "ts / 86400",
            "strftime('%Y-%m-%d', (ts / 86400) * 86400, 'unixepoch')",
        )
    };
    let rows: Vec<Point> = store
        .prepare(&format!(
            "SELECT {label}, COUNT(DISTINCT visitor), SUM(name = 'pageview'), SUM(name <> 'pageview')
             {from} GROUP BY {bucket} ORDER BY {bucket}"
        ))?
        .query_map(rusqlite::params_from_iter(args.iter()), point)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(fill(p, rows))
}

/// [`breakdown`] from the raw events, filtered.
pub fn raw_breakdown(
    store: &Connection,
    project_id: i64,
    p: Period,
    f: &Filters,
    dim: Dim,
    limit: i64,
) -> rusqlite::Result<Vec<Row>> {
    let (from, mut args) = raw_where(project_id, p, f);
    args.push(rusqlite::types::Value::Integer(limit));
    let col = dim.column();
    let sql = match dim {
        Dim::Entry => format!(
            "SELECT path, COUNT(*), COUNT(*) FROM (
                 SELECT ts / 86400 AS d, visitor, path, MIN(id) {from} AND name = 'pageview'
                 GROUP BY d, visitor
             ) GROUP BY path ORDER BY 2 DESC, path LIMIT ?"
        ),
        _ => {
            let kind = if dim == Dim::Event {
                "AND name <> 'pageview'".to_string()
            } else if dim.counts_empty() {
                "AND name = 'pageview'".to_string()
            } else {
                format!("AND name = 'pageview' AND {col} <> ''")
            };
            format!(
                "SELECT v, SUM(c), COUNT(*) FROM (
                     SELECT {col} AS v, ts / 86400 AS d, visitor, COUNT(*) AS c {from} {kind}
                     GROUP BY v, d, visitor
                 ) GROUP BY v ORDER BY 3 DESC, 2 DESC, v LIMIT ?"
            )
        }
    };
    store
        .prepare(&sql)?
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(Row {
                value: r.get(0)?,
                count: r.get(1)?,
                visitors: r.get(2)?,
            })
        })?
        .collect()
}

/// The first day with raw events of the project (filters can't reach
/// before it).
pub fn raw_first_day(store: &Connection, project_id: i64) -> rusqlite::Result<Option<i64>> {
    let ts: Option<i64> = store
        .prepare_cached(
            "SELECT MIN(ts) FROM analytics_events WHERE project_id = ?1 AND source = 'web'",
        )?
        .query_row([project_id], |r| r.get(0))?;
    Ok(ts.map(day_number))
}

/// Distinct visitors in the last [`LIVE_SECONDS`] before `now`.
pub fn current_visitors(store: &Connection, project_id: i64, now: i64) -> rusqlite::Result<i64> {
    store
        .prepare_cached(
            "SELECT COUNT(DISTINCT visitor) FROM analytics_events
             WHERE project_id = ?1 AND source = 'web' AND ts > ?2 AND ts <= ?3",
        )?
        .query_row(params![project_id, now - LIVE_SECONDS, now + 60], |r| {
            r.get(0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        let today = day_of(jiff::civil::date(2026, 10, 22));
        let (p, name) = Period::parse(None, None, None, today).unwrap();
        assert_eq!(
            (p.from_date().as_str(), p.to_date().as_str()),
            ("2026-10-16", "2026-10-22")
        );
        assert_eq!(name, "7d");
        let (p, _) = Period::parse(Some("today"), None, None, today).unwrap();
        assert!(p.hourly() && p.days() == 1);
        let (p, _) = Period::parse(Some("30d"), None, None, today).unwrap();
        assert_eq!(p.days(), 30);
        let (p, name) = Period::parse(None, Some("2026-10-01"), Some("2026-10-31"), today).unwrap();
        assert_eq!(name, "custom");
        assert_eq!(
            (p.from_date().as_str(), p.to_date().as_str()),
            ("2026-10-01", "2026-10-22")
        );
        assert!(
            Period::parse(
                Some("custom"),
                Some("2026-10-10"),
                Some("2026-10-01"),
                today
            )
            .is_err()
        );
        assert!(Period::parse(Some("custom"), Some("2020-01-01"), None, today).is_err());
        assert!(Period::parse(Some("401d"), None, None, today).is_err());
        assert!(Period::parse(Some("week"), None, None, today).is_err());
        assert!(Period::parse(None, Some("22.10.2026"), None, today).is_err());
    }

    #[test]
    fn filters() {
        let f = Filters::from_pairs([
            ("browser", "Firefox"),
            ("page", "/a"),
            ("period", "7d"),
            ("entry", "/x"),
            ("page", "/b"),
        ]);
        assert_eq!(
            f.pairs(),
            [
                ("page", "/a".to_string()),
                ("browser", "Firefox".to_string())
            ]
        );
        assert_eq!(f.get(Dim::Browser), Some("Firefox"));
        let g = f.with(Dim::Referrer, "news.example").without(Dim::Page);
        assert_eq!(
            g.pairs(),
            [
                ("referrer", "news.example".to_string()),
                ("browser", "Firefox".to_string())
            ]
        );
        assert!(Filters::default().is_empty());
    }
}
