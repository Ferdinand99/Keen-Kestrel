//! REST API v1: app analytics (docs/design.md §5.3, §6): a time series with
//! daily, weekly and monthly active users, sessions and signals; a
//! breakdown by one dimension; the signal types; a type's payload keys and
//! values; version adoption. For a period and filters (version, platform,
//! type, test signals). Only counts: never a user hash.

use axum::extract::{Extension, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{Map, Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{Id, V1Error, V1Result, json_ok};
use vole_kit::state::Core;

use crate::analytics::app::pages::{self as app, AppView};
use crate::analytics::app::query::{AppPoint, AppTotals};
use crate::analytics::app::rollup::OTHER;
use crate::analytics::app::{APP, AppDim};
use crate::analytics::pages::today;
use crate::analytics::query::Row;
use crate::ingest::Ingest;

/// Values a breakdown returns by default and at most.
const DEFAULT_LIMIT: i64 = 10;
const MAX_LIMIT: i64 = 100;

async fn project(core: &Core, id: i64) -> V1Result<()> {
    core.with_conn(move |c| crate::db::projects::get(c, id))
        .await?
        .map(|_| ())
        .ok_or_else(V1Error::not_found)
}

/// The period and filters of a request, or a validation error.
pub fn view(query: Option<&str>) -> V1Result<AppView> {
    AppView::try_parse(query.unwrap_or(""), today()).map_err(|why| V1Error::field("period", why))
}

fn pairs(q: Option<&str>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.unwrap_or("").as_bytes())
        .into_owned()
        .collect()
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// `limit` (1–`max`, default `default`).
fn limit(pairs: &[(String, String)], default: i64, max: i64) -> V1Result<i64> {
    match get(pairs, "limit") {
        None => Ok(default),
        Some(l) => l
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| V1Error::field("limit", format!("1–{max}"))),
    }
}

pub fn totals_json(t: &AppTotals) -> Value {
    json!({
        "signals": t.signals,
        "sessions": t.sessions,
        "user_days": t.user_days,
        "dau": t.dau,
        "wau": t.wau,
        "mau": t.mau,
    })
}

pub fn filters_json(v: &AppView) -> Value {
    let f = &v.filters;
    json!({
        "version": f.version,
        "platform": f.platform,
        "type": f.kind,
        "include_test": f.include_test,
    })
}

fn point_json(p: &AppPoint) -> Value {
    json!({
        "start": p.start,
        "users": p.users,
        "wau": p.wau,
        "mau": p.mau,
        "sessions": p.sessions,
        "signals": p.signals,
    })
}

pub fn row_json(r: &Row) -> Value {
    json!({
        "value": r.value,
        "label": app::value_label(&r.value),
        "users": r.visitors,
        "count": r.count,
    })
}

fn head(v: &AppView) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("source".into(), APP.into());
    m.insert("period".into(), v.name.clone().into());
    m.insert("from".into(), v.period.from_date().into());
    m.insert("to".into(), v.period.to_date().into());
    m.insert("filters".into(), filters_json(v));
    m
}

/// The time series and totals of `v`, as the API and MCP answer them.
pub async fn timeseries_json(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
) -> V1Result<Value> {
    let data = app::load(core, ingest, project_id, v, false).await?;
    let mut m = head(v);
    m.insert(
        "interval".into(),
        if v.period.hourly() { "hour" } else { "day" }.into(),
    );
    m.insert("totals".into(), totals_json(&data.totals));
    m.insert("current_users".into(), data.current.into());
    m.insert(
        "data".into(),
        data.series
            .iter()
            .map(point_json)
            .collect::<Vec<_>>()
            .into(),
    );
    Ok(Value::Object(m))
}

/// `GET /api/v1/projects/{id}/analytics/app/timeseries`.
pub async fn timeseries(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    RawQuery(q): RawQuery,
) -> V1Result<Response> {
    project(&core, id).await?;
    let v = view(q.as_deref())?;
    let body = timeseries_json(&core, &ingest, id, &v).await?;
    Ok(json_ok(&headers, &body))
}

/// A breakdown as the API and MCP answer it.
pub async fn breakdown_json(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    dim: AppDim,
    limit: i64,
) -> V1Result<Value> {
    let rows = app::top(core, ingest, project_id, v, dim, limit).await?;
    let mut m = head(v);
    m.insert("dimension".into(), dim.code().into());
    m.insert("metric".into(), "signals".into());
    m.insert(
        "data".into(),
        rows.iter().map(row_json).collect::<Vec<_>>().into(),
    );
    Ok(Value::Object(m))
}

/// The dimensions, for error messages.
pub fn dimension_help() -> String {
    let all: Vec<&str> = AppDim::ALL.iter().map(|d| d.code()).collect();
    format!("one of {}", all.join(", "))
}

/// `GET /api/v1/projects/{id}/analytics/app/breakdown?dimension=`.
pub async fn breakdown(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    RawQuery(q): RawQuery,
) -> V1Result<Response> {
    project(&core, id).await?;
    let v = view(q.as_deref())?;
    let pairs = pairs(q.as_deref());
    let dim = get(&pairs, "dimension")
        .and_then(AppDim::parse)
        .ok_or_else(|| V1Error::field("dimension", dimension_help()))?;
    let limit = limit(&pairs, DEFAULT_LIMIT, MAX_LIMIT)?;
    let body = breakdown_json(&core, &ingest, id, &v, dim, limit).await?;
    Ok(json_ok(&headers, &body))
}

/// `GET /api/v1/projects/{id}/analytics/app/signal-types`.
pub async fn signal_types(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    RawQuery(q): RawQuery,
) -> V1Result<Response> {
    project(&core, id).await?;
    let mut v = view(q.as_deref())?;
    v.filters.kind = None;
    let limit = limit(&pairs(q.as_deref()), 50, 500)?;
    let mut body = breakdown_json(&core, &ingest, id, &v, AppDim::Type, limit).await?;
    if let Some(m) = body.as_object_mut() {
        m.remove("dimension");
    }
    Ok(json_ok(&headers, &body))
}

/// A type's payload as the API answers it.
pub async fn payload_json(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    kind: &str,
    limit: usize,
) -> V1Result<Value> {
    let keys = app::payload(core, ingest, project_id, v, kind, limit).await?;
    let mut m = head(v);
    m.insert("type".into(), kind.into());
    m.insert(
        "data".into(),
        keys.iter()
            .map(|(k, rows)| {
                json!({
                    "key": k,
                    "values": rows.iter().map(|r| json!({
                        "value": (r.value != OTHER).then_some(&r.value),
                        "label": app::value_label(&r.value),
                        "count": r.count,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>()
            .into(),
    );
    Ok(Value::Object(m))
}

/// `GET /api/v1/projects/{id}/analytics/app/payload?type=`.
pub async fn payload(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    RawQuery(q): RawQuery,
) -> V1Result<Response> {
    project(&core, id).await?;
    let v = view(q.as_deref())?;
    let pairs = pairs(q.as_deref());
    let kind = get(&pairs, "type")
        .and_then(crate::analytics::app::signal_type)
        .ok_or_else(|| V1Error::field("type", "a signal type, e.g. Paywall.shown"))?;
    let limit = limit(&pairs, DEFAULT_LIMIT, MAX_LIMIT)?;
    let body = payload_json(&core, &ingest, id, &v, &kind, limit as usize).await?;
    Ok(json_ok(&headers, &body))
}

/// `GET /api/v1/projects/{id}/analytics/app/adoption`.
pub async fn adoption(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    RawQuery(q): RawQuery,
) -> V1Result<Response> {
    project(&core, id).await?;
    let v = view(q.as_deref())?;
    let limit = limit(&pairs(q.as_deref()), 5, 20)?;
    let a = app::adoption(&core, &ingest, id, &v, limit).await?;
    let round = |x: f64| (x * 100.0).round() / 100.0;
    let mut m = head(&v);
    m.insert("days".into(), a.days.clone().into());
    m.insert(
        "versions".into(),
        a.versions
            .iter()
            .map(|(version, shares, users)| {
                json!({
                    "version": version,
                    "users": users,
                    "shares": shares.iter().map(|s| round(*s)).collect::<Vec<_>>(),
                    "latest": round(shares.last().copied().unwrap_or(0.0)),
                })
            })
            .collect::<Vec<_>>()
            .into(),
    );
    Ok(json_ok(&headers, &Value::Object(m)))
}
