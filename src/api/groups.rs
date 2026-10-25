//! `/api/v1/project-groups`: list, create, rename, delete and reorder the
//! groups of the project list. Projects name theirs with `group_id`
//! (`PATCH /api/v1/projects/{id}`).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::rest::{Body, Id, V1Error, V1Result, created, json_ok, no_content};
use vole_kit::i18n;
use vole_kit::state::Core;

use super::require_editor;
use crate::db::groups::{self as db, Group};
use crate::groups::{self, GroupError};

/// A group as JSON, with how many projects are in it.
pub fn group_json(g: &Group, projects: i64) -> Value {
    json!({
        "id": g.id,
        "name": g.name,
        "sort_order": g.sort_order,
        "projects": projects,
        "created_at": g.created_at,
    })
}

/// Every group, in the project list's order, as JSON.
pub fn list_json(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Value>> {
    let counts = db::project_counts(conn)?;
    Ok(db::list(conn)?
        .iter()
        .map(|g| {
            let n = counts
                .iter()
                .find(|(id, _)| *id == g.id)
                .map_or(0, |(_, n)| *n);
            group_json(g, n)
        })
        .collect())
}

fn one_json(conn: &rusqlite::Connection, id: i64) -> rusqlite::Result<Option<Value>> {
    Ok(list_json(conn)?
        .into_iter()
        .find(|g| g["id"].as_i64() == Some(id)))
}

/// A group error as the API's: a bad name is a validation error on `name`,
/// a taken name or too many groups a conflict.
pub fn group_error(err: GroupError) -> V1Error {
    match err {
        GroupError::Name(key) => V1Error::field("name", i18n::t(key)),
        GroupError::Taken | GroupError::TooMany => V1Error::conflict(i18n::t(err.key())),
    }
}

pub async fn list(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
) -> V1Result<Response> {
    let data = core.with_conn(|c| list_json(c)).await?;
    Ok(json_ok(
        &headers,
        &json!({ "data": data, "next_cursor": null }),
    ))
}

pub async fn show(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
) -> V1Result<Response> {
    let group = core
        .with_conn(move |c| one_json(c, id))
        .await?
        .ok_or_else(V1Error::not_found)?;
    Ok(json_ok(&headers, &group))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupInput {
    name: Option<String>,
}

pub async fn create(
    State(core): State<Core>,
    user: CurrentUser,
    Body(input): Body<GroupInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let name = input.name.unwrap_or_default();
    let made = core
        .with_conn(move |c| {
            Ok(match groups::create(c, &name)? {
                Ok(g) => Ok(one_json(c, g.id)?.unwrap_or_else(|| group_json(&g, 0))),
                Err(e) => Err(e),
            })
        })
        .await?
        .map_err(group_error)?;
    let id = made["id"].as_i64().unwrap_or_default();
    tracing::info!(user = %user.username, id, "project group created (API)");
    Ok(created(format!("/api/v1/project-groups/{id}"), &made))
}

pub async fn rename(
    State(core): State<Core>,
    user: CurrentUser,
    Id(id): Id<i64>,
    Body(input): Body<GroupInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let Some(name) = input.name else {
        // Nothing to change.
        let group = core
            .with_conn(move |c| one_json(c, id))
            .await?
            .ok_or_else(V1Error::not_found)?;
        return Ok(axum::Json(group).into_response());
    };
    let renamed = core
        .with_conn(move |c| {
            Ok(match groups::rename(c, id, &name)? {
                None => None,
                Some(Ok(_)) => Some(Ok(one_json(c, id)?.unwrap_or_default())),
                Some(Err(e)) => Some(Err(e)),
            })
        })
        .await?
        .ok_or_else(V1Error::not_found)?
        .map_err(group_error)?;
    Ok(axum::Json(renamed).into_response())
}

pub async fn delete(
    State(core): State<Core>,
    user: CurrentUser,
    Id(id): Id<i64>,
) -> V1Result<Response> {
    require_editor(&user)?;
    if !core.with_conn(move |c| db::delete(c, id)).await? {
        return Err(V1Error::not_found());
    }
    tracing::info!(user = %user.username, id, "project group deleted (API); its projects are in no group");
    Ok(no_content())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderInput {
    ids: Vec<i64>,
}

pub async fn reorder(
    State(core): State<Core>,
    user: CurrentUser,
    Body(input): Body<OrderInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let ids = input.ids;
    let data = core
        .with_conn(move |c| {
            if !db::set_order(c, &ids)? {
                return Ok(None);
            }
            list_json(c).map(Some)
        })
        .await?
        .ok_or_else(|| V1Error::field("ids", i18n::t("groups.err_order")))?;
    Ok(axum::Json(json!({ "data": data, "next_cursor": null })).into_response())
}

/// `group_id` of a project write: `Ok(None)` clears it; an unknown group
/// is a validation error.
pub async fn check_group(core: &Core, group_id: Option<i64>) -> V1Result<Option<i64>> {
    if core.with_conn(move |c| groups::exists(c, group_id)).await? {
        Ok(group_id)
    } else {
        Err(V1Error::field("group_id", i18n::t("groups.err_unknown")))
    }
}
