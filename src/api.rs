//! REST API v1 (`/api/v1/**`): projects and their DSN keys, and the route
//! table ([`DOCS`]) the kit turns into `/api-docs`, the OpenAPI document,
//! `/api-docs.md` and `/llms.txt`.
//!
//! Conventions are the kit's (`vole_kit::http::rest`): bearer tokens (read
//! tokens act as viewers), `{"error": {"code", "message", "fields"}}`, lists
//! as `{"data": [...], "next_cursor": ...}` with `?limit=&cursor=`, weak
//! ETags, `201 Created` with `Location`, `204 No Content` on delete.

use std::fmt::Write as _;

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::docs::{DocsSpec, RouteDoc, r};
use vole_kit::http::rest::{
    Body, Id, Params, V1Error, V1Result, created, json_ok, limit, no_content, paginate,
    parse_cursor,
};
use vole_kit::i18n;
use vole_kit::state::Core;

use crate::db::projects::{self as db, KeyChanges, Project, ProjectKey};
use crate::dsn;
use crate::project::{self, Platform};

// ---------------------------------------------------------------------------
// Reference
// ---------------------------------------------------------------------------

const LIMIT_Q: &str = "limit: page size (1–500, default 50)";
const CURSOR_Q: &str = "cursor: next_cursor from the previous page";

const PROJECT_EXAMPLE: &str = r#"{
  "id": 1,
  "name": "Sylo for iOS",
  "slug": "sylo-ios",
  "platform": "apple",
  "created_at": "2026-10-22 08:00:00"
}"#;

const KEY_EXAMPLE: &str = r#"{
  "id": 3,
  "project_id": 1,
  "name": "Default",
  "public_key": "6f1b0c2d9e8a4b7c8d1e2f3a4b5c6d7e",
  "dsn": "https://6f1b0c2d9e8a4b7c8d1e2f3a4b5c6d7e@kestrel.example.com/1",
  "enabled": true,
  "rate_limit_per_minute": null,
  "allowed_origins": [],
  "created_at": "2026-10-22 08:00:00"
}"#;

const ERROR_EXAMPLE: &str = r#"{
  "error": {
    "code": "validation_failed",
    "message": "Enter a name.",
    "fields": {"name": "Enter a name."}
  }
}"#;

/// Every documented endpoint, in display order.
pub static ROUTES: &[RouteDoc] = &[
    // -- Projects --
    r("GET", "/api/v1/projects", "viewer", "Projects", "List projects, oldest first.")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …project… ], "next_cursor": null}"#),
    r("POST", "/api/v1/projects", "editor", "Projects", "Create a project. It gets one enabled key (\"Default\"). The slug is made from the name when you leave it out; platform is apple, javascript, rust, python or other (default).")
        .req(r#"{"name": "Sylo for iOS", "platform": "apple"}"#)
        .res(PROJECT_EXAMPLE),
    r("GET", "/api/v1/projects/{id}", "viewer", "Projects", "Get one project.").res(PROJECT_EXAMPLE),
    r("PATCH", "/api/v1/projects/{id}", "editor", "Projects", "Change a project's name, slug or platform; fields you leave out stay.")
        .req(r#"{"name": "Sylo", "slug": "sylo"}"#)
        .res(PROJECT_EXAMPLE),
    r("DELETE", "/api/v1/projects/{id}", "editor", "Projects", "Delete a project and its keys. Its id is never reused."),
    // -- Keys --
    r("GET", "/api/v1/projects/{id}/keys", "viewer", "Keys", "List a project's DSN keys, oldest first, each with its DSN.")
        .res(r#"{"data": [ …key… ], "next_cursor": null}"#),
    r("POST", "/api/v1/projects/{id}/keys", "editor", "Keys", "Create a key with a new random public key. A project has at most 20 keys.")
        .req(r#"{"name": "Production", "rate_limit_per_minute": 600, "allowed_origins": ["https://sylo.app"]}"#)
        .res(KEY_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/keys/{key_id}", "viewer", "Keys", "Get one key.").res(KEY_EXAMPLE),
    r("PATCH", "/api/v1/projects/{id}/keys/{key_id}", "editor", "Keys", "Change a key: name, enabled, rate_limit_per_minute (null removes the limit) or allowed_origins (an empty list allows any origin).")
        .req(r#"{"enabled": false, "rate_limit_per_minute": null}"#)
        .res(KEY_EXAMPLE),
    r("DELETE", "/api/v1/projects/{id}/keys/{key_id}", "editor", "Keys", "Delete a key. SDKs that use its DSN can no longer send."),
    // -- System --
    r("GET", "/api/v1/openapi.json", "viewer", "System", "This reference as an OpenAPI 3.1 document."),
    r("POST", "/mcp", "api token", "System", "Model Context Protocol server (Streamable HTTP, JSON-RPC 2.0) for AI assistants, with the tool list_projects. Bearer API token; GET answers 405.")
        .req(r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_projects", "arguments": {}}}"#),
    r("GET", "/healthz", "public", "System", "Liveness probe: always \"ok\"."),
];

fn curl_path_param(_route: &RouteDoc, segment: &str) -> Option<String> {
    match segment {
        "{id}" => Some("1".into()),
        "{key_id}" => Some("3".into()),
        _ => None,
    }
}

fn markdown_intro(out: &mut String, base: &str) {
    let version = crate::VERSION;
    let _ = write!(
        out,
        "# Keen Kestrel API\n\n\
         > REST API of a Keen Kestrel instance (version {version}), generated from the server's own route table. \
         Keen Kestrel is a self-hosted, Sentry-compatible error tracker; this version manages projects and their DSN keys.\n\n\
         - Instance: {base}\n\
         - API base URL: {base}/api/v1\n\
         - Browsable reference: {base}/api-docs (sign in) · OpenAPI 3.1: {base}/api/v1/openapi.json\n\
         - The web interface is translated; this document, field names, values and error codes are English.\n\n\
         Examples use two shell variables; never paste a real token into a chat or commit it:\n\n\
         ```bash\n\
         export KK_URL=\"{base}\"\n\
         export KK_TOKEN=\"<API token from Settings → API tokens>\"\n\
         curl -H \"Authorization: Bearer $KK_TOKEN\" \"$KK_URL/api/v1/projects\"\n\
         ```\n\n\
         ## Authentication\n\n\
         - Create a token in the web app under **Settings → API tokens** (`/settings/tokens`). Tokens start with `kk_`, \
         are shown once and can expire (30, 90, 365 days or never).\n\
         - Send it on every request: `Authorization: Bearer $KK_TOKEN`.\n\
         - Scope `read` acts as a **viewer** (every GET); scope `write` acts as the token's owner with their role.\n\
         - Roles: **viewer** reads everything; **editor** and **admin** also create, change and delete projects and keys.\n\
         - API tokens work only under `/api/` and for `/mcp`; they can't open the web interface.\n\
         - A project's DSN (`https://<public key>@<host>/<project id>`) is not an API token: it can only send events.\n\n\
         ## Conventions\n\n\
         - JSON in and out; send bodies with `Content-Type: application/json`.\n\
         - Lists return `{{\"data\": [...], \"next_cursor\": \"42\" | null}}`. Pass `?limit=` (1–500, default 50) and \
         `?cursor=<next_cursor>` for the next page; stop when `next_cursor` is `null`.\n\
         - GETs send a weak `ETag` and answer `If-None-Match` with `304 Not Modified`.\n\
         - Times are UTC strings `YYYY-MM-DD HH:MM:SS`.\n\
         - `POST` that creates returns `201 Created` with a `Location` header; `DELETE` returns `204 No Content`.\n\
         - `PATCH` changes only the fields you send.\n\
         - Cookie-authenticated writes must come from the app's own pages; token requests are exempt.\n\n"
    );
}

fn markdown_middle(out: &mut String) {
    out.push_str(
        "## Projects and keys\n\n\
         - A project is one app or website. Its integer `id` is part of every DSN and is never reused.\n\
         - `slug`: lowercase letters, digits and dashes (at most 50), unique; `sentry-cli` can use it as the project.\n\
         - `platform`: `apple`, `javascript`, `rust`, `python` or `other`. It only picks the setup guide shown first.\n\
         - A key has a random 32-character hex `public_key` and a `dsn` built from the instance's public URL.\n\
         - `enabled: false` makes the key's DSN refused; `rate_limit_per_minute` (1–1,000,000 or `null`) caps the events it may send; \
         `allowed_origins` lists the browser origins (`https://example.com`, or `*`) that may use it, and an empty list allows any.\n\n",
    );
}

fn nothing(_: &mut String) {}

/// What Kestrel contributes to the kit's API reference.
pub static DOCS: DocsSpec = DocsSpec {
    routes: ROUTES,
    openapi_title: "Keen Kestrel API",
    openapi_description: "REST API for Keen Kestrel. Authenticate with `Authorization: Bearer kk_…` (create tokens under Settings → API tokens). Read-only tokens act as viewers. Errors use `{\"error\": {\"code\", \"message\", \"fields\"}}`; lists use `?limit=&cursor=` and return `next_cursor`.",
    bearer_description: "API token (kk_…)",
    agent_token_description: "Not used by Keen Kestrel.",
    error_example: ERROR_EXAMPLE,
    config_file: "kestrel.json",
    agent_token_access: "not used",
    curl_path_param,
    markdown_intro,
    markdown_middle,
    markdown_outro: nothing,
    page_subtitle: "api.docs_subtitle",
    openapi_path: "/api/v1/openapi.json",
    api_base: "/api/v1",
    example_path: "/api/v1/projects",
    metrics_path: None,
};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Protected routes (behind `require_auth`), nested under `/api/v1`.
pub fn router() -> Router<Core> {
    let api = Router::new()
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/projects/{id}",
            get(show_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route("/projects/{id}/keys", get(list_keys).post(create_key))
        .route(
            "/projects/{id}/keys/{key_id}",
            get(show_key).patch(update_key).delete(delete_key),
        )
        .fallback(fallback)
        .route_layer(middleware::from_fn(
            vole_kit::http::rest::session_write_guard,
        ));
    Router::new().nest("/api/v1", api)
}

async fn fallback() -> V1Error {
    V1Error::not_found()
}

fn require_editor(user: &CurrentUser) -> V1Result<()> {
    user.require_editor().map_err(V1Error::from)
}

/// A validation error on `field`, in the request's language.
fn invalid(field: &str, key: &'static str) -> V1Error {
    V1Error::field(field, i18n::t(key))
}

pub fn project_json(p: &Project) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "slug": p.slug,
        "platform": p.platform.as_str(),
        "created_at": p.created_at,
    })
}

fn key_json(k: &ProjectKey, base: &str) -> Value {
    json!({
        "id": k.id,
        "project_id": k.project_id,
        "name": k.name,
        "public_key": k.public_key,
        "dsn": dsn::dsn(base, &k.public_key, k.project_id),
        "enabled": k.enabled,
        "rate_limit_per_minute": k.rate_limit_per_minute,
        "allowed_origins": k.allowed_origins,
        "created_at": k.created_at,
    })
}

/// The `id` of a JSON object (for paging).
fn json_id(v: &Value) -> i64 {
    v["id"].as_i64().unwrap_or_default()
}

async fn load(core: &Core, id: i64) -> V1Result<Project> {
    core.with_conn(move |c| db::get(c, id))
        .await?
        .ok_or_else(V1Error::not_found)
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_projects(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Params(q): Params<ListQuery>,
) -> V1Result<Response> {
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let projects = core.with_conn(|c| db::list(c)).await?;
    let data: Vec<Value> = projects.iter().map(project_json).collect();
    let page = paginate(data, json_id, cursor, limit(q.limit));
    Ok(json_ok(&headers, &page))
}

async fn show_project(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
) -> V1Result<Response> {
    let project = load(&core, id).await?;
    Ok(json_ok(&headers, &project_json(&project)))
}

/// `null` and a missing field differ: `Some(None)` clears.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectInput {
    name: Option<String>,
    slug: Option<String>,
    platform: Option<String>,
}

fn parse_platform(raw: Option<&str>) -> V1Result<Option<Platform>> {
    raw.map(|p| {
        Platform::parse(p).ok_or_else(|| {
            V1Error::field(
                "platform",
                "Use one of: apple, javascript, rust, python, other.",
            )
        })
    })
    .transpose()
}

async fn create_project(
    State(core): State<Core>,
    user: CurrentUser,
    Body(input): Body<ProjectInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let name = project::check_name(input.name.as_deref().unwrap_or_default())
        .map_err(|k| invalid("name", k))?;
    let platform = parse_platform(input.platform.as_deref())?.unwrap_or(Platform::Other);
    let slug = input
        .slug
        .as_deref()
        .map(|s| project::check_slug(s).map_err(|k| invalid("slug", k)))
        .transpose()?;
    let key_name = i18n::t("keys.default_name");
    let stored = name.clone();
    let created_project = core
        .with_conn(move |c| {
            let slug = match slug {
                Some(slug) if db::slug_taken(c, &slug, None)? => return Ok(None),
                Some(slug) => slug,
                None => db::free_slug(c, &project::slugify(&stored))?,
            };
            let id = db::insert(c, &stored, &slug, platform, key_name)?;
            db::get(c, id)
        })
        .await?;
    let Some(p) = created_project else {
        return Err(V1Error::conflict(i18n::t("projects.err_slug_taken")));
    };
    tracing::info!(user = %user.username, project = %name, id = p.id, "project created (API)");
    Ok(created(
        format!("/api/v1/projects/{}", p.id),
        &project_json(&p),
    ))
}

async fn update_project(
    State(core): State<Core>,
    user: CurrentUser,
    Id(id): Id<i64>,
    Body(input): Body<ProjectInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let current = load(&core, id).await?;
    let name = match input.name.as_deref() {
        Some(n) => project::check_name(n).map_err(|k| invalid("name", k))?,
        None => current.name,
    };
    let slug = match input.slug.as_deref() {
        Some(s) => project::check_slug(s).map_err(|k| invalid("slug", k))?,
        None => current.slug,
    };
    let platform = parse_platform(input.platform.as_deref())?.unwrap_or(current.platform);
    let updated = core
        .with_conn(move |c| {
            if db::slug_taken(c, &slug, Some(id))? {
                return Ok(None);
            }
            db::update(c, id, &name, &slug, platform)?;
            db::get(c, id)
        })
        .await?;
    let Some(p) = updated else {
        return Err(V1Error::conflict(i18n::t("projects.err_slug_taken")));
    };
    Ok(axum::Json(project_json(&p)).into_response())
}

async fn delete_project(
    State(core): State<Core>,
    user: CurrentUser,
    Id(id): Id<i64>,
) -> V1Result<Response> {
    require_editor(&user)?;
    if !core.with_conn(move |c| db::delete(c, id)).await? {
        return Err(V1Error::not_found());
    }
    tracing::info!(user = %user.username, id, "project deleted (API)");
    Ok(no_content())
}

async fn list_keys(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Params(q): Params<ListQuery>,
) -> V1Result<Response> {
    load(&core, id).await?;
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let keys = core.with_conn(move |c| db::keys(c, id)).await?;
    let base = dsn::public_base(core.base_url().await, &headers);
    let data: Vec<Value> = keys.iter().map(|k| key_json(k, &base)).collect();
    let page = paginate(data, json_id, cursor, limit(q.limit));
    Ok(json_ok(&headers, &page))
}

async fn show_key(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, key_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    let key = core
        .with_conn(move |c| db::get_key(c, id, key_id))
        .await?
        .ok_or_else(V1Error::not_found)?;
    let base = dsn::public_base(core.base_url().await, &headers);
    Ok(json_ok(&headers, &key_json(&key, &base)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInput {
    name: Option<String>,
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    rate_limit_per_minute: Option<Option<i64>>,
    allowed_origins: Option<Vec<String>>,
}

fn key_changes(input: KeyInput) -> V1Result<KeyChanges> {
    Ok(KeyChanges {
        name: input
            .name
            .as_deref()
            .map(|n| project::check_name(n).map_err(|k| invalid("name", k)))
            .transpose()?,
        enabled: input.enabled,
        rate_limit_per_minute: input
            .rate_limit_per_minute
            .map(|l| project::check_rate_limit(l).map_err(|k| invalid("rate_limit_per_minute", k)))
            .transpose()?,
        allowed_origins: input
            .allowed_origins
            .as_ref()
            .map(|o| {
                project::check_origins(o.iter().map(String::as_str))
                    .map_err(|k| invalid("allowed_origins", k))
            })
            .transpose()?,
    })
}

async fn create_key(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Body(input): Body<KeyInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    load(&core, id).await?;
    let mut changes = key_changes(input)?;
    let name = changes
        .name
        .take()
        .unwrap_or_else(|| i18n::t("keys.default_name").to_string());
    let key = core
        .with_conn(move |c| {
            if db::keys(c, id)?.len() >= project::MAX_KEYS {
                return Ok(None);
            }
            let key_id = db::insert_key(c, id, &name)?;
            db::update_key(c, id, key_id, &changes)?;
            db::get_key(c, id, key_id)
        })
        .await?;
    let Some(key) = key else {
        return Err(V1Error::conflict(i18n::t("keys.err_too_many")));
    };
    let base = dsn::public_base(core.base_url().await, &headers);
    Ok(created(
        format!("/api/v1/projects/{id}/keys/{}", key.id),
        &key_json(&key, &base),
    ))
}

async fn update_key(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    Id((id, key_id)): Id<(i64, i64)>,
    Body(input): Body<KeyInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let changes = key_changes(input)?;
    let key = core
        .with_conn(move |c| {
            if !db::update_key(c, id, key_id, &changes)? {
                return Ok(None);
            }
            db::get_key(c, id, key_id)
        })
        .await?
        .ok_or_else(V1Error::not_found)?;
    let base = dsn::public_base(core.base_url().await, &headers);
    Ok(axum::Json(key_json(&key, &base)).into_response())
}

async fn delete_key(
    State(core): State<Core>,
    user: CurrentUser,
    Id((id, key_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    require_editor(&user)?;
    if !core
        .with_conn(move |c| db::delete_key(c, id, key_id))
        .await?
    {
        return Err(V1Error::not_found());
    }
    Ok(no_content())
}
