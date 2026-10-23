//! sentry-cli's release commands against Kestrel (docs/design.md §4.5:
//! "releases are created on first sight; `sentry-cli releases
//! new/finalize` works against a stub").
//!
//! `releases new`, `finalize`, `info`, `list`, `set-commits` and `deploys
//! new` get answers in Sentry's shape. A release is created (or found) per
//! named project; `finalize` notes `dateReleased`. Commits, refs and deploys
//! are accepted and not kept: Kestrel has no repository integration, and
//! the organization has no repositories (so `set-commits --auto` has nothing
//! to match; `--local` and `--commit` are accepted).
//!
//! Like sentry-cli's other routes these take API tokens only. Reading needs
//! any token; creating and changing needs one that may upload debug files
//! (an editor's read-and-write token, or a token with the `difs:write`
//! scope), which is what a build machine has.

use axum::Router;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::AuthMethod;
use vole_kit::auth::CurrentUser;
use vole_kit::state::Core;

use crate::db::projects::{self, Project};
use crate::db::releases as db;
use crate::difs::upload::may_upload;

pub fn router() -> Router<Core> {
    let org = get(org_list).post(org_create);
    let org_one = get(org_info).put(org_update);
    let proj = get(project_list).post(project_create);
    let proj_one = get(project_info).put(project_update);
    let deploys = get(deploy_list).post(deploy_create);
    Router::new()
        .route("/api/0/organizations/{org}/releases/", org.clone())
        .route("/api/0/organizations/{org}/releases", org)
        .route(
            "/api/0/organizations/{org}/releases/{version}/",
            org_one.clone(),
        )
        .route("/api/0/organizations/{org}/releases/{version}", org_one)
        .route(
            "/api/0/organizations/{org}/releases/{version}/deploys/",
            deploys.clone(),
        )
        .route(
            "/api/0/organizations/{org}/releases/{version}/deploys",
            deploys,
        )
        .route("/api/0/organizations/{org}/repos/", get(repos))
        .route("/api/0/organizations/{org}/repos", get(repos))
        .route("/api/0/projects/{org}/{project}/releases/", proj.clone())
        .route("/api/0/projects/{org}/{project}/releases", proj)
        .route(
            "/api/0/projects/{org}/{project}/releases/{version}/",
            proj_one.clone(),
        )
        .route(
            "/api/0/projects/{org}/{project}/releases/{version}",
            proj_one,
        )
}

type Method = Option<Extension<AuthMethod>>;

fn detail(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(json!({ "detail": msg }))).into_response()
}

fn not_found() -> Response {
    detail(
        StatusCode::NOT_FOUND,
        "The requested resource does not exist",
    )
}

/// `None` when the token may create and change releases, else the refusal.
fn may_write(user: &CurrentUser, method: Method) -> Option<Response> {
    match may_upload(user, method.map(|Extension(m)| m), false) {
        Ok(()) => None,
        Err(_) => Some(detail(
            StatusCode::FORBIDDEN,
            "this token can't manage releases: use one with the difs:write scope \
             (Debug files page) or a read-and-write token of an editor",
        )),
    }
}

fn iso(time: &str) -> String {
    format!("{}Z", time.replace(' ', "T"))
}

/// A release as sentry-cli reads it (`ReleaseInfo`), across the projects
/// that have it.
fn release_json(version: &str, found: &[(Project, db::Release)]) -> Value {
    let created = found
        .iter()
        .map(|(_, r)| r.first_seen.clone())
        .min()
        .unwrap_or_default();
    let released = found
        .iter()
        .filter_map(|(_, r)| r.date_released.clone())
        .max();
    let last = found.iter().map(|(_, r)| r.last_seen.clone()).max();
    let new_groups: i64 = found.iter().map(|(_, r)| r.new_issues).sum();
    json!({
        "version": version,
        "shortVersion": version,
        "ref": null,
        "url": null,
        "dateCreated": iso(&created),
        "dateReleased": released.as_deref().map(iso),
        "dateStarted": null,
        "lastEvent": last.as_deref().map(iso),
        "newGroups": new_groups,
        "commitCount": 0,
        "lastCommit": null,
        "deployCount": 0,
        "lastDeploy": null,
        "authors": [],
        "data": {},
        "projects": found.iter().map(|(p, _)| json!({"id": p.id, "slug": p.slug, "name": p.name})).collect::<Vec<_>>(),
    })
}

/// A project by slug or numeric id.
fn project_by(c: &rusqlite::Connection, segment: &str) -> rusqlite::Result<Option<Project>> {
    match segment.trim().parse::<i64>() {
        Ok(id) if id > 0 => projects::get(c, id),
        _ => projects::by_slug(c, segment.trim()),
    }
}

/// The release `version` in each project that has it (`only`: just these).
fn find(
    c: &rusqlite::Connection,
    version: &str,
    only: Option<&[Project]>,
    now: &str,
) -> rusqlite::Result<Vec<(Project, db::Release)>> {
    let candidates = match only {
        Some(list) => list.to_vec(),
        None => projects::list(c)?,
    };
    let mut out = Vec::new();
    for p in candidates {
        if let Some(id) = db::id_of(c, p.id, version)?
            && let Some(r) = db::get(c, p.id, id, now)?
        {
            out.push((p, r));
        }
    }
    Ok(out)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ReleaseBody {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    projects: Option<Vec<String>>,
    #[serde(default)]
    date_released: Option<String>,
}

/// `YYYY-MM-DD HH:MM:SS` of an RFC 3339 time, else now.
fn sql_time(raw: Option<&str>) -> String {
    raw.and_then(|r| r.parse::<jiff::Timestamp>().ok())
        .map(|t| crate::event::normalize::sql_time(crate::event::normalize::ts_seconds(t)))
        .unwrap_or_else(crate::issues::now)
}

fn check_version(v: &str) -> Option<String> {
    let v = v.trim();
    (!v.is_empty() && v.len() <= 200 && !v.contains(['/', '\n', '\r']) && v != "." && v != "..")
        .then(|| v.to_string())
}

/// Creates or updates `version` in `projects` (slugs or ids; every project
/// that has it when `None`). Returns the answer.
async fn upsert(
    core: &Core,
    version: String,
    projects: Option<Vec<String>>,
    released: Option<String>,
    create: bool,
) -> Response {
    let Some(version) = check_version(&version) else {
        return detail(StatusCode::BAD_REQUEST, "invalid release version");
    };
    let result = core
        .with_conn(move |c| {
            let now = crate::issues::now();
            let named = match &projects {
                Some(list) => {
                    let mut found = Vec::new();
                    for slug in list {
                        match project_by(c, slug)? {
                            Some(p) => found.push(p),
                            None => return Ok(Err(format!("unknown project {slug}"))),
                        }
                    }
                    Some(found)
                }
                None => None,
            };
            let targets: Vec<Project> = match named {
                Some(list) => list,
                None => find(c, &version, None, &now)?
                    .into_iter()
                    .map(|(p, _)| p)
                    .collect(),
            };
            if targets.is_empty() {
                return Ok(Err(if create {
                    "name at least one project".to_string()
                } else {
                    String::new()
                }));
            }
            let released = released.as_deref().map(|r| sql_time(Some(r)));
            for p in &targets {
                db::ensure(c, p.id, &version, &now, released.as_deref())?;
            }
            Ok(Ok(release_json(
                &version,
                &find(c, &version, Some(&targets), &now)?,
            )))
        })
        .await;
    match result {
        Ok(Ok(json)) => {
            let status = if create {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, axum::Json(json)).into_response()
        }
        Ok(Err(msg)) if msg.is_empty() => not_found(),
        Ok(Err(msg)) => detail(StatusCode::BAD_REQUEST, &msg),
        Err(e) => e.into_response(),
    }
}

/// `POST /api/0/organizations/{org}/releases/` (`releases new`).
async fn org_create(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    axum::Json(body): axum::Json<ReleaseBody>,
) -> Response {
    if let Some(refused) = may_write(&user, method) {
        return refused;
    }
    let version = body.version.unwrap_or_default();
    tracing::info!(user = %user.username, %version, "release created (sentry-cli)");
    upsert(
        &core,
        version,
        Some(body.projects.unwrap_or_default()),
        body.date_released,
        true,
    )
    .await
}

/// `PUT /api/0/organizations/{org}/releases/{version}/` (`finalize`,
/// `set-commits`).
async fn org_update(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    Path((_org, version)): Path<(String, String)>,
    axum::Json(body): axum::Json<ReleaseBody>,
) -> Response {
    if let Some(refused) = may_write(&user, method) {
        return refused;
    }
    upsert(&core, version, body.projects, body.date_released, false).await
}

/// `GET /api/0/organizations/{org}/releases/{version}/` (`releases info`).
async fn org_info(
    State(core): State<Core>,
    _user: CurrentUser,
    Path((_org, version)): Path<(String, String)>,
) -> Response {
    info(&core, version, None).await
}

async fn info(core: &Core, version: String, project: Option<String>) -> Response {
    let result = core
        .with_conn(move |c| {
            let now = crate::issues::now();
            let only = match project {
                Some(slug) => match project_by(c, &slug)? {
                    Some(p) => Some(vec![p]),
                    None => return Ok(None),
                },
                None => None,
            };
            let found = find(c, &version, only.as_deref(), &now)?;
            Ok((!found.is_empty()).then(|| release_json(&version, &found)))
        })
        .await;
    match result {
        Ok(Some(json)) => axum::Json(json).into_response(),
        Ok(None) => not_found(),
        Err(e) => e.into_response(),
    }
}

/// Every release of the given projects (or all), newest first.
async fn listing(core: &Core, project: Option<String>) -> Response {
    let result = core
        .with_conn(move |c| {
            let now = crate::issues::now();
            let targets = match project {
                Some(slug) => match project_by(c, &slug)? {
                    Some(p) => vec![p],
                    None => return Ok(None),
                },
                None => projects::list(c)?,
            };
            let mut versions: Vec<(i64, String)> = Vec::new();
            for p in &targets {
                for r in db::all(c, p.id, &now)? {
                    if !versions.iter().any(|(_, v)| *v == r.version) {
                        versions.push((r.id, r.version));
                    }
                }
            }
            versions.sort_by_key(|v| std::cmp::Reverse(v.0));
            let mut out = Vec::new();
            for (_, v) in versions.iter().take(100) {
                out.push(release_json(v, &find(c, v, Some(&targets), &now)?));
            }
            Ok(Some(out))
        })
        .await;
    match result {
        Ok(Some(list)) => axum::Json(list).into_response(),
        Ok(None) => not_found(),
        Err(e) => e.into_response(),
    }
}

async fn org_list(State(core): State<Core>, _user: CurrentUser) -> Response {
    listing(&core, None).await
}

async fn project_list(
    State(core): State<Core>,
    _user: CurrentUser,
    Path((_org, project)): Path<(String, String)>,
) -> Response {
    listing(&core, Some(project)).await
}

/// `POST /api/0/projects/{org}/{project}/releases/` (older sentry-cli).
async fn project_create(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    Path((_org, project)): Path<(String, String)>,
    axum::Json(body): axum::Json<ReleaseBody>,
) -> Response {
    if let Some(refused) = may_write(&user, method) {
        return refused;
    }
    upsert(
        &core,
        body.version.unwrap_or_default(),
        Some(vec![project]),
        body.date_released,
        true,
    )
    .await
}

async fn project_info(
    State(core): State<Core>,
    _user: CurrentUser,
    Path((_org, project, version)): Path<(String, String, String)>,
) -> Response {
    info(&core, version, Some(project)).await
}

async fn project_update(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    Path((_org, project, version)): Path<(String, String, String)>,
    axum::Json(body): axum::Json<ReleaseBody>,
) -> Response {
    if let Some(refused) = may_write(&user, method) {
        return refused;
    }
    upsert(
        &core,
        version,
        Some(vec![project]),
        body.date_released,
        false,
    )
    .await
}

/// `GET /api/0/organizations/{org}/repos/`: Kestrel knows no repositories.
async fn repos(_user: CurrentUser) -> Response {
    axum::Json(json!([])).into_response()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DeployBody {
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    date_started: Option<String>,
    #[serde(default)]
    date_finished: Option<String>,
}

/// `POST …/releases/{version}/deploys/` (`deploys new`): answered, not kept.
async fn deploy_create(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    Path((_org, version)): Path<(String, String)>,
    axum::Json(body): axum::Json<DeployBody>,
) -> Response {
    if let Some(refused) = may_write(&user, method) {
        return refused;
    }
    let exists = core
        .with_conn(move |c| {
            let now = crate::issues::now();
            Ok(!find(c, &version, None, &now)?.is_empty())
        })
        .await
        .unwrap_or(false);
    if !exists {
        return not_found();
    }
    let finished = iso(&sql_time(body.date_finished.as_deref()));
    (
        StatusCode::CREATED,
        axum::Json(json!({
            "id": "1",
            "environment": body.environment.unwrap_or_else(|| "production".into()),
            "name": body.name,
            "url": body.url,
            "dateStarted": body.date_started,
            "dateFinished": finished,
        })),
    )
        .into_response()
}

async fn deploy_list(_user: CurrentUser) -> Response {
    axum::Json(json!([])).into_response()
}
