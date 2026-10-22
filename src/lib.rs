//! Keen Kestrel: self-hosted, Sentry-compatible error tracking and
//! privacy-friendly analytics, built on vole-kit.
//!
//! This is the skeleton: sign-in, users, API tokens, Security and the API
//! reference come from the kit; Kestrel adds projects and their DSN keys
//! (pages, REST API, one MCP tool). Ingest comes next (docs/design.md §11).

#![forbid(unsafe_code)]

pub mod api;
pub mod config;
pub mod db;
pub mod dsn;
pub mod mcp;
pub mod pages;
pub mod product;
pub mod project;
pub mod server;

use std::path::Path;

use axum::Router;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use vole_kit::auth::{guard, pages as kit_pages};
use vole_kit::db::settings;
use vole_kit::state::Core;

pub use product::{VERSION, install};

/// Why Kestrel could not start.
pub type StartError = Box<dyn std::error::Error + Send + Sync>;

/// Installs the configuration, opens and migrates the database in
/// `data_dir` and returns the state. `public_url` (`KK_PUBLIC_URL`) is kept in
/// the `base_url` setting, which the kit's same-origin checks, API examples
/// and DSNs read; without one the setting is cleared.
pub async fn start(data_dir: &Path, public_url: Option<&str>) -> Result<Core, StartError> {
    install();
    std::fs::create_dir_all(data_dir).map_err(|e| {
        format!(
            "can't create the data directory {}: {e}",
            data_dir.display()
        )
    })?;
    let pool = vole_kit::db::build_pool(&data_dir.join(product::DB_FILE));
    let report = vole_kit::db::migrate(&pool, &data_dir.join("backups")).await?;
    if !report.applied.is_empty() {
        tracing::info!(applied = ?report.applied, "database migrated");
    }
    let http = reqwest::Client::builder()
        .user_agent(product::USER_AGENT)
        .build()?;
    let core = Core {
        db: pool,
        session_key: vole_kit::auth::load_or_create_key(data_dir),
        http,
        cfg: &product::KIT,
    };
    let public_url = public_url.map(str::to_string);
    core.with_conn(move |c| settings::set_opt(c, settings::BASE_URL, public_url.as_deref()))
        .await?;
    Ok(core)
}

/// The whole app: Kestrel's pages and API, the kit's pages, static files,
/// MCP and the health probe, behind the kit's middleware.
pub fn router(core: Core) -> Router {
    let protected = Router::new()
        .merge(pages::router())
        .merge(api::router())
        .merge(kit_pages::router())
        .route_layer(from_fn_with_state(core.clone(), guard::require_auth));
    let public = Router::new()
        .route("/healthz", get(vole_kit::http::healthz))
        .route("/static/{*path}", get(vole_kit::http::assets::static_asset))
        .route("/mcp", post(mcp::endpoint).get(vole_kit::mcp::not_offered))
        .merge(kit_pages::public_router());
    protected
        .merge(public)
        .fallback(vole_kit::http::error::not_found)
        .layer(from_fn_with_state(core.clone(), guard::csrf_guard))
        .layer(from_fn_with_state(core.clone(), vole_kit::i18n::layer))
        .layer(from_fn(vole_kit::http::error::negotiate))
        .layer(from_fn(vole_kit::http::headers::security_headers))
        .with_state(core)
}
