//! Ingest limits (docs/design.md §7): what an anonymous client holding only
//! the public DSN key can make the server do. Envelopes are limited by
//! structure (items, events, header sizes) and the key is checked before the
//! body is read; the IP and key budgets count items, not requests, with a
//! default for keys without a limit and one budget per IPv6 /64; attachments
//! and small items are capped; the writer queue is bounded by bytes; a full
//! data volume stops the store from taking payloads but not from counting.

mod common;

use std::io::Write as _;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use common::{App, Res, Web, envelope_request, envelope_with, new_id};
use keen_kestrel::ingest::Settings;
use keen_kestrel::writer::WriterSettings;
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// Settings without the IP and per-key budgets in the way.
fn open() -> Settings {
    Settings {
        ip_rate_limit_per_minute: 0,
        key_rate_limit_per_minute: 0,
        ..Settings::default()
    }
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// One envelope item: its header (with `extra` JSON members) and payload.
fn item(ty: &str, extra: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "{{\"type\":\"{ty}\",{extra}\"length\":{}}}\n",
        payload.len()
    )
    .into_bytes();
    out.extend_from_slice(payload);
    out.push(b'\n');
    out
}

fn envelope(header: &str, items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = format!("{header}\n").into_bytes();
    for i in items {
        out.extend_from_slice(i);
    }
    out
}

/// An event of its own kind (one issue each, up to 26).
fn distinct_event(n: usize) -> Vec<u8> {
    let ty = format!("Kind{}", (b'a' + u8::try_from(n).unwrap()) as char);
    item(
        "event",
        "",
        json!({"event_id": new_id(), "exception": {"values": [{"type": ty, "value": "it broke"}]}})
            .to_string()
            .as_bytes(),
    )
}

/// An item an SDK sends all the time and Kestrel doesn't keep.
fn tiny() -> Vec<u8> {
    item("client_report", "", b"{}")
}

fn event_item(message: &str) -> Vec<u8> {
    item(
        "event",
        "",
        json!({"event_id": new_id(), "message": message})
            .to_string()
            .as_bytes(),
    )
}

async fn post(app: &App, items: &[Vec<u8>]) -> Res {
    app.send(envelope_request(
        app.project_id,
        &app.key,
        envelope("{}", items),
    ))
    .await
}

/// A request that names its key only in the envelope's `dsn`.
fn dsn_request(project_id: i64, key: &str, rest: &[u8], encoding: Option<&str>) -> Request<Body> {
    let mut body =
        format!("{{\"dsn\":\"http://{key}@kestrel.test:3002/{project_id}\"}}\n").into_bytes();
    body.extend_from_slice(rest);
    let body = if encoding.is_some() {
        gzip(&body)
    } else {
        body
    };
    let mut req = Request::post(format!("/api/{project_id}/envelope/"))
        .header(header::CONTENT_TYPE, "application/x-sentry-envelope");
    if let Some(e) = encoding {
        req = req.header(header::CONTENT_ENCODING, e);
    }
    req.body(Body::from(body)).unwrap()
}

/// Sends `req` from `peer` (an IP address), without a forwarded-for header.
async fn send_from(app: &App, mut req: Request<Body>, peer: &str) -> Res {
    req.headers_mut()
        .insert(header::HOST, "kestrel.test:3002".parse().unwrap());
    let addr: SocketAddr = if peer.contains(':') {
        format!("[{peer}]:40000").parse().unwrap()
    } else {
        format!("{peer}:40000").parse().unwrap()
    };
    req.extensions_mut().insert(ConnectInfo(addr));
    let res = app.router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    Res {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// KK-01: structure, authentication first, items as the unit of cost
// ---------------------------------------------------------------------------

#[tokio::test]
async fn envelopes_are_limited_in_items_events_and_header_size() {
    let app = App::with(open()).await;
    // 100 items are what an envelope may hold; the 101st is too many.
    assert_eq!(post(&app, &vec![tiny(); 100]).await.status, StatusCode::OK);
    let res = post(&app, &vec![tiny(); 101]).await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", res.body);
    assert!(res.body.contains("too many items"), "{}", res.body);

    // 20 events are fine, 21 are refused whole: none of them is written.
    let twenty: Vec<_> = (0..20).map(distinct_event).collect();
    assert_eq!(post(&app, &twenty).await.status, StatusCode::OK);
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 20);
    let twenty_one: Vec<_> = (0..21).map(distinct_event).collect();
    let res = post(&app, &twenty_one).await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", res.body);
    assert!(res.body.contains("too many events"), "{}", res.body);
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 20);

    // An item header is at most 16 KiB.
    let big = item(
        "client_report",
        &format!("\"pad\":\"{}\",", "a".repeat(20_000)),
        b"{}",
    );
    let res = post(&app, &[big]).await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", res.body);
    assert!(res.body.contains("item header"), "{}", res.body);
}

/// Two million items fit in 8 MB, which gzip makes about 8 KB: the server
/// reads 101 item headers and answers.
#[tokio::test]
async fn a_gzip_of_millions_of_items_is_refused_cheaply() {
    let app = App::with(open()).await;
    let mut body = b"{}\n".to_vec();
    for _ in 0..2_000_000 {
        body.extend_from_slice(b"{}\n\n");
    }
    let packed = gzip(&body);
    assert!(packed.len() < 64 * 1024, "{} bytes", packed.len());
    let mut req = envelope_request(app.project_id, &app.key, packed);
    req.headers_mut()
        .insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
    let started = Instant::now();
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", res.body);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 0);
}

#[tokio::test]
async fn the_key_in_the_envelope_header_is_checked_before_the_body_is_decoded() {
    let app = App::with(open()).await;
    // The right key: the envelope is read and its event kept.
    let rest = event_item("by dsn");
    let res = app
        .send(dsn_request(app.project_id, &app.key, &rest, Some("gzip")))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 1);

    // An unknown key with a body that would decode to 30 MB (over the 20 MB
    // limit): answered 401 from the first 64 KiB, the rest is never decoded.
    let bomb = vec![0u8; 30 * 1024 * 1024];
    let res = app
        .send(dsn_request(
            app.project_id,
            "ffffffffffffffffffffffffffffffff",
            &bomb,
            Some("gzip"),
        ))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
    assert!(res.body.contains("unknown DSN key"), "{}", res.body);
    // The same bomb under the right key is refused for its size, as before.
    let res = app
        .send(dsn_request(app.project_id, &app.key, &bomb, Some("gzip")))
        .await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", res.body);

    // No dsn, a malformed key, a header that never ends, or an unreadable body.
    let none = Request::post(format!("/api/{}/envelope/", app.project_id))
        .body(Body::from("{}\n"))
        .unwrap();
    assert_eq!(app.send(none).await.status, StatusCode::UNAUTHORIZED);
    let res = app
        .send(dsn_request(app.project_id, "not-a-key", b"", None))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
    let endless = Request::post(format!("/api/{}/envelope/", app.project_id))
        .body(Body::from(format!("{{\"pad\":\"{}", "a".repeat(100_000))))
        .unwrap();
    assert_eq!(
        app.send(endless).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let garbage = Request::post(format!("/api/{}/envelope/", app.project_id))
        .header(header::CONTENT_ENCODING, "gzip")
        .body(Body::from(vec![7u8; 100]))
        .unwrap();
    assert_eq!(app.send(garbage).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_key_for_another_project_is_refused_before_the_body_is_read() {
    let app = App::with(open()).await;
    let res = app
        .send(dsn_request(app.project_id + 1, &app.key, b"", None))
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
}

#[tokio::test]
async fn the_buckets_are_charged_for_every_item() {
    // The key's budget: 10 items a minute.
    let app = App::with(Settings {
        key_rate_limit_per_minute: 10,
        ..open()
    })
    .await;
    assert_eq!(post(&app, &vec![tiny(); 6]).await.status, StatusCode::OK);
    let res = post(&app, &vec![tiny(); 6]).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);
    assert!(res.header("x-sentry-rate-limits").ends_with("::key"));
    // Four are left: an envelope of three still fits.
    assert_eq!(post(&app, &vec![tiny(); 3]).await.status, StatusCode::OK);

    // The IP's budget counts the same way.
    let app = App::with(Settings {
        ip_rate_limit_per_minute: 10,
        key_rate_limit_per_minute: 0,
        ..Settings::default()
    })
    .await;
    assert_eq!(post(&app, &vec![tiny(); 6]).await.status, StatusCode::OK);
    let res = post(&app, &vec![tiny(); 6]).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);
    assert!(
        res.header("x-sentry-rate-limits")
            .ends_with("::organization")
    );
}

// ---------------------------------------------------------------------------
// KK-02: budgets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_key_without_a_limit_gets_the_default_and_the_page_says_so() {
    assert_eq!(keen_kestrel::ingest::DEFAULT_KEY_RATE_LIMIT, 1000);
    let app = App::with(Settings {
        key_rate_limit_per_minute: 3,
        ..open()
    })
    .await;
    for _ in 0..3 {
        assert_eq!(post(&app, &[tiny()]).await.status, StatusCode::OK);
    }
    let res = post(&app, &[tiny()]).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(res.header("x-sentry-rate-limits").ends_with("::key"));
    // A limit of its own replaces the default (and is the key's alone).
    app.exec(format!(
        "UPDATE project_keys SET rate_limit_per_minute = 100000 WHERE project_id = {}",
        app.project_id
    ))
    .await;
    // (The key's bucket refills at the new rate.)
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(post(&app, &[tiny()]).await.status, StatusCode::OK);

    // The project settings say what a key without a limit gets.
    let app = App::new().await;
    let admin = Web::new(&app, "ada", "admin").await;
    let mut admin = admin;
    let page = admin
        .get(&format!("/projects/{}/settings", app.project_id))
        .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body.contains("Default (1,000 per minute)"),
        "{}",
        page.body
    );
}

#[tokio::test]
async fn an_ipv6_client_has_one_budget_for_its_64() {
    let app = App::with(Settings {
        ip_rate_limit_per_minute: 3,
        key_rate_limit_per_minute: 0,
        ..Settings::default()
    })
    .await;
    let send = |ip: &'static str| {
        let app = &app;
        async move {
            send_from(
                app,
                envelope_request(app.project_id, &app.key, b"{}\n".to_vec()),
                ip,
            )
            .await
            .status
        }
    };
    for n in 1..=3 {
        assert_eq!(
            send(["2001:db8:1:2::1", "2001:db8:1:2::2", "2001:db8:1:2:aaaa::3"][n - 1]).await,
            StatusCode::OK
        );
    }
    // Another address in the same /64 finds the budget spent.
    assert_eq!(
        send("2001:db8:1:2:ffff:ffff:ffff:ffff").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // The next /64 and an IPv4 client have their own.
    assert_eq!(send("2001:db8:1:3::1").await, StatusCode::OK);
    assert_eq!(send("198.51.100.4").await, StatusCode::OK);
    // An IPv4-mapped address is the IPv4 one.
    for _ in 0..2 {
        assert_eq!(send("::ffff:198.51.100.4").await, StatusCode::OK);
    }
    assert_eq!(send("198.51.100.4").await, StatusCode::TOO_MANY_REQUESTS);
}

fn attachment(name: &str, size: usize) -> Vec<u8> {
    item(
        "attachment",
        &format!("\"filename\":\"{name}\",\"attachment_type\":\"event.attachment\","),
        &vec![b'a'; size],
    )
}

#[tokio::test]
async fn attachments_and_small_items_are_capped() {
    let app = App::with(open()).await;
    let id = new_id();
    let event = item(
        "event",
        "",
        json!({"event_id": id, "message": "with attachments"})
            .to_string()
            .as_bytes(),
    );
    let mut items = vec![event];
    // Over 1 MiB: dropped, and it doesn't count. Exactly 1 MiB: kept. Then
    // six small ones, of which four fit under the limit of five per event.
    items.push(attachment("too-big.bin", 1024 * 1024 + 1));
    items.push(attachment("exact.bin", 1024 * 1024));
    for n in 0..6 {
        items.push(attachment(&format!("small-{n}.txt"), 100));
    }
    // Sessions, user reports and feedback are small by nature: 4 KiB at most.
    let session = |pad: usize| {
        json!({"sid": new_id(), "did": "d", "init": true, "status": "ok", "errors": 0,
               "started": "2026-10-27T10:00:00Z", "attrs": {"release": "shop@1.0", "environment": "production"},
               "pad": "x".repeat(pad)})
        .to_string()
    };
    items.push(item("session", "", session(10).as_bytes()));
    items.push(item("session", "", session(5000).as_bytes()));
    let report = |len: usize| {
        json!({"event_id": id, "name": "Ann", "email": "a@example.com", "comments": "c".repeat(len)})
            .to_string()
    };
    items.push(item("user_report", "", report(20).as_bytes()));
    items.push(item("user_report", "", report(5000).as_bytes()));
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            envelope(&format!("{{\"event_id\":\"{id}\"}}"), &items),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert!(
        res.header("x-sentry-rate-limits").is_empty(),
        "nothing is rate limited"
    );
    app.flush().await;
    assert_eq!(app.store_count("SELECT COUNT(*) FROM attachments").await, 5);
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM attachments WHERE size = 1048576")
            .await,
        1
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM attachments WHERE size > 1048576")
            .await,
        0
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM attachments WHERE filename = 'small-3.txt'")
            .await,
        1
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM attachments WHERE filename = 'small-4.txt'")
            .await,
        0,
        "the sixth small one is over the limit of five"
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM session_items").await,
        1
    );
    assert_eq!(app.store_count("SELECT COUNT(*) FROM feedback").await, 1);
}

#[tokio::test]
async fn a_sessions_release_and_environment_are_cut_like_an_events() {
    let app = App::with(open()).await;
    let session = |release: &str, env: &str| {
        json!({"sid": new_id(), "did": "d", "init": true, "status": "ok", "errors": 0,
               "started": "2026-10-27T10:00:00Z", "attrs": {"release": release, "environment": env}})
        .to_string()
    };
    let items = [
        item(
            "session",
            "",
            session(&"r".repeat(900), &"e".repeat(500)).as_bytes(),
        ),
        item("session", "", session("shop@1.0", "a/b").as_bytes()),
        item("session", "", session("shop@1.0", "  staging ").as_bytes()),
    ];
    assert_eq!(post(&app, &items).await.status, StatusCode::OK);
    app.flush().await;
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM session_items WHERE length(release) = 200 AND length(environment) = 64")
            .await,
        1
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM session_items WHERE environment = 'production'")
            .await,
        1,
        "an environment with a slash is the default"
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM session_items WHERE environment = 'staging'")
            .await,
        1
    );
}

/// The writer holds the first batch for 30 seconds, so what is queued stays
/// queued until the flush: the queue's bytes are the only thing that changes.
#[tokio::test]
async fn the_writer_queue_is_bounded_by_bytes() {
    let app = App::with(Settings {
        writer: WriterSettings {
            queue_max_bytes: 2000,
            batch_interval: Duration::from_secs(30),
            ..WriterSettings::default()
        },
        ..open()
    })
    .await;
    let id = new_id();
    let body = || {
        let mut noise = Vec::with_capacity(900);
        let mut x: u32 = 12345;
        for _ in 0..900 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            noise.push((x >> 24) as u8);
        }
        envelope(
            &format!("{{\"event_id\":\"{id}\"}}"),
            &[
                item(
                    "event",
                    "",
                    json!({"event_id": id, "message": "queued"})
                        .to_string()
                        .as_bytes(),
                ),
                item("attachment", "\"filename\":\"n.bin\",", &noise),
            ],
        )
    };
    // The first request finds the queue empty and takes more than the budget.
    let first = app
        .send(envelope_request(app.project_id, &app.key, body()))
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert!(
        !app.kestrel.ingest.writer().has_room(),
        "over the byte budget"
    );
    // The next is answered 429 before its body is read.
    let res = app
        .send(envelope_request(app.project_id, &app.key, body()))
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);
    assert_eq!(res.header("retry-after"), "1");
    // The job itself is refused too when the budget is spent.
    let big = |n: usize| {
        keen_kestrel::writer::Job::Feedback(keen_kestrel::writer::FeedbackItem {
            project_id: app.project_id,
            event_id: None,
            received_at: "2026-10-27 10:00:00".into(),
            data: "x".repeat(n),
        })
    };
    assert!(app.kestrel.ingest.writer().submit(vec![big(10)]).is_err());
    // Once the writer has written, there is room again.
    app.flush().await;
    assert!(app.kestrel.ingest.writer().has_room());
    assert!(app.kestrel.ingest.writer().submit(vec![big(5000)]).is_ok());
    assert!(
        app.kestrel.ingest.writer().submit(vec![big(5000)]).is_err(),
        "5 KB is more than the 2000 bytes, so a second batch waits"
    );
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 1);
}

/// `store.sqlite` over its limit (a tiny one here): events are still counted,
/// nothing else is kept, and the SDK is told to leave attachments out.
#[tokio::test]
async fn a_full_store_counts_events_and_keeps_nothing_else() {
    let app = App::with(Settings {
        writer: WriterSettings {
            max_store_bytes: Some(1),
            ..WriterSettings::default()
        },
        ..open()
    })
    .await;
    assert!(app.kestrel.ingest.writer().store_full());
    let id = new_id();
    let items = [
        item(
            "event",
            "",
            json!({"event_id": id, "message": "counted"})
                .to_string()
                .as_bytes(),
        ),
        attachment("a.txt", 100),
        item(
            "user_report",
            "",
            json!({"event_id": id, "comments": "hi"})
                .to_string()
                .as_bytes(),
        ),
        item(
            "session",
            "",
            json!({"sid": new_id(), "did": "d", "init": true, "status": "ok", "errors": 0,
                   "started": "2026-10-27T10:00:00Z", "attrs": {"release": "shop@1.0"}})
            .to_string()
            .as_bytes(),
        ),
    ];
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            envelope(&format!("{{\"event_id\":\"{id}\"}}"), &items),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.header("x-sentry-rate-limits"),
        "300:attachment:organization"
    );
    app.flush().await;
    // The issue and its count are there; its details are not kept.
    assert_eq!(app.count("SELECT times_seen FROM issues").await, 1);
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, 0);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM payloads").await, 0);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM attachments").await, 0);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM feedback").await, 0);
    // Release health still counts (the items are rolled up and deleted).
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM session_items").await,
        1
    );
    // A second event of the issue still counts.
    let again = envelope_with(&json!({"event_id": new_id(), "message": "counted"}));
    assert_eq!(
        app.send(envelope_request(app.project_id, &app.key, again))
            .await
            .status,
        StatusCode::OK
    );
    app.flush().await;
    assert_eq!(app.count("SELECT times_seen FROM issues").await, 2);

    // Web events and app signals are only raw rows: 429 with a minute's wait.
    app.exec(format!(
        "UPDATE projects SET web_analytics = 1, app_analytics = 1, app_secret = randomblob(32) WHERE id = {}",
        app.project_id
    ))
    .await;
    let web = Request::post("/ingest/v1/event")
        .header(header::CONTENT_TYPE, "text/plain")
        .header(
            header::USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64) Firefox/130.0",
        )
        .header("x-kestrel-key", &app.key)
        .body(Body::from(
            json!({"n": "pageview", "u": "https://shop.example/a"}).to_string(),
        ))
        .unwrap();
    let res = app.send(web).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);
    assert_eq!(res.header("retry-after"), "60");
    let signals = Request::post("/ingest/v1/signals")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-kestrel-key", &app.key)
        .body(Body::from(
            json!({"signals": [{"type": "App.launched", "install_id": "i"}]}).to_string(),
        ))
        .unwrap();
    let res = app.send(signals).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);
    assert_eq!(res.header("retry-after"), "60");
}

#[tokio::test]
async fn a_store_with_room_keeps_everything() {
    // The default limits (no size limit, 1 GiB free) don't trip here.
    let app = App::with(open()).await;
    assert!(!app.kestrel.ingest.writer().store_full());
    let id = new_id();
    let items = [
        item(
            "event",
            "",
            json!({"event_id": id, "message": "kept"})
                .to_string()
                .as_bytes(),
        ),
        attachment("a.txt", 100),
    ];
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            envelope(&format!("{{\"event_id\":\"{id}\"}}"), &items),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert!(res.header("x-sentry-rate-limits").is_empty());
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, 1);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM attachments").await, 1);
}

/// What the answer of a refused request says, for the SDKs: a 413 for the
/// structure limits is a plain status with `x-sentry-error`.
#[tokio::test]
async fn refusals_carry_the_sentry_error_header() {
    let app = App::with(open()).await;
    let res = post(&app, &vec![tiny(); 101]).await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(res.header("x-sentry-error").contains("too many items"));
    let json: Value = res.json();
    assert!(json["detail"].as_str().unwrap().contains("too many items"));
}
