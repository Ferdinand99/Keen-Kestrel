//! The project's Debug files page (`/projects/{id}/debug-files`): the
//! uploaded files, how to upload them (sentry-cli, an Xcode build phase,
//! fastlane, a zip), the tokens that may upload, and deleting files.
//! Everyone signed in reads it; editors and admins delete files and make
//! upload tokens.

use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use vole_kit::auth::{CurrentUser, TokenScope, tokens};
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use crate::db::difs as db;
use crate::db::projects::{self, Project};
use crate::dsn;
use crate::ingest::Ingest;
use crate::pages::ProjectView;

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/{id}/debug-files", get(page))
        .route("/projects/{id}/debug-files/tokens", post(create_token))
        .route("/projects/{id}/debug-files/{dif_id}/delete", post(delete))
}

/// One uploaded debug file as the list shows it.
pub struct DifRow {
    pub id: i64,
    pub debug_id: String,
    pub object_name: String,
    pub arch: String,
    pub kind: String,
    pub size: String,
    pub has_debug_info: bool,
    pub uploaded_at: String,
    pub last_used_at: Option<String>,
    pub release: Option<String>,
}

/// A token that may upload debug files.
pub struct TokenRow {
    pub name: String,
    pub prefix: String,
    pub username: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
}

/// One way to upload, with its code.
pub struct Recipe {
    pub id: &'static str,
    pub label: &'static str,
    pub intro_key: &'static str,
    pub code: String,
}

#[derive(Template)]
#[template(path = "debug_files.html")]
struct DebugFilesTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    files: Vec<DifRow>,
    tokens: Vec<TokenRow>,
    /// A token made just now, shown once.
    new_token: Option<String>,
    recipes: Vec<Recipe>,
    settings_href: String,
    tokens_href: &'static str,
}

/// `1.6 MB`, `840 KB`, `512 B`.
pub fn size_label(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{} GB", i18n::fmt_dec(b / (1024.0 * 1024.0 * 1024.0), 1))
    } else if b >= 1024.0 * 1024.0 {
        format!("{} MB", i18n::fmt_dec(b / (1024.0 * 1024.0), 1))
    } else if b >= 1024.0 {
        format!("{} KB", i18n::fmt_dec(b / 1024.0, 0))
    } else {
        format!("{} B", i18n::fmt_int(bytes.max(0)))
    }
}

/// The ways to upload, with this instance's address and the project.
pub fn recipes(base: &str, project: &Project) -> Vec<Recipe> {
    let slug = &project.slug;
    let id = project.id;
    vec![
        Recipe {
            id: "cli",
            label: "sentry-cli",
            intro_key: "difs.recipe_cli",
            code: format!(
                "export SENTRY_AUTH_TOKEN=kk_…\n\
                 sentry-cli --url {base} debug-files upload \\\n  \
                 --org any --project {slug} --wait \\\n  \
                 path/to/MyApp.app.dSYM"
            ),
        },
        Recipe {
            id: "xcode",
            label: "Xcode",
            intro_key: "difs.recipe_xcode",
            code: format!(
                "# Build Phases → New Run Script Phase (after \"Compile Sources\").\n\
                 # Input files: ${{DWARF_DSYM_FOLDER_PATH}}/${{DWARF_DSYM_FILE_NAME}}\n\
                 # Keep the token out of the project, e.g. in ~/.kestrel-token.\n\
                 if [ \"$CONFIGURATION\" = \"Release\" ] && which sentry-cli >/dev/null; then\n  \
                 export SENTRY_URL=\"{base}\"\n  \
                 export SENTRY_ORG=any\n  \
                 export SENTRY_PROJECT=\"{slug}\"\n  \
                 export SENTRY_AUTH_TOKEN=\"$(cat \"$HOME/.kestrel-token\")\"\n  \
                 sentry-cli debug-files upload \"$DWARF_DSYM_FOLDER_PATH\" || echo \"warning: dSYM upload to Keen Kestrel failed\"\n\
                 fi"
            ),
        },
        Recipe {
            id: "fastlane",
            label: "fastlane",
            intro_key: "difs.recipe_fastlane",
            code: format!(
                "sentry_debug_files_upload(\n  \
                 url: \"{base}\",\n  \
                 auth_token: ENV[\"KESTREL_TOKEN\"],\n  \
                 org_slug: \"any\",\n  \
                 project_slug: \"{slug}\"\n\
                 )"
            ),
        },
        Recipe {
            id: "zip",
            label: "zip",
            intro_key: "difs.recipe_zip",
            code: format!(
                "ditto -c -k --keepParent MyApp.app.dSYM MyApp.dSYM.zip\n\
                 curl -H \"Authorization: Bearer $KESTREL_TOKEN\" \\\n  \
                 -H \"Content-Type: application/zip\" \\\n  \
                 --data-binary @MyApp.dSYM.zip \\\n  \
                 {base}/api/v1/projects/{id}/dsyms"
            ),
        },
    ]
}

async fn load(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| projects::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

fn rows(files: Vec<db::DifFile>) -> Vec<DifRow> {
    files
        .into_iter()
        .map(|d| DifRow {
            has_debug_info: d.has_debug_info(),
            size: size_label(d.size),
            id: d.id,
            debug_id: d.debug_id,
            object_name: d.object_name,
            arch: d.arch,
            kind: d.kind,
            uploaded_at: d.uploaded_at,
            last_used_at: d.last_used_at,
            release: d.release,
        })
        .collect()
}

async fn render(
    core: &Core,
    user: &CurrentUser,
    jar: SignedCookieJar,
    headers: &HeaderMap,
    project: Project,
    new_token: Option<String>,
) -> ApiResult<Response> {
    let id = project.id;
    // Editors see every upload token; the others only their own.
    let owner = (!user.can_edit()).then_some(user.id);
    let (files, granted) = core
        .with_conn(move |c| Ok((db::list(c, id)?, db::upload_tokens(c, owner)?)))
        .await?;
    let base = dsn::public_base(core.base_url().await, headers);
    let title = i18n::tf("difs.title_of", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(core, Some(user), &jar, title, "projects", "").await;
    let page = DebugFilesTemplate {
        ctx,
        tab: "debug-files",
        files: rows(files),
        tokens: granted
            .into_iter()
            .map(|t| TokenRow {
                name: t.name,
                prefix: t.prefix,
                username: t.username,
                created_at: t.created_at,
                last_used_at: t.last_used_at,
                expires_at: t.expires_at,
            })
            .collect(),
        new_token,
        recipes: recipes(&base, &project),
        settings_href: format!("/projects/{id}/settings#retention"),
        tokens_href: "/settings/tokens",
        project: ProjectView::from(&project),
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

async fn page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let project = load(&core, id).await?;
    render(&core, &user, jar, &headers, project, None).await
}

#[derive(Deserialize)]
struct TokenForm {
    #[serde(default)]
    name: String,
}

/// Makes an API token that may upload debug files (and only read
/// otherwise), and shows it once.
async fn create_token(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<TokenForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load(&core, id).await?;
    let name: String = match form.name.trim() {
        "" => i18n::tf("difs.token_default_name", &[("project", &project.slug)]),
        n => n.chars().take(100).collect(),
    };
    let token = tokens::generate();
    let (hash, prefix) = (tokens::hash(&token), tokens::display_prefix(&token));
    let uid = user.id;
    let stored = name.clone();
    core.with_conn(move |c| {
        let tx = c.transaction()?;
        let token_id = vole_kit::db::api_tokens::insert(
            &tx,
            uid,
            &stored,
            &prefix,
            &hash,
            TokenScope::Read,
            None,
        )?;
        db::grant(&tx, token_id, db::DIFS_WRITE)?;
        tx.commit()
    })
    .await?;
    tracing::info!(user = %user.username, token = %name, "debug file upload token created");
    render(&core, &user, jar, &headers, project, Some(token)).await
}

async fn delete(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, dif_id)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    user.require_editor()?;
    load(&core, id).await?;
    if !super::upload::delete(&core, ingest.difs(), id, dif_id).await? {
        return Err(ApiError::NotFound);
    }
    tracing::info!(user = %user.username, project = id, dif = dif_id, "debug file deleted");
    Ok((
        flash(jar, "success", i18n::t("difs.deleted")),
        Redirect::to(&format!("/projects/{id}/debug-files")),
    )
        .into_response())
}
