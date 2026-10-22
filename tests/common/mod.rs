//! Shared by the ingest tests: the recorded SDK fixtures and a Kestrel
//! driven in-process.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use keen_kestrel::Kestrel;
use keen_kestrel::ingest::{body, envelope};
use serde_json::Value;
use tower::ServiceExt as _;

/// The key the fixtures were recorded with.
pub const FIXTURE_KEY: &str = "0123456789abcdef0123456789abcdef";
/// The address every test request comes from. It must never be stored.
pub const CLIENT_IP: &str = "203.0.113.7";

/// One recorded request: `tests/fixtures/sentry/<sdk>/<version>/<name>`.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub sdk: String,
    pub version: String,
    pub name: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Fixture {
    /// `sdk/version/name`.
    pub fn id(&self) -> String {
        format!("{}/{}/{}", self.sdk, self.version, self.name)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body with its content encoding undone.
    pub fn decoded(&self) -> Vec<u8> {
        body::decode(
            self.header("content-encoding"),
            &self.body,
            body::MAX_ENVELOPE,
        )
        .unwrap()
    }

    pub fn envelope(&self) -> envelope::Envelope {
        envelope::parse(&self.decoded()).unwrap()
    }

    /// The event payloads in the request, with the envelope's event id.
    pub fn events(&self) -> Vec<(Vec<u8>, Option<String>)> {
        let env = self.envelope();
        let id = env.header_str("event_id").map(str::to_string);
        env.items
            .iter()
            .filter(|i| i.ty() == "event")
            .map(|i| (i.payload.clone(), id.clone()))
            .collect()
    }

    /// The id ingest should answer with: the envelope's event id, else the
    /// first event's, as 32 hex characters; `None` without an event.
    pub fn expected_id(&self) -> Option<String> {
        let env = self.envelope();
        let norm = |s: &str| s.replace('-', "").to_ascii_lowercase();
        env.header_str("event_id").map(norm).or_else(|| {
            env.items.iter().find(|i| i.ty() == "event").and_then(|i| {
                serde_json::from_slice::<Value>(&i.payload)
                    .ok()?
                    .get("event_id")?
                    .as_str()
                    .map(norm)
            })
        })
    }

    /// The request as the SDK sent it, to project `project_id` with `key`.
    pub fn request(&self, project_id: i64, key: &str) -> Request<Body> {
        let uri = self
            .uri
            .replacen("/api/1/", &format!("/api/{project_id}/"), 1)
            .replace(FIXTURE_KEY, key);
        let mut req = Request::post(uri);
        for (name, value) in &self.headers {
            if matches!(name.as_str(), "host" | "content-length" | "connection") {
                continue;
            }
            let value = if name == "x-sentry-auth" {
                let before = value.split("sentry_key=").next().unwrap_or("");
                let after = value
                    .split("sentry_key=")
                    .nth(1)
                    .and_then(|rest| rest.find(',').map(|i| &rest[i..]))
                    .unwrap_or("");
                format!("{before}sentry_key={key}{after}")
            } else {
                value.clone()
            };
            req = req.header(name.as_str(), value);
        }
        req.body(Body::from(self.body.clone())).unwrap()
    }
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sentry")
}

/// Every fixture, sorted by SDK, version and name.
pub fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    let mut sdks: Vec<_> = std::fs::read_dir(fixtures_dir())
        .unwrap()
        .flatten()
        .collect();
    sdks.sort_by_key(|e| e.file_name());
    for sdk in sdks {
        let mut versions: Vec<_> = std::fs::read_dir(sdk.path()).unwrap().flatten().collect();
        versions.sort_by_key(|e| e.file_name());
        for version in versions {
            let mut metas: Vec<PathBuf> = std::fs::read_dir(version.path())
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            metas.sort();
            for meta_path in metas {
                let meta: Value =
                    serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
                let headers = meta["headers"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().unwrap().to_string()))
                    .collect();
                out.push(Fixture {
                    sdk: sdk.file_name().to_string_lossy().into_owned(),
                    version: version.file_name().to_string_lossy().into_owned(),
                    name: meta_path
                        .file_stem()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    uri: meta["uri"].as_str().unwrap().to_string(),
                    headers,
                    body: std::fs::read(meta_path.with_extension("body")).unwrap(),
                });
            }
        }
    }
    assert!(out.len() >= 30, "fixtures missing: {}", out.len());
    out
}

pub struct Res {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Res {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

static N: AtomicUsize = AtomicUsize::new(0);

/// A Kestrel in a temporary directory, with one project.
pub struct App {
    pub kestrel: Kestrel,
    pub router: Router,
    pub dir: PathBuf,
    pub project_id: i64,
    pub key: String,
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl App {
    pub async fn new() -> App {
        App::with(keen_kestrel::ingest::Settings::default()).await
    }

    /// Starts Kestrel with `settings` and makes project 1 with the
    /// fixtures' key.
    pub async fn with(settings: keen_kestrel::ingest::Settings) -> App {
        let dir = std::env::temp_dir().join(format!(
            "keen-kestrel-ingest-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let kestrel = keen_kestrel::start_with(&dir, None, settings)
            .await
            .unwrap();
        let project_id = kestrel
            .core
            .with_conn(|c| {
                let id = keen_kestrel::db::projects::insert(
                    c,
                    "Fixtures",
                    "fixtures",
                    keen_kestrel::project::Platform::Other,
                    "Default",
                )?;
                c.execute(
                    "UPDATE project_keys SET public_key = ?1 WHERE project_id = ?2",
                    rusqlite::params![FIXTURE_KEY, id],
                )?;
                Ok(id)
            })
            .await
            .unwrap();
        App {
            router: keen_kestrel::router(kestrel.clone()),
            kestrel,
            dir,
            project_id,
            key: FIXTURE_KEY.to_string(),
        }
    }

    /// Sends `req` from [`CLIENT_IP`].
    pub async fn send(&self, mut req: Request<Body>) -> Res {
        send_to(&self.router, &mut req).await
    }

    pub async fn send_fixture(&self, f: &Fixture) -> Res {
        self.send(f.request(self.project_id, &self.key)).await
    }

    /// Waits until the writer has written everything accepted so far.
    pub async fn flush(&self) {
        self.kestrel.ingest.flush().await;
    }

    pub async fn count(&self, sql: &'static str) -> i64 {
        self.kestrel
            .core
            .with_conn(move |c| c.query_row(sql, [], |r| r.get(0)))
            .await
            .unwrap()
    }

    pub async fn store_count(&self, sql: &'static str) -> i64 {
        let conn = self.kestrel.ingest.store().get().await.unwrap();
        conn.interact(move |c| c.query_row(sql, [], |r| r.get::<_, i64>(0)))
            .await
            .unwrap()
            .unwrap()
    }

    /// Runs SQL on the main database.
    pub async fn exec(&self, sql: String) {
        self.kestrel
            .core
            .with_conn(move |c| c.execute_batch(&sql))
            .await
            .unwrap();
    }
}

pub async fn send_to(router: &Router, req: &mut Request<Body>) -> Res {
    let h = req.headers_mut();
    if !h.contains_key(header::HOST) {
        h.insert(header::HOST, "kestrel.test:3002".parse().unwrap());
    }
    h.insert("x-forwarded-for", CLIENT_IP.parse().unwrap());
    let peer: SocketAddr = format!("{CLIENT_IP}:40000").parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));
    let req = std::mem::replace(req, Request::new(Body::empty()));
    let res = router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    Res {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

/// A minimal envelope with one event.
pub fn envelope_with(event: &Value) -> Vec<u8> {
    let payload = event.to_string();
    format!(
        "{{\"event_id\":\"{}\"}}\n{{\"type\":\"event\",\"length\":{}}}\n{payload}\n",
        event["event_id"].as_str().unwrap_or(""),
        payload.len()
    )
    .into_bytes()
}

/// `POST /api/{project}/envelope/` with `X-Sentry-Auth`.
pub fn envelope_request(project_id: i64, key: &str, body: Vec<u8>) -> Request<Body> {
    Request::post(format!("/api/{project_id}/envelope/"))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_version=7, sentry_client=test/1.0, sentry_key={key}"),
        )
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope")
        .body(Body::from(body))
        .unwrap()
}

/// A fresh random event id.
pub fn new_id() -> String {
    keen_kestrel::event::normalize::new_event_id()
}
