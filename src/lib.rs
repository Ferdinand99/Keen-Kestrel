//! Keen Kestrel: self-hosted, Sentry-compatible error tracking and
//! privacy-friendly analytics, built on vole-kit.
//!
//! Sign-in, users, API tokens, Security and the API reference come from the
//! kit; Kestrel adds projects and their DSN keys, Sentry-compatible ingest
//! (normalisation, privacy scrubbing, grouping into issues, the single batch
//! writer), the issue pages with search and triage, retention, debug files
//! with symbolication of native crashes, alerts through the kit's
//! notification providers, release health and the releases pages, web
//! analytics (the k.js snippet, rollups, the Web tab), app analytics
//! (KestrelKit's signals, the App tab and the signal explorer), and the REST
//! API and MCP tools for all of it (docs/design.md §4–§6).

#![forbid(unsafe_code)]

pub mod alerts;
pub mod analytics;
pub mod api;
pub mod config;
pub mod db;
pub mod difs;
pub mod dsn;
pub mod event;
pub mod groups;
pub mod ingest;
pub mod issues;
pub mod mcp;
pub mod pages;
pub mod product;
pub mod project;
pub mod releases;
pub mod retention;
pub mod server;
pub mod store;
pub mod writer;

use std::path::Path;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use axum::{Extension, Router};
use vole_kit::auth::{guard, pages as kit_pages};
use vole_kit::db::settings;
use vole_kit::state::Core;

pub use product::{VERSION, install};

/// Why Kestrel could not start.
pub type StartError = Box<dyn std::error::Error + Send + Sync>;

/// A running Kestrel: the kit's state, ingest's (writer, store, buckets)
/// and the alerts.
#[derive(Clone)]
pub struct Kestrel {
    pub core: Core,
    pub ingest: ingest::Ingest,
    pub alerts: alerts::Alerts,
}

impl Kestrel {
    /// Waits until everything ingest accepted is written and the alert
    /// rules have seen it (sending happens on its own).
    pub async fn flush(&self) {
        self.ingest.flush().await;
        self.alerts.flush().await;
    }
}

/// [`start_with`] with the default ingest settings.
pub async fn start(data_dir: &Path, public_url: Option<&str>) -> Result<Kestrel, StartError> {
    start_with(data_dir, public_url, ingest::Settings::default()).await
}

/// Installs the configuration, opens and migrates both databases in
/// `data_dir`, starts the writer and returns the state. `public_url`
/// (`KK_PUBLIC_URL`), when given, is stored in the `base_url` setting, which
/// the kit's same-origin checks, API examples and DSNs read; without one
/// the setting keeps what the General page saved.
pub async fn start_with(
    data_dir: &Path,
    public_url: Option<&str>,
    settings: ingest::Settings,
) -> Result<Kestrel, StartError> {
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
    let core = Core::new(
        pool,
        vole_kit::auth::load_or_create_key(data_dir),
        http,
        &product::KIT,
    );
    // Channels saved before secrets were sealed (vole-kit 0.5) get sealed
    // now, once; sealed values are left as they are.
    let key = core.session_key.clone();
    if let Err(err) = core
        .with_conn(move |c| vole_kit::notify::sealed::seal_stored(c, &key))
        .await
    {
        tracing::warn!(?err, "couldn't seal the secrets of notification channels");
    }
    if let Some(url) = public_url.map(str::to_string) {
        core.with_conn(move |c| settings::set(c, settings::BASE_URL, &url))
            .await?;
    }
    // The Hedgerow app channel's push client: once per process, before any
    // alert can fire. (The product's slug names it to the relay.)
    vole_kit::push::install(&core);
    let alerts = alerts::Alerts::start(core.clone(), settings.alerts.clone());
    let ingest =
        ingest::Ingest::start(core.clone(), data_dir, settings, Some(alerts.sink())).await?;
    Ok(Kestrel {
        core,
        ingest,
        alerts,
    })
}

/// The whole app: Kestrel's pages and API, ingest, the kit's pages, static
/// files, MCP and the health probe, behind the kit's middleware. The issue
/// pages reach the writer and the store through an [`Extension`] with
/// ingest's state.
pub fn router(app: Kestrel) -> Router {
    let core = app.core;
    let protected = Router::new()
        .merge(pages::router())
        .merge(groups::router())
        .merge(issues::router())
        .merge(releases::router())
        .merge(releases::cli::router())
        .merge(analytics::pages::router())
        .merge(analytics::app::pages::router())
        .merge(alerts::pages::router())
        .merge(alerts::channels::router())
        .merge(difs::pages::router())
        .merge(difs::upload::router())
        .merge(api::router())
        .merge(kit_pages::router())
        .route_layer(from_fn_with_state(core.clone(), guard::require_auth))
        .layer(Extension(app.ingest.clone()))
        .layer(Extension(app.alerts.clone()));
    let mcp = Router::new()
        .route("/mcp", post(mcp::endpoint).get(vole_kit::mcp::not_offered))
        .layer(Extension(app.ingest.clone()));
    let public = Router::new()
        .route("/healthz", get(vole_kit::http::healthz))
        .route("/static/{*path}", get(vole_kit::http::assets::static_asset))
        .merge(mcp)
        .merge(app.ingest.router())
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

/// Ingest alone, for the separate ingest port (`KK_INGEST_PORT`): the ingest
/// routes and the health probe, nothing of the admin interface.
pub fn ingest_router(app: Kestrel) -> Router {
    Router::new()
        .route("/healthz", get(vole_kit::http::healthz))
        .merge(app.ingest.router())
        .fallback(vole_kit::http::error::not_found)
        .layer(from_fn(vole_kit::http::error::negotiate))
        .layer(from_fn(vole_kit::http::headers::security_headers))
        .with_state(app.core)
}
