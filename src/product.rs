//! What makes this build Keen Kestrel as far as vole-kit is concerned: names,
//! cookies, token prefix, migrations, translations, API reference, the token
//! scope for debug file uploads, the General settings page, the webhook's
//! words and what the shared layouts show.

use rust_embed::Embed;
use vole_kit::config::{ExtraScope, KitConfig};
use vole_kit::http::assets::StaticFile;
use vole_kit::notify::webhook::Wording;
use vole_kit::page::{NavItem, SettingsTab, Ui};
use vole_kit::settings::GeneralSettings;

/// The product's version (`CARGO_PKG_VERSION`), reported by the OpenAPI
/// document and the MCP server.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The product's name.
pub const NAME: &str = "Keen Kestrel";
/// `User-Agent` of outgoing requests.
pub const USER_AGENT: &str = concat!("KeenKestrel/", env!("CARGO_PKG_VERSION"));
/// Name of the signed session cookie.
pub const SESSION_COOKIE: &str = "kk_session";
/// Prefix of API tokens.
pub const TOKEN_PREFIX: &str = "kk_";
/// The database file in the data directory.
pub const DB_FILE: &str = "kestrel.sqlite";
/// The token scope that lets an otherwise read-only API token upload debug
/// files and manage releases with sentry-cli (docs/design.md §4.8).
pub const DIFS_WRITE: &str = "difs:write";

/// Kestrel's own static files: icons and the web app manifest. The design
/// system (stylesheet, scripts, icon sprite, fonts) is the kit's.
#[derive(Embed)]
#[folder = "static/"]
struct StaticAssets;

/// Looks up one of Kestrel's static files (for the kit's `/static/` handler).
pub fn static_file(path: &str) -> Option<StaticFile> {
    let file = StaticAssets::get(path)?;
    Some(StaticFile {
        sha256: file.metadata.sha256_hash(),
        data: file.data,
    })
}

/// Every static file Kestrel serves itself, sorted.
pub fn static_paths() -> Vec<String> {
    let mut paths: Vec<String> = StaticAssets::iter().map(|p| p.into_owned()).collect();
    paths.sort();
    paths
}

pub static KIT: KitConfig = KitConfig {
    product: NAME,
    slug: "keen-kestrel",
    short_name: "Kestrel",
    session_cookie: SESSION_COOKIE,
    pending_2fa_cookie: "kk_2fa",
    token_prefix: TOKEN_PREFIX,
    other_token_prefixes: &[],
    version: VERSION,
    user_agent: USER_AGENT,
    backup_prefix: "kestrel-before-",
    flash_cookie: "kk_flash",
    env_prefix: "KK",
    // `/api/0`: sentry-cli's debug file upload, which takes API tokens only.
    // Ingest (`/api/{project_id}/envelope/`, `/api/{project_id}/store/`,
    // `/ingest/v1/event`, `/ingest/v1/signals`): SDKs, k.js and KestrelKit
    // post from anywhere with a DSN key, never with a session cookie.
    csrf_exempt_prefixes: &[
        "/api/v1",
        "/api/0",
        "/mcp",
        "/api/*/envelope",
        "/api/*/store",
        "/ingest/v1/event",
        "/ingest/v1/signals",
    ],
    csrf_exempt_path: no_exempt_path,
    extra_common_passwords: &["keenkestrel", "kestrel123"],
    migrations: &crate::db::MIGRATIONS,
    embeddable_prefixes: &[],
    static_file,
    machine_path_prefixes: &["/static/", "/api/", "/ingest/", "/js/"],
    machine_paths: &["/healthz", "/mcp"],
    browser_language_prefixes: &[],
    docs: &crate::api::DOCS,
    locales: &[
        ("en", include_str!("../locales/en.toml")),
        ("nb", include_str!("../locales/nb.toml")),
        ("ar", include_str!("../locales/ar.toml")),
        ("cs", include_str!("../locales/cs.toml")),
        ("da", include_str!("../locales/da.toml")),
        ("de", include_str!("../locales/de.toml")),
        ("es", include_str!("../locales/es.toml")),
        ("fa", include_str!("../locales/fa.toml")),
        ("fi", include_str!("../locales/fi.toml")),
        ("fr", include_str!("../locales/fr.toml")),
        ("he", include_str!("../locales/he.toml")),
        ("hu", include_str!("../locales/hu.toml")),
        ("it", include_str!("../locales/it.toml")),
        ("ja", include_str!("../locales/ja.toml")),
        ("ko", include_str!("../locales/ko.toml")),
        ("nl", include_str!("../locales/nl.toml")),
        ("pl", include_str!("../locales/pl.toml")),
        ("pt-BR", include_str!("../locales/pt-BR.toml")),
        ("pt-PT", include_str!("../locales/pt-PT.toml")),
        ("ro", include_str!("../locales/ro.toml")),
        ("ru", include_str!("../locales/ru.toml")),
        ("sv", include_str!("../locales/sv.toml")),
        ("tr", include_str!("../locales/tr.toml")),
        ("uk", include_str!("../locales/uk.toml")),
        ("zh-CN", include_str!("../locales/zh-CN.toml")),
        ("zh-TW", include_str!("../locales/zh-TW.toml")),
    ],
    ui: &Ui {
        logo: "/static/favicon.svg",
        favicon: "/static/favicon.svg",
        apple_touch_icon: "/static/apple-touch-icon.png",
        manifest: "/static/site.webmanifest",
        nav: &[
            NavItem {
                id: "projects",
                href: "/projects",
                label: "projects.title",
                short_label: "projects.title",
                icon: "box",
            },
            NavItem {
                id: "settings",
                href: "/settings",
                label: "nav.settings",
                short_label: "nav.settings",
                icon: "gear",
            },
        ],
        settings_tabs: &[
            SettingsTab::GENERAL,
            SettingsTab::USERS,
            SettingsTab::SECURITY,
            SettingsTab::TOKENS,
            crate::alerts::channels::TAB,
        ],
        home_url: "https://hedgerow.tools/Keen-Kestrel/",
    },
    // Set on the General page; `KK_PUBLIC_URL` stores it at start-up.
    public_url_setting: Some("Settings → General"),
    webhook: Wording {
        subject: "issue",
        duration: "duration",
        url_example: "https://example.com/hooks/kestrel…",
    },
    token_scopes: &[ExtraScope {
        name: DIFS_WRITE,
        label: "difs.scope",
        help: "difs.scope_help",
        implies_read: true,
        paths: &[],
    }],
    // Site name, public URL, time zone and language; Kestrel's own settings
    // (retention, analytics) are per project.
    general: Some(&GeneralSettings::CORE_ONLY),
    // The signed-in footer links the running version to its release notes.
    releases_url: Some("https://github.com/Ferdinand99/Keen-Kestrel/releases/tag/v{version}"),
};

/// Every exempt path is a prefix above.
fn no_exempt_path(_: &str) -> bool {
    false
}

/// Installs [`KIT`]. Call it first thing at start-up, and in test set-up.
pub fn install() {
    vole_kit::config::install(&KIT);
}
