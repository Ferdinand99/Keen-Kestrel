//! REST API v1: a project's releases with their health (crash-free
//! sessions and users, adoption of the sessions and, with app analytics, of
//! the app's users) and a release's health per day.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{Id, Params, V1Error, V1Result, json_ok, limit, parse_cursor};
use vole_kit::state::Core;

use crate::db::releases::{self as db, Release};
use crate::releases::health::{self, Health};

/// A percentage rounded to two decimals (`null` without data).
fn pct(v: Option<f64>) -> Value {
    v.map_or(Value::Null, |p| json!((p * 100.0).round() / 100.0))
}

pub fn health_json(h: &Health) -> Value {
    json!({
        "sessions": h.sessions,
        "errored": h.errored,
        "crashed": h.crashed,
        "abnormal": h.abnormal,
        "users": h.users,
        "crashed_users": h.crashed_users,
        "crash_free_sessions": pct(h.crash_free_sessions()),
        "crash_free_users": pct(h.crash_free_users()),
    })
}

pub fn release_json(r: &Release) -> Value {
    json!({
        "id": r.id,
        "project_id": r.project_id,
        "version": r.version,
        "first_seen": r.first_seen,
        "last_seen": r.last_seen,
        "date_released": r.date_released,
        "first_crash_at": r.first_crash_at,
        "events": r.events,
        "new_issues": r.new_issues,
        "health": health_json(&r.health),
        "adoption_7d": pct(r.adoption),
        "adoption_users_7d": pct(r.adoption_users),
        "path": format!("/projects/{}/releases/{}", r.project_id, r.id),
    })
}

async fn project(core: &Core, id: i64) -> V1Result<()> {
    core.with_conn(move |c| crate::db::projects::get(c, id))
        .await?
        .map(|_| ())
        .ok_or_else(V1Error::not_found)
}

#[derive(Deserialize)]
pub struct PageQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// `GET /api/v1/projects/{id}/releases`: newest first.
pub async fn list(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Params(q): Params<PageQuery>,
) -> V1Result<Response> {
    project(&core, id).await?;
    let offset = parse_cursor(q.cursor.as_deref())?.unwrap_or(0).max(0);
    let size = limit(q.limit);
    let now = crate::issues::now();
    let (rows, total) = core
        .with_conn(move |c| db::list(c, id, size, offset, &now))
        .await?;
    let data: Vec<Value> = rows.iter().map(release_json).collect();
    let next = (offset + size < total).then(|| (offset + size).to_string());
    Ok(json_ok(
        &headers,
        &json!({"data": data, "next_cursor": next, "total": total}),
    ))
}

/// `GET /api/v1/projects/{id}/releases/{release_id}`.
pub async fn show(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, release_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    let now = crate::issues::now();
    let r = core
        .with_conn(move |c| db::get(c, id, release_id, &now))
        .await?
        .ok_or_else(V1Error::not_found)?;
    Ok(json_ok(&headers, &release_json(&r)))
}

#[derive(Deserialize)]
pub struct HealthQuery {
    days: Option<i64>,
    environment: Option<String>,
}

/// The health of a release over `days` days up to now: totals and per day.
pub async fn release_health(
    core: &Core,
    project_id: i64,
    release_id: i64,
    days: i64,
    environment: Option<String>,
) -> V1Result<Value> {
    let now = crate::issues::now();
    let days = days.clamp(1, crate::retention::ROLLUPS_KEPT_DAYS);
    let env = environment.filter(|e| !e.trim().is_empty());
    let found = core
        .with_conn(move |c| {
            let Some(r) = db::get(c, project_id, release_id, &now)? else {
                return Ok(None);
            };
            let (from, to): (String, String) = c.query_row(
                "SELECT date(?1, ?2), date(?1)",
                rusqlite::params![now, format!("-{} days", days - 1)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let series = match &env {
                None => health::daily(c, r.id, &from, &to)?,
                Some(e) => daily_in(c, r.id, &from, &to, e)?,
            };
            let totals = health::totals(c, r.id, Some(&from), env.as_deref())?;
            let environments: Vec<String> = c
                .prepare(
                    "SELECT DISTINCT environment FROM release_health WHERE release_id = ?1 ORDER BY 1",
                )?
                .query_map([r.id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some((r, series, totals, from, to, environments)))
        })
        .await?;
    let (r, series, totals, from, to, environments) = found.ok_or_else(V1Error::not_found)?;
    Ok(json!({
        "release": {"id": r.id, "version": r.version},
        "from": from,
        "to": to,
        "environments": environments,
        "totals": health_json(&totals),
        "days": series.iter().map(|(day, h)| {
            let mut v = health_json(h);
            v["day"] = Value::from(day.clone());
            v
        }).collect::<Vec<_>>(),
    }))
}

/// [`health::daily`] for one environment.
fn daily_in(
    c: &rusqlite::Connection,
    release_id: i64,
    from: &str,
    to: &str,
    environment: &str,
) -> rusqlite::Result<Vec<(String, Health)>> {
    let all = health::daily(c, release_id, from, to)?;
    let rows: Vec<(String, Health)> = c
        .prepare(
            "SELECT day, sessions, errored, crashed, abnormal, users, crashed_users
             FROM release_health WHERE release_id = ?1 AND environment = ?2 AND day >= ?3 AND day <= ?4",
        )?
        .query_map(rusqlite::params![release_id, environment, from, to], |r| {
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
    Ok(all
        .into_iter()
        .map(|(day, _)| {
            let h = rows
                .iter()
                .find(|(d, _)| *d == day)
                .map(|(_, h)| *h)
                .unwrap_or_default();
            (day, h)
        })
        .collect())
}

/// `GET /api/v1/projects/{id}/releases/{release_id}/health`.
pub async fn show_health(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, release_id)): Id<(i64, i64)>,
    Params(q): Params<HealthQuery>,
) -> V1Result<Response> {
    let v = release_health(&core, id, release_id, q.days.unwrap_or(30), q.environment).await?;
    Ok(json_ok(&headers, &v))
}
