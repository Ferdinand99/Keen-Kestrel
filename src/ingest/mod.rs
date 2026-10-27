//! Sentry-compatible ingest (docs/design.md §4.2): `POST
//! /api/{project_id}/envelope/` and the legacy `POST /api/{project_id}/store/`;
//! web analytics ([`web`]: `POST /ingest/v1/event`, with `GET /js/k.js`) and
//! app signals ([`signals`]: `POST /ingest/v1/signals`).
//!
//! A request is checked in this order: the per-IP bucket, the project id,
//! the writer's queue (a full one answers 429 before anything is read), the
//! DSN key (`X-Sentry-Auth` or `?sentry_key=`, else the `dsn` in the
//! envelope's header, read from the first 64 KiB of the body alone) with its
//! project, origin and bucket, and only then the body: read, decoded
//! (bounded) and scanned item by item ([`envelope::Scanner`]). The IP and key
//! buckets are charged for every item of the envelope. The items are
//! normalised and grouped, and the jobs go to the writer's queue; the answer
//! is `{"id": "<event_id>"}` without waiting for the write.
//!
//! Envelope items: `event` is kept; `session` / `sessions` and
//! `user_report` / `feedback` are kept for later phases (up to 4 KiB each,
//! larger ones are dropped); `attachment` is kept when its event is (up to 5
//! of 1 MiB each); `transaction`, `span`, `profile`, `replay_*`,
//! `check_in`, `statsd`, `metrics`, `log`, `client_report`, minidumps and
//! unknown types are accepted and dropped. An envelope with more than 100
//! items or 20 events is refused with 413 (docs/design.md §7).

pub mod auth;
pub mod body;
pub mod capture;
pub mod envelope;
pub mod signals;
pub mod web;

use std::borrow::Cow;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::{Body, HttpBody as _};
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::map_response;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use deadpool_sqlite::Pool;
use futures_util::StreamExt as _;
use serde_json::{Map, Value, json};
use vole_kit::auth::guard::PeerAddr;
use vole_kit::auth::session;
use vole_kit::db::settings;
use vole_kit::http::headers;
use vole_kit::http::ratelimit::{Rate, TokenBuckets, retry_after_secs};
use vole_kit::state::Core;

use crate::difs::Difs;
use crate::difs::worker::{Symbolicator, Work, WorkerSettings};
use crate::event::{self, Context, normalize, scrub};
use crate::writer::{AttachmentItem, FeedbackItem, Job, SessionItem, Writer, WriterSettings};
use auth::KeyInfo;
use body::{BodyError, MAX_ATTACHMENT, MAX_ENVELOPE, MAX_EVENT};
use envelope::EnvelopeError;

/// The setting that holds the instance's key for user hashes.
const HASH_KEY_SETTING: &str = "ingest_hash_key";

/// Items per minute a key without a limit of its own may send, per DSN key
/// (`rate_limit_per_minute` NULL): events, sessions, pageviews or signals,
/// one token each. A project that sends more sets a limit on its key.
pub const DEFAULT_KEY_RATE_LIMIT: u32 = 1000;
/// Attachments kept per event at most (more are accepted and dropped).
pub const MAX_ATTACHMENTS: usize = 5;
/// A session, session aggregate, user report or feedback item's size at
/// most (larger ones are accepted and dropped).
pub const MAX_SMALL_ITEM: usize = 4 * 1024;

/// A release name's length at most, in characters (as for events).
const MAX_RELEASE: usize = 200;
/// An environment name's length at most, in characters (as for events).
const MAX_ENVIRONMENT: usize = 64;

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
    /// Items per minute from one IP address; 0 turns the limit off. An IPv6
    /// address stands for its /64.
    pub ip_rate_limit_per_minute: u32,
    /// Items per minute a key without a limit of its own may send (0: no
    /// limit for those keys).
    pub key_rate_limit_per_minute: u32,
    pub writer: WriterSettings,
    /// The symbolication queue (docs/design.md §4.8).
    pub symbolication: WorkerSettings,
    /// Alert delivery after the writer (docs/design.md §4.7).
    pub alerts: crate::alerts::Settings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            capture_dir: None,
            ip_rate_limit_per_minute: 6000,
            key_rate_limit_per_minute: DEFAULT_KEY_RATE_LIMIT,
            writer: WriterSettings::default(),
            symbolication: WorkerSettings::default(),
            alerts: crate::alerts::Settings::default(),
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
    difs: Difs,
    symbolicator: Symbolicator,
    store: Pool,
    settings: Settings,
    hash_key: [u8; 32],
    /// Items per DSN key and limit (its `rate_limit_per_minute`, else the
    /// default), in memory. A changed limit starts a full bucket.
    key_buckets: TokenBuckets<(i64, u32)>,
    /// Items per client (`ip_rate_limit_per_minute`): an IPv4 address or an
    /// IPv6 /64.
    ip_buckets: TokenBuckets<IpAddr>,
    /// The web visitor ids' salt (memory only, docs/design.md §5.2).
    salt: crate::analytics::DailySalt,
    /// App signals dropped for breaking the protocol's limits.
    dropped_signals: std::sync::atomic::AtomicU64,
}

/// The address a client's budget is kept under: an IPv6 client owns a whole
/// /64 (one customer's prefix), so its addresses can't be used to dodge the
/// limit; an IPv4-mapped address counts as the IPv4 one.
pub fn bucket_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(Ipv6Addr::from(octets))
            }
        },
    }
}

impl Inner {
    /// Takes `n` items from `ip`'s budget (`ip_rate_limit_per_minute`, 0 for
    /// none); `scope` names it in `X-Sentry-Rate-Limits`.
    fn take_ip(
        &self,
        ip: Option<IpAddr>,
        n: u32,
        scope: &'static str,
        now: Instant,
    ) -> Result<(), Reject> {
        let per_ip = self.settings.ip_rate_limit_per_minute;
        match ip {
            Some(ip) if per_ip > 0 => self
                .ip_buckets
                .take_n(&bucket_ip(ip), n.min(per_ip), Rate::per_minute(per_ip), now)
                .map_err(|wait| Reject::RateLimited {
                    secs: retry_after_secs(wait),
                    scope,
                }),
            _ => Ok(()),
        }
    }

    /// Takes `n` items from DSN key `key_id`'s budget: its own limit, else
    /// the default (`key_rate_limit_per_minute`).
    fn take_key(
        &self,
        key_id: i64,
        per_minute: Option<i64>,
        n: u32,
        now: Instant,
    ) -> Result<(), Reject> {
        let limit = match per_minute {
            Some(limit) => u32::try_from(limit).unwrap_or(u32::MAX),
            None => self.settings.key_rate_limit_per_minute,
        };
        if limit == 0 {
            return Ok(());
        }
        self.key_buckets
            .take_n(&(key_id, limit), n.min(limit), Rate::per_minute(limit), now)
            .map_err(|wait| Reject::RateLimited {
                secs: retry_after_secs(wait),
                scope: "key",
            })
    }

    /// Takes `n` items from both budgets of a request.
    fn charge(
        &self,
        ip: Option<IpAddr>,
        key: &KeyInfo,
        n: u32,
        scope: &'static str,
        now: Instant,
    ) -> Result<(), Reject> {
        self.take_ip(ip, n, scope, now)?;
        self.take_key(key.id, key.rate_limit_per_minute, n, now)
    }
}

/// Why a request is refused.
#[derive(Debug)]
pub enum Reject {
    BadRequest(String),
    Unauthorized(&'static str),
    Forbidden(&'static str),
    TooLarge(&'static str),
    RateLimited {
        secs: u64,
        scope: &'static str,
    },
    Unavailable(&'static str),
    /// 503 with `Retry-After` (app signals: the queue is full).
    Busy {
        secs: u64,
        detail: &'static str,
    },
    Unsupported(&'static str),
}

impl Reject {
    fn status(&self) -> StatusCode {
        match self {
            Reject::BadRequest(_) => StatusCode::BAD_REQUEST,
            Reject::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Reject::Forbidden(_) => StatusCode::FORBIDDEN,
            Reject::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Reject::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Reject::Unavailable(_) | Reject::Busy { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Reject::Unsupported(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        }
    }

    /// Seconds for `Retry-After`, when the client should wait.
    fn retry_after(&self) -> Option<u64> {
        match self {
            Reject::RateLimited { secs, .. } | Reject::Busy { secs, .. } => Some(*secs),
            _ => None,
        }
    }

    fn detail(&self) -> String {
        match self {
            Reject::BadRequest(d) => d.clone(),
            Reject::Unauthorized(d)
            | Reject::Forbidden(d)
            | Reject::TooLarge(d)
            | Reject::Unavailable(d)
            | Reject::Unsupported(d)
            | Reject::Busy { detail: d, .. } => (*d).to_string(),
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
        if let Some(secs) = self.retry_after() {
            h.insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        if let Reject::RateLimited { secs, scope } = self
            && let Ok(v) = HeaderValue::from_str(&format!("{secs}::{scope}"))
        {
            h.insert("x-sentry-rate-limits", v);
        }
        res
    }
}

impl From<EnvelopeError> for Reject {
    fn from(e: EnvelopeError) -> Self {
        match e {
            EnvelopeError::Limit(why) => Reject::TooLarge(why),
            invalid @ EnvelopeError::Invalid(_) => Reject::BadRequest(invalid.to_string()),
        }
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

/// What a request that was accepted answers.
struct Done {
    /// The event id (`{"id": …}`), if there is one.
    id: Option<String>,
    /// `X-Sentry-Rate-Limits` for the SDK on a 200: it should stop sending
    /// these categories for a while.
    limits: Option<&'static str>,
}

/// What an envelope turned into.
struct Built {
    work: Vec<Work>,
    id: Option<String>,
    /// The event payloads' size, for the symbolication queue's byte budget.
    bytes: usize,
    limits: Option<&'static str>,
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
    /// Opens the store, loads (or makes) the hash key and starts the writer,
    /// which reports to `alerts`.
    pub async fn start(
        core: Core,
        data_dir: &Path,
        settings: Settings,
        alerts: Option<crate::alerts::Sink>,
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
            alerts,
        )?;
        let dif_ids = core
            .with_conn(|c| crate::db::difs::all_ids(c))
            .await
            .map_err(|_| "can't read the debug files")?;
        let difs = Difs::open(data_dir, dif_ids).map_err(|e| {
            format!(
                "can't open the debug files directory in {}: {e}",
                data_dir.display()
            )
        })?;
        let symbolicator =
            Symbolicator::start(difs.clone(), writer.clone(), settings.symbolication);
        if let Some(dir) = &settings.capture_dir {
            tracing::warn!(dir = %dir.display(), "capture mode: every ingest request is written to disk");
        }
        Ok(Ingest {
            inner: Arc::new(Inner {
                core,
                writer,
                difs,
                symbolicator,
                store,
                settings,
                hash_key,
                key_buckets: TokenBuckets::new(),
                ip_buckets: TokenBuckets::new(),
                salt: crate::analytics::DailySalt::new(),
                dropped_signals: std::sync::atomic::AtomicU64::new(0),
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

    /// The debug files.
    pub fn difs(&self) -> &Difs {
        &self.inner.difs
    }

    pub fn symbolicator(&self) -> &Symbolicator {
        &self.inner.symbolicator
    }

    /// Waits until every accepted job is symbolicated and written.
    pub async fn flush(&self) {
        self.inner.symbolicator.flush().await;
        self.inner.writer.flush().await;
    }

    /// App signals dropped so far for breaking the protocol's limits.
    pub fn dropped_signals(&self) -> u64 {
        self.inner
            .dropped_signals
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The key session device ids and user ids are hashed with.
    pub fn hash_key(&self) -> &[u8; 32] {
        &self.inner.hash_key
    }

    /// The ingest routes, for the main router and the ingest port: SDK and
    /// k.js endpoints with the headers of an endpoint for programs, and
    /// k.js itself as a file other sites load.
    pub fn router<S: Clone + Send + Sync + 'static>(&self) -> Router<S> {
        let snippet = Router::new()
            .route("/js/k.js", axum::routing::get(crate::analytics::kjs::serve))
            .route_layer(map_response(headers::public_asset));
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
            .route(
                "/ingest/v1/event",
                post(web::event_route).options(web::preflight),
            )
            .route("/ingest/v1/signals", post(signals::route))
            .route_layer(map_response(headers::machine))
            .merge(snippet)
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
            Ok(Done { id, limits }) => {
                let mut res = match id {
                    Some(id) => axum::Json(json!({ "id": id })).into_response(),
                    None => axum::Json(json!({})).into_response(),
                };
                if let Some(limits) = limits {
                    res.headers_mut()
                        .insert("x-sentry-rate-limits", HeaderValue::from_static(limits));
                }
                res
            }
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
    ) -> Result<Done, Reject> {
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
        let ip = session::client_ip(headers, peer, session::trust_proxy());
        inner.take_ip(ip, 1, "organization", now)?;
        let project_id: i64 = project
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| Reject::BadRequest("invalid project id".into()))?;
        let limit = match endpoint {
            Endpoint::Envelope => MAX_ENVELOPE,
            Endpoint::Store => MAX_EVENT + STORE_SLACK,
        };
        if body.size_hint().upper().is_some_and(|n| n > limit as u64) {
            return Err(Reject::TooLarge("the request body is too large"));
        }
        // A full queue is answered before anything is read or decoded.
        if !inner.writer.has_room() {
            return Err(queue_full());
        }

        // Who is sending comes before what: the key in a header or the query
        // is checked before the body is read at all.
        let mut key = match auth::from_request(headers, query.as_deref()) {
            Some(Ok(k)) => Some(self.check_key(&k, project_id, origin, cors).await?),
            Some(Err(())) => return Err(Reject::Unauthorized("invalid sentry_key")),
            None => None,
        };
        if let Some(key) = &key {
            inner.take_key(key.id, key.rate_limit_per_minute, 1, now)?;
        }
        let encoding = headers
            .get(header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut stream = body.into_data_stream();
        let mut raw: Vec<u8> = Vec::new();
        if key.is_none() {
            // Only the envelope's own `dsn` can name the key now. Read just
            // enough of the body to see the envelope header, and check the
            // key before the rest is read or decoded.
            if endpoint == Endpoint::Store {
                return Err(Reject::Unauthorized("missing or invalid authentication"));
            }
            read_into(&mut stream, &mut raw, PEEK, limit).await?;
            let head = body::decode_prefix(encoding.as_deref(), &raw, PEEK)?;
            let line = match head.iter().position(|b| *b == b'\n') {
                Some(end) => &head[..end],
                None if head.len() <= envelope::MAX_HEADER => &head[..],
                None => return Err(Reject::TooLarge("the envelope header is too large")),
            };
            let (header, _) = envelope::header(line).map_err(Reject::from)?;
            let public_key = header
                .get("dsn")
                .and_then(Value::as_str)
                .and_then(auth::from_dsn)
                .and_then(|(k, _)| auth::checked(&k))
                .ok_or(Reject::Unauthorized("missing or invalid authentication"))?;
            let found = self
                .check_key(&public_key, project_id, origin, cors)
                .await?;
            inner.take_key(found.id, found.rate_limit_per_minute, 1, now)?;
            key = Some(found);
        }
        let Some(key) = key else {
            return Err(Reject::Unauthorized("missing authentication"));
        };
        read_into(&mut stream, &mut raw, usize::MAX, limit).await?;
        drop(stream);

        let capture = inner
            .settings
            .capture_dir
            .clone()
            .map(|dir| (dir, uri.to_string(), headers.clone()));
        let decoded = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, Reject> {
            if let Some((dir, uri, headers)) = capture
                && let Err(err) = capture::write(&dir, "POST", &uri, &headers, &raw)
            {
                tracing::warn!(%err, "can't write the capture file");
            }
            let decoded_limit = match endpoint {
                Endpoint::Envelope => MAX_ENVELOPE,
                Endpoint::Store => MAX_EVENT,
            };
            // A body without an encoding is used as it is, not copied.
            let owned = match body::decode_cow(encoding.as_deref(), &raw, decoded_limit)? {
                Cow::Owned(data) => Some(data),
                Cow::Borrowed(_) => None,
            };
            Ok(owned.unwrap_or(raw))
        })
        .await
        .map_err(|_| Reject::Unavailable("ingest failed"))??;

        // The queues may have filled while the body came in; a project with
        // debug files also needs room in the symbolication queue.
        if !inner.writer.has_room()
            || (inner.difs.has_any(key.project_id) && !inner.symbolicator.has_room())
        {
            return Err(queue_full());
        }
        let this = self.clone();
        let built =
            tokio::task::spawn_blocking(move || this.build(endpoint, &decoded, &key, ip, now))
                .await
                .map_err(|_| Reject::Unavailable("ingest failed"))??;
        let Built {
            work,
            id,
            bytes,
            limits,
        } = built;
        if work.iter().any(|w| matches!(w, Work::Event(_))) {
            inner
                .symbolicator
                .submit(work, bytes)
                .map_err(|_| queue_full())?;
        } else {
            let jobs = work
                .into_iter()
                .filter_map(|w| match w {
                    Work::Job(j) => Some(j),
                    Work::Event(_) => None,
                })
                .collect();
            inner.writer.submit(jobs).map_err(|_| queue_full())?;
        }
        Ok(Done { id, limits })
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

    /// An event as a writer job, or as work for the symbolication queue when
    /// the project has a debug file for one of its images.
    fn event_work(&self, prepared: event::Prepared) -> Work {
        let difs = &self.inner.difs;
        let project_id = prepared.project_id;
        if difs.has_any(project_id)
            && crate::difs::symbolicate::wants(&prepared.event, |id| difs.has(project_id, id))
        {
            Work::Event(Box::new(prepared))
        } else {
            Work::Job(Job::Event(Box::new(event::finish(prepared))))
        }
    }

    /// Turns a decoded body into writer jobs (some through the symbolication
    /// queue). An envelope is scanned for its items first, which reads their
    /// headers only, and the IP and key budgets are charged for all of them
    /// before any payload is looked at. Returns the event id to answer with.
    fn build(
        &self,
        endpoint: Endpoint,
        data: &[u8],
        key: &KeyInfo,
        ip: Option<IpAddr>,
        now: Instant,
    ) -> Result<Built, Reject> {
        let received = jiff::Timestamp::now();
        let received_at = normalize::sql_time(normalize::ts_seconds(received));
        let ctx = Context {
            project_id: key.project_id,
            keep_user_details: key.keep_user_details,
            hash_key: &self.inner.hash_key,
            received,
        };
        let bad = |e: event::EventError| Reject::BadRequest(e.to_string());
        if endpoint == Endpoint::Store {
            let prepared = event::parse(data, &ctx, None).map_err(bad)?;
            let id = prepared.event_id.clone();
            return Ok(Built {
                work: vec![self.event_work(prepared)],
                id: Some(id),
                bytes: data.len(),
                limits: None,
            });
        }
        let (env_header, rest) = envelope::header(data).map_err(Reject::from)?;
        let mut scanner = envelope::Scanner::new(rest);
        let mut items = Vec::new();
        while let Some(item) = scanner.next_item().map_err(Reject::from)? {
            items.push(item);
        }
        // One item has been paid for (the request); the others now.
        if let Some(more) = u32::try_from(items.len().saturating_sub(1))
            .ok()
            .filter(|n| *n > 0)
        {
            self.inner.charge(ip, key, more, "organization", now)?;
        }
        let store_full = self.inner.writer.store_full();
        let mut limits = None;
        let header_id = env_header
            .get("event_id")
            .and_then(Value::as_str)
            .and_then(normalize::event_id);
        let mut answer = header_id.clone();
        let mut work = Vec::new();
        let mut attachments = Vec::new();
        let mut bytes = 0;
        for item in items {
            match item.ty() {
                "event" => {
                    if item.payload.len() > MAX_EVENT {
                        return Err(Reject::TooLarge("the event is too large"));
                    }
                    bytes += item.payload.len();
                    let prepared =
                        event::parse(&item.payload, &ctx, header_id.as_deref()).map_err(bad)?;
                    answer.get_or_insert_with(|| prepared.event_id.clone());
                    work.push(self.event_work(prepared));
                }
                // The small ones: a larger one is not what an SDK sends.
                "session" | "sessions" | "user_report" | "feedback"
                    if item.payload.len() > MAX_SMALL_ITEM =>
                {
                    tracing::debug!(size = item.payload.len(), "dropped an oversized small item");
                }
                "session" => work.extend(
                    self.session(&item.payload, &ctx, &received_at, false)
                        .map(|s| Work::Job(Job::Session(s))),
                ),
                "sessions" => work.extend(
                    self.session(&item.payload, &ctx, &received_at, true)
                        .map(|s| Work::Job(Job::Session(s))),
                ),
                "user_report" | "feedback" if store_full => {}
                "user_report" | "feedback" => {
                    work.extend(
                        feedback(&item, &ctx, &received_at, header_id.as_deref())
                            .map(|f| Work::Job(Job::Feedback(f))),
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
        let mut kept = 0;
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
            if store_full {
                // Tell the SDK to leave attachments out for a while.
                limits = Some("300:attachment:organization");
                continue;
            }
            if kept >= MAX_ATTACHMENTS {
                tracing::debug!("dropped an attachment: more than {MAX_ATTACHMENTS} for the event");
                continue;
            }
            if item.payload.len() > MAX_ATTACHMENT {
                tracing::debug!(
                    size = item.payload.len(),
                    "dropped an attachment over 1 MiB"
                );
                continue;
            }
            kept += 1;
            bytes += item.payload.len();
            work.push(Work::Job(Job::Attachment(Box::new(AttachmentItem {
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
            }))));
        }
        Ok(Built {
            work,
            id: answer,
            bytes,
            limits,
        })
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
        // Cut like an event's release and environment: they end up in the
        // releases list for good.
        let release: String = attrs
            .get("release")?
            .as_str()?
            .trim()
            .chars()
            .take(MAX_RELEASE)
            .collect();
        if release.is_empty() {
            return None;
        }
        let environment: String = attrs
            .get("environment")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|e| !e.is_empty() && !e.contains(['\n', '/']))
            .unwrap_or("production")
            .chars()
            .take(MAX_ENVIRONMENT)
            .collect();
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
    item: &envelope::Item<'_>,
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

/// The data volume is low on space or the store is over its limit
/// (`KK_MAX_STORE_MB`): web events and app signals, which are only raw rows,
/// get 429 with a minute's wait. Errors are still counted
/// (docs/design.md §7).
fn store_full() -> Reject {
    Reject::RateLimited {
        secs: 60,
        scope: "organization",
    }
}

/// The writer (or the symbolication queue) is full: 429 with a short wait.
fn queue_full() -> Reject {
    Reject::RateLimited {
        secs: 1,
        scope: "organization",
    }
}

/// The compressed bytes of an envelope's start that are read before its key
/// is known (the envelope header decoded from them is at most
/// [`envelope::MAX_HEADER`]).
const PEEK: usize = envelope::MAX_HEADER + 1;
/// What a `/store/` body may be besides its event, compressed.
const STORE_SLACK: usize = 64 * 1024;

/// Reads `stream` into `raw` until `raw` holds `until` bytes or the stream
/// ends, but never more than `limit` bytes in all.
async fn read_into(
    stream: &mut axum::body::BodyDataStream,
    raw: &mut Vec<u8>,
    until: usize,
    limit: usize,
) -> Result<(), Reject> {
    while raw.len() < until {
        match stream.next().await {
            None => return Ok(()),
            Some(Err(_)) => return Err(Reject::BadRequest("can't read the request body".into())),
            Some(Ok(chunk)) => {
                if raw.len() + chunk.len() > limit {
                    return Err(Reject::TooLarge("the request body is too large"));
                }
                raw.extend_from_slice(&chunk);
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv6_client_is_its_64_and_a_mapped_address_is_ipv4() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(bucket_ip(ip("203.0.113.7")), ip("203.0.113.7"));
        assert_eq!(
            bucket_ip(ip("2001:db8:1:2:aaaa:bbbb:cccc:dddd")),
            ip("2001:db8:1:2::")
        );
        assert_eq!(
            bucket_ip(ip("2001:db8:1:2::1")),
            bucket_ip(ip("2001:db8:1:2::2"))
        );
        assert_ne!(
            bucket_ip(ip("2001:db8:1:2::1")),
            bucket_ip(ip("2001:db8:1:3::1"))
        );
        assert_eq!(bucket_ip(ip("::ffff:203.0.113.7")), ip("203.0.113.7"));
    }
}
