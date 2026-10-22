//! Triage: what the issue pages read and the lifecycle actions they take
//! (docs/design.md §4.5): resolve (now or in the next release), unresolve,
//! ignore (forever, until a count, until a time), merge and delete, each
//! noted in the issue's activity log.
//!
//! Reads work on any connection to `kestrel.sqlite`. The actions run on the
//! writer's connection (`store.sqlite` attached as `store`), inside one
//! transaction each, so they never race the ingest of the same issue.

use std::collections::HashMap;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde_json::{Value, json};

use super::issues::log_activity;
use crate::issues::search::{Query, Sort, fts_match};

/// An issue as the pages show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRow {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub culprit: String,
    pub level: String,
    pub platform: String,
    pub status: String,
    pub substatus: String,
    pub first_seen: String,
    pub last_seen: String,
    pub times_seen: i64,
    pub users_seen: i64,
    pub first_release: Option<String>,
    pub last_release: Option<String>,
    pub resolved_in_release: Option<String>,
    pub resolved_at: Option<String>,
    pub ignore_until: Option<String>,
    pub ignore_until_count: Option<i64>,
    pub grouping_config: String,
    pub unsymbolicated: bool,
}

const ISSUE_COLUMNS: &str = "i.id, i.project_id, i.title, i.culprit, i.level, i.platform, i.status,
    i.substatus, i.first_seen, i.last_seen, i.times_seen, i.users_seen, fr.version, lr.version,
    i.resolved_in_release, i.resolved_at, i.ignore_until, i.ignore_until_count,
    i.grouping_config, i.unsymbolicated";

const ISSUE_FROM: &str = "issues i
    LEFT JOIN releases fr ON fr.id = i.first_release_id
    LEFT JOIN releases lr ON lr.id = i.last_release_id";

fn issue_row(r: &Row<'_>) -> rusqlite::Result<IssueRow> {
    Ok(IssueRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        title: r.get(2)?,
        culprit: r.get(3)?,
        level: r.get(4)?,
        platform: r.get(5)?,
        status: r.get(6)?,
        substatus: r.get(7)?,
        first_seen: r.get(8)?,
        last_seen: r.get(9)?,
        times_seen: r.get(10)?,
        users_seen: r.get(11)?,
        first_release: r.get(12)?,
        last_release: r.get(13)?,
        resolved_in_release: r.get(14)?,
        resolved_at: r.get(15)?,
        ignore_until: r.get(16)?,
        ignore_until_count: r.get(17)?,
        grouping_config: r.get(18)?,
        unsymbolicated: r.get(19)?,
    })
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

/// One page of a project's issues.
#[derive(Debug, Clone)]
pub struct ListParams<'a> {
    pub project_id: i64,
    /// `issues.status`, `None` for any.
    pub status: Option<&'a str>,
    pub query: &'a Query,
    pub sort: Sort,
    pub limit: i64,
    pub offset: i64,
}

/// `WHERE` (without the keyword) and its parameters for a list.
fn list_filter(p: &ListParams<'_>) -> (String, Vec<SqlValue>) {
    let q = p.query;
    let mut sql = vec!["i.project_id = ?".to_string()];
    let mut args: Vec<SqlValue> = vec![p.project_id.into()];
    if let Some(status) = p.status {
        sql.push("i.status = ?".into());
        args.push(status.to_string().into());
    }
    if let Some(sub) = &q.substatus {
        sql.push("i.substatus = ?".into());
        args.push(sub.clone().into());
    }
    if q.unsymbolicated {
        sql.push("i.unsymbolicated = 1".into());
    }
    let wanted: Vec<&String> = q
        .levels
        .iter()
        .filter(|(_, n)| !n)
        .map(|(l, _)| l)
        .collect();
    if !wanted.is_empty() {
        sql.push(format!(
            "i.level IN ({})",
            vec!["?"; wanted.len()].join(", ")
        ));
        args.extend(wanted.into_iter().map(|l| SqlValue::from(l.clone())));
    }
    for (level, _) in q.levels.iter().filter(|(_, n)| *n) {
        sql.push("i.level <> ?".into());
        args.push(level.clone().into());
    }
    for tag in &q.tags {
        let keys = tag.keys();
        let patterns = tag.patterns();
        sql.push(format!(
            "{}EXISTS (SELECT 1 FROM issue_tag_values t WHERE t.issue_id = i.id AND t.key IN ({}) AND ({}))",
            if tag.negated { "NOT " } else { "" },
            vec!["?"; keys.len()].join(", "),
            vec!["t.value LIKE ? ESCAPE '\\'"; patterns.len()].join(" OR ")
        ));
        args.extend(keys.into_iter().map(SqlValue::from));
        args.extend(patterns.into_iter().map(SqlValue::from));
    }
    if let Some(m) = fts_match(q) {
        sql.push("i.id IN (SELECT rowid FROM issues_fts WHERE issues_fts MATCH ?)".into());
        args.push(m.into());
    }
    if !q.event_ids.is_empty() {
        sql.push(format!(
            "i.id IN (SELECT issue_id FROM events WHERE project_id = ? AND event_id IN ({}))",
            vec!["?"; q.event_ids.len()].join(", ")
        ));
        args.push(p.project_id.into());
        args.extend(q.event_ids.iter().map(|e| SqlValue::from(e.clone())));
    }
    (sql.join(" AND "), args)
}

/// One page of issues and how many match in all.
pub fn list(c: &Connection, p: &ListParams<'_>) -> rusqlite::Result<(Vec<IssueRow>, i64)> {
    let (filter, args) = list_filter(p);
    let total: i64 = c.query_row(
        &format!("SELECT COUNT(*) FROM issues i WHERE {filter}"),
        params_from_iter(args.iter()),
        |r| r.get(0),
    )?;
    let mut page_args = args;
    page_args.push(p.limit.into());
    page_args.push(p.offset.into());
    let rows = c
        .prepare(&format!(
            "SELECT {ISSUE_COLUMNS} FROM {ISSUE_FROM} WHERE {filter} ORDER BY {} LIMIT ? OFFSET ?",
            p.sort.order_by()
        ))?
        .query_map(params_from_iter(page_args.iter()), issue_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((rows, total))
}

/// Events per UTC day for each issue, the last `days` days up to `today`
/// (oldest first; `today` is `YYYY-MM-DD`).
pub fn daily_events(
    c: &Connection,
    issue_ids: &[i64],
    today: &str,
    days: i64,
) -> rusqlite::Result<HashMap<i64, Vec<i64>>> {
    let mut out: HashMap<i64, Vec<i64>> = HashMap::new();
    let first: String = c.query_row(
        "SELECT date(?1, ?2)",
        params![today, format!("-{} days", days - 1)],
        |r| r.get(0),
    )?;
    let mut stmt = c.prepare_cached(
        "SELECT substr(hour, 1, 10), SUM(events) FROM issue_hours
         WHERE issue_id = ?1 AND hour >= ?2 GROUP BY 1",
    )?;
    for id in issue_ids {
        let mut series = vec![0; usize::try_from(days).unwrap_or(0)];
        let rows = stmt
            .query_map(params![id, first], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (day, n) in rows {
            if let Some(i) = day_index(&first, &day, days) {
                series[i] += n;
            }
        }
        out.insert(*id, series);
    }
    Ok(out)
}

/// Index of `day` in the `days` days starting at `first` (both `YYYY-MM-DD`).
fn day_index(first: &str, day: &str, days: i64) -> Option<usize> {
    let a: jiff::civil::Date = first.parse().ok()?;
    let b: jiff::civil::Date = day.parse().ok()?;
    let n = i64::from(b.since(a).ok()?.get_days());
    (0..days).contains(&n).then_some(n as usize)
}

/// The project overview's numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    /// Any issue at all (otherwise the page shows how to send the first event).
    pub has_issues: bool,
    pub unresolved: i64,
    pub events_24h: i64,
    pub events_14d: i64,
    /// Events per UTC day, the last 14 days, oldest first.
    pub daily: Vec<i64>,
}

/// The overview's counts at `now` (`YYYY-MM-DD HH:MM:SS`).
pub fn overview(c: &Connection, project_id: i64, now: &str) -> rusqlite::Result<Overview> {
    let has_issues: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM issues WHERE project_id = ?1)",
        [project_id],
        |r| r.get(0),
    )?;
    if !has_issues {
        return Ok(Overview::default());
    }
    let unresolved: i64 = c.query_row(
        "SELECT COUNT(*) FROM issues WHERE project_id = ?1 AND status = 'unresolved'",
        [project_id],
        |r| r.get(0),
    )?;
    let (hour_cutoff, first_day, today): (String, String, String) = c.query_row(
        "SELECT strftime('%Y-%m-%d %H:00:00', ?1, '-23 hours'), date(?1, '-13 days'), date(?1)",
        [now],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let events_24h: i64 = c.query_row(
        "SELECT coalesce(SUM(events), 0) FROM issue_hours WHERE project_id = ?1 AND hour >= ?2",
        params![project_id, hour_cutoff],
        |r| r.get(0),
    )?;
    let mut daily = vec![0; 14];
    let rows = c
        .prepare(
            "SELECT substr(hour, 1, 10), SUM(events) FROM issue_hours
             WHERE project_id = ?1 AND hour >= ?2 GROUP BY 1",
        )?
        .query_map(params![project_id, first_day], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (day, n) in rows {
        if day <= today
            && let Some(i) = day_index(&first_day, &day, 14)
        {
            daily[i] += n;
        }
    }
    Ok(Overview {
        has_issues,
        unresolved,
        events_24h,
        events_14d: daily.iter().sum(),
        daily,
    })
}

/// Unresolved issues with the most events since `since` (`YYYY-MM-DD …`),
/// and those counts.
pub fn most_frequent(
    c: &Connection,
    project_id: i64,
    since: &str,
    limit: i64,
) -> rusqlite::Result<Vec<(IssueRow, i64)>> {
    c.prepare(&format!(
        "SELECT {ISSUE_COLUMNS}, h.n FROM {ISSUE_FROM} JOIN (
             SELECT issue_id, SUM(events) AS n FROM issue_hours
             WHERE project_id = ?1 AND hour >= ?2 GROUP BY issue_id
         ) h ON h.issue_id = i.id
         WHERE i.status = 'unresolved'
         ORDER BY h.n DESC, i.last_seen DESC LIMIT ?3"
    ))?
    .query_map(params![project_id, since, limit], |r| {
        Ok((issue_row(r)?, r.get(20)?))
    })?
    .collect()
}

// ---------------------------------------------------------------------------
// One issue
// ---------------------------------------------------------------------------

pub fn get(c: &Connection, project_id: i64, issue_id: i64) -> rusqlite::Result<Option<IssueRow>> {
    c.query_row(
        &format!("SELECT {ISSUE_COLUMNS} FROM {ISSUE_FROM} WHERE i.project_id = ?1 AND i.id = ?2"),
        [project_id, issue_id],
        issue_row,
    )
    .optional()
}

/// A stored event's index row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub event_id: String,
    pub timestamp: String,
    pub received_at: String,
    pub level: String,
    pub release: Option<String>,
    pub environment: Option<String>,
    pub user_hash: Option<String>,
    /// `None` once retention removed its details.
    pub payload_id: Option<i64>,
}

const EVENT_COLUMNS: &str = "e.id, e.event_id, e.timestamp, e.received_at, e.level, r.version,
    v.name, e.user_hash, e.payload_id";
const EVENT_FROM: &str = "events e
    LEFT JOIN releases r ON r.id = e.release_id
    LEFT JOIN environments v ON v.id = e.environment_id";

fn event_row(r: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: r.get(0)?,
        event_id: r.get(1)?,
        timestamp: r.get(2)?,
        received_at: r.get(3)?,
        level: r.get(4)?,
        release: r.get(5)?,
        environment: r.get(6)?,
        user_hash: r.get(7)?,
        payload_id: r.get(8)?,
    })
}

/// An issue's stored events, newest first, and how many there are.
pub fn events(
    c: &Connection,
    issue_id: i64,
    limit: i64,
    offset: i64,
) -> rusqlite::Result<(Vec<EventRow>, i64)> {
    let total: i64 = c.query_row(
        "SELECT COUNT(*) FROM events WHERE issue_id = ?1",
        [issue_id],
        |r| r.get(0),
    )?;
    let rows = c
        .prepare(&format!(
            "SELECT {EVENT_COLUMNS} FROM {EVENT_FROM} WHERE e.issue_id = ?1
             ORDER BY e.timestamp DESC, e.id DESC LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![issue_id, limit, offset], event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((rows, total))
}

/// The newest stored event that still has its details, else the newest.
pub fn latest_event(c: &Connection, issue_id: i64) -> rusqlite::Result<Option<EventRow>> {
    c.query_row(
        &format!(
            "SELECT {EVENT_COLUMNS} FROM {EVENT_FROM} WHERE e.issue_id = ?1
             ORDER BY e.payload_id IS NULL, e.timestamp DESC, e.id DESC LIMIT 1"
        ),
        [issue_id],
        event_row,
    )
    .optional()
}

/// One of an issue's events by its event id.
pub fn event(c: &Connection, issue_id: i64, event_id: &str) -> rusqlite::Result<Option<EventRow>> {
    c.query_row(
        &format!(
            "SELECT {EVENT_COLUMNS} FROM {EVENT_FROM} WHERE e.issue_id = ?1 AND e.event_id = ?2"
        ),
        params![issue_id, event_id],
        event_row,
    )
    .optional()
}

/// The event ids before (older) and after (newer) `e` in its issue.
pub fn neighbours(
    c: &Connection,
    issue_id: i64,
    e: &EventRow,
) -> rusqlite::Result<(Option<String>, Option<String>)> {
    let older = c
        .query_row(
            "SELECT event_id FROM events WHERE issue_id = ?1 AND (timestamp, id) < (?2, ?3)
             ORDER BY timestamp DESC, id DESC LIMIT 1",
            params![issue_id, e.timestamp, e.id],
            |r| r.get(0),
        )
        .optional()?;
    let newer = c
        .query_row(
            "SELECT event_id FROM events WHERE issue_id = ?1 AND (timestamp, id) > (?2, ?3)
             ORDER BY timestamp, id LIMIT 1",
            params![issue_id, e.timestamp, e.id],
            |r| r.get(0),
        )
        .optional()?;
    Ok((older, newer))
}

/// One tag key of an issue: its most frequent values and their counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSummary {
    pub key: String,
    /// Events counted for this key (all values).
    pub total: i64,
    /// Distinct values counted.
    pub distinct: i64,
    /// `(value, events, last seen)`, most frequent first.
    pub top: Vec<(String, i64, String)>,
}

/// Every tag key of an issue with its `top` most frequent values; the
/// standard keys first, in their usual order, then the custom ones by name.
pub fn tags(c: &Connection, issue_id: i64, top: usize) -> rusqlite::Result<Vec<TagSummary>> {
    let rows = c
        .prepare_cached(
            "SELECT key, value, times_seen, last_seen FROM issue_tag_values WHERE issue_id = ?1
             ORDER BY key, times_seen DESC, value",
        )?
        .query_map([issue_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out: Vec<TagSummary> = Vec::new();
    for (key, value, n, last) in rows {
        match out.last_mut() {
            Some(t) if t.key == key => {
                t.total += n;
                t.distinct += 1;
                if t.top.len() < top {
                    t.top.push((value, n, last));
                }
            }
            _ => out.push(TagSummary {
                key,
                total: n,
                distinct: 1,
                top: vec![(value, n, last)],
            }),
        }
    }
    let rank = |k: &str| {
        super::issues::STANDARD_TAGS
            .iter()
            .position(|s| *s == k)
            .unwrap_or(usize::MAX)
    };
    out.sort_by(|a, b| rank(&a.key).cmp(&rank(&b.key)).then(a.key.cmp(&b.key)));
    Ok(out)
}

/// One grouping hash of an issue and what went into it.
#[derive(Debug, Clone, PartialEq)]
pub struct HashRow {
    pub hash: String,
    pub grouping_config: String,
    pub components: Value,
}

pub fn hashes(c: &Connection, issue_id: i64) -> rusqlite::Result<Vec<HashRow>> {
    c.prepare_cached(
        "SELECT hash, grouping_config, components FROM issue_hashes WHERE issue_id = ?1 ORDER BY id",
    )?
    .query_map([issue_id], |r| {
        let components: String = r.get(2)?;
        Ok(HashRow {
            hash: r.get(0)?,
            grouping_config: r.get(1)?,
            components: serde_json::from_str(&components).unwrap_or(Value::Null),
        })
    })?
    .collect()
}

/// One entry of an issue's activity log.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub kind: String,
    /// The user's name, `None` for Kestrel itself.
    pub actor: Option<String>,
    pub data: Value,
    pub created_at: String,
}

/// An issue's activity, newest first.
pub fn activity(c: &Connection, issue_id: i64) -> rusqlite::Result<Vec<Activity>> {
    c.prepare_cached(
        "SELECT kind, actor, data, created_at FROM issue_activity WHERE issue_id = ?1
         ORDER BY id DESC LIMIT 200",
    )?
    .query_map([issue_id], |r| {
        let data: String = r.get(2)?;
        Ok(Activity {
            kind: r.get(0)?,
            actor: r.get(1)?,
            data: serde_json::from_str(&data).unwrap_or(Value::Null),
            created_at: r.get(3)?,
        })
    })?
    .collect()
}

/// The project's releases, newest first (for the release filter).
pub fn releases(c: &Connection, project_id: i64, limit: i64) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached(
        "SELECT version FROM releases WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?
    .query_map([project_id, limit], |r| r.get(0))?
    .collect()
}

/// The project's environments, by name.
pub fn environments(c: &Connection, project_id: i64) -> rusqlite::Result<Vec<String>> {
    c.prepare_cached("SELECT name FROM environments WHERE project_id = ?1 ORDER BY name LIMIT 100")?
        .query_map([project_id], |r| r.get(0))?
        .collect()
}

/// The project's newest release (the one "resolve in the next release"
/// waits past), if it has any.
pub fn latest_release(c: &Connection, project_id: i64) -> rusqlite::Result<Option<String>> {
    c.query_row(
        "SELECT version FROM releases WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
        [project_id],
        |r| r.get(0),
    )
    .optional()
}

// ---------------------------------------------------------------------------
// Lifecycle (on the writer's connection)
// ---------------------------------------------------------------------------

/// How long an issue stays ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ignore {
    Forever,
    /// Until this many more events arrive.
    Count(i64),
    /// Until this UTC time (`YYYY-MM-DD HH:MM:SS`).
    Until(String),
}

/// The ids among `ids` that are issues of `project_id`.
fn own(c: &Connection, project_id: i64, ids: &[i64]) -> rusqlite::Result<Vec<i64>> {
    let mut stmt =
        c.prepare_cached("SELECT EXISTS (SELECT 1 FROM issues WHERE id = ?1 AND project_id = ?2)")?;
    let mut out = Vec::new();
    for id in ids {
        if !out.contains(id) && stmt.query_row(params![id, project_id], |r| r.get::<_, bool>(0))? {
            out.push(*id);
        }
    }
    Ok(out)
}

/// Resolves issues, now or (with `next_release`) in the release after the
/// project's newest one: events from that release or older then don't
/// regress them. Without any release it resolves now. Returns how many.
pub fn resolve(
    c: &Connection,
    project_id: i64,
    ids: &[i64],
    next_release: bool,
    actor: &str,
) -> rusqlite::Result<usize> {
    let ids = own(c, project_id, ids)?;
    let release = if next_release {
        latest_release(c, project_id)?
    } else {
        None
    };
    for id in &ids {
        c.execute(
            "UPDATE issues SET status = 'resolved', resolved_at = datetime('now'),
                 resolved_in_release = ?2, ignore_until = NULL, ignore_until_count = NULL
             WHERE id = ?1",
            params![id, release],
        )?;
        match &release {
            Some(r) => log_activity(
                c,
                *id,
                "resolved_in_release",
                Some(actor),
                &json!({ "release": r }),
            )?,
            None => log_activity(c, *id, "resolved", Some(actor), &json!({}))?,
        }
    }
    Ok(ids.len())
}

/// Makes issues unresolved again (from resolved or ignored).
pub fn unresolve(
    c: &Connection,
    project_id: i64,
    ids: &[i64],
    actor: &str,
) -> rusqlite::Result<usize> {
    let ids = own(c, project_id, ids)?;
    for id in &ids {
        c.execute(
            "UPDATE issues SET status = 'unresolved', substatus = 'ongoing', resolved_at = NULL,
                 resolved_in_release = NULL, ignore_until = NULL, ignore_until_count = NULL
             WHERE id = ?1",
            [id],
        )?;
        log_activity(c, *id, "unresolved", Some(actor), &json!({}))?;
    }
    Ok(ids.len())
}

/// Ignores issues forever, until `n` more events, or until a time.
pub fn ignore(
    c: &Connection,
    project_id: i64,
    ids: &[i64],
    how: &Ignore,
    actor: &str,
) -> rusqlite::Result<usize> {
    let ids = own(c, project_id, ids)?;
    for id in &ids {
        let (until, count, data) = match how {
            Ignore::Forever => (None, None, json!({})),
            Ignore::Count(n) => {
                let seen: i64 =
                    c.query_row("SELECT times_seen FROM issues WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })?;
                (None, Some(seen + n), json!({ "count": n }))
            }
            Ignore::Until(t) => (Some(t.clone()), None, json!({ "until": t })),
        };
        c.execute(
            "UPDATE issues SET status = 'ignored', resolved_at = NULL, resolved_in_release = NULL,
                 ignore_until = ?2, ignore_until_count = ?3
             WHERE id = ?1",
            params![id, until, count],
        )?;
        log_activity(c, *id, "ignored", Some(actor), &data)?;
    }
    Ok(ids.len())
}

/// What a merge gathers from one issue.
struct MergeSource {
    id: i64,
    title: String,
    first_seen: String,
    last_seen: String,
    times_seen: i64,
    first_release_id: Option<i64>,
    last_release_id: Option<i64>,
    unsymbolicated: bool,
}

/// Merges issues into the one with the most events (the oldest of those
/// when several have as many): their grouping hashes, events, tag values,
/// users and hourly counts move over and the counts add up; the others are
/// deleted. Returns the issue that is left, `None` with fewer than two.
pub fn merge(
    c: &Connection,
    project_id: i64,
    ids: &[i64],
    actor: &str,
) -> rusqlite::Result<Option<i64>> {
    let ids = own(c, project_id, ids)?;
    if ids.len() < 2 {
        return Ok(None);
    }
    let mut sources: Vec<MergeSource> = Vec::new();
    for id in &ids {
        sources.push(c.query_row(
            "SELECT id, title, first_seen, last_seen, times_seen, first_release_id,
                    last_release_id, unsymbolicated
             FROM issues WHERE id = ?1",
            [id],
            |r| {
                Ok(MergeSource {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    first_seen: r.get(2)?,
                    last_seen: r.get(3)?,
                    times_seen: r.get(4)?,
                    first_release_id: r.get(5)?,
                    last_release_id: r.get(6)?,
                    unsymbolicated: r.get(7)?,
                })
            },
        )?);
    }
    sources.sort_by(|a, b| {
        b.times_seen
            .cmp(&a.times_seen)
            .then(a.first_seen.cmp(&b.first_seen))
            .then(a.id.cmp(&b.id))
    });
    let target = sources.remove(0);
    let t = target.id;
    let mut first = (target.first_seen.clone(), target.first_release_id);
    let mut last = (target.last_seen.clone(), target.last_release_id);
    let mut times = target.times_seen;
    let mut unsym = target.unsymbolicated;
    let mut merged = Vec::new();
    for s in &sources {
        c.execute(
            "UPDATE issue_hashes SET issue_id = ?1 WHERE issue_id = ?2",
            [t, s.id],
        )?;
        c.execute(
            "UPDATE events SET issue_id = ?1 WHERE issue_id = ?2",
            [t, s.id],
        )?;
        c.execute(
            "INSERT INTO issue_tag_values (issue_id, key, value, times_seen, first_seen, last_seen)
             SELECT ?1, key, value, times_seen, first_seen, last_seen FROM issue_tag_values
             WHERE issue_id = ?2
             ON CONFLICT (issue_id, key, value) DO UPDATE SET
                 times_seen = times_seen + excluded.times_seen,
                 first_seen = min(first_seen, excluded.first_seen),
                 last_seen = max(last_seen, excluded.last_seen)",
            [t, s.id],
        )?;
        c.execute(
            "INSERT OR IGNORE INTO issue_users (issue_id, user_hash)
             SELECT ?1, user_hash FROM issue_users WHERE issue_id = ?2",
            [t, s.id],
        )?;
        c.execute(
            "INSERT INTO issue_hours (issue_id, hour, project_id, events)
             SELECT ?1, hour, project_id, events FROM issue_hours WHERE issue_id = ?2
             ON CONFLICT (issue_id, hour) DO UPDATE SET events = events + excluded.events",
            [t, s.id],
        )?;
        if s.first_seen < first.0 || first.1.is_none() && s.first_seen == first.0 {
            first = (s.first_seen.clone(), s.first_release_id.or(first.1));
        }
        if s.last_seen > last.0 || last.1.is_none() && s.last_seen == last.0 {
            last = (s.last_seen.clone(), s.last_release_id.or(last.1));
        }
        times += s.times_seen;
        unsym |= s.unsymbolicated;
        merged.push(json!({ "id": s.id, "title": s.title, "events": s.times_seen }));
        c.execute("DELETE FROM issues WHERE id = ?1", [s.id])?;
    }
    c.execute(
        "UPDATE issues SET times_seen = ?2, first_seen = ?3, first_release_id = ?4,
             last_seen = ?5, last_release_id = ?6, unsymbolicated = ?7,
             users_seen = (SELECT COUNT(*) FROM issue_users WHERE issue_id = ?1)
         WHERE id = ?1",
        params![t, times, first.0, first.1, last.0, last.1, unsym],
    )?;
    log_activity(c, t, "merged", Some(actor), &json!({ "issues": merged }))?;
    Ok(Some(t))
}

/// Removes the details (payloads, attachments, feedback) of the events of
/// issues, from `store.sqlite`.
fn delete_details(c: &Connection, project_id: i64, issue_id: i64) -> rusqlite::Result<()> {
    c.execute(
        "DELETE FROM store.payloads WHERE id IN
             (SELECT payload_id FROM events WHERE issue_id = ?1 AND payload_id IS NOT NULL)",
        [issue_id],
    )?;
    c.execute(
        "DELETE FROM store.attachments WHERE project_id = ?1 AND event_id IN
             (SELECT event_id FROM events WHERE issue_id = ?2)",
        [project_id, issue_id],
    )?;
    c.execute(
        "DELETE FROM store.feedback WHERE project_id = ?1 AND event_id IN
             (SELECT event_id FROM events WHERE issue_id = ?2)",
        [project_id, issue_id],
    )?;
    Ok(())
}

/// Deletes issues with their events, hashes, counts, activity and the
/// events' details. New events with the same grouping make a new issue.
pub fn delete(c: &Connection, project_id: i64, ids: &[i64]) -> rusqlite::Result<usize> {
    let ids = own(c, project_id, ids)?;
    for id in &ids {
        delete_details(c, project_id, *id)?;
        c.execute("DELETE FROM issues WHERE id = ?1", [id])?;
    }
    Ok(ids.len())
}

/// Ends ignoring that ran out by time at `now`; returns how many issues.
pub fn expire_ignored(c: &Connection, now: &str) -> rusqlite::Result<usize> {
    let ids: Vec<i64> = c
        .prepare_cached(
            "SELECT id FROM issues WHERE status = 'ignored' AND ignore_until IS NOT NULL
             AND ignore_until <= ?1",
        )?
        .query_map([now], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &ids {
        c.execute(
            "UPDATE issues SET status = 'unresolved', substatus = 'ongoing', ignore_until = NULL,
                 ignore_until_count = NULL
             WHERE id = ?1",
            [id],
        )?;
        log_activity(c, *id, "unignored", None, &json!({}))?;
    }
    Ok(ids.len())
}

/// New issues first seen, and regressed ones last regressed, more than 7
/// days before `now` become ongoing. Returns how many changed.
pub fn settle_substatus(c: &Connection, now: &str) -> rusqlite::Result<usize> {
    let new = c.execute(
        "UPDATE issues SET substatus = 'ongoing'
         WHERE substatus = 'new' AND first_seen < datetime(?1, '-7 days')",
        [now],
    )?;
    let regressed = c.execute(
        "UPDATE issues SET substatus = 'ongoing'
         WHERE substatus = 'regressed' AND coalesce(
             (SELECT max(created_at) FROM issue_activity a
              WHERE a.issue_id = issues.id AND a.kind = 'regressed'),
             last_seen) < datetime(?1, '-7 days')",
        [now],
    )?;
    Ok(new + regressed)
}

// ---------------------------------------------------------------------------
// Retention (on the writer's connection)
// ---------------------------------------------------------------------------

/// A project's retention settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub event_days: i64,
    pub issue_days: i64,
    pub payloads_per_day: i64,
    /// Debug files neither uploaded nor used for this long go …
    pub dif_days: i64,
    /// … unless one of this many newest releases used them.
    pub dif_releases: i64,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            event_days: 30,
            issue_days: 90,
            payloads_per_day: super::issues::MAX_PAYLOADS_PER_DAY,
            dif_days: 180,
            dif_releases: 5,
        }
    }
}

pub fn retention(c: &Connection, project_id: i64) -> rusqlite::Result<Option<Retention>> {
    c.query_row(
        "SELECT retention_event_days, retention_issue_days, payloads_per_day, retention_dif_days,
             retention_dif_releases
         FROM projects WHERE id = ?1",
        [project_id],
        |r| {
            Ok(Retention {
                event_days: r.get(0)?,
                issue_days: r.get(1)?,
                payloads_per_day: r.get(2)?,
                dif_days: r.get(3)?,
                dif_releases: r.get(4)?,
            })
        },
    )
    .optional()
}

pub fn set_retention(c: &Connection, project_id: i64, r: Retention) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE projects SET retention_event_days = ?2, retention_issue_days = ?3,
             payloads_per_day = ?4, retention_dif_days = ?5, retention_dif_releases = ?6
         WHERE id = ?1",
        params![
            project_id,
            r.event_days,
            r.issue_days,
            r.payloads_per_day,
            r.dif_days,
            r.dif_releases
        ],
    )? > 0)
}

/// Every project's id and retention.
pub fn all_retention(c: &Connection) -> rusqlite::Result<Vec<(i64, Retention)>> {
    c.prepare(
        "SELECT id, retention_event_days, retention_issue_days, payloads_per_day, retention_dif_days,
             retention_dif_releases
         FROM projects ORDER BY id",
    )?
    .query_map([], |r| {
        Ok((
            r.get(0)?,
            Retention {
                event_days: r.get(1)?,
                issue_days: r.get(2)?,
                payloads_per_day: r.get(3)?,
                dif_days: r.get(4)?,
                dif_releases: r.get(5)?,
            },
        ))
    })?
    .collect()
}

/// Removes up to `batch` event details (payloads with their attachments and
/// feedback) of `project_id` that arrived before `cutoff`; the events keep
/// their index rows. Returns how many payloads went.
pub fn prune_details(
    c: &Connection,
    project_id: i64,
    cutoff: &str,
    batch: i64,
) -> rusqlite::Result<usize> {
    let ids: Vec<(i64, String)> = c
        .prepare_cached(
            "SELECT id, event_id FROM store.payloads WHERE project_id = ?1 AND received_at < ?2
             ORDER BY id LIMIT ?3",
        )?
        .query_map(params![project_id, cutoff, batch], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut del = c.prepare_cached("DELETE FROM store.payloads WHERE id = ?1")?;
    let mut unlink = c.prepare_cached(
        "UPDATE events SET payload_id = NULL WHERE project_id = ?1 AND event_id = ?2",
    )?;
    for (id, event_id) in &ids {
        del.execute([id])?;
        unlink.execute(params![project_id, event_id])?;
    }
    c.execute(
        "DELETE FROM store.attachments WHERE project_id = ?1 AND received_at < ?2",
        params![project_id, cutoff],
    )?;
    c.execute(
        "DELETE FROM store.feedback WHERE project_id = ?1 AND received_at < ?2",
        params![project_id, cutoff],
    )?;
    Ok(ids.len())
}

/// Deletes up to `batch` issues of `project_id` last seen before `cutoff`,
/// with their events and details. Returns how many.
pub fn prune_issues(
    c: &Connection,
    project_id: i64,
    cutoff: &str,
    batch: i64,
) -> rusqlite::Result<usize> {
    let ids: Vec<i64> = c
        .prepare_cached(
            "SELECT id FROM issues WHERE project_id = ?1 AND last_seen < ?2 ORDER BY id LIMIT ?3",
        )?
        .query_map(params![project_id, cutoff, batch], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    delete(c, project_id, &ids)
}

/// Deletes a project with everything it has in both databases: issues,
/// events and keys in `kestrel.sqlite` (by cascade), payloads, attachments,
/// session items and feedback in `store.sqlite`.
pub fn delete_project(c: &Connection, project_id: i64) -> rusqlite::Result<bool> {
    for table in ["payloads", "attachments", "session_items", "feedback"] {
        c.execute(
            &format!("DELETE FROM store.{table} WHERE project_id = ?1"),
            [project_id],
        )?;
    }
    Ok(c.execute("DELETE FROM projects WHERE id = ?1", [project_id])? > 0)
}

/// Removes up to `batch` rows per table of `store.sqlite` whose project no
/// longer exists (left by deletions before the store went with them).
pub fn prune_orphans(c: &Connection, batch: i64) -> rusqlite::Result<usize> {
    let mut n = 0;
    for table in ["payloads", "attachments", "session_items", "feedback"] {
        n += c.execute(
            &format!(
                "DELETE FROM store.{table} WHERE id IN (SELECT id FROM store.{table}
                 WHERE project_id NOT IN (SELECT id FROM main.projects) LIMIT ?1)"
            ),
            [batch],
        )?;
    }
    Ok(n)
}

/// Deletes a project's hourly counts older than `cutoff`. Returns how many
/// rows.
pub fn prune_hours(c: &Connection, project_id: i64, cutoff: &str) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM issue_hours WHERE project_id = ?1 AND hour < ?2",
        params![project_id, cutoff],
    )
}
