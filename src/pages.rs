//! The project pages: list, create, overview (DSN and setup snippets) and
//! settings (rename, keys, delete). Everyone signed in can read them;
//! editors and admins change things.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use crate::db::projects::{self as db, KeyChanges, Project, ProjectKey};
use crate::dsn::{self, Snippet};
use crate::project::{self, Platform};

pub fn router() -> Router<Core> {
    Router::new()
        .route("/", get(home))
        .route("/settings", get(settings_home))
        .route("/projects", get(list).post(create))
        .route("/projects/new", get(new_form))
        .route("/projects/{id}", get(overview))
        .route("/projects/{id}/settings", get(settings).post(save_settings))
        .route("/projects/{id}/delete", post(delete))
        .route("/projects/{id}/keys", post(create_key))
        .route("/projects/{id}/keys/{key_id}", post(update_key))
        .route("/projects/{id}/keys/{key_id}/delete", post(delete_key))
}

async fn home() -> Redirect {
    Redirect::to("/projects")
}

/// The settings tab everyone can see.
async fn settings_home(user: CurrentUser) -> Redirect {
    Redirect::to(if user.is_admin() {
        "/settings/users"
    } else {
        "/settings/security"
    })
}

// ---------------------------------------------------------------------------
// View models
// ---------------------------------------------------------------------------

struct ProjectRow {
    id: i64,
    name: String,
    slug: String,
    platform: &'static str,
    icon: &'static str,
    enabled_keys: i64,
}

struct ProjectView {
    id: i64,
    name: String,
    slug: String,
    platform: &'static str,
    created_at: String,
}

impl From<&Project> for ProjectView {
    fn from(p: &Project) -> Self {
        ProjectView {
            id: p.id,
            name: p.name.clone(),
            slug: p.slug.clone(),
            platform: p.platform.label(),
            created_at: p.created_at.clone(),
        }
    }
}

struct PlatformChoice {
    value: &'static str,
    label: &'static str,
    icon: &'static str,
    selected: bool,
}

fn platform_choices(selected: Platform) -> Vec<PlatformChoice> {
    Platform::ALL
        .into_iter()
        .map(|p| PlatformChoice {
            value: p.as_str(),
            label: p.label(),
            icon: p.icon(),
            selected: p == selected,
        })
        .collect()
}

struct KeyView {
    id: i64,
    name: String,
    public_key: String,
    dsn: String,
    enabled: bool,
    rate_limit: Option<i64>,
    rate_limit_value: String,
    origins: Vec<String>,
    origins_text: String,
    created_at: String,
    /// Unique element ids per key.
    dom_id: String,
    dom_sfx: String,
}

impl KeyView {
    fn new(k: ProjectKey, base: &str) -> Self {
        KeyView {
            dsn: dsn::dsn(base, &k.public_key, k.project_id),
            dom_id: format!("dsn-{}", k.id),
            dom_sfx: format!("-key-{}", k.id),
            rate_limit_value: k
                .rate_limit_per_minute
                .map(|n| n.to_string())
                .unwrap_or_default(),
            origins_text: k.allowed_origins.join("\n"),
            name: if k.name.trim().is_empty() {
                i18n::t("keys.unnamed").to_string()
            } else {
                k.name
            },
            id: k.id,
            public_key: k.public_key,
            enabled: k.enabled,
            rate_limit: k.rate_limit_per_minute,
            origins: k.allowed_origins,
            created_at: k.created_at,
        }
    }
}

#[derive(Template)]
#[template(path = "projects.html")]
struct ListTemplate {
    ctx: PageCtx,
    projects: Vec<ProjectRow>,
}

#[derive(Template)]
#[template(path = "project_new.html")]
struct NewTemplate {
    ctx: PageCtx,
    name: String,
    name_error: String,
    platforms: Vec<PlatformChoice>,
}

#[derive(Template)]
#[template(path = "project.html")]
struct OverviewTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    dsn: Option<String>,
    snippets: Vec<Snippet>,
    first_tab: &'static str,
    settings_href: String,
}

#[derive(Template)]
#[template(path = "project_settings.html")]
struct SettingsTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    form_name: String,
    form_slug: String,
    name_error: String,
    slug_error: String,
    platforms: Vec<PlatformChoice>,
    keys: Vec<KeyView>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn load(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| db::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

fn back_to(jar: SignedCookieJar, kind: &str, message: &str, to: String) -> Response {
    (flash(jar, kind, message), Redirect::to(&to)).into_response()
}

fn settings_url(id: i64) -> String {
    format!("/projects/{id}/settings")
}

fn keys_url(id: i64) -> String {
    format!("/projects/{id}/settings#keys")
}

// ---------------------------------------------------------------------------
// List and create
// ---------------------------------------------------------------------------

async fn list(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
) -> ApiResult<Response> {
    let (projects, counts) = core
        .with_conn(|c| Ok((db::list(c)?, db::key_counts(c)?)))
        .await?;
    let mut rows: Vec<ProjectRow> = projects
        .into_iter()
        .map(|p| ProjectRow {
            enabled_keys: counts
                .iter()
                .find(|(id, _, _)| *id == p.id)
                .map_or(0, |(_, enabled, _)| *enabled),
            platform: p.platform.label(),
            icon: p.platform.icon(),
            id: p.id,
            name: p.name,
            slug: p.slug,
        })
        .collect();
    rows.sort_by_key(|r| r.name.to_lowercase());
    let (ctx, jar) = PageCtx::build(
        &core,
        Some(&user),
        &jar,
        i18n::t("projects.title"),
        "projects",
        "",
    )
    .await;
    Ok((
        jar,
        HtmlTemplate(ListTemplate {
            ctx,
            projects: rows,
        }),
    )
        .into_response())
}

async fn new_form(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
) -> ApiResult<Response> {
    user.require_editor()?;
    let (ctx, jar) = PageCtx::build(
        &core,
        Some(&user),
        &jar,
        i18n::t("projects.new"),
        "projects",
        "",
    )
    .await;
    let page = NewTemplate {
        ctx,
        name: String::new(),
        name_error: String::new(),
        platforms: platform_choices(Platform::Apple),
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

#[derive(Deserialize)]
struct CreateForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    platform: String,
}

async fn create(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Form(form): Form<CreateForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let platform = Platform::parse(&form.platform).unwrap_or(Platform::Other);
    let name = match project::check_name(&form.name) {
        Ok(name) => name,
        Err(key) => {
            let (ctx, jar) = PageCtx::build(
                &core,
                Some(&user),
                &jar,
                i18n::t("projects.new"),
                "projects",
                "",
            )
            .await;
            let page = NewTemplate {
                ctx,
                name: form.name,
                name_error: i18n::t(key).to_string(),
                platforms: platform_choices(platform),
            };
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, jar, HtmlTemplate(page)).into_response());
        }
    };
    let key_name = i18n::t("keys.default_name");
    let stored = name.clone();
    let id = core
        .with_conn(move |c| {
            let slug = db::free_slug(c, &project::slugify(&stored))?;
            db::insert(c, &stored, &slug, platform, key_name)
        })
        .await?;
    tracing::info!(user = %user.username, project = %name, id, "project created");
    Ok(back_to(
        jar,
        "success",
        i18n::t("projects.created"),
        format!("/projects/{id}"),
    ))
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

async fn overview(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let project = load(&core, id).await?;
    let keys = core.with_conn(move |c| db::keys(c, id)).await?;
    let base = dsn::public_base(core.base_url().await, &headers);
    let dsn = keys
        .iter()
        .find(|k| k.enabled)
        .map(|k| dsn::dsn(&base, &k.public_key, id));
    let snippets = dsn.as_deref().map(dsn::snippets).unwrap_or_default();
    let (ctx, jar) = PageCtx::build(
        &core,
        Some(&user),
        &jar,
        project.name.clone(),
        "projects",
        "",
    )
    .await;
    let page = OverviewTemplate {
        ctx,
        tab: "overview",
        first_tab: dsn::first_snippet(project.platform),
        project: ProjectView::from(&project),
        dsn,
        snippets,
        settings_href: keys_url(id),
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// The form's state when it is shown again with errors.
#[derive(Default)]
struct Attempt {
    name: String,
    slug: String,
    platform: Option<Platform>,
    name_error: String,
    slug_error: String,
}

async fn render_settings(
    core: &Core,
    user: &CurrentUser,
    jar: SignedCookieJar,
    headers: &HeaderMap,
    project: Project,
    attempt: Option<Attempt>,
) -> ApiResult<Response> {
    let id = project.id;
    let keys = core.with_conn(move |c| db::keys(c, id)).await?;
    let base = dsn::public_base(core.base_url().await, headers);
    let invalid = attempt.is_some();
    let attempt = attempt.unwrap_or_else(|| Attempt {
        name: project.name.clone(),
        slug: project.slug.clone(),
        ..Attempt::default()
    });
    let title = i18n::tf("project.settings_title", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(core, Some(user), &jar, title, "projects", "").await;
    let page = SettingsTemplate {
        ctx,
        tab: "settings",
        form_name: attempt.name,
        form_slug: attempt.slug,
        name_error: attempt.name_error,
        slug_error: attempt.slug_error,
        platforms: platform_choices(attempt.platform.unwrap_or(project.platform)),
        keys: keys.into_iter().map(|k| KeyView::new(k, &base)).collect(),
        project: ProjectView::from(&project),
    };
    let status = if invalid {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    Ok((status, jar, HtmlTemplate(page)).into_response())
}

async fn settings(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let project = load(&core, id).await?;
    render_settings(&core, &user, jar, &headers, project, None).await
}

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    platform: String,
}

async fn save_settings(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<SettingsForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load(&core, id).await?;
    let platform = Platform::parse(&form.platform).unwrap_or(project.platform);
    let name = project::check_name(&form.name);
    let mut slug = project::check_slug(&form.slug);
    if let Ok(s) = &slug {
        let s = s.clone();
        if core
            .with_conn(move |c| db::slug_taken(c, &s, Some(id)))
            .await?
        {
            slug = Err("projects.err_slug_taken");
        }
    }
    let (name, slug) = match (name, slug) {
        (Ok(name), Ok(slug)) => (name, slug),
        (name, slug) => {
            let attempt = Attempt {
                name: form.name,
                slug: form.slug,
                platform: Some(platform),
                name_error: name.err().map(i18n::t).unwrap_or_default().to_string(),
                slug_error: slug.err().map(i18n::t).unwrap_or_default().to_string(),
            };
            return render_settings(&core, &user, jar, &headers, project, Some(attempt)).await;
        }
    };
    core.with_conn(move |c| db::update(c, id, &name, &slug, platform))
        .await?;
    tracing::info!(user = %user.username, id, "project updated");
    Ok(back_to(
        jar,
        "success",
        i18n::t("project.saved"),
        settings_url(id),
    ))
}

async fn delete(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load(&core, id).await?;
    core.with_conn(move |c| db::delete(c, id)).await?;
    tracing::info!(user = %user.username, project = %project.name, id, "project deleted");
    Ok(back_to(
        jar,
        "success",
        i18n::t("project.deleted"),
        "/projects".into(),
    ))
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct NewKeyForm {
    #[serde(default)]
    name: String,
}

async fn create_key(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Form(form): Form<NewKeyForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    load(&core, id).await?;
    let name = match project::check_name(&form.name) {
        Ok(name) => name,
        Err(key) => return Ok(back_to(jar, "error", i18n::t(key), keys_url(id))),
    };
    let count = core.with_conn(move |c| db::keys(c, id)).await?.len();
    if count >= project::MAX_KEYS {
        return Ok(back_to(
            jar,
            "error",
            i18n::t("keys.err_too_many"),
            keys_url(id),
        ));
    }
    core.with_conn(move |c| db::insert_key(c, id, &name))
        .await?;
    tracing::info!(user = %user.username, project = id, "project key created");
    Ok(back_to(
        jar,
        "success",
        i18n::t("keys.created"),
        keys_url(id),
    ))
}

#[derive(Deserialize)]
struct KeyForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    rate_limit: String,
    #[serde(default)]
    allowed_origins: String,
}

/// The changes a key form asks for, or the translation key of the problem.
fn key_changes(form: &KeyForm) -> Result<KeyChanges, &'static str> {
    let name = project::check_name(&form.name)?;
    let limit = match form.rate_limit.trim() {
        "" => None,
        text => Some(
            text.replace([',', '.', ' ', '\u{a0}', '\u{202f}'], "")
                .parse::<i64>()
                .map_err(|_| "keys.err_rate_limit")?,
        ),
    };
    let limit = project::check_rate_limit(limit)?;
    let origins = project::check_origins(form.allowed_origins.lines())?;
    Ok(KeyChanges {
        name: Some(name),
        enabled: Some(form.enabled),
        rate_limit_per_minute: Some(limit),
        allowed_origins: Some(origins),
    })
}

async fn update_key(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, key_id)): Path<(i64, i64)>,
    Form(form): Form<KeyForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let changes = match key_changes(&form) {
        Ok(changes) => changes,
        Err(key) => return Ok(back_to(jar, "error", i18n::t(key), keys_url(id))),
    };
    let found = core
        .with_conn(move |c| db::update_key(c, id, key_id, &changes))
        .await?;
    if !found {
        return Err(ApiError::NotFound);
    }
    tracing::info!(user = %user.username, project = id, key = key_id, "project key updated");
    Ok(back_to(jar, "success", i18n::t("keys.saved"), keys_url(id)))
}

async fn delete_key(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, key_id)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let found = core
        .with_conn(move |c| db::delete_key(c, id, key_id))
        .await?;
    if !found {
        return Err(ApiError::NotFound);
    }
    tracing::info!(user = %user.username, project = id, key = key_id, "project key deleted");
    Ok(back_to(
        jar,
        "success",
        i18n::t("keys.deleted"),
        keys_url(id),
    ))
}
