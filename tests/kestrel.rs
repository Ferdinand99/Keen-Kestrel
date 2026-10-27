//! Keen Kestrel driven in-process: setup and sign-in, projects and keys in the
//! pages and the API, roles, branding, versions and the schema.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use vole_kit::auth::{TokenScope, tokens};
use vole_kit::db;
use vole_kit::state::Core;

const ADMIN: &str = "ada";
const PASSWORD: &str = "Kestrel-Owner-Pass-2026!";
const HOST: &str = "kestrel.test:3002";

struct Res {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Res {
    fn location(&self) -> &str {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

struct App {
    core: Core,
    router: Router,
    cookies: BTreeMap<String, String>,
    dir: PathBuf,
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static N: AtomicUsize = AtomicUsize::new(0);

impl App {
    async fn fresh() -> App {
        let dir = std::env::temp_dir().join(format!(
            "keen-kestrel-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let kestrel = keen_kestrel::start(&dir, None).await.unwrap();
        App {
            router: keen_kestrel::router(kestrel.clone()),
            core: kestrel.core,
            cookies: BTreeMap::new(),
            dir,
        }
    }

    /// After setup: the admin signed in.
    async fn set_up() -> App {
        let mut app = App::fresh().await;
        let res = app
            .post(
                "/setup",
                &[
                    ("username", ADMIN),
                    ("password", PASSWORD),
                    ("confirm", PASSWORD),
                    ("site_name", ""),
                    ("timezone", "UTC"),
                ],
            )
            .await;
        assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
        app
    }

    async fn send(&mut self, mut req: Request<Body>) -> Res {
        let h = req.headers_mut();
        h.insert(header::HOST, HeaderValue::from_static(HOST));
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            h.insert(header::COOKIE, HeaderValue::from_str(&cookie).unwrap());
        }
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        let res = self.router.clone().oneshot(req).await.unwrap();
        for v in res.headers().get_all(header::SET_COOKIE) {
            let v = v.to_str().unwrap();
            let pair = v.split(';').next().unwrap();
            if let Some((k, val)) = pair.split_once('=') {
                if val.is_empty() || v.contains("Max-Age=0") {
                    self.cookies.remove(k);
                } else {
                    self.cookies.insert(k.into(), val.into());
                }
            }
        }
        let (status, headers) = (res.status(), res.headers().clone());
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        Res {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }

    async fn get(&mut self, path: &str) -> Res {
        self.send(Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    /// Follows one redirect (flash messages are shown on the next page).
    async fn follow(&mut self, res: &Res) -> Res {
        assert!(res.status.is_redirection(), "{}: {}", res.status, res.body);
        let to = res.location().to_string();
        self.get(&to).await
    }

    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Res {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let req = Request::post(path)
            .header(header::ORIGIN, format!("http://{HOST}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        self.send(req).await
    }

    async fn bearer(&mut self, method: &str, path: &str, token: &str, json: Option<Value>) -> Res {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json");
        let body = json.map_or_else(Body::empty, |j| Body::from(j.to_string()));
        let saved = std::mem::take(&mut self.cookies);
        let res = self.send(req.body(body).unwrap()).await;
        self.cookies = saved;
        res
    }

    /// A token of user `user_id`, made the way the Tokens page makes one.
    async fn token(&self, user_id: i64, scope: TokenScope) -> String {
        let token = tokens::generate();
        let (hash, prefix) = (tokens::hash(&token), tokens::display_prefix(&token));
        self.core
            .with_conn(move |c| {
                db::api_tokens::insert(c, user_id, "test", &prefix, &hash, scope, None)
            })
            .await
            .unwrap();
        token
    }

    /// Adds a user with `role` (its id).
    async fn user(&self, name: &'static str, role: &'static str) -> i64 {
        let hash = vole_kit::auth::hash_password(PASSWORD);
        self.core
            .with_conn(move |c| {
                c.execute(
                    "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
                    rusqlite::params![name, hash, role],
                )?;
                Ok(c.last_insert_rowid())
            })
            .await
            .unwrap()
    }

    /// Signs in as `name` (after signing out).
    async fn sign_in(&mut self, name: &str) {
        self.cookies.clear();
        let res = self
            .post("/login", &[("username", name), ("password", PASSWORD)])
            .await;
        assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
        assert!(self.cookies.contains_key("kk_session"));
    }

    async fn project_ids(&self) -> Vec<i64> {
        self.core
            .with_conn(|c| {
                c.prepare("SELECT id FROM projects ORDER BY id")?
                    .query_map([], |r| r.get(0))?
                    .collect()
            })
            .await
            .unwrap()
    }
}

/// Branding checks for every page: Kestrel's name and icons, never Vigilant
/// Vole's.
fn assert_kestrel_branded(path: &str, html: &str) {
    assert!(
        html.contains("Keen Kestrel"),
        "{path}: no product name\n{html}"
    );
    assert!(html.contains("/static/favicon.svg"), "{path}: no icon");
    for foreign in [
        "Vigilant", "vigilant", "Vole", "vv_", "VV_", "kit-demo", "Kit Demo",
    ] {
        assert!(!html.contains(foreign), "{path}: contains {foreign:?}");
    }
}

/// `https://<32 hex>@<host>/<id>`.
fn assert_dsn(dsn: &str, project_id: i64) -> String {
    let rest = dsn.strip_prefix("http://").expect("scheme");
    let (key, host_path) = rest.split_once('@').expect("@");
    assert_eq!(key.len(), 32, "{dsn}");
    assert!(
        key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "{dsn}"
    );
    assert_eq!(host_path, format!("{HOST}/{project_id}"), "{dsn}");
    key.to_string()
}

fn dsns_in(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("http://") {
        rest = &rest[i..];
        let end = rest
            .find(|c: char| matches!(c, '"' | '<' | '&') || c.is_whitespace())
            .unwrap_or(rest.len());
        let candidate = &rest[..end];
        if candidate.contains('@') && !out.iter().any(|d: &String| d == candidate) {
            out.push(candidate.to_string());
        }
        rest = &rest[end..];
    }
    out
}

#[tokio::test]
async fn setup_sign_in_project_key_and_dsn() {
    let mut app = App::fresh().await;
    // No users yet: everything leads to setup.
    assert_eq!(app.get("/").await.location(), "/setup");
    assert_eq!(app.get("/projects").await.location(), "/setup");
    let setup = app.get("/setup").await;
    assert_eq!(setup.status, StatusCode::OK);
    assert_kestrel_branded("/setup", &setup.body);

    let res = app
        .post(
            "/setup",
            &[
                ("username", ADMIN),
                ("password", PASSWORD),
                ("confirm", PASSWORD),
                ("site_name", ""),
                ("timezone", "Europe/Oslo"),
            ],
        )
        .await;
    assert_eq!((res.status, res.location()), (StatusCode::SEE_OTHER, "/"));
    assert!(
        app.cookies.keys().all(|k| k.starts_with("kk_")),
        "{:?}",
        app.cookies
    );
    assert_eq!(app.get("/").await.location(), "/projects");

    // Sign out and in again.
    let res = app.post("/logout", &[]).await;
    assert_eq!(res.location(), "/login");
    assert!(!app.cookies.contains_key("kk_session"));
    let login = app.get("/login").await;
    assert_kestrel_branded("/login", &login.body);
    // The version footer is for signed-in pages only.
    let version = env!("CARGO_PKG_VERSION");
    for needle in ["app-foot", "/releases/tag/", version] {
        assert!(!login.body.contains(needle), "/login shows {needle:?}");
    }
    app.sign_in(ADMIN).await;

    // An empty list invites you to create the first project.
    let empty = app.get("/projects").await;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(empty.body.contains("No projects yet"), "{}", empty.body);
    assert!(empty.body.contains(r#"href="/projects/new""#));
    assert_kestrel_branded("/projects", &empty.body);
    let footer = format!(
        "<footer class=\"app-foot container\">\n    Keen Kestrel <a href=\"https://github.com/Ferdinand99/Keen-Kestrel/releases/tag/v{version}\">{version}</a>\n  </footer>"
    );
    assert!(
        empty.body.replace("\r\n", "\n").contains(&footer),
        "no version footer\n{}",
        empty.body
    );

    let form = app.get("/projects/new").await;
    assert_eq!(form.status, StatusCode::OK);
    let bad = app
        .post("/projects", &[("name", "  "), ("platform", "apple")])
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.body.contains("Enter a name."));

    let res = app
        .post(
            "/projects",
            &[("name", "Sylo for iOS"), ("platform", "apple")],
        )
        .await;
    let ids = app.project_ids().await;
    assert_eq!(ids.len(), 1);
    let id = ids[0];
    assert_eq!(res.location(), format!("/projects/{id}"));
    let overview = app.follow(&res).await;
    assert_eq!(overview.status, StatusCode::OK);
    assert!(overview.body.contains("Project created."));
    assert!(overview.body.contains("No events yet"));
    assert_kestrel_branded("overview", &overview.body);
    // The first key's DSN, in the copy field and in all four snippets.
    let dsns = dsns_in(&overview.body);
    assert_eq!(dsns.len(), 1, "{dsns:?}");
    let first_key = assert_dsn(&dsns[0], id);
    for sdk in [
        "SentrySDK.start",
        "@sentry/browser",
        "sentry::init",
        "sentry_sdk.init",
    ] {
        assert!(overview.body.contains(sdk), "no {sdk} snippet");
    }
    assert_eq!(
        overview
            .body
            .matches(&format!("{first_key}@{HOST}/{id}"))
            .count(),
        5,
        "copy field + four snippets"
    );

    // A second key gets its own DSN.
    let res = app
        .post(&format!("/projects/{id}/keys"), &[("name", "Production")])
        .await;
    assert_eq!(res.location(), format!("/projects/{id}/settings#keys"));
    let settings = app.follow(&res).await;
    assert!(settings.body.contains("Key created."));
    assert!(settings.body.contains("Production"));
    assert_kestrel_branded("settings", &settings.body);
    let dsns = dsns_in(&settings.body);
    assert_eq!(dsns.len(), 2, "{dsns:?}");
    let keys: Vec<String> = dsns.iter().map(|d| assert_dsn(d, id)).collect();
    assert_ne!(keys[0], keys[1]);

    // Disable the first key, with a rate limit and origins.
    let key_id: i64 = app
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT id FROM project_keys WHERE project_id = ?1 ORDER BY id LIMIT 1",
                [id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    let res = app
        .post(
            &format!("/projects/{id}/keys/{key_id}"),
            &[
                ("name", "Default"),
                ("rate_limit", "600"),
                (
                    "allowed_origins",
                    "https://Sylo.app/\n\nhttp://localhost:5173",
                ),
            ],
        )
        .await;
    let page = app.follow(&res).await;
    assert!(page.body.contains("Key saved."), "{}", page.body);
    assert!(page.body.contains("600 per minute"));
    assert!(
        page.body
            .contains("https://sylo.app, http://localhost:5173")
    );
    assert!(page.body.contains("Disabled"));
    let bad = app
        .post(
            &format!("/projects/{id}/keys/{key_id}"),
            &[("name", "Default"), ("allowed_origins", "sylo.app/path")],
        )
        .await;
    assert!(app.follow(&bad).await.body.contains("without a path"));

    // The overview now uses the enabled key.
    let overview = app.get(&format!("/projects/{id}")).await;
    assert_eq!(dsns_in(&overview.body), [dsns[1].clone()]);

    // Rename, then a clashing slug is refused.
    let res = app
        .post(
            &format!("/projects/{id}/settings"),
            &[("name", "Sylo"), ("slug", "sylo"), ("platform", "apple")],
        )
        .await;
    assert!(app.follow(&res).await.body.contains("Project saved."));
    app.post("/projects", &[("name", "Tofa"), ("platform", "rust")])
        .await;
    let tofa = *app.project_ids().await.last().unwrap();
    let clash = app
        .post(
            &format!("/projects/{tofa}/settings"),
            &[("name", "Tofa"), ("slug", "sylo"), ("platform", "rust")],
        )
        .await;
    assert_eq!(clash.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        clash
            .body
            .contains("Another project already uses this slug.")
    );
    let list = app.get("/projects").await;
    assert!(list.body.contains("Sylo") && list.body.contains("Tofa"));

    // Delete a key, then the project.
    let res = app
        .post(&format!("/projects/{id}/keys/{key_id}/delete"), &[])
        .await;
    assert!(app.follow(&res).await.body.contains("Key deleted."));
    let res = app.post(&format!("/projects/{id}/delete"), &[]).await;
    assert_eq!(res.location(), "/projects");
    assert!(app.follow(&res).await.body.contains("Project deleted."));
    assert_eq!(app.project_ids().await, [tofa]);
    assert_eq!(
        app.get(&format!("/projects/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    let left: i64 = app
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM project_keys WHERE project_id = ?1",
                [id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn the_public_url_makes_the_dsn() {
    let dir = std::env::temp_dir().join(format!("keen-kestrel-url-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = keen_kestrel::start(&dir, Some("https://errors.example.com"))
        .await
        .unwrap()
        .core;
    assert_eq!(
        core.base_url().await.as_deref(),
        Some("https://errors.example.com")
    );
    let (key, id) = core
        .with_conn(|c| {
            let id = keen_kestrel::db::projects::insert(
                c,
                "Web",
                "web",
                keen_kestrel::project::Platform::JavaScript,
                "Default",
            )?;
            Ok((
                keen_kestrel::db::projects::keys(c, id)?[0]
                    .public_key
                    .clone(),
                id,
            ))
        })
        .await
        .unwrap();
    let base = keen_kestrel::dsn::public_base(core.base_url().await, &HeaderMap::new());
    assert_eq!(
        keen_kestrel::dsn::dsn(&base, &key, id),
        format!("https://{key}@errors.example.com/{id}")
    );
    // Without KK_PUBLIC_URL, what was stored (by it or on the General page)
    // stays.
    let core = keen_kestrel::start(&dir, None).await.unwrap().core;
    assert_eq!(
        core.base_url().await.as_deref(),
        Some("https://errors.example.com")
    );
    // A new KK_PUBLIC_URL replaces it at start-up.
    let core = keen_kestrel::start(&dir, Some("https://kestrel.example.org"))
        .await
        .unwrap()
        .core;
    assert_eq!(
        core.base_url().await.as_deref(),
        Some("https://kestrel.example.org")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The kit's General page (admins only) with Kestrel's texts: the site
/// name, the time zone and the public URL the DSNs are made with.
#[tokio::test]
async fn the_general_page_sets_the_public_url_of_the_dsns() {
    // The setup page says where the time zone can be changed.
    let setup = App::fresh().await.get("/setup").await;
    assert!(
        setup.body.contains(". You can change this in Settings."),
        "{}",
        setup.body
    );

    let mut app = App::set_up().await;
    let page = app.get("/settings/general").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_kestrel_branded("/settings/general", &page.body);
    for text in [
        "How this instance names itself, where it is reached and how it tells time.",
        "used in DSNs and in links in alerts",
        r#"name="site_name""#,
        r#"name="base_url""#,
        r#"name="timezone""#,
        r#"name="language""#,
        "Acme errors…",
    ] {
        assert!(page.body.contains(text), "{text}\n{}", page.body);
    }

    let bad = app
        .post(
            "/settings/general",
            &[
                ("site_name", "Acme errors"),
                ("base_url", "errors.example.com"),
                ("timezone", "Europe/Oslo"),
                ("language", "en"),
            ],
        )
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", bad.body);
    assert_eq!(app.core.base_url().await, None);

    let ok = app
        .post(
            "/settings/general",
            &[
                ("site_name", "Acme errors"),
                ("base_url", "https://errors.example.com/"),
                ("timezone", "Europe/Oslo"),
                ("language", "en"),
            ],
        )
        .await;
    assert_eq!(
        (ok.status, ok.location()),
        (StatusCode::SEE_OTHER, "/settings/general"),
        "{}",
        ok.body
    );
    let site = app
        .core
        .with_conn(|c| vole_kit::settings::SiteSettings::load(c))
        .await
        .unwrap();
    assert_eq!(site.site_name.as_deref(), Some("Acme errors"));
    assert_eq!(site.base_url.as_deref(), Some("https://errors.example.com"));
    assert_eq!(site.timezone.as_deref(), Some("Europe/Oslo"));

    // New DSNs use the public URL instead of the request's address.
    app.post("/projects", &[("name", "Shop"), ("platform", "python")])
        .await;
    let id = app.project_ids().await[0];
    let settings = app.get(&format!("/projects/{id}/settings")).await.body;
    assert!(
        settings.contains(&format!("@errors.example.com/{id}")),
        "{settings}"
    );
    assert!(dsns_in(&settings).is_empty(), "no http:// DSN is left");

    // Admins only.
    app.user("eve", "editor").await;
    app.sign_in("eve").await;
    assert_eq!(
        app.get("/settings/general").await.status,
        StatusCode::FORBIDDEN
    );
    let res = app
        .post("/settings/general", &[("site_name", "Theirs")])
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(app.get("/settings").await.location(), "/settings/security");
}

#[tokio::test]
async fn viewers_can_read_but_not_change_projects() {
    let mut app = App::set_up().await;
    app.post("/projects", &[("name", "Sylo"), ("platform", "apple")])
        .await;
    let id = app.project_ids().await[0];
    let viewer = app.user("vic", "viewer").await;
    app.sign_in("vic").await;

    let list = app.get("/projects").await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("Sylo"));
    assert!(!list.body.contains(r#"href="/projects/new""#));
    let overview = app.get(&format!("/projects/{id}")).await;
    assert_eq!(overview.status, StatusCode::OK);
    assert_eq!(dsns_in(&overview.body).len(), 1);
    let settings = app.get(&format!("/projects/{id}/settings")).await;
    assert_eq!(settings.status, StatusCode::OK);
    assert!(
        !settings
            .body
            .contains(&format!("action=\"/projects/{id}/delete\""))
    );
    assert!(!settings.body.contains("id=\"new-key\""));

    let key_id: i64 = app
        .core
        .with_conn(|c| c.query_row("SELECT id FROM project_keys", [], |r| r.get(0)))
        .await
        .unwrap();
    for (path, form) in [
        ("/projects".to_string(), vec![("name", "Mine")]),
        (
            format!("/projects/{id}/settings"),
            vec![("name", "X"), ("slug", "x")],
        ),
        (format!("/projects/{id}/delete"), vec![]),
        (format!("/projects/{id}/keys"), vec![("name", "Mine")]),
        (format!("/projects/{id}/keys/{key_id}"), vec![("name", "X")]),
        (format!("/projects/{id}/keys/{key_id}/delete"), vec![]),
    ] {
        let res = app.post(&path, &form).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(app.get("/projects/new").await.status, StatusCode::FORBIDDEN);

    // A viewer's write token is still a viewer.
    let token = app.token(viewer, TokenScope::Write).await;
    let res = app
        .bearer(
            "POST",
            "/api/v1/projects",
            &token,
            Some(json!({"name": "Mine"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    assert_eq!(res.json()["error"]["code"], "forbidden");
    let res = app
        .bearer("DELETE", &format!("/api/v1/projects/{id}"), &token, None)
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.bearer("GET", "/api/v1/projects", &token, None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(app.project_ids().await, [id]);
}

#[tokio::test]
async fn the_api_manages_projects_and_keys() {
    let mut app = App::set_up().await;
    let token = app.token(1, TokenScope::Write).await;
    assert!(token.starts_with("kk_"));

    let res = app.bearer("GET", "/api/v1/projects", &token, None).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json(), json!({"data": [], "next_cursor": null}));

    let res = app
        .bearer(
            "POST",
            "/api/v1/projects",
            &token,
            Some(json!({"name": "Sylo for iOS", "platform": "apple"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let project = res.json();
    let id = project["id"].as_i64().unwrap();
    assert_eq!(res.location(), format!("/api/v1/projects/{id}"));
    assert_eq!(project["slug"], "sylo-for-ios");
    assert_eq!(project["platform"], "apple");

    // Validation and conflicts use the error envelope.
    let res = app
        .bearer(
            "POST",
            "/api/v1/projects",
            &token,
            Some(json!({"name": ""})),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(res.json()["error"]["code"], "validation_failed");
    assert!(res.json()["error"]["fields"]["name"].is_string());
    let res = app
        .bearer(
            "POST",
            "/api/v1/projects",
            &token,
            Some(json!({"name": "Other", "slug": "sylo-for-ios"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CONFLICT, "{}", res.body);
    let res = app
        .bearer(
            "POST",
            "/api/v1/projects",
            &token,
            Some(json!({"name": "X", "platform": "cobol"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);

    let res = app
        .bearer(
            "PATCH",
            &format!("/api/v1/projects/{id}"),
            &token,
            Some(json!({"name": "Sylo", "slug": "sylo"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["name"], "Sylo");
    assert_eq!(res.json()["platform"], "apple");
    let res = app
        .bearer("GET", &format!("/api/v1/projects/{id}"), &token, None)
        .await;
    assert_eq!(res.json()["slug"], "sylo");
    assert!(res.headers.contains_key(header::ETAG));

    // Keys: the default one, then a new one.
    let res = app
        .bearer("GET", &format!("/api/v1/projects/{id}/keys"), &token, None)
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let keys = res.json()["data"].as_array().unwrap().clone();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["name"], "Default");
    assert_dsn(keys[0]["dsn"].as_str().unwrap(), id);

    let res = app
        .bearer(
            "POST",
            &format!("/api/v1/projects/{id}/keys"),
            &token,
            Some(json!({"name": "Web", "rate_limit_per_minute": 60, "allowed_origins": ["https://sylo.app/"]})),
        )
        .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let key = res.json();
    let key_id = key["id"].as_i64().unwrap();
    assert_eq!(key["rate_limit_per_minute"], 60);
    assert_eq!(key["allowed_origins"], json!(["https://sylo.app"]));
    assert_eq!(key["enabled"], true);
    let public_key = assert_dsn(key["dsn"].as_str().unwrap(), id);
    assert_eq!(key["public_key"], public_key.as_str());

    let res = app
        .bearer(
            "PATCH",
            &format!("/api/v1/projects/{id}/keys/{key_id}"),
            &token,
            Some(json!({"enabled": false, "rate_limit_per_minute": null})),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["enabled"], false);
    assert_eq!(res.json()["rate_limit_per_minute"], Value::Null);
    assert_eq!(res.json()["allowed_origins"], json!(["https://sylo.app"]));
    let res = app
        .bearer(
            "PATCH",
            &format!("/api/v1/projects/{id}/keys/{key_id}"),
            &token,
            Some(json!({"rate_limit_per_minute": 0})),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    let res = app
        .bearer(
            "GET",
            &format!("/api/v1/projects/{id}/keys/{key_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(res.json()["enabled"], false);

    let res = app
        .bearer(
            "DELETE",
            &format!("/api/v1/projects/{id}/keys/{key_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    let res = app
        .bearer(
            "DELETE",
            &format!("/api/v1/projects/{id}/keys/{key_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);

    // A read token reads but can't write.
    let read = app.token(1, TokenScope::Read).await;
    assert_eq!(
        app.bearer("GET", &format!("/api/v1/projects/{id}"), &read, None)
            .await
            .status,
        StatusCode::OK
    );
    let res = app
        .bearer(
            "PATCH",
            &format!("/api/v1/projects/{id}"),
            &read,
            Some(json!({"name": "X"})),
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    // Pages of one.
    app.bearer(
        "POST",
        "/api/v1/projects",
        &token,
        Some(json!({"name": "Tofa"})),
    )
    .await;
    let res = app
        .bearer("GET", "/api/v1/projects?limit=1", &token, None)
        .await;
    let page = res.json();
    assert_eq!(page["data"].as_array().unwrap().len(), 1);
    let next = page["next_cursor"].as_str().unwrap().to_string();
    let res = app
        .bearer(
            "GET",
            &format!("/api/v1/projects?limit=1&cursor={next}"),
            &token,
            None,
        )
        .await;
    assert_eq!(res.json()["data"][0]["name"], "Tofa");
    assert_eq!(res.json()["next_cursor"], Value::Null);

    let res = app
        .bearer("DELETE", &format!("/api/v1/projects/{id}"), &token, None)
        .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    let res = app
        .bearer("GET", &format!("/api/v1/projects/{id}"), &token, None)
        .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    assert_eq!(res.json()["error"]["code"], "not_found");
    let res = app.bearer("GET", "/api/v1/nothing", &token, None).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    // No token: 401 in the envelope.
    let res = app.bearer("GET", "/api/v1/projects", "kk_nope", None).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn every_page_is_branded_kestrel() {
    let mut app = App::set_up().await;
    app.post("/projects", &[("name", "Sylo"), ("platform", "apple")])
        .await;
    let id = app.project_ids().await[0];
    let project = format!("/projects/{id}");
    let project_settings = format!("/projects/{id}/settings");
    let issues = format!("/projects/{id}/issues");
    for path in [
        "/projects",
        "/projects/new",
        project.as_str(),
        project_settings.as_str(),
        issues.as_str(),
        "/settings/general",
        "/settings/security",
        "/settings/users",
        "/settings/tokens",
        "/api-docs",
    ] {
        let res = app.get(path).await;
        assert_eq!(res.status, StatusCode::OK, "{path}: {}", res.body);
        assert_kestrel_branded(path, &res.body);
        assert!(res.body.contains("/static/apple-touch-icon.png"), "{path}");
        assert!(res.body.contains("/static/site.webmanifest"), "{path}");
        // The texts the kit's pages take from Kestrel are Kestrel's own.
        assert!(!res.body.to_lowercase().contains("monitor"), "{path}");
    }
    let login = App::fresh().await.get("/login").await;
    let login = if login.status.is_redirection() {
        String::new()
    } else {
        login.body
    };
    assert!(!login.contains("monitors"));
    assert_eq!(app.get("/settings").await.location(), "/settings/general");

    let docs = app.get("/api-docs").await.body;
    assert!(docs.contains(r#"download="kestrel-openapi.json""#));
    assert!(docs.contains("Bearer kk_…"));
    assert!(docs.contains("/api/v1/projects/{id}/keys"));
    let tabs = app.get("/settings/users").await.body;
    for tab in [
        "/settings/general",
        "/settings/users",
        "/settings/security",
        "/settings/tokens",
    ] {
        assert!(tabs.contains(&format!("href=\"{tab}\"")), "{tab}");
    }

    // Tokens are made with Kestrel's prefix.
    let res = app
        .post(
            "/settings/tokens",
            &[("name", "CI"), ("scope", "read"), ("expires", "30")],
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.body.contains("Bearer kk_"), "{}", res.body);
    assert!(res.body.contains("/api/v1/projects"));
    assert!(!res.body.contains("/metrics"));

    // Errors use Kestrel's icon.
    let missing = app.get("/no-such-page").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.body.contains("/static/favicon.svg"));

    // The Markdown reference and llms.txt.
    for path in ["/api-docs.md", "/llms.txt"] {
        let md = app.get(path).await;
        assert!(
            md.body.starts_with("# Keen Kestrel API"),
            "{path}: {}",
            md.body
        );
        assert!(md.body.contains("$KK_TOKEN"));
        assert!(
            md.body
                .contains("### PATCH /api/v1/projects/{id}/keys/{key_id}")
        );
        assert!(md.body.contains("$KK_URL/api/v1/projects/1/keys/3"));
        for foreign in ["Vigilant", "Vole", "vv_", "VV_"] {
            assert!(!md.body.contains(foreign), "{path}: {foreign}");
        }
    }

    // Translated pages say the same in Norwegian.
    app.core
        .with_conn(|c| db::users::set_language(c, 1, Some("nb")))
        .await
        .unwrap();
    let nb = app.get("/projects").await;
    assert!(nb.body.contains("lang=\"nb\""));
    assert!(nb.body.contains("Prosjekter"), "{}", nb.body);
    assert_kestrel_branded("/projects (nb)", &nb.body);
}

#[tokio::test]
async fn static_files_and_health() {
    let mut app = App::fresh().await;
    for (path, kind) in [
        ("/static/style.css", "text/css"),
        ("/static/app.js", "javascript"),
        ("/static/passkeys.js", "javascript"),
        ("/static/favicon.svg", "image/svg+xml"),
        ("/static/apple-touch-icon.png", "image/png"),
        ("/static/icon-192.png", "image/png"),
        ("/static/icon-512.png", "image/png"),
        ("/static/icon-maskable-512.png", "image/png"),
        ("/static/site.webmanifest", "application/manifest+json"),
        ("/static/analytics.css", "text/css"),
    ] {
        let res = app.get(path).await;
        assert_eq!(res.status, StatusCode::OK, "{path}");
        let content_type = res.headers[header::CONTENT_TYPE].to_str().unwrap();
        assert!(content_type.contains(kind), "{path}: {content_type}");
    }
    let manifest: Value =
        serde_json::from_str(&app.get("/static/site.webmanifest").await.body).unwrap();
    assert_eq!(manifest["name"], "Keen Kestrel");
    // Kestrel's own files don't shadow the kit's design system.
    for path in keen_kestrel::product::static_paths() {
        assert!(vole_kit::http::assets::kit_file(&path).is_none(), "{path}");
    }
    let health = app.get("/healthz").await;
    assert_eq!(
        (health.status, health.body.as_str()),
        (StatusCode::OK, "ok")
    );
}

#[tokio::test]
async fn openapi_and_mcp_report_kestrels_version() {
    let mut app = App::set_up().await;
    app.post("/projects", &[("name", "Sylo"), ("platform", "apple")])
        .await;
    let token = app.token(1, TokenScope::Read).await;

    let res = app
        .bearer("GET", "/api/v1/openapi.json", &token, None)
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let doc = res.json();
    assert_eq!(keen_kestrel::VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(doc["info"]["version"], keen_kestrel::VERSION);
    assert_eq!(doc["info"]["title"], "Keen Kestrel API");
    assert_eq!(
        doc["components"]["securitySchemes"]["sessionCookie"]["name"],
        "kk_session"
    );
    for (path, method) in [
        ("/api/v1/projects", "get"),
        ("/api/v1/projects", "post"),
        ("/api/v1/projects/{id}", "patch"),
        ("/api/v1/projects/{id}", "delete"),
        ("/api/v1/projects/{id}/keys", "post"),
        ("/api/v1/projects/{id}/keys/{key_id}", "patch"),
        ("/api/v1/projects/{id}/keys/{key_id}", "delete"),
    ] {
        assert!(doc["paths"][path][method].is_object(), "{method} {path}");
    }

    let rpc = |id: i64, method: &str, params: Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let res = app
        .bearer(
            "POST",
            "/mcp",
            &token,
            Some(rpc(
                1,
                "initialize",
                json!({"protocolVersion": "2025-06-18"}),
            )),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let info = &res.json()["result"]["serverInfo"];
    assert_eq!(info["version"], keen_kestrel::VERSION);
    assert_eq!(info["name"], "kestrel");
    assert_eq!(info["title"], "Keen Kestrel");

    let res = app
        .bearer(
            "POST",
            "/mcp",
            &token,
            Some(rpc(2, "tools/list", json!({}))),
        )
        .await;
    assert_eq!(res.json()["result"]["tools"][0]["name"], "list_projects");
    let res = app
        .bearer(
            "POST",
            "/mcp",
            &token,
            Some(rpc(
                3,
                "tools/call",
                json!({"name": "list_projects", "arguments": {}}),
            )),
        )
        .await;
    let result = &res.json()["result"];
    assert_eq!(result["structuredContent"]["projects"][0]["name"], "Sylo");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Sylo")
    );
    let res = app.get("/mcp").await;
    assert_eq!(res.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn a_fresh_database_has_the_core_tables_and_kestrels() {
    let app = App::fresh().await;
    assert!(app.dir.join("kestrel.sqlite").exists());
    let (tables, applied) = app
        .core
        .with_conn(|c| {
            let tables: Vec<String> = c
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table'
                     AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let applied: Vec<String> = c
                .prepare("SELECT name FROM schema_migrations ORDER BY rowid")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            Ok((tables, applied))
        })
        .await
        .unwrap();
    assert_eq!(
        tables,
        [
            "alert_cooldowns",
            "alert_log",
            "alert_rule_channels",
            "alert_rules",
            "analytics_breakdown",
            "analytics_days",
            "analytics_hours",
            "api_token_scopes",
            "api_tokens",
            "dif_files",
            "difs_token_projects",
            "environments",
            "events",
            "issue_activity",
            "issue_hashes",
            "issue_hours",
            "issue_tag_values",
            "issue_users",
            "issues",
            "issues_fts",
            "issues_fts_config",
            "issues_fts_data",
            "issues_fts_docsize",
            "issues_fts_idx",
            "notification_channels",
            "passkey_challenges",
            "passkeys",
            "project_groups",
            "project_keys",
            "projects",
            "release_health",
            "release_health_seen",
            "releases",
            "schema_migrations",
            "settings",
            "users"
        ]
    );
    assert_eq!(
        applied,
        [
            "kit/0001_core.sql",
            "kit/0002_token_scopes.sql",
            "kit/0003_passkeys.sql",
            "kit/0004_sealed_secrets.sql",
            "kit/0005_sealed_totp.sql",
            "0001_projects.sql",
            "0002_errors.sql",
            "0003_triage.sql",
            "0004_debug_files.sql",
            "0005_alerts_releases.sql",
            "0006_analytics.sql",
            "0007_app_analytics.sql",
            "0008_token_scopes.sql",
            "0009_project_groups.sql",
            "0010_difs_token_projects.sql"
        ]
    );
    assert!(app.dir.join("store.sqlite").exists());

    // Starting again changes nothing and takes no backup.
    let report = vole_kit::db::migrate(&app.core.db, &app.dir.join("backups"))
        .await
        .unwrap();
    assert!(report.applied.is_empty() && report.backup.is_none());
}

/// Provider forms name Kestrel, never Vigilant Vole.
#[test]
fn notification_forms_name_kestrel() {
    keen_kestrel::install();
    let mut shown = Vec::new();
    for p in vole_kit::notify::PROVIDERS {
        for f in p.fields {
            let text = format!("{} {}", f.placeholder_text(), f.default_value());
            assert!(
                !text.contains("Vigilant") && !text.contains("Vole"),
                "{}.{}: {text}",
                p.kind,
                f.key
            );
            if text.contains("Kestrel") {
                shown.push(format!("{}.{}", p.kind, f.key));
            }
        }
    }
    assert!(shown.contains(&"slack.username".to_string()), "{shown:?}");
}

/// Passkeys at a `http://localhost` public URL: after the password, a signed-in
/// user gets registration options, registers a (software) passkey, signs out
/// and signs in with it alone.
#[tokio::test]
async fn passkey_registers_and_signs_in_at_localhost() {
    use vole_kit::auth::webauthn::{b64, soft::SoftAuthenticator, unb64};
    const ORIGIN: &str = "http://localhost:3002";
    let mut app = App::set_up().await;
    app.core
        .with_conn(|c| db::settings::set(c, "base_url", ORIGIN))
        .await
        .unwrap();
    let json_post = |path: &str, body: &Value| {
        Request::post(path)
            .header(header::ORIGIN, format!("http://{HOST}"))
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let options = |res: &Res| {
        assert_eq!(res.status, StatusCode::OK, "{}", res.body);
        let o = res.json()["publicKey"].clone();
        let challenge = unb64(o["challenge"].as_str().unwrap(), "c").unwrap();
        (o, challenge)
    };

    let res = app
        .post(
            "/settings/security/passkeys/options",
            &[("password", "wrong")],
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = app
        .post(
            "/settings/security/passkeys/options",
            &[("password", PASSWORD)],
        )
        .await;
    let (o, challenge) = options(&res);
    assert_eq!(o["rp"]["id"], "localhost");
    assert_eq!(o["user"]["name"], ADMIN);
    let handle = unb64(o["user"]["id"].as_str().unwrap(), "h").unwrap();

    let mut auth = SoftAuthenticator::es256();
    let (client, att) = auth.register("localhost", ORIGIN, &challenge, &handle);
    let id = b64(&auth.credential_id);
    let body = json!({"name": "", "credential": {"id": id, "rawId": id, "type": "public-key",
        "response": {"clientDataJSON": b64(&client), "attestationObject": b64(&att)}}});
    let res = app
        .send(json_post("/settings/security/passkeys", &body))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);

    app.post("/logout", &[]).await;
    let res = app.post("/login/passkey/options", &[]).await;
    let (o, challenge) = options(&res);
    assert_eq!(o["rpId"], "localhost");
    let (client, data, sig) = auth.assert("localhost", ORIGIN, &challenge);
    let body = json!({"next": "/projects", "credential": {"id": id, "rawId": id, "type": "public-key",
        "response": {"clientDataJSON": b64(&client), "authenticatorData": b64(&data),
            "signature": b64(&sig), "userHandle": b64(&auth.user_handle)}}});
    let res = app.send(json_post("/login/passkey", &body)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["redirect"], "/projects");
    assert_eq!(app.get("/projects").await.status, StatusCode::OK);
}
