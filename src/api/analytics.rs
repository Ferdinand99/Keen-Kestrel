//! REST API v1: web analytics (docs/design.md §5.3, §6): a time series with
//! totals and a breakdown by one dimension, for a period and filters. Only
//! counts: never a visitor id.

use axum::extract::{Extension, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{Map, Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{Id, V1Error, V1Result, json_ok};
use vole_kit::state::Core;

use crate::analytics::pages::{self as web, View};
use crate::analytics::query::{Point, Row, Totals};
use crate::analytics::{Dim, WEB};
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
pub fn view(query: Option<&str>) -> V1Result<View> {
    View::try_parse(query.unwrap_or(""), web::today()).map_err(|why| V1Error::field("period", why))
}

pub fn totals_json(t: &Totals) -> Value {
    json!({"visitors": t.visitors, "pageviews": t.pageviews, "events": t.events})
}

pub fn filters_json(v: &View) -> Value {
    let map: Map<String, Value> = v
        .filters
        .pairs()
        .into_iter()
        .map(|(k, val)| (k.to_string(), Value::from(val)))
        .collect();
    Value::Object(map)
}

fn point_json(p: &Point) -> Value {
    json!({"start": p.start, "visitors": p.visitors, "pageviews": p.pageviews, "events": p.events})
}

/// What a breakdown's `count` counts.
pub fn metric(dim: Dim) -> &'static str {
    match dim {
        Dim::Entry => "entries",
        Dim::Event => "events",
        _ => "pageviews",
    }
}

pub fn row_json(dim: Dim, r: &Row) -> Value {
    json!({
        "value": r.value,
        "label": web::value_label(dim, &r.value),
        "visitors": r.visitors,
        "count": r.count,
    })
}

/// The time series and totals of `v`, as the API and MCP answer them.
pub async fn timeseries_json(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &View,
) -> V1Result<Value> {
    let data = web::load(core, ingest, project_id, v, false).await?;
    Ok(json!({
        "source": WEB,
        "period": v.name,
        "from": v.period.from_date(),
        "to": v.period.to_date(),
        "interval": if v.period.hourly() { "hour" } else { "day" },
        "filters": filters_json(v),
        "totals": totals_json(&data.totals),
        "current_visitors": data.current,
        "data": data.series.iter().map(point_json).collect::<Vec<_>>(),
    }))
}

/// `GET /api/v1/projects/{id}/analytics/timeseries`.
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
    v: &View,
    dim: Dim,
    limit: i64,
) -> V1Result<Value> {
    let rows = web::top(core, ingest, project_id, v, dim, limit).await?;
    Ok(json!({
        "source": WEB,
        "dimension": dim.code(),
        "metric": metric(dim),
        "period": v.name,
        "from": v.period.from_date(),
        "to": v.period.to_date(),
        "filters": filters_json(v),
        "data": rows.iter().map(|r| row_json(dim, r)).collect::<Vec<_>>(),
    }))
}

/// `GET /api/v1/projects/{id}/analytics/breakdown?dimension=`.
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
    let pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(q.as_deref().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let get = |k: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    let dim = get("dimension")
        .and_then(Dim::parse)
        .ok_or_else(|| V1Error::field("dimension", dimension_help()))?;
    let limit = match get("limit") {
        None => DEFAULT_LIMIT,
        Some(l) => l
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| V1Error::field("limit", "1–100"))?,
    };
    let body = breakdown_json(&core, &ingest, id, &v, dim, limit).await?;
    Ok(json_ok(&headers, &body))
}

/// The dimensions, for error messages.
pub fn dimension_help() -> String {
    let all: Vec<&str> = Dim::ALL.iter().map(|d| d.code()).collect();
    format!("one of {}", all.join(", "))
}
