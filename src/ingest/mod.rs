//! Sentry-compatible ingest (docs/design.md §4.2): `POST
//! /api/{project_id}/envelope/` and the legacy `POST /api/{project_id}/store/`.
//!
//! A request is checked in this order: the per-IP bucket, the project id,
//! the body size, the DSN key (`X-Sentry-Auth`, `?sentry_key=` or the
//! envelope's `dsn`) and its project, the origin, the key's own bucket. Then
//! the body is decoded (bounded), the items are normalised and grouped, and
//! the jobs go to the writer's queue; the answer is `{"id": "<event_id>"}`
//! without waiting for the write.
//!
//! Envelope items: `event` is kept; `session` / `sessions` and
//! `user_report` / `feedback` are kept for later phases; `attachment` is kept
//! when its event is; `transaction`, `span`, `profile`, `replay_*`,
//! `check_in`, `statsd`, `metrics`, `log`, `client_report`, minidumps and
//! unknown types are accepted and dropped.

pub mod auth;
pub mod body;
pub mod capture;
pub mod envelope;
pub mod limit;

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use deadpool_sqlite::Pool;
use serde_json::{Map, Value, json};
use vole_kit::auth::guard::PeerAddr;
use vole_kit::auth::session;
use vole_kit::db::settings;
use vole_kit::state::Core;

use crate::event::{self, Context, normalize, scrub};
use crate::writer::{AttachmentItem, FeedbackItem, Job, SessionItem, Writer, WriterSettings};
use auth::KeyInfo;
use body::{BodyError, MAX_ATTACHMENT, MAX_ENVELOPE, MAX_EVENT};
use limit::{Buckets, retry_secs};

/// The setting that holds the instance's key for user hashes.
const HASH_KEY_SETTING: &str = "ingest_hash_key";

/// Headers a browser SDK may send (CORS preflight).
const ALLOW_HEADERS: &str =
    "content-type, content-encoding, x-sentry-auth, authorization, sentry-trace, baggage";
/// Headers a browser SDK may read.
const EXPOSE_HEADERS: &str = "x-sentry-error, x-sentry-rate-limits, retry-after";

/// How ingest runs.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `KESTREL_CAPTURE_DIR`: write every request there.
    pub capture_dir: Option<PathBuf>,
    /// Requests per minute from one IP address; 0 turns the limit off.
    pub ip_rate_limit_per_minute: u32,
    pub writer: WriterSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            capture_dir: None,
            ip_rate_limit_per_minute: 6000,
            writer: WriterSettings::default(),
        }
    }
}

/// Ingest's state: the writer, the store, the buckets.
#[derive(Clone)]
pub struct Ingest {
    inner: Arc<Inner>,
}

struct Inner {
    core: Core,
    writer: Writer,
    store: Pool,
    settings: Settings,
    hash_key: [u8; 32],
    key_buckets: Buckets<i64>,
    ip_buckets: Buckets<IpAddr>,
}

/// Why a request is refused.
#[derive(Debug)]
pub enum Reject {
    BadRequest(String),
    Unauthorized(&'static str),
    Forbidden(&'static str),
    TooLarge(&'static str),
    RateLimited { secs: u64, scope: &'static str },
    Unavailable(&'static str),
}

impl Reject {
    fn status(&self) -> StatusCode {
        match self {
            Reject::BadRequest(_) => StatusCode::BAD_REQUEST,
            Reject::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Reject::Forbidden(_) => StatusCode::FORBIDDEN,
            Reject::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Reject::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Reject::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn detail(&self) -> String {
        match self {
            Reject::BadRequest(d) => d.clone(),
            Reject::Unauthorized(d)
            | Reject::Forbidden(d)
            | Reject::TooLarge(d)
            | Reject::Unavailable(d) => (*d).to_string(),
            Reject::RateLimited { .. } => "rate limited".to_string(),
        }
    }
}

impl IntoResponse for Reject {
    fn into_response(self) -> Response {
        let detail = self.detail();
        let mut res = (self.status(), axum::Json(json!({ "detail": detail }))).into_response();
        let h = res.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&detail) {
            h.insert("x-sentry-error", v);
        }
        if let Reject::RateLimited { secs, scope } = self {
            h.insert(header::RETRY_AFTER, HeaderValue::from(secs));
            if let Ok(v) = HeaderValue::from_str(&format!("{secs}::{scope}")) {
                h.insert("x-sentry-rate-limits", v);
            }
        }
        res
    }
}

impl From<BodyError> for Reject {
    fn from(e: BodyError) -> Self {
        match e {
            BodyError::TooLarge => Reject::TooLarge("the request body is too large"),
            BodyError::Invalid(why) => Reject::BadRequest(why),
        }
    }
}

/// What CORS headers an answer gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cors {
    Any,
    Echo,
}

/// The decoded body.
enum Parsed {
    Envelope(envelope::Envelope),
    Event(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Envelope,
    Store,
}

/// One ingest request as the routes hand it over.
struct Incoming {
    endpoint: Endpoint,
    project: String,
    uri: Uri,
    query: Option<String>,
    peer: Option<SocketAddr>,
    headers: HeaderMap,
    body: Body,
}

impl Ingest {
    /// Opens the store, loads (or makes) the hash key and starts the writer.
    pub async fn start(
        core: Core,
        data_dir: &Path,
        settings: Settings,
    ) -> Result<Ingest, crate::StartError> {
        let store = crate::store::open(data_dir).await?;
        let hash_key = core
            .with_conn(|c| {
                if let Some(hex_key) = settings::get(c, HASH_KEY_SETTING)?
                    && let Ok(bytes) = hex::decode(hex_key.trim())
                    && let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice())
                {
                    return Ok(key);
                }
                let mut key = [0u8; 32];
                rand::fill(&mut key);
                settings::set(c, HASH_KEY_SETTING, &hex::encode(key))?;
                Ok(key)
            })
            .await
            .map_err(|_| "can't load or store the ingest hash key")?;
        let writer = Writer::start(
            &data_dir.join(crate::product::DB_FILE),
            &data_dir.join(crate::store::STORE_FILE),
            settings.writer,
        )?;
        if let Some(dir) = &settings.capture_dir {
            tracing::warn!(dir = %dir.display(), "capture mode: every ingest request is written to disk");
        }
        Ok(Ingest {
            inner: Arc::new(Inner {
                core,
                writer,
                store,
                settings,
                hash_key,
                key_buckets: Buckets::new(),
                ip_buckets: Buckets::new(),
            }),
        })
    }

    /// The store's pool (`store.sqlite`).
    pub fn store(&self) -> &Pool {
        &self.inner.store
    }

    pub fn writer(&self) -> &Writer {
        &self.inner.writer
    }

    /// Waits until every accepted job is written.
    pub async fn flush(&self) {
        self.inner.writer.flush().await;
    }

    /// The ingest routes, for the main router and the ingest port.
    pub fn router<S: Clone + Send + Sync + 'static>(&self) -> Router<S> {
        Router::new()
            .route(
                "/api/{project_id}/envelope/",
                post(envelope_route).options(preflight),
            )
            .route(
                "/api/{project_id}/envelope",
                post(envelope_route).options(preflight),
            )
            .route(
                "/api/{project_id}/store/",
                post(store_route).options(preflight),
            )
            .route(
                "/api/{project_id}/store",
                post(store_route).options(preflight),
            )
            .with_state(self.clone())
    }

    async fn handle(&self, req: Incoming) -> Response {
        let origin = req
            .headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut cors = Cors::Any;
        let result = self.process(req, origin.as_deref(), &mut cors).await;
        let mut res = match result {
            Ok(Some(id)) => axum::Json(json!({ "id": id })).into_response(),
            Ok(None) => axum::Json(json!({})).into_response(),
            Err(reject) => reject.into_response(),
        };
        if let Some(origin) = origin {
            cors_headers(res.headers_mut(), cors, &origin);
        }
        res
    }

    async fn process(
        &self,
        req: Incoming,
        origin: Option<&str>,
        cors: &mut Cors,
    ) -> Result<Option<String>, Reject> {
        let Incoming {
            endpoint,
            project,
            uri,
            query,
            peer,
            headers,
            body,
        } = req;
        let headers = &headers;
        let inner = &self.inner;
        let now = Instant::now();
        let per_ip = inner.settings.ip_rate_limit_per_minute;
        if per_ip > 0
            && let Some(ip) = session::client_ip(headers, peer, session::trust_proxy())
        {
            inner
                .ip_buckets
                .take(&ip, per_ip, now)
                .map_err(|wait| Reject::RateLimited {
                    secs: retry_secs(wait),
                    scope: "organization",
                })?;
        }
        let project_id: i64 = project
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| Reject::BadRequest("invalid project id".into()))?;
        let raw = axum::body::to_bytes(body, MAX_ENVELOPE)
            .await
            .map_err(|_| Reject::TooLarge("the request body is too large"))?;

        let mut key = match auth::from_request(headers, query.as_deref()) {
            Some(Ok(k)) => Some(self.check_key(&k, project_id, origin, cors).await?),
            Some(Err(())) => return Err(Reject::Unauthorized("invalid sentry_key")),
            None => None,
        };

        let encoding = headers
            .get(header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let capture = inner
            .settings
            .capture_dir
            .clone()
            .map(|dir| (dir, uri.to_string(), headers.clone()));
        let parsed = tokio::task::spawn_blocking(move || -> Result<Parsed, Reject> {
            if let Some((dir, uri, headers)) = capture
                && let Err(err) = capture::write(&dir, "POST", &uri, &headers, &raw)
            {
                tracing::warn!(%err, "can't write the capture file");
            }
            match endpoint {
                Endpoint::Envelope => {
                    let data = body::decode(encoding.as_deref(), &raw, MAX_ENVELOPE)?;
                    envelope::parse(&data)
                        .map(Parsed::Envelope)
                        .map_err(|e| Reject::BadRequest(e.to_string()))
                }
                Endpoint::Store => Ok(Parsed::Event(body::decode(
                    encoding.as_deref(),
                    &raw,
                    MAX_EVENT,
                )?)),
            }
        })
        .await
        .map_err(|_| Reject::Unavailable("ingest failed"))??;

        if key.is_none() {
            let dsn = match &parsed {
                Parsed::Envelope(env) => env.header_str("dsn"),
                Parsed::Event(_) => None,
            };
            let public_key = dsn
                .and_then(auth::from_dsn)
                .and_then(|(k, _)| auth::checked(&k))
                .ok_or(Reject::Unauthorized("missing or invalid authentication"))?;
            key = Some(
                self.check_key(&public_key, project_id, origin, cors)
                    .await?,
            );
        }
        let Some(key) = key else {
            return Err(Reject::Unauthorized("missing authentication"));
        };
        if let Some(limit) = key.rate_limit_per_minute {
            let limit = u32::try_from(limit).unwrap_or(u32::MAX);
            inner
                .key_buckets
                .take(&key.id, limit, now)
                .map_err(|wait| Reject::RateLimited {
                    secs: retry_secs(wait),
                    scope: "key",
                })?;
        }

        let this = self.clone();
        let (jobs, id) = tokio::task::spawn_blocking(move || this.jobs(parsed, &key))
            .await
            .map_err(|_| Reject::Unavailable("ingest failed"))??;
        inner.writer.submit(jobs).map_err(|_| Reject::RateLimited {
            secs: 1,
            scope: "organization",
        })?;
        Ok(id)
    }

    /// Finds the key and checks it may send to `project_id` from `origin`.
    async fn check_key(
        &self,
        public_key: &str,
        project_id: i64,
        origin: Option<&str>,
        cors: &mut Cors,
    ) -> Result<KeyInfo, Reject> {
        let k = public_key.to_string();
        let info = self
            .inner
            .core
            .with_conn(move |c| auth::lookup(c, &k))
            .await
            .map_err(|_| Reject::Unavailable("database unavailable"))?
            .ok_or(Reject::Unauthorized("unknown DSN key"))?;
        if info.project_id != project_id {
            return Err(Reject::Forbidden("the DSN key belongs to another project"));
        }
        if !info.enabled {
            return Err(Reject::Forbidden("the DSN key is disabled"));
        }
        if let Some(origin) = origin {
            if !info.allows_origin(origin) {
                return Err(Reject::Forbidden(
                    "this origin may not send events with this key",
                ));
            }
            if !info.any_origin() {
                *cors = Cors::Echo;
            }
        }
        Ok(info)
    }

    /// Turns the body into writer jobs. Returns the event id to answer with.
    fn jobs(&self, parsed: Parsed, key: &KeyInfo) -> Result<(Vec<Job>, Option<String>), Reject> {
        let received = jiff::Timestamp::now();
        let received_at = normalize::sql_time(normalize::ts_seconds(received));
        let ctx = Context {
            project_id: key.project_id,
            keep_user_details: key.keep_user_details,
            hash_key: &self.inner.hash_key,
            received,
        };
        let bad = |e: event::EventError| Reject::BadRequest(e.to_string());
        let env = match parsed {
            Parsed::Event(data) => {
                let record = event::prepare(&data, &ctx, None).map_err(bad)?;
                let id = record.event_id.clone();
                return Ok((vec![Job::Event(Box::new(record))], Some(id)));
            }
            Parsed::Envelope(env) => env,
        };
        let header_id = env.header_str("event_id").and_then(normalize::event_id);
        let mut answer = header_id.clone();
        let mut jobs = Vec::new();
        let mut attachments = Vec::new();
        for item in env.items {
            match item.ty() {
                "event" => {
                    if item.payload.len() > MAX_EVENT {
                        return Err(Reject::TooLarge("the event is too large"));
                    }
                    let record =
                        event::prepare(&item.payload, &ctx, header_id.as_deref()).map_err(bad)?;
                    answer.get_or_insert_with(|| record.event_id.clone());
                    jobs.push(Job::Event(Box::new(record)));
                }
                "session" => jobs.extend(
                    self.session(&item.payload, &ctx, &received_at, false)
                        .map(Job::Session),
                ),
                "sessions" => jobs.extend(
                    self.session(&item.payload, &ctx, &received_at, true)
                        .map(Job::Session),
                ),
                "user_report" | "feedback" => {
                    jobs.extend(
                        feedback(&item, &ctx, &received_at, header_id.as_deref())
                            .map(Job::Feedback),
                    );
                }
                "attachment" => attachments.push(item),
                // transaction, span, profile, profile_chunk, replay_event,
                // replay_recording, replay_video, check_in, statsd,
                // metric_buckets, log, otel_log, client_report and whatever
                // comes next: accepted, not kept.
                _ => {}
            }
        }
        for item in attachments {
            let kind = item
                .header_str("attachment_type")
                .unwrap_or("event.attachment");
            let Some(event_id) = header_id.clone() else {
                continue;
            };
            if matches!(kind, "event.minidump" | "event.applecrashreport")
                || kind.starts_with("unreal.")
            {
                continue;
            }
            if item.payload.len() > MAX_ATTACHMENT {
                tracing::info!(
                    size = item.payload.len(),
                    "dropped an attachment over 10 MB"
                );
                continue;
            }
            jobs.push(Job::Attachment(Box::new(AttachmentItem {
                project_id: key.project_id,
                event_id,
                received_at: received_at.clone(),
                filename: item
                    .header_str("filename")
                    .unwrap_or("attachment")
                    .chars()
                    .take(200)
                    .collect(),
                content_type: item.header_str("content_type").map(str::to_string),
                attachment_type: kind.to_string(),
                size: item.payload.len() as i64,
                data: crate::store::compress(&item.payload),
            })));
        }
        Ok((jobs, answer))
    }

    /// A session update (or aggregate) without IP address or user agent and
    /// with the device id replaced by a hash that changes every day.
    fn session(
        &self,
        payload: &[u8],
        ctx: &Context<'_>,
        received_at: &str,
        aggregate: bool,
    ) -> Option<SessionItem> {
        let Ok(Value::Object(mut s)) = serde_json::from_slice::<Value>(payload) else {
            return None;
        };
        let attrs = s.get_mut("attrs")?.as_object_mut()?;
        attrs.retain(|k, _| !scrub::is_ip_key(k) && k != "user_agent");
        let release = attrs.get("release")?.as_str()?.trim().to_string();
        if release.is_empty() {
            return None;
        }
        let environment = attrs
            .get("environment")
            .and_then(Value::as_str)
            .unwrap_or("production")
            .to_string();
        let day = format!("did:{}", &received_at[..10]);
        let hash_did = |obj: &mut Map<String, Value>| {
            if let Some(did) = obj.get("did").and_then(Value::as_str).map(str::to_string) {
                let h = scrub::keyed_hash(&self.inner.hash_key, ctx.project_id, &day, &did);
                obj.insert("did".into(), Value::from(h));
            }
        };
        if aggregate {
            if let Some(Value::Array(rows)) = s.get_mut("aggregates") {
                rows.iter_mut()
                    .filter_map(Value::as_object_mut)
                    .for_each(hash_did);
            }
        } else {
            hash_did(&mut s);
        }
        Some(SessionItem {
            project_id: ctx.project_id,
            received_at: received_at.to_string(),
            release,
            environment,
            data: Value::Object(s).to_string(),
        })
    }
}

/// A `user_report` (`{event_id, name, email, comments}`) or `feedback` item
/// (an event with a feedback context), reduced to what the issue page will
/// show, with the email hashed unless the project keeps user details.
fn feedback(
    item: &envelope::Item,
    ctx: &Context<'_>,
    received_at: &str,
    header_id: Option<&str>,
) -> Option<FeedbackItem> {
    let Ok(Value::Object(v)) = serde_json::from_slice::<Value>(&item.payload) else {
        return None;
    };
    let s = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
    let (event_id, name, email, message, url) = if item.ty() == "user_report" {
        (
            s(&v, "event_id"),
            s(&v, "name"),
            s(&v, "email"),
            s(&v, "comments"),
            None,
        )
    } else {
        let fb = v
            .get("contexts")
            .and_then(|c| c.get("feedback"))
            .and_then(Value::as_object)?;
        (
            s(fb, "associated_event_id").or_else(|| s(&v, "event_id")),
            s(fb, "name"),
            s(fb, "contact_email"),
            s(fb, "message"),
            s(fb, "url"),
        )
    };
    let email = email.filter(|e| !e.trim().is_empty()).map(|e| {
        if ctx.keep_user_details {
            e
        } else {
            format!(
                "hash:{}",
                scrub::keyed_hash(ctx.hash_key, ctx.project_id, "email", e.trim())
            )
        }
    });
    let message = message.map(|m| scrub::scrub_text(&m).unwrap_or(m));
    let url = url.map(|u| scrub::scrub_query_in_url(&u).unwrap_or(u));
    let event_id = event_id
        .as_deref()
        .and_then(normalize::event_id)
        .or_else(|| header_id.and_then(normalize::event_id));
    let data = json!({"name": name, "email": email, "message": message, "url": url});
    Some(FeedbackItem {
        project_id: ctx.project_id,
        event_id,
        received_at: received_at.to_string(),
        data: data.to_string(),
    })
}

fn cors_headers(h: &mut HeaderMap, cors: Cors, origin: &str) {
    match (cors, HeaderValue::from_str(origin)) {
        (Cors::Echo, Ok(v)) => {
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
            h.append(header::VARY, HeaderValue::from_static("Origin"));
        }
        _ => {
            h.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        }
    }
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(EXPOSE_HEADERS),
    );
}

async fn envelope_route(
    State(ingest): State<Ingest>,
    UrlPath(project): UrlPath<String>,
    RawQuery(query): RawQuery,
    PeerAddr(peer): PeerAddr,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    ingest
        .handle(Incoming {
            endpoint: Endpoint::Envelope,
            project,
            uri,
            query,
            peer,
            headers,
            body,
        })
        .await
}

async fn store_route(
    State(ingest): State<Ingest>,
    UrlPath(project): UrlPath<String>,
    RawQuery(query): RawQuery,
    PeerAddr(peer): PeerAddr,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    ingest
        .handle(Incoming {
            endpoint: Endpoint::Store,
            project,
            uri,
            query,
            peer,
            headers,
            body,
        })
        .await
}

/// CORS preflight. The key isn't known yet (browsers send no credentials
/// with a preflight), so any origin may ask; the POST itself is checked.
async fn preflight() -> Response {
    let mut res = StatusCode::OK.into_response();
    let h = res.headers_mut();
    cors_headers(h, Cors::Any, "*");
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(ALLOW_HEADERS),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("3600"),
    );
    res
}

/// Whether `path` is an ingest route (exempt from the CSRF check).
pub fn is_ingest_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/") else {
        return false;
    };
    let Some((id, tail)) = rest.split_once('/') else {
        return false;
    };
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && matches!(tail, "envelope" | "envelope/" | "store" | "store/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_paths() {
        for p in [
            "/api/1/envelope/",
            "/api/42/store/",
            "/api/7/envelope",
            "/api/7/store",
        ] {
            assert!(is_ingest_path(p), "{p}");
        }
        for p in [
            "/api/v1/projects",
            "/api//envelope/",
            "/api/1/envelope/x",
            "/api/1x/store/",
            "/apix/1/store/",
        ] {
            assert!(!is_ingest_path(p), "{p}");
        }
    }
}
