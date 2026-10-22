//! REST API v1 for errors: a project's issues (the search syntax of the
//! issue list), one issue, its status (resolve, resolve in the next
//! release, ignore, unresolve), merging and deleting, an issue's events and
//! an event by its id.
//!
//! Issue and event lists are newest first; their `next_cursor` is an
//! opaque position (pass it back as `?cursor=`).

use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{
    Body, Id, Params, V1Error, V1Result, json_ok, limit, no_content, parse_cursor,
};
use vole_kit::state::Core;

use crate::db::triage::{self, EventRow, IssueRow, ListParams};
use crate::event::normalize;
use crate::ingest::Ingest;
use crate::issues::search::{self, Sort, Status};
use crate::issues::{Action, IgnoreFor};

/// An issue as the API (and the MCP tools) show it.
pub fn issue_json(i: &IssueRow) -> Value {
    json!({
        "id": i.id,
        "project_id": i.project_id,
        "title": i.title,
        "culprit": i.culprit,
        "level": i.level,
        "platform": i.platform,
        "status": i.status,
        "substatus": i.substatus,
        "first_seen": i.first_seen,
        "last_seen": i.last_seen,
        "events": i.times_seen,
        "users": i.users_seen,
        "first_release": i.first_release,
        "last_release": i.last_release,
        "resolved_in_release": i.resolved_in_release,
        "ignore_until": i.ignore_until,
        "ignore_until_count": i.ignore_until_count,
        "grouping_config": i.grouping_config,
        "unsymbolicated": i.unsymbolicated,
        "path": format!("/projects/{}/issues/{}", i.project_id, i.id),
    })
}

/// An event's index row (and its payload, when asked and still stored).
pub fn event_json(project_id: i64, issue_id: i64, e: &EventRow, payload: Option<Value>) -> Value {
    let mut v = json!({
        "event_id": e.event_id,
        "issue_id": issue_id,
        "project_id": project_id,
        "timestamp": e.timestamp,
        "received_at": e.received_at,
        "level": e.level,
        "release": e.release,
        "environment": e.environment,
        "has_details": e.payload_id.is_some(),
        "path": format!("/projects/{project_id}/issues/{issue_id}/events/{}", e.event_id),
    });
    if let Some(p) = payload {
        v["payload"] = p;
    }
    v
}

async fn project(core: &Core, id: i64) -> V1Result<()> {
    core.with_conn(move |c| crate::db::projects::get(c, id))
        .await?
        .map(|_| ())
        .ok_or_else(V1Error::not_found)
}

async fn issue(core: &Core, id: i64, issue_id: i64) -> V1Result<IssueRow> {
    core.with_conn(move |c| triage::get(c, id, issue_id))
        .await?
        .ok_or_else(V1Error::not_found)
}

/// One page of issues for a search (the issue list's syntax).
pub struct IssuePage {
    pub issues: Vec<IssueRow>,
    pub total: i64,
    pub next_offset: Option<i64>,
    /// Search terms that couldn't be understood.
    pub unknown: Vec<String>,
}

/// Searches a project's issues: `query` in the search syntax, `status` when
/// the query names none (unresolved by default), `sort`
/// (`last_seen`, `first_seen`, `events`, `users`).
pub async fn search_issues(
    core: &Core,
    project_id: i64,
    query: &str,
    status: Option<&str>,
    sort: Option<&str>,
    limit: i64,
    offset: i64,
) -> V1Result<IssuePage> {
    let q = query.trim().chars().take(500).collect::<String>();
    let parsed = search::parse(&q);
    let status = match parsed.status {
        Some(s) => s,
        None => match status {
            Some(raw) => Status::parse(raw).ok_or_else(|| {
                V1Error::field("status", "Use one of: unresolved, resolved, ignored, all.")
            })?,
            None => Status::Unresolved,
        },
    };
    let sort = match sort {
        Some(raw) => Sort::parse(raw).ok_or_else(|| {
            V1Error::field("sort", "Use one of: last_seen, first_seen, events, users.")
        })?,
        None => Sort::default(),
    };
    let unknown = parsed.unknown.clone();
    let (issues, total) = core
        .with_conn(move |c| {
            triage::list(
                c,
                &ListParams {
                    project_id,
                    status: status.column(),
                    query: &parsed,
                    sort,
                    limit,
                    offset,
                },
            )
        })
        .await?;
    let next_offset = (offset + limit < total).then_some(offset + limit);
    Ok(IssuePage {
        issues,
        total,
        next_offset,
        unknown,
    })
}

#[derive(Deserialize)]
pub struct IssueQuery {
    query: Option<String>,
    q: Option<String>,
    status: Option<String>,
    sort: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

/// `GET /api/v1/projects/{id}/issues`.
pub async fn list(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Params(q): Params<IssueQuery>,
) -> V1Result<Response> {
    project(&core, id).await?;
    let offset = parse_cursor(q.cursor.as_deref())?.unwrap_or(0).max(0);
    let text = q.query.or(q.q).unwrap_or_default();
    let page = search_issues(
        &core,
        id,
        &text,
        q.status.as_deref(),
        q.sort.as_deref(),
        limit(q.limit),
        offset,
    )
    .await?;
    let data: Vec<Value> = page.issues.iter().map(issue_json).collect();
    Ok(json_ok(
        &headers,
        &json!({
            "data": data,
            "next_cursor": page.next_offset.map(|o| o.to_string()),
            "total": page.total,
            "unknown_terms": page.unknown,
        }),
    ))
}

/// `GET /api/v1/projects/{id}/issues/{issue_id}`.
pub async fn show(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, issue_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    let i = issue(&core, id, issue_id).await?;
    let hashes = core.with_conn(move |c| triage::hashes(c, issue_id)).await?;
    let mut v = issue_json(&i);
    v["grouping"] = Value::Array(
        hashes
            .iter()
            .map(|h| json!({"hash": h.hash, "config": h.grouping_config, "components": h.components}))
            .collect(),
    );
    Ok(json_ok(&headers, &v))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusInput {
    status: String,
    #[serde(default)]
    in_next_release: bool,
    ignore_events: Option<i64>,
    ignore_hours: Option<i64>,
}

/// The action a status change asks for.
fn action_of(input: &StatusInput) -> V1Result<Action> {
    match input.status.trim() {
        "resolved" if input.in_next_release => Ok(Action::ResolveNextRelease),
        "resolved" => Ok(Action::Resolve),
        "unresolved" => Ok(Action::Unresolve),
        "ignored" => match (input.ignore_events, input.ignore_hours) {
            (None, None) => Ok(Action::Ignore(IgnoreFor::Forever)),
            (Some(n), None) if (1..=1_000_000).contains(&n) => {
                Ok(Action::Ignore(IgnoreFor::Events(n)))
            }
            (None, Some(h)) if (1..=8760).contains(&h) => Ok(Action::Ignore(IgnoreFor::Hours(h))),
            (Some(_), Some(_)) => Err(V1Error::field(
                "ignore_events",
                "Send ignore_events or ignore_hours, not both.",
            )),
            (Some(_), None) => Err(V1Error::field(
                "ignore_events",
                "Use a whole number from 1 to 1000000.",
            )),
            (None, Some(_)) => Err(V1Error::field(
                "ignore_hours",
                "Use a whole number from 1 to 8760.",
            )),
        },
        _ => Err(V1Error::field(
            "status",
            "Use one of: resolved, unresolved, ignored.",
        )),
    }
}

/// Runs a status action on one issue as `user`; returns the issue after.
pub async fn set_status(
    core: &Core,
    ingest: &Ingest,
    user: &CurrentUser,
    project_id: i64,
    issue_id: i64,
    action: Action,
) -> V1Result<IssueRow> {
    issue(core, project_id, issue_id).await?;
    crate::issues::apply(
        ingest,
        project_id,
        vec![issue_id],
        action.clone(),
        user.username.clone(),
    )
    .await?;
    tracing::info!(user = %user.username, project = project_id, issue = issue_id, ?action, "issue changed (API)");
    issue(core, project_id, issue_id).await
}

/// `PATCH /api/v1/projects/{id}/issues/{issue_id}`.
pub async fn update(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    Id((id, issue_id)): Id<(i64, i64)>,
    Body(input): Body<StatusInput>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    let action = action_of(&input)?;
    let i = set_status(&core, &ingest, &user, id, issue_id, action).await?;
    Ok(axum::Json(issue_json(&i)).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeInput {
    ids: Vec<i64>,
}

/// `POST /api/v1/projects/{id}/issues/merge`.
pub async fn merge(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    Id(id): Id<i64>,
    Body(input): Body<MergeInput>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    project(&core, id).await?;
    let mut ids = input.ids;
    ids.sort_unstable();
    ids.dedup();
    if ids.len() < 2 || ids.len() > 500 {
        return Err(V1Error::field("ids", "Send from 2 to 500 issue ids."));
    }
    let (_, left) =
        crate::issues::apply(&ingest, id, ids, Action::Merge, user.username.clone()).await?;
    let Some(left) = left else {
        return Err(V1Error::field(
            "ids",
            "At least two of the ids must be issues of this project.",
        ));
    };
    tracing::info!(user = %user.username, project = id, issue = left, "issues merged (API)");
    let i = issue(&core, id, left).await?;
    Ok(axum::Json(issue_json(&i)).into_response())
}

/// `DELETE /api/v1/projects/{id}/issues/{issue_id}`.
pub async fn delete(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    Id((id, issue_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    issue(&core, id, issue_id).await?;
    crate::issues::apply(
        &ingest,
        id,
        vec![issue_id],
        Action::Delete,
        user.username.clone(),
    )
    .await?;
    tracing::info!(user = %user.username, project = id, issue = issue_id, "issue deleted (API)");
    Ok(no_content())
}

#[derive(Deserialize)]
pub struct PageQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// `GET /api/v1/projects/{id}/issues/{issue_id}/events`.
pub async fn events(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, issue_id)): Id<(i64, i64)>,
    Params(q): Params<PageQuery>,
) -> V1Result<Response> {
    issue(&core, id, issue_id).await?;
    let offset = parse_cursor(q.cursor.as_deref())?.unwrap_or(0).max(0);
    let size = limit(q.limit);
    let (rows, total) = core
        .with_conn(move |c| triage::events(c, issue_id, size, offset))
        .await?;
    let data: Vec<Value> = rows
        .iter()
        .map(|e| event_json(id, issue_id, e, None))
        .collect();
    let next = (offset + size < total).then(|| (offset + size).to_string());
    Ok(json_ok(
        &headers,
        &json!({"data": data, "next_cursor": next, "total": total}),
    ))
}

/// `GET /api/v1/projects/{id}/issues/{issue_id}/events/latest`.
pub async fn latest_event(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, issue_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    issue(&core, id, issue_id).await?;
    let row = core
        .with_conn(move |c| triage::latest_event(c, issue_id))
        .await?
        .ok_or_else(V1Error::not_found)?;
    let payload = match row.payload_id {
        Some(p) => crate::issues::load_payload(&ingest, p).await,
        None => None,
    };
    Ok(json_ok(&headers, &event_json(id, issue_id, &row, payload)))
}

/// `GET /api/v1/projects/{id}/events/{event_id}`.
pub async fn event(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, event_id)): Id<(i64, String)>,
) -> V1Result<Response> {
    project(&core, id).await?;
    let event_id = normalize::event_id(&event_id).ok_or_else(V1Error::not_found)?;
    let found = core
        .with_conn(move |c| {
            use rusqlite::OptionalExtension as _;
            let issue_id: Option<i64> = c
                .query_row(
                    "SELECT issue_id FROM events WHERE project_id = ?1 AND event_id = ?2",
                    rusqlite::params![id, event_id],
                    |r| r.get(0),
                )
                .optional()?;
            match issue_id {
                Some(issue_id) => Ok(triage::event(c, issue_id, &event_id)?.map(|e| (issue_id, e))),
                None => Ok(None),
            }
        })
        .await?;
    let (issue_id, row) = found.ok_or_else(V1Error::not_found)?;
    let payload = match row.payload_id {
        Some(p) => crate::issues::load_payload(&ingest, p).await,
        None => None,
    };
    Ok(json_ok(&headers, &event_json(id, issue_id, &row, payload)))
}
