//! Project groups: the rules the pages, the REST API and MCP share (names,
//! which group a project may be in) and the "Groups" page next to the
//! project list (create, rename, reorder, delete). Everyone signed in sees
//! the groups; editors and admins change them, as with projects.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::SignedCookieJar;
use rusqlite::Connection;
use serde::Deserialize;
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use crate::db::groups::{self as db, Group};
use crate::project;

/// Most groups an instance can have.
pub const MAX_GROUPS: usize = 100;

/// Why a group can't be created or renamed (translation keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    Name(&'static str),
    Taken,
    TooMany,
}

impl GroupError {
    pub fn key(self) -> &'static str {
        match self {
            GroupError::Name(key) => key,
            GroupError::Taken => "groups.err_name_taken",
            GroupError::TooMany => "groups.err_too_many",
        }
    }
}

/// Creates a group after the others. `Ok(Err(_))` for a bad or taken name.
pub fn create(conn: &Connection, raw_name: &str) -> rusqlite::Result<Result<Group, GroupError>> {
    let name = match project::check_name(raw_name) {
        Ok(name) => name,
        Err(key) => return Ok(Err(GroupError::Name(key))),
    };
    if db::name_taken(conn, &name, None)? {
        return Ok(Err(GroupError::Taken));
    }
    if db::list(conn)?.len() >= MAX_GROUPS {
        return Ok(Err(GroupError::TooMany));
    }
    let id = db::insert(conn, &name)?;
    Ok(db::get(conn, id)?.ok_or(GroupError::Taken))
}

/// Renames a group. `Ok(None)`: no such group.
pub fn rename(
    conn: &Connection,
    id: i64,
    raw_name: &str,
) -> rusqlite::Result<Option<Result<Group, GroupError>>> {
    if db::get(conn, id)?.is_none() {
        return Ok(None);
    }
    let name = match project::check_name(raw_name) {
        Ok(name) => name,
        Err(key) => return Ok(Some(Err(GroupError::Name(key)))),
    };
    if db::name_taken(conn, &name, Some(id))? {
        return Ok(Some(Err(GroupError::Taken)));
    }
    db::rename(conn, id, &name)?;
    Ok(db::get(conn, id)?.map(Ok))
}

/// Whether `group_id` (`None`: no group) may be a project's group.
pub fn exists(conn: &Connection, group_id: Option<i64>) -> rusqlite::Result<bool> {
    match group_id {
        None => Ok(true),
        Some(id) => Ok(db::get(conn, id)?.is_some()),
    }
}

// ---------------------------------------------------------------------------
// The "Groups" page
// ---------------------------------------------------------------------------

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/groups", get(page).post(create_group))
        .route("/projects/groups/{id}", post(rename_group))
        .route("/projects/groups/{id}/move", post(move_group))
        .route("/projects/groups/{id}/delete", post(delete_group))
}

struct GroupRow {
    id: i64,
    name: String,
    projects: i64,
    first: bool,
    last: bool,
    /// Unique element ids for the rename sheet's field.
    dom_sfx: String,
}

#[derive(Template)]
#[template(path = "project_groups.html")]
struct GroupsTemplate {
    ctx: PageCtx,
    groups: Vec<GroupRow>,
    name: String,
    name_error: String,
}

const PAGE: &str = "/projects/groups";

fn back(jar: SignedCookieJar, kind: &str, message: &str) -> Response {
    (flash(jar, kind, message), Redirect::to(PAGE)).into_response()
}

async fn render(
    core: &Core,
    user: &CurrentUser,
    jar: SignedCookieJar,
    name: String,
    name_error: String,
) -> ApiResult<Response> {
    let (groups, counts) = core
        .with_conn(|c| Ok((db::list(c)?, db::project_counts(c)?)))
        .await?;
    let n = groups.len();
    let rows = groups
        .into_iter()
        .enumerate()
        .map(|(i, g)| GroupRow {
            projects: counts
                .iter()
                .find(|(id, _)| *id == g.id)
                .map_or(0, |(_, n)| *n),
            dom_sfx: format!("-group-{}", g.id),
            id: g.id,
            name: g.name,
            first: i == 0,
            last: i + 1 == n,
        })
        .collect();
    let invalid = !name_error.is_empty();
    let (ctx, jar) = PageCtx::build(
        core,
        Some(user),
        &jar,
        i18n::t("groups.title"),
        "projects",
        "",
    )
    .await;
    let page = GroupsTemplate {
        ctx,
        groups: rows,
        name,
        name_error,
    };
    let status = if invalid {
        axum::http::StatusCode::UNPROCESSABLE_ENTITY
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, jar, HtmlTemplate(page)).into_response())
}

async fn page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
) -> ApiResult<Response> {
    render(&core, &user, jar, String::new(), String::new()).await
}

#[derive(Deserialize)]
struct NameForm {
    #[serde(default)]
    name: String,
}

async fn create_group(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Form(form): Form<NameForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let name = form.name.clone();
    match core.with_conn(move |c| create(c, &name)).await? {
        Ok(group) => {
            tracing::info!(user = %user.username, group = %group.name, id = group.id, "project group created");
            Ok(back(jar, "success", i18n::t("groups.created")))
        }
        Err(err) => render(&core, &user, jar, form.name, i18n::t(err.key()).to_string()).await,
    }
}

async fn rename_group(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Form(form): Form<NameForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let name = form.name;
    match core
        .with_conn(move |c| rename(c, id, &name))
        .await?
        .ok_or(ApiError::NotFound)?
    {
        Ok(group) => {
            tracing::info!(user = %user.username, group = %group.name, id, "project group renamed");
            Ok(back(jar, "success", i18n::t("groups.renamed")))
        }
        Err(err) => Ok(back(jar, "error", i18n::t(err.key()))),
    }
}

#[derive(Deserialize)]
struct MoveForm {
    #[serde(default)]
    direction: String,
}

async fn move_group(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Form(form): Form<MoveForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let up = match form.direction.as_str() {
        "up" => true,
        "down" => false,
        _ => return Err(ApiError::bad_request("direction is up or down")),
    };
    if !core.with_conn(move |c| db::shift(c, id, up)).await? {
        return Err(ApiError::NotFound);
    }
    Ok(back(jar, "success", i18n::t("groups.moved")))
}

async fn delete_group(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let deleted = core
        .with_conn(move |c| {
            let Some(group) = db::get(c, id)? else {
                return Ok(None);
            };
            db::delete(c, id)?;
            Ok(Some(group))
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    tracing::info!(user = %user.username, group = %deleted.name, id, "project group deleted; its projects are in no group");
    Ok(back(jar, "success", i18n::t("groups.deleted")))
}
