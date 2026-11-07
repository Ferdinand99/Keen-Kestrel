//! REST API v1: `GET /api/v1/summary`, one small read for dashboards and
//! start pages: per project, the issues that need a look, crash-free
//! sessions and today's visitors and app users; and the same added up over
//! the instance.
//!
//! It adds no numbers of its own. Each figure is what an existing read
//! gives, asked once for all projects instead of once per project:
//!
//! * issues: the issue list (`is:unresolved`, `is:new`, `is:regressed`,
//!   sorted by `first_seen`), see [`crate::db::triage::list`];
//! * crash-free sessions: [`crate::releases::health::Health`] summed over
//!   the project's releases, the figure `release_health` gives for one;
//! * visitors and app users today: `analytics_days`, the rollups the
//!   analytics summary reads for the period `today` (UTC).
//!
//! Five statements on one connection, whatever the number of projects.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{Params, V1Error, V1Result, json_ok};
use vole_kit::state::Core;

use super::releases::pct;
use crate::analytics::WEB;
use crate::analytics::app::APP;
use crate::db::projects as db;
use crate::releases::health::Health;

/// The window when `hours` is left out.
pub const DEFAULT_HOURS: i64 = 24;
/// The longest window (a week).
pub const MAX_HOURS: i64 = 168;

#[derive(Deserialize)]
pub struct SummaryQuery {
    hours: Option<i64>,
}

/// `hours`, or the default; 422 `validation_failed` (field `hours`) when it is outside 1–168.
pub fn parse_hours(raw: Option<i64>) -> V1Result<i64> {
    let hours = raw.unwrap_or(DEFAULT_HOURS);
    if (1..=MAX_HOURS).contains(&hours) {
        Ok(hours)
    } else {
        Err(V1Error::field(
            "hours",
            format!("hours must be a whole number from 1 to {MAX_HOURS}."),
        ))
    }
}

/// A project's issue counts.
#[derive(Clone, Copy, Default)]
struct IssueCounts {
    unresolved: i64,
    new: i64,
    regressed: i64,
}

/// Unresolved issues per project, how many of them are new (first seen in
/// the window) and how many regressed in it. A regression is dated like
/// `settle_substatus` dates it: its newest `regressed` activity, else the
/// issue's last event.
const ISSUE_COUNTS: &str = "
    SELECT i.project_id,
           COUNT(*),
           COALESCE(SUM(i.substatus = 'new' AND i.first_seen >= datetime(?1, ?2)), 0),
           COALESCE(SUM(i.substatus = 'regressed' AND COALESCE(
               (SELECT MAX(a.created_at) FROM issue_activity a
                WHERE a.issue_id = i.id AND a.kind = 'regressed'),
               i.last_seen) >= datetime(?1, ?2)), 0)
    FROM issues i WHERE i.status = 'unresolved' GROUP BY i.project_id";

/// Each project's newest unresolved issue by `first_seen`, ties by id: the
/// first row of the issue list sorted `first_seen`.
const NEWEST_ISSUE: &str = "
    SELECT id, project_id, title, level, first_seen FROM (
        SELECT id, project_id, title, level, first_seen,
               ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY first_seen DESC, id DESC) AS rn
        FROM issues WHERE status = 'unresolved')
    WHERE rn = 1";

/// Sessions and crashes per project over the UTC days from the one that
/// holds the window's start (release health is rolled up per UTC day).
const SESSIONS: &str = "
    SELECT project_id, SUM(sessions), SUM(crashed)
    FROM release_health WHERE day >= date(?1, ?2) GROUP BY project_id";

/// Per project: whether web and app analytics have any rollup, and today's
/// visitors and active users (UTC day). Every subquery is a seek on the
/// table's primary key.
fn analytics_query() -> String {
    let any = |source: &str| {
        format!(
            "EXISTS (SELECT 1 FROM analytics_days d WHERE d.project_id = p.id AND d.source = '{source}')"
        )
    };
    let today = |source: &str| {
        format!(
            "(SELECT d.visitors FROM analytics_days d
              WHERE d.project_id = p.id AND d.source = '{source}' AND d.day = date(?1))"
        )
    };
    format!(
        "SELECT p.id, {}, {}, {}, {} FROM projects p",
        any(WEB),
        today(WEB),
        any(APP),
        today(APP)
    )
}

/// Today's count of a project: `None` when it has no rollup of that source
/// at all, else the day's value (0 when the day has none).
fn today_count(any: bool, today: Option<i64>) -> Option<i64> {
    any.then(|| today.unwrap_or(0))
}

/// The summary as of `now` (`YYYY-MM-DD HH:MM:SS`, UTC) over the last
/// `hours` hours. The statements: projects, issue counts, newest issues,
/// sessions, analytics.
pub fn summary_at(c: &Connection, now: &str, hours: i64) -> rusqlite::Result<Value> {
    let back = format!("-{hours} hours");
    let projects = db::list(c)?;

    let counts: HashMap<i64, IssueCounts> = c
        .prepare_cached(ISSUE_COUNTS)?
        .query_map(params![now, back], |r| {
            Ok((
                r.get(0)?,
                IssueCounts {
                    unresolved: r.get(1)?,
                    new: r.get(2)?,
                    regressed: r.get(3)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let newest: HashMap<i64, Value> = c
        .prepare_cached(NEWEST_ISSUE)?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(1)?,
                json!({
                    "id": r.get::<_, i64>(0)?,
                    "title": r.get::<_, String>(2)?,
                    "level": r.get::<_, String>(3)?,
                    "first_seen": r.get::<_, String>(4)?,
                }),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let sessions: HashMap<i64, Health> = c
        .prepare_cached(SESSIONS)?
        .query_map(params![now, back], |r| {
            Ok((
                r.get(0)?,
                Health {
                    sessions: r.get(1)?,
                    crashed: r.get(2)?,
                    ..Health::default()
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    type Today = (Option<i64>, Option<i64>);
    let today: HashMap<i64, Today> = c
        .prepare_cached(&analytics_query())?
        .query_map([now], |r| {
            Ok((
                r.get(0)?,
                (
                    today_count(r.get(1)?, r.get(2)?),
                    today_count(r.get(3)?, r.get(4)?),
                ),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut total = IssueCounts::default();
    let mut total_health = Health::default();
    let (mut visitors, mut app_users) = (0, 0);
    let mut rows = Vec::with_capacity(projects.len());
    for p in &projects {
        let n = counts.get(&p.id).copied().unwrap_or_default();
        let health = sessions.get(&p.id).copied().unwrap_or_default();
        let (web, app) = today.get(&p.id).copied().unwrap_or((None, None));
        total.unresolved += n.unresolved;
        total.new += n.new;
        total.regressed += n.regressed;
        total_health.add(&health);
        visitors += web.unwrap_or(0);
        app_users += app.unwrap_or(0);
        rows.push(json!({
            "id": p.id,
            "slug": p.slug,
            "name": p.name,
            "platform": p.platform.as_str(),
            "new_issues": n.new,
            "regressed_issues": n.regressed,
            "unresolved_issues": n.unresolved,
            "crash_free_sessions": pct(health.crash_free_sessions()),
            "sessions": health.sessions,
            "visitors_today": web,
            "app_users_today": app,
            "newest_issue": newest.get(&p.id).cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(json!({
        "hours": hours,
        "totals": {
            "projects": projects.len(),
            "new_issues": total.new,
            "regressed_issues": total.regressed,
            "unresolved_issues": total.unresolved,
            "crash_free_sessions": pct(total_health.crash_free_sessions()),
            "sessions": total_health.sessions,
            "visitors_today": visitors,
            "app_users_today": app_users,
        },
        "projects": rows,
    }))
}

/// The summary now, as the API (and the MCP tool) answers it.
pub async fn summary_json(core: &Core, hours: i64) -> V1Result<Value> {
    let now = crate::issues::now();
    Ok(core.with_conn(move |c| summary_at(c, &now, hours)).await?)
}

/// `GET /api/v1/summary`.
pub async fn summary(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Params(q): Params<SummaryQuery>,
) -> V1Result<Response> {
    let hours = parse_hours(q.hours)?;
    let body = summary_json(&core, hours).await?;
    Ok(json_ok(&headers, &body))
}
