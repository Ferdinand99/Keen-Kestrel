//! `POST /ingest/v1/event` (docs/design.md §5.3): pageviews and custom
//! events from `k.js`. The body is JSON, sent as `text/plain` (no CORS
//! preflight; what `sendBeacon` and k.js's `fetch` send) or as
//! `application/json`:
//!
//! ```json
//! {"k": "<public key>", "n": "pageview", "u": "https://shop.example/pricing?utm_source=x",
//!  "r": "https://www.google.com/", "w": 1440, "h": 0, "p": {"plan": "pro"}}
//! ```
//!
//! (`key`, `name`, `url`, `referrer`, `screen_width`, `hash` and `props` are
//! accepted too.) The key may also come in `X-Kestrel-Key` or `?key=`.
//!
//! Checked in this order: the per-IP bucket, Global Privacy Control
//! (`Sec-GPC: 1`: answered 202 and dropped; `DNT` isn't acted on), bots
//! (answered 202 and dropped), the body (64 KB at most), the key (it must exist and be
//! enabled, and its project must take web analytics), the page's origin
//! (`Origin`, else `Referer`) against the key's allowed origins, the key's
//! own bucket. Then one [`WebEvent`] goes to the writer and the answer is
//! `202 Accepted` with an empty body. Nothing here sets a cookie, and the IP
//! address and user agent only feed the visitor hash and the browser, OS
//! and device class.

use std::net::SocketAddr;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use vole_kit::auth::guard::PeerAddr;
use vole_kit::auth::session;

use super::{Cors, Ingest, Reject, auth, body, cors_headers};
use crate::analytics::{self, PAGEVIEW, WebEvent, clean, ua};
use crate::writer::Job;

/// Largest request body, compressed or not.
pub const MAX_BODY: usize = 64 * 1024;

/// Headers a page may send with `application/json` (preflight).
const ALLOW_HEADERS: &str = "content-type, x-kestrel-key";

/// The origin of the page that sent the request: `Origin`, else the
/// `Referer`'s scheme, host and port.
fn page_origin(headers: &HeaderMap) -> Option<String> {
    let get = |h: header::HeaderName| {
        headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "null")
            .map(str::to_string)
    };
    get(header::ORIGIN).or_else(|| {
        let referer = get(header::REFERER)?;
        let url = url::Url::parse(&referer).ok()?;
        Some(url.origin().ascii_serialization()).filter(|o| o != "null")
    })
}

/// Whether the browser sends Global Privacy Control (`Sec-GPC: 1`): the
/// visitor doesn't want to be counted, so nothing is stored.
pub fn gpc(headers: &HeaderMap) -> bool {
    headers
        .get("sec-gpc")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "1")
}

fn str_field<'a>(body: &'a Map<String, Value>, short: &str, long: &str) -> Option<&'a str> {
    body.get(short)
        .or_else(|| body.get(long))
        .and_then(Value::as_str)
}

fn reject(r: Reject) -> Response {
    let detail = r.detail();
    let status = r.status();
    let mut res = (status, axum::Json(json!({ "detail": detail }))).into_response();
    if let Reject::RateLimited { secs, .. } = r {
        res.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    }
    res
}

fn accepted() -> Response {
    let mut res = StatusCode::ACCEPTED.into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// `POST /ingest/v1/event`.
pub async fn event_route(
    State(ingest): State<Ingest>,
    RawQuery(query): RawQuery,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let mut cors = Cors::Any;
    let mut res = match ingest
        .web_event(query.as_deref(), peer, &headers, body, &mut cors)
        .await
    {
        Ok(()) => accepted(),
        Err(r) => reject(r),
    };
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        cors_headers(res.headers_mut(), cors, origin);
    }
    res
}

/// CORS preflight for `application/json` and `X-Kestrel-Key` (k.js itself
/// sends `text/plain`, which needs none).
pub async fn preflight() -> Response {
    let mut res = StatusCode::NO_CONTENT.into_response();
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
        HeaderValue::from_static("86400"),
    );
    res
}

impl Ingest {
    async fn web_event(
        &self,
        query: Option<&str>,
        peer: Option<SocketAddr>,
        headers: &HeaderMap,
        raw_body: Body,
        cors: &mut Cors,
    ) -> Result<(), Reject> {
        let inner = &self.inner;
        let now = Instant::now();
        let ip = session::client_ip(headers, peer, session::trust_proxy());
        inner.take_ip(ip, 1, "ip", now)?;
        if gpc(headers) {
            return Ok(());
        }
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if ua::is_bot(user_agent) || ua::is_automated(headers) {
            return Ok(());
        }
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let media = content_type.split(';').next().unwrap_or("").trim();
        if !matches!(media, "" | "text/plain" | "application/json") {
            return Err(Reject::Unsupported(
                "send JSON as text/plain or application/json",
            ));
        }
        let raw = axum::body::to_bytes(raw_body, MAX_BODY)
            .await
            .map_err(|_| Reject::TooLarge("the request body is too large"))?;
        let encoding = headers
            .get(header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok());
        let data = body::decode(encoding, &raw, MAX_BODY)?;
        let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(&data) else {
            return Err(Reject::BadRequest("the body must be a JSON object".into()));
        };

        let given = headers
            .get("x-kestrel-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| str_field(&fields, "k", "key").map(str::to_string))
            .or_else(|| {
                url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
                    .find(|(k, _)| k == "key")
                    .map(|(_, v)| v.into_owned())
            })
            .ok_or(Reject::Unauthorized("missing key"))?;
        let public_key = auth::checked(&given).ok_or(Reject::Unauthorized("invalid key"))?;
        let k = public_key.clone();
        let key = inner
            .core
            .with_conn(move |c| auth::lookup(c, &k))
            .await
            .map_err(|_| Reject::Unavailable("database unavailable"))?
            .ok_or(Reject::Unauthorized("unknown key"))?;
        if !key.enabled {
            return Err(Reject::Forbidden("the key is disabled"));
        }
        if !key.web_analytics {
            return Err(Reject::Forbidden("web analytics is off for this project"));
        }
        if !key.any_origin() {
            match page_origin(headers) {
                Some(origin) if key.allows_origin(&origin) => *cors = Cors::Echo,
                _ => {
                    return Err(Reject::Forbidden(
                        "this origin may not send events with this key",
                    ));
                }
            }
        }
        inner.take_key(key.id, key.rate_limit_per_minute, 1, now)?;
        if inner.writer.store_full() {
            return Err(super::store_full());
        }

        let name = match str_field(&fields, "n", "name").map(str::trim) {
            None | Some(PAGEVIEW) | Some("") => PAGEVIEW.to_string(),
            Some(other) => clean::event_name(other)
                .ok_or_else(|| Reject::BadRequest("invalid event name".into()))?,
        };
        let hash = matches!(
            fields.get("h").or_else(|| fields.get("hash")),
            Some(Value::Bool(true))
        ) || fields
            .get("h")
            .or_else(|| fields.get("hash"))
            .and_then(Value::as_i64)
            == Some(1);
        let page = str_field(&fields, "u", "url")
            .and_then(|u| clean::page(u, hash))
            .ok_or_else(|| Reject::BadRequest("missing or invalid page URL".into()))?;
        let referrer = clean::referrer_domain(str_field(&fields, "r", "referrer"), &page.hostname);
        let width = fields
            .get("w")
            .or_else(|| fields.get("screen_width"))
            .and_then(Value::as_f64)
            .filter(|w| w.is_finite() && *w > 0.0)
            .map(|w| w as u64);
        let props = if name == PAGEVIEW {
            None
        } else {
            clean::props(fields.get("p").or_else(|| fields.get("props")))
        };
        let agent = ua::parse(user_agent);
        let ts = jiff::Timestamp::now().as_second();
        let visitor = inner.salt.visitor(
            analytics::day_number(ts),
            key.project_id,
            ip,
            user_agent,
            &page.hostname,
        );
        let event = WebEvent {
            project_id: key.project_id,
            ts,
            visitor,
            name,
            page,
            referrer,
            browser: agent.browser,
            os: agent.os,
            device: agent.device,
            screen: clean::screen_bucket(width),
            props,
        };
        inner
            .writer
            .submit(vec![Job::Web(Box::new(event))])
            .map_err(|_| Reject::RateLimited {
                secs: 1,
                scope: "organization",
            })
    }
}
