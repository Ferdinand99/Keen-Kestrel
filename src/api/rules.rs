//! REST API v1: a project's alert rules, the alerts they sent, and the
//! notification channels rules can send to (names and kinds only: a
//! channel's settings hold secrets and stay on the Notifications page).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{
    Body, Id, Params, V1Error, V1Result, created, json_ok, limit, no_content, paginate,
    parse_cursor,
};
use vole_kit::state::Core;

use crate::alerts::pages::check_rule;
use crate::db::alerts::{self as db, LogEntry, Rule, RuleInput, RuleKind};

pub fn rule_json(r: &Rule) -> Value {
    json!({
        "id": r.id,
        "project_id": r.project_id,
        "name": r.name,
        "kind": r.kind.as_str(),
        "enabled": r.enabled,
        "environment": r.environment,
        "threshold": r.threshold,
        "multiplier": r.multiplier,
        "cooldown_minutes": r.cooldown_minutes,
        "digest_minutes": r.digest_minutes,
        "channel_ids": r.channel_ids,
        "created_at": r.created_at,
        "updated_at": r.updated_at,
    })
}

fn log_json(e: &LogEntry) -> Value {
    json!({
        "id": e.id,
        "rule_id": e.rule_id,
        "rule_name": e.rule_name,
        "channel_id": e.channel_id,
        "channel_name": e.channel_name,
        "issue_id": e.issue_id,
        "kind": e.kind,
        "status": e.status,
        "attempts": e.attempts,
        "error": e.error,
        "created_at": e.created_at,
        "sent_at": e.sent_at,
        "summary": crate::alerts::message::headline(&e.data),
        "issue_title": e.data.get("issue").and_then(|i| i.get("title")).cloned().unwrap_or(Value::Null),
    })
}

async fn project(core: &Core, id: i64) -> V1Result<()> {
    core.with_conn(move |c| crate::db::projects::get(c, id))
        .await?
        .map(|_| ())
        .ok_or_else(V1Error::not_found)
}

fn json_id(v: &Value) -> i64 {
    v["id"].as_i64().unwrap_or_default()
}

#[derive(Deserialize)]
pub struct PageQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// `GET /api/v1/projects/{id}/alert-rules`.
pub async fn list(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Params(q): Params<PageQuery>,
) -> V1Result<Response> {
    project(&core, id).await?;
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let rules = core.with_conn(move |c| db::rules(c, id)).await?;
    let data: Vec<Value> = rules.iter().map(rule_json).collect();
    Ok(json_ok(
        &headers,
        &paginate(data, json_id, cursor, limit(q.limit)),
    ))
}

/// `GET /api/v1/projects/{id}/alert-rules/{rule_id}`.
pub async fn show(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, rule_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    let r = core
        .with_conn(move |c| db::rule(c, id, rule_id))
        .await?
        .ok_or_else(V1Error::not_found)?;
    Ok(json_ok(&headers, &rule_json(&r)))
}

/// A rule's fields as the API takes them; left out means the default (on
/// create) or unchanged (on update).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RuleBody {
    name: Option<String>,
    kind: Option<String>,
    enabled: Option<bool>,
    environment: Option<String>,
    threshold: Option<i64>,
    multiplier: Option<f64>,
    cooldown_minutes: Option<i64>,
    #[serde(default, deserialize_with = "double_option")]
    digest_minutes: Option<Option<i64>>,
    channel_ids: Option<Vec<i64>>,
}

/// `null` and a missing field differ: `Some(None)` clears.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

fn apply(body: RuleBody, mut r: RuleInput) -> V1Result<RuleInput> {
    if let Some(kind) = body.kind {
        r.kind = RuleKind::parse(&kind).ok_or_else(|| {
            V1Error::field(
                "kind",
                "Use one of: new_issue, regression, spike, first_crash.",
            )
        })?;
    }
    if let Some(v) = body.name {
        r.name = v.trim().to_string();
    }
    if let Some(v) = body.enabled {
        r.enabled = v;
    }
    if let Some(v) = body.environment {
        r.environment = v.trim().to_string();
    }
    if let Some(v) = body.threshold {
        r.threshold = v;
    }
    if let Some(v) = body.multiplier {
        r.multiplier = v;
    }
    if let Some(v) = body.cooldown_minutes {
        r.cooldown_minutes = v;
    }
    if let Some(v) = body.digest_minutes {
        r.digest_minutes = v;
    }
    if let Some(v) = body.channel_ids {
        r.channel_ids = v;
    }
    let errors = check_rule(&r);
    if !errors.is_empty() {
        return Err(V1Error::fields(errors));
    }
    Ok(r)
}

/// The ids among `ids` that are no channel.
async fn unknown_channels(core: &Core, ids: Vec<i64>) -> V1Result<Vec<i64>> {
    Ok(core
        .with_conn(move |c| {
            let mut out = Vec::new();
            for id in ids {
                if db::channel(c, id)?.is_none() {
                    out.push(id);
                }
            }
            Ok(out)
        })
        .await?)
}

async fn check_channels(core: &Core, r: &RuleInput) -> V1Result<()> {
    let unknown = unknown_channels(core, r.channel_ids.clone()).await?;
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(V1Error::field(
            "channel_ids",
            format!("No such notification channel: {unknown:?}"),
        ))
    }
}

/// `POST /api/v1/projects/{id}/alert-rules`.
pub async fn create(
    State(core): State<Core>,
    user: CurrentUser,
    Id(id): Id<i64>,
    Body(body): Body<RuleBody>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    project(&core, id).await?;
    let kind = body
        .kind
        .as_deref()
        .and_then(RuleKind::parse)
        .unwrap_or(RuleKind::NewIssue);
    let input = apply(body, RuleInput::new(kind))?;
    check_channels(&core, &input).await?;
    let rule = core
        .with_conn(move |c| {
            let rule_id = db::insert_rule(c, id, &input)?;
            db::rule(c, id, rule_id)
        })
        .await?
        .ok_or_else(|| V1Error::internal("the new rule is gone"))?;
    tracing::info!(user = %user.username, project = id, rule = rule.id, "alert rule created (API)");
    Ok(created(
        format!("/api/v1/projects/{id}/alert-rules/{}", rule.id),
        &rule_json(&rule),
    ))
}

/// `PATCH /api/v1/projects/{id}/alert-rules/{rule_id}`.
pub async fn update(
    State(core): State<Core>,
    user: CurrentUser,
    Id((id, rule_id)): Id<(i64, i64)>,
    Body(body): Body<RuleBody>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    let current = core
        .with_conn(move |c| db::rule(c, id, rule_id))
        .await?
        .ok_or_else(V1Error::not_found)?;
    let input = apply(body, RuleInput::from_rule(&current))?;
    check_channels(&core, &input).await?;
    let rule = core
        .with_conn(move |c| {
            db::update_rule(c, id, rule_id, &input)?;
            db::rule(c, id, rule_id)
        })
        .await?
        .ok_or_else(V1Error::not_found)?;
    Ok(axum::Json(rule_json(&rule)).into_response())
}

/// `DELETE /api/v1/projects/{id}/alert-rules/{rule_id}`.
pub async fn delete(
    State(core): State<Core>,
    user: CurrentUser,
    Id((id, rule_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    if !core
        .with_conn(move |c| db::delete_rule(c, id, rule_id))
        .await?
    {
        return Err(V1Error::not_found());
    }
    tracing::info!(user = %user.username, project = id, rule = rule_id, "alert rule deleted (API)");
    Ok(no_content())
}

/// `GET /api/v1/projects/{id}/alerts`: what the rules sent, newest first.
pub async fn history(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Params(q): Params<PageQuery>,
) -> V1Result<Response> {
    project(&core, id).await?;
    let offset = parse_cursor(q.cursor.as_deref())?.unwrap_or(0).max(0);
    let size = limit(q.limit);
    let (rows, total) = core
        .with_conn(move |c| db::history(c, id, size, offset))
        .await?;
    let next = (offset + size < total).then(|| (offset + size).to_string());
    Ok(json_ok(
        &headers,
        &json!({
            "data": rows.iter().map(log_json).collect::<Vec<_>>(),
            "next_cursor": next,
            "total": total,
        }),
    ))
}

/// `GET /api/v1/notification-channels`: id, name, kind and state of each
/// channel, never its settings.
pub async fn channels(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    Params(q): Params<PageQuery>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let mut list = core.with_conn(|c| db::channels(c)).await?;
    list.sort_by_key(|c| c.id);
    let data: Vec<Value> = list
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "kind": c.kind,
                "active": c.active,
                "language": c.language,
                "created_at": c.created_at,
            })
        })
        .collect();
    Ok(json_ok(
        &headers,
        &paginate(data, json_id, cursor, limit(q.limit)),
    ))
}
