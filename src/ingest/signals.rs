//! `POST /ingest/v1/signals` (docs/design.md §5.3): app signals from
//! KestrelKit, as its protocol describes them (KestrelKit's
//! `docs/protocol.md`, v1 with proposals P1–P13):
//!
//! ```json
//! {"sent_at":"2026-10-22T19:00:00.000Z","sdk":{"name":"kestrelkit","version":"0.1.0"},
//!  "signals":[{"type":"App.launched","time":"2026-10-22T18:59:58.123Z",
//!    "install_id":"8F3C…","session_id":"…","app":{"bundle_id":"…","version":"1.4.0","build":"57"},
//!    "device":{"platform":"iOS","os_version":"18.2","model":"iPhone16,2","locale":"nb_NO"},
//!    "flags":{"debug":false,"testflight":false,"simulator":false},"payload":{"plan":"pro"}}]}
//! ```
//!
//! The key is the project's public key in `X-Kestrel-Key`. Checked in this
//! order: the per-IP bucket (429), the key (401 missing or unknown, 403
//! disabled, or app analytics off for its project), the key's bucket (429; a
//! key without a limit of its own gets the default), a full store (429), the
//! body (256 KB at most, also once gzip is undone: 413), JSON with a
//! `signals` array (400), at most 100 signals (413). Signals that break a
//! limit on their own are cut or dropped, not the batch (P5), among them the
//! ones more than 48 hours old; `accepted` counts the stored ones, and each
//! of them takes one token from the IP's and the key's bucket. A full queue
//! answers 503. 429 and 503 carry `Retry-After` in seconds (P8). The user agent and IP address only feed
//! the in-memory rate limit; nothing here sets a cookie.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use vole_kit::auth::guard::PeerAddr;
use vole_kit::auth::session;

use super::{Ingest, Reject, auth, body};
use crate::analytics::app::{self, BatchError, Ctx, MAX_BODY};
use crate::writer::Job;

fn reject(r: Reject) -> Response {
    let detail = r.detail();
    let status = r.status();
    let mut res = (status, axum::Json(json!({ "detail": detail }))).into_response();
    if let Some(secs) = r.retry_after() {
        res.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    }
    no_store(res)
}

fn no_store(mut res: Response) -> Response {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// `POST /ingest/v1/signals`.
pub async fn route(
    State(ingest): State<Ingest>,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    body: Body,
) -> Response {
    match ingest.signals(peer, &headers, body).await {
        Ok((accepted, dropped)) => {
            let body = if dropped > 0 {
                json!({"accepted": accepted, "dropped": dropped})
            } else {
                json!({"accepted": accepted})
            };
            no_store((StatusCode::ACCEPTED, axum::Json(body)).into_response())
        }
        Err(r) => reject(r),
    }
}

impl Ingest {
    /// Checks, decodes and cleans a batch and queues its signals; returns
    /// how many were accepted and dropped.
    async fn signals(
        &self,
        peer: Option<SocketAddr>,
        headers: &HeaderMap,
        raw_body: Body,
    ) -> Result<(usize, usize), Reject> {
        let inner = &self.inner;
        let now = Instant::now();
        let ip = session::client_ip(headers, peer, session::trust_proxy());
        inner.take_ip(ip, 1, "ip", now)?;
        let given = headers
            .get("x-kestrel-key")
            .and_then(|v| v.to_str().ok())
            .ok_or(Reject::Unauthorized(
                "missing key: send it in X-Kestrel-Key",
            ))?;
        let public_key = auth::checked(given).ok_or(Reject::Unauthorized("invalid key"))?;
        let key = inner
            .core
            .with_conn(move |c| auth::lookup(c, &public_key))
            .await
            .map_err(|_| Reject::Busy {
                secs: 5,
                detail: "database unavailable",
            })?
            .ok_or(Reject::Unauthorized("unknown key"))?;
        if !key.enabled {
            return Err(Reject::Forbidden("the key is disabled"));
        }
        if !key.app_analytics {
            return Err(Reject::Forbidden("app analytics is off for this project"));
        }
        if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
            && !key.allows_origin(origin)
        {
            return Err(Reject::Forbidden(
                "this origin may not send signals with this key",
            ));
        }
        if key.app_secret.len() < 16 {
            return Err(Reject::Busy {
                secs: 60,
                detail: "the project has no secret for user hashes",
            });
        }
        inner.take_key(key.id, key.rate_limit_per_minute, 1, now)?;
        if inner.writer.store_full() {
            return Err(super::store_full());
        }

        let raw = axum::body::to_bytes(raw_body, MAX_BODY)
            .await
            .map_err(|_| Reject::TooLarge("the request body is over 256 KB"))?;
        let encoding = headers
            .get(header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let capture = inner
            .settings
            .capture_dir
            .clone()
            .map(|dir| (dir, headers.clone()));
        let now_ms = jiff::Timestamp::now().as_millisecond();
        let cleaning = key.clone();
        let (signals, dropped) = tokio::task::spawn_blocking(move || {
            let key = cleaning;
            if let Some((dir, headers)) = capture
                && let Err(err) =
                    super::capture::write(&dir, "POST", "/ingest/v1/signals", &headers, &raw)
            {
                tracing::warn!(%err, "can't write the capture file");
            }
            let data = body::decode(encoding.as_deref(), &raw, MAX_BODY).map_err(|e| match e {
                body::BodyError::TooLarge => {
                    Reject::TooLarge("the request body is over 256 KB once decompressed")
                }
                body::BodyError::Invalid(why) => Reject::BadRequest(why),
            })?;
            let batch = app::parse_batch(&data).map_err(|e| match e {
                BatchError::Malformed(why) => Reject::BadRequest(why.into()),
                BatchError::TooMany => Reject::TooLarge("at most 100 signals per request"),
            })?;
            let ctx = Ctx {
                project_id: key.project_id,
                secret: &key.app_secret,
                strict: key.app_strict,
                now_ms,
                oldest: app::oldest_accepted(now_ms, key.raw_days),
            };
            Ok::<_, Reject>(app::clean_batch(&batch, &ctx))
        })
        .await
        .map_err(|_| Reject::Busy {
            secs: 5,
            detail: "ingest failed",
        })??;
        if dropped > 0 {
            inner
                .dropped_signals
                .fetch_add(dropped as u64, Ordering::Relaxed);
            tracing::debug!(
                dropped,
                "signals that break the protocol's limits were dropped"
            );
        }
        let accepted = signals.len();
        // The request paid for one signal; the others are paid for now.
        if let Some(more) = u32::try_from(accepted.saturating_sub(1))
            .ok()
            .filter(|n| *n > 0)
        {
            inner.charge(ip, &key, more, "ip", now)?;
        }
        if accepted > 0 {
            inner
                .writer
                .submit(vec![Job::App(signals)])
                .map_err(|_| Reject::Busy {
                    secs: 2,
                    detail: "the queue is full",
                })?;
        }
        Ok((accepted, dropped))
    }
}
