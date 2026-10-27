//! App analytics (docs/design.md §5, phase 9): the KestrelKit protocol
//! contract of `POST /ingest/v1/signals` (golden requests, gzip, limits,
//! every answer, clock skew), privacy (no install id, IP address, user
//! agent or email in either database; strict mode), daily/weekly/monthly
//! active users, rollups and retention, the App tab and the signal
//! explorer, release adoption, the REST API and MCP.
//!
//! `tests/fixtures/kestrelkit/` holds request bodies exactly as KestrelKit
//! builds them, copied from its tests: `golden_batch.json` is
//! `WireTests.goldenBatch` (KestrelKit 0.0.0, sorted keys, `sdk` first, no
//! trailing newline) and `protocol_example.json` the example of its
//! `docs/protocol.md`.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::{App, CLIENT_IP, Res, Web, add_user, api_token, bearer};
use keen_kestrel::analytics::app::{self, AppSignal};
use keen_kestrel::analytics::{day_number, rollup};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use vole_kit::auth::TokenScope;

const GOLDEN: &str = include_str!("fixtures/kestrelkit/golden_batch.json");
const EXAMPLE: &str = include_str!("fixtures/kestrelkit/protocol_example.json");
const UA: &str = "KestrelKit/0.1.0";

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn ts(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms).unwrap().to_string()
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn request(key: Option<&str>, body: Vec<u8>, encoding: Option<&str>) -> Request<Body> {
    let mut r = Request::post("/ingest/v1/signals")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, UA);
    if let Some(k) = key {
        r = r.header("x-kestrel-key", k);
    }
    if let Some(e) = encoding {
        r = r.header(header::CONTENT_ENCODING, e);
    }
    r.body(Body::from(body)).unwrap()
}

/// One signal as KestrelKit sends it.
#[derive(Clone)]
struct Sig {
    kind: &'static str,
    install: String,
    session: String,
    version: &'static str,
    build: &'static str,
    platform: &'static str,
    model: &'static str,
    locale: &'static str,
    time: i64,
    payload: Value,
    flags: (bool, bool, bool),
}

impl Sig {
    fn new(kind: &'static str, install: &str) -> Sig {
        Sig {
            kind,
            install: install.to_string(),
            session: format!("{install}-session"),
            version: "1.4.0",
            build: "57",
            platform: "iOS",
            model: "iPhone16,2",
            locale: "nb_NO",
            time: now_ms() - 1000,
            payload: json!({}),
            flags: (false, false, false),
        }
    }

    fn json(&self) -> Value {
        json!({
            "type": self.kind,
            "time": ts(self.time),
            "install_id": self.install,
            "session_id": self.session,
            "app": {"bundle_id": "com.example.app", "version": self.version, "build": self.build},
            "device": {"platform": self.platform, "os_version": "18.2", "model": self.model, "locale": self.locale},
            "flags": {"debug": self.flags.0, "testflight": self.flags.1, "simulator": self.flags.2},
            "payload": self.payload,
        })
    }
}

fn batch(sent_at: i64, signals: &[Sig]) -> Vec<u8> {
    json!({
        "sdk": {"name": "kestrelkit", "version": "0.1.0"},
        "sent_at": ts(sent_at),
        "signals": signals.iter().map(Sig::json).collect::<Vec<_>>(),
    })
    .to_string()
    .into_bytes()
}

async fn send(app: &App, signals: &[Sig]) -> Res {
    let res = app
        .send(request(Some(&app.key), batch(now_ms(), signals), None))
        .await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.body);
    res
}

/// Writes what was accepted and rolls it up.
async fn roll(app: &App) -> rollup::Report {
    app.flush().await;
    rollup::run(app.kestrel.ingest.writer()).await.unwrap()
}

async fn raw_rows(app: &App) -> Vec<Value> {
    app.flush().await;
    let conn = app.kestrel.ingest.store().get().await.unwrap();
    conn.interact(|c| -> rusqlite::Result<Vec<Value>> {
        c.prepare(
            "SELECT ts, visitor, session, name, bundle_id, app_version, build, os, os_version,
                    model, locale, flags, props FROM analytics_events WHERE source = 'app' ORDER BY id",
        )?
        .query_map([], |r| {
            Ok(json!({
                "ts": r.get::<_, i64>(0)?, "user": r.get::<_, i64>(1)?, "session": r.get::<_, i64>(2)?,
                "type": r.get::<_, String>(3)?, "bundle_id": r.get::<_, String>(4)?,
                "version": r.get::<_, String>(5)?, "build": r.get::<_, String>(6)?,
                "platform": r.get::<_, String>(7)?, "os_version": r.get::<_, String>(8)?,
                "model": r.get::<_, String>(9)?, "locale": r.get::<_, String>(10)?,
                "flags": r.get::<_, i64>(11)?, "payload": r.get::<_, Option<String>>(12)?,
            }))
        })?
        .collect()
    })
    .await
    .unwrap()
    .unwrap()
}

async fn set(app: &App, sql: &str) {
    app.exec(sql.replace("{p}", &app.project_id.to_string()))
        .await;
}

// ---------------------------------------------------------------------------
// The protocol contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kestrelkits_golden_requests_are_accepted_and_stored_field_by_field() {
    let app = App::new().await;
    // As KestrelKit sends it: the golden body, its sent_at days or years
    // from now, so the device clock is corrected (P10) and the signal lands
    // 1.877 s before the server's now.
    let before = now_ms();
    let res = app
        .send(request(Some(&app.key), GOLDEN.as_bytes().to_vec(), None))
        .await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.body);
    assert_eq!(res.json(), json!({"accepted": 1}));
    assert!(res.headers.get(header::SET_COOKIE).is_none());
    assert_eq!(res.header("cache-control"), "no-store");
    // gzip, as KestrelKit compresses bodies of 1 KB and more.
    let res = app
        .send(request(
            Some(&app.key),
            gzip(GOLDEN.as_bytes()),
            Some("gzip"),
        ))
        .await;
    assert_eq!(res.json(), json!({"accepted": 1}));
    // The protocol document's example (whitespace, other key order).
    let res = app
        .send(request(Some(&app.key), EXAMPLE.as_bytes().to_vec(), None))
        .await;
    assert_eq!(res.json(), json!({"accepted": 1}));
    let after = now_ms();

    let rows = raw_rows(&app).await;
    assert_eq!(rows.len(), 3);
    let r = &rows[0];
    assert_eq!(r["type"], "App.launched");
    assert_eq!(r["bundle_id"], "com.example.app");
    assert_eq!(
        (r["version"].as_str(), r["build"].as_str()),
        (Some("1.4.0"), Some("57"))
    );
    assert_eq!(r["platform"], "iOS");
    assert_eq!(r["os_version"], "18.2");
    assert_eq!(r["model"], "iPhone16,2");
    assert_eq!(r["locale"], "nb_NO");
    assert_eq!(r["flags"], 0);
    assert_eq!(r["payload"], r#"{"plan":"pro"}"#);
    let t = r["ts"].as_i64().unwrap();
    assert!(
        t >= (before - 1877) / 1000 - 1 && t <= after / 1000,
        "clock corrected: {t}"
    );
    // One install, one session: the same hashes every time.
    assert!(
        rows.iter()
            .all(|x| x["user"] == r["user"] && x["session"] == r["session"])
    );
    assert_ne!(r["user"], r["session"]);
}

#[tokio::test]
async fn every_answer_of_the_protocol() {
    let app = App::new().await;
    let one = || batch(now_ms(), &[Sig::new("A.b", "i-1")]);
    let status = |res: &Res| res.status;
    // 401: no key, a malformed key, an unknown key.
    for key in [None, Some("nope"), Some("ffffffffffffffffffffffffffffffff")] {
        let res = app.send(request(key, one(), None)).await;
        assert_eq!(status(&res), StatusCode::UNAUTHORIZED, "{key:?}");
        assert!(res.headers.get(header::SET_COOKIE).is_none());
    }
    // 400: not JSON, no signals array, broken gzip.
    for body in [
        &b"not json"[..],
        b"[]",
        br#"{"signals": {}}"#,
        br#"{"sent_at": "x"}"#,
    ] {
        let res = app.send(request(Some(&app.key), body.to_vec(), None)).await;
        assert_eq!(
            status(&res),
            StatusCode::BAD_REQUEST,
            "{}",
            String::from_utf8_lossy(body)
        );
    }
    let res = app
        .send(request(Some(&app.key), b"not gzip".to_vec(), Some("gzip")))
        .await;
    assert_eq!(status(&res), StatusCode::BAD_REQUEST);
    // 413: more than 100 signals; more than 256 KB, sent or decompressed.
    let many: Vec<Sig> = (0..101)
        .map(|i| Sig::new("A.b", &format!("i-{i}")))
        .collect();
    let res = app
        .send(request(Some(&app.key), batch(now_ms(), &many), None))
        .await;
    assert_eq!(status(&res), StatusCode::PAYLOAD_TOO_LARGE);
    let hundred: Vec<Sig> = (0..100)
        .map(|i| Sig::new("A.b", &format!("i-{i}")))
        .collect();
    assert_eq!(
        app.send(request(Some(&app.key), batch(now_ms(), &hundred), None))
            .await
            .json(),
        json!({"accepted": 100})
    );
    let big = format!(r#"{{"signals": [], "pad": "{}"}}"#, "x".repeat(300 * 1024));
    let res = app
        .send(request(Some(&app.key), big.clone().into_bytes(), None))
        .await;
    assert_eq!(status(&res), StatusCode::PAYLOAD_TOO_LARGE);
    let bomb = gzip(big.as_bytes());
    assert!(bomb.len() < 4096);
    let res = app.send(request(Some(&app.key), bomb, Some("gzip"))).await;
    assert_eq!(status(&res), StatusCode::PAYLOAD_TOO_LARGE, "{}", res.body);
    // 202: unknown fields are ignored (P6), missing payload and flags too.
    let body = json!({"sent_at": ts(now_ms()), "batch_id": "x", "signals": [
        {"type": "A.b", "time": ts(now_ms()), "install_id": "i", "future": {"x": 1}}
    ]});
    let res = app
        .send(request(Some(&app.key), body.to_string().into_bytes(), None))
        .await;
    assert_eq!(res.json(), json!({"accepted": 1}));
    // Lenient per signal (P5): the good ones are stored and counted.
    let body = json!({"signals": [
        {"type": "Good.one", "install_id": "i"},
        {"type": "Bad type", "install_id": "i"},
        {"type": "No.install"},
        "not an object",
        {"type": "Too.old", "install_id": "i", "time": ts(now_ms() - 60 * 86_400_000)},
        {"type": "Good.two", "install_id": "i", "payload": {"plan": "pro", "nested": {"x": 1}}}
    ]});
    let res = app
        .send(request(Some(&app.key), body.to_string().into_bytes(), None))
        .await;
    assert_eq!(status(&res), StatusCode::ACCEPTED);
    assert_eq!(res.json(), json!({"accepted": 2, "dropped": 4}));
    assert_eq!(app.kestrel.ingest.dropped_signals(), 4);
    // All signals invalid: still 202, nothing stored.
    let res = app
        .send(request(
            Some(&app.key),
            br#"{"signals": [{}]}"#.to_vec(),
            None,
        ))
        .await;
    assert_eq!(res.json(), json!({"accepted": 0, "dropped": 1}));
    // 403: a disabled key, then app analytics off for the project.
    set(
        &app,
        "UPDATE project_keys SET enabled = 0 WHERE project_id = {p}",
    )
    .await;
    assert_eq!(
        status(&app.send(request(Some(&app.key), one(), None)).await),
        StatusCode::FORBIDDEN
    );
    set(
        &app,
        "UPDATE project_keys SET enabled = 1 WHERE project_id = {p}",
    )
    .await;
    set(&app, "UPDATE projects SET app_analytics = 0 WHERE id = {p}").await;
    let res = app.send(request(Some(&app.key), one(), None)).await;
    assert_eq!(status(&res), StatusCode::FORBIDDEN);
    assert!(res.body.contains("app analytics is off"), "{}", res.body);
    set(&app, "UPDATE projects SET app_analytics = 1 WHERE id = {p}").await;
    // Web analytics off doesn't matter.
    set(&app, "UPDATE projects SET web_analytics = 0 WHERE id = {p}").await;
    assert_eq!(
        status(&app.send(request(Some(&app.key), one(), None)).await),
        StatusCode::ACCEPTED
    );
    // 429 with Retry-After in seconds (P8): the key's bucket.
    set(
        &app,
        "UPDATE project_keys SET rate_limit_per_minute = 1 WHERE project_id = {p}",
    )
    .await;
    let first = app.send(request(Some(&app.key), one(), None)).await;
    assert_eq!(status(&first), StatusCode::ACCEPTED);
    let res = app.send(request(Some(&app.key), one(), None)).await;
    assert_eq!(status(&res), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.header("retry-after").parse::<u64>().unwrap() >= 1);
    // Everything accepted was written.
    assert_eq!(raw_rows(&app).await.len(), 100 + 1 + 2 + 1 + 1);
}

#[tokio::test]
async fn a_full_queue_answers_503_and_the_ip_limit_429() {
    let settings = keen_kestrel::ingest::Settings {
        ip_rate_limit_per_minute: 2,
        writer: keen_kestrel::writer::WriterSettings {
            queue_capacity: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let app = App::with(settings).await;
    // Keep the writer busy, so the queue's one place stays taken.
    let writer = app.kestrel.ingest.writer().clone();
    let busy = tokio::spawn(async move {
        writer
            .run(|_| {
                std::thread::sleep(Duration::from_millis(1500));
                Ok(())
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let one = || batch(now_ms(), &[Sig::new("A.b", "i")]);
    assert_eq!(
        app.send(request(Some(&app.key), one(), None)).await.status,
        StatusCode::ACCEPTED
    );
    let res = app.send(request(Some(&app.key), one(), None)).await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE, "{}", res.body);
    assert!(res.header("retry-after").parse::<u64>().unwrap() >= 1);
    busy.await.unwrap().unwrap();
    let res = app.send(request(Some(&app.key), one(), None)).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "the IP's third");
    assert!(res.header("retry-after").parse::<u64>().unwrap() >= 1);
}

/// A signal's time is the client's word, and a signal for a day before
/// yesterday makes the rollup rebuild that whole day (KK-04): beyond 48 hours
/// back a signal is dropped, whatever the raw retention; the future is
/// clamped to the arrival.
#[tokio::test]
async fn signals_older_than_48_hours_are_dropped() {
    assert_eq!(app::MAX_BACKDATE_SECS, 48 * 3600);
    let app = App::new().await;
    let now = now_ms();
    let hours = |h: i64| now - h * 3_600_000;
    let body = json!({"signals": [
        {"type": "Fresh.one", "install_id": "i", "time": ts(hours(1))},
        {"type": "Offline.day", "install_id": "i", "time": ts(hours(30))},
        {"type": "Offline.edge", "install_id": "i", "time": ts(hours(47))},
        {"type": "Too.old.49h", "install_id": "i", "time": ts(hours(49))},
        {"type": "Too.old.5d", "install_id": "i", "time": ts(hours(120))},
        {"type": "Too.old.25d", "install_id": "i", "time": ts(hours(25 * 24))},
        {"type": "From.future", "install_id": "i", "time": ts(now + 7 * 86_400_000)},
    ]});
    let res = app
        .send(request(Some(&app.key), body.to_string().into_bytes(), None))
        .await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.body);
    assert_eq!(res.json(), json!({"accepted": 4, "dropped": 3}));
    let rows = raw_rows(&app).await;
    let kinds: BTreeSet<String> = rows
        .iter()
        .map(|r| r["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from(
            ["Fresh.one", "Offline.day", "Offline.edge", "From.future"].map(String::from)
        )
    );
    let future = rows.iter().find(|r| r["type"] == "From.future").unwrap();
    assert!(
        (future["ts"].as_i64().unwrap() - now / 1000).abs() <= 2,
        "clamped to now"
    );

    // Clock correction doesn't bring a back-dated signal back in: a device
    // whose clock is 3 days behind and says so reports 3-day-old times that
    // are right once corrected.
    let mut s = Sig::new("Clock.behind.fresh", "i");
    s.time = now - 3 * 86_400_000 - 60_000;
    let res = app
        .send(request(
            Some(&app.key),
            batch(now - 3 * 86_400_000, &[s]),
            None,
        ))
        .await;
    assert_eq!(res.json(), json!({"accepted": 1}));
    // The window itself is a function of the raw retention too.
    assert_eq!(
        app::oldest_accepted(1_000_000_000_000, 30),
        1_000_000_000 - 172_800
    );
    assert_eq!(
        app::oldest_accepted(1_000_000_000_000, 1),
        1_000_000_000 - 86_400
    );
    assert_eq!(
        app::oldest_accepted(1_000_000_000_000, 0),
        1_000_000_000 - 86_400
    );
}

#[tokio::test]
async fn clocks_are_corrected_with_sent_at() {
    let app = App::new().await;
    let now = now_ms();
    // A device 3 hours ahead sends a signal it took a minute ago.
    let mut s = Sig::new("Clock.ahead", "i");
    s.time = now + 3 * 3_600_000 - 60_000;
    let body = batch(now + 3 * 3_600_000, &[s]);
    app.send(request(Some(&app.key), body, None)).await;
    // A device 2 days behind, an hour ago by its clock.
    let mut s = Sig::new("Clock.behind", "i");
    s.time = now - 2 * 86_400_000 - 3_600_000;
    app.send(request(
        Some(&app.key),
        batch(now - 2 * 86_400_000, &[s]),
        None,
    ))
    .await;
    // No sent_at: a time in the future is clamped to now.
    let body = json!({"signals": [{"type": "Clock.future", "install_id": "i", "time": ts(now + 86_400_000)}]});
    app.send(request(Some(&app.key), body.to_string().into_bytes(), None))
        .await;
    let rows = raw_rows(&app).await;
    let at = |kind: &str| {
        rows.iter().find(|r| r["type"] == kind).unwrap()["ts"]
            .as_i64()
            .unwrap()
    };
    let secs = now / 1000;
    assert!((at("Clock.ahead") - (secs - 60)).abs() <= 2);
    assert!((at("Clock.behind") - (secs - 3600)).abs() <= 2);
    assert!((at("Clock.future") - secs).abs() <= 2);
}

#[tokio::test]
async fn the_ingest_port_takes_signals_and_answers_any_origin() {
    let app = App::new().await;
    let ingest = keen_kestrel::ingest_router(app.kestrel.clone());
    let mut req = request(Some(&app.key), GOLDEN.as_bytes().to_vec(), None);
    assert_eq!(
        common::send_to(&ingest, &mut req).await.status,
        StatusCode::ACCEPTED
    );
    // The CSRF guard leaves the route alone; an Origin is fine for a key
    // without allowed origins and refused for one limited to others.
    let mut req = request(Some(&app.key), GOLDEN.as_bytes().to_vec(), None);
    req.headers_mut()
        .insert(header::ORIGIN, "https://elsewhere.example".parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::ACCEPTED);
    set(
        &app,
        "UPDATE project_keys SET allowed_origins = 'https://shop.example' WHERE project_id = {p}",
    )
    .await;
    let mut req = request(Some(&app.key), GOLDEN.as_bytes().to_vec(), None);
    req.headers_mut()
        .insert(header::ORIGIN, "https://elsewhere.example".parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::FORBIDDEN);
    // Apps send no Origin: still accepted.
    let req = request(Some(&app.key), GOLDEN.as_bytes().to_vec(), None);
    assert_eq!(app.send(req).await.status, StatusCode::ACCEPTED);
}

// ---------------------------------------------------------------------------
// Privacy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nothing_personal_is_stored_from_signals() {
    let app = App::new().await;
    const INSTALL: &str = "PRIVACY-INSTALL-1234-ABCD-PROBE";
    const SESSION: &str = "PRIVACY-SESSION-5678-EFGH-PROBE";
    const AGENT: &str = "KestrelKit/0.1.0 PrivacyProbe/7.7";
    const EMAIL: &str = "ada.lovelace@example.com";
    let mut s = Sig::new("Signed.up", INSTALL);
    s.session = SESSION.into();
    s.payload = json!({
        "email": EMAIL,
        "note": format!("write to {EMAIL} or call +47 912 34 567"),
        "card": "4111 1111 1111 1111",
        "ip": CLIENT_IP,
        "from": format!("from {CLIENT_IP}"),
        "plan": "pro",
    });
    let mut req = request(Some(&app.key), batch(now_ms(), &[s.clone()]), None);
    req.headers_mut()
        .insert(header::USER_AGENT, AGENT.parse().unwrap());
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.body);
    s.kind = "Paywall.shown";
    let mut req = request(Some(&app.key), gzip(&batch(now_ms(), &[s])), Some("gzip"));
    req.headers_mut()
        .insert(header::USER_AGENT, AGENT.parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::ACCEPTED);
    roll(&app).await;
    assert!(
        app.count("SELECT COUNT(*) FROM analytics_breakdown WHERE source = 'app'")
            .await
            > 8
    );
    let rows = raw_rows(&app).await;
    let payload: Value = serde_json::from_str(rows[0]["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["email"], "[Filtered]");
    assert_eq!(payload["note"], "write to [Filtered] or call [Filtered]");
    assert_eq!(payload["card"], "[Filtered]");
    assert_eq!(payload["from"], "from [Filtered]");
    assert_eq!(payload["plan"], "pro");
    assert!(payload.get("ip").is_none());

    let needles = [
        INSTALL,
        &INSTALL.to_lowercase(),
        SESSION,
        CLIENT_IP,
        AGENT,
        "PrivacyProbe",
        EMAIL,
        "ada.lovelace",
        "912 34 567",
        "4111 1111 1111 1111",
    ];
    let mut scanned = 0;
    for file in ["kestrel.sqlite", "store.sqlite"] {
        let c = Connection::open_with_flags(app.dir.join(file), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
        let tables: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for table in tables {
            let mut stmt = c.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for i in 0..columns {
                    let bytes: Vec<u8> = match row.get_ref(i).unwrap() {
                        rusqlite::types::ValueRef::Text(t) => t.to_vec(),
                        rusqlite::types::ValueRef::Blob(b)
                            if b.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) =>
                        {
                            keen_kestrel::store::decompress(b, 64 << 20).unwrap()
                        }
                        rusqlite::types::ValueRef::Blob(b) => b.to_vec(),
                        _ => continue,
                    };
                    scanned += 1;
                    let text = String::from_utf8_lossy(&bytes);
                    for needle in &needles {
                        assert!(!text.contains(needle), "{needle} in {file} {table}: {text}");
                    }
                }
            }
        }
    }
    assert!(scanned > 50, "{scanned}");
}

#[tokio::test]
async fn users_are_keyed_hashes_and_strict_mode_rotates_them_daily() {
    let app = App::new().await;
    let p = app.project_id;
    let secret: Vec<u8> = app
        .kestrel
        .core
        .with_conn(move |c| {
            c.query_row("SELECT app_secret FROM projects WHERE id = ?1", [p], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap();
    assert_eq!(secret.len(), 32, "a random secret per project");
    let now = now_ms();
    let today = day_number(now / 1000);
    let two_days = |kind: &'static str| {
        let mut a = Sig::new(kind, "install-1");
        a.time = now - 1000;
        let mut b = Sig::new(kind, "install-1");
        b.time = now - 86_400_000;
        [a, b]
    };
    send(&app, &two_days("Normal")).await;
    set(&app, "UPDATE projects SET app_strict = 1 WHERE id = {p}").await;
    send(&app, &two_days("Strict")).await;
    let rows = raw_rows(&app).await;
    let users = |kind: &str| -> Vec<i64> {
        rows.iter()
            .filter(|r| r["type"] == kind)
            .map(|r| r["user"].as_i64().unwrap())
            .collect()
    };
    let normal = users("Normal");
    assert_eq!(normal[0], normal[1], "stable across days");
    assert_eq!(
        normal[0],
        app::user_hash(&secret, false, today, "install-1")
    );
    let strict = users("Strict");
    assert_ne!(strict[0], strict[1], "a new id every day");
    assert_ne!(strict[0], normal[0]);
    assert_eq!(strict[0], app::user_hash(&secret, true, today, "install-1"));
    assert_eq!(
        strict[1],
        app::user_hash(&secret, true, today - 1, "install-1")
    );
    // Another project's secret gives other users.
    let other: Vec<u8> = app
        .kestrel
        .core
        .with_conn(|c| {
            let id = keen_kestrel::db::projects::insert(
                c,
                "Other",
                "other",
                keen_kestrel::project::Platform::Apple,
                "Default",
            )?;
            c.query_row("SELECT app_secret FROM projects WHERE id = ?1", [id], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap();
    assert_eq!(other.len(), 32);
    assert_ne!(other, secret);
    // The secret is never in the API.
    let uid = add_user(&app, "ana", "viewer").await;
    let token = api_token(&app, uid, TokenScope::Read).await;
    let res = bearer(&app, "GET", &format!("/api/v1/projects/{p}"), &token, None).await;
    assert!(!res.body.contains("secret"), "{}", res.body);
}

// ---------------------------------------------------------------------------
// Active users, rollups, retention
// ---------------------------------------------------------------------------

/// 2026-10-22 00:00:00 UTC.
const DAY: i64 = 1_792_627_200;

async fn insert(app: &App, signals: Vec<AppSignal>) {
    let pid = app.project_id;
    app.kestrel
        .ingest
        .writer()
        .run(move |c| {
            for mut s in signals {
                s.project_id = pid;
                app::insert(c, &s)?;
            }
            Ok(())
        })
        .await
        .unwrap();
}

async fn rollup_at(app: &App, now: i64) -> rollup::Report {
    app.kestrel
        .ingest
        .writer()
        .run(move |c| rollup::rollup_all(c, now))
        .await
        .unwrap()
}

fn signal(day: i64, second: i64, user: i64) -> AppSignal {
    AppSignal {
        ts: day * 86_400 + second,
        user,
        session: user * 1000 + day,
        kind: "App.launched".into(),
        bundle_id: "com.example.app".into(),
        version: if day % 2 == 0 { "1.4.0" } else { "1.3.0" }.into(),
        build: "57".into(),
        platform: "iOS".into(),
        os_version: "18.2".into(),
        model: "iPhone16,2".into(),
        locale: "nb_NO".into(),
        ..AppSignal::default()
    }
}

/// `(day, dau, wau, mau)` of every app day row.
async fn day_rows(app: &App) -> BTreeMap<i64, (i64, i64, i64, i64, i64)> {
    app.kestrel
        .core
        .with_conn(|c| {
            c.prepare(
                "SELECT day, visitors, wau, mau, sessions, events FROM analytics_days
                 WHERE source = 'app' ORDER BY day",
            )?
            .query_map([], |r| {
                let day: String = r.get(0)?;
                Ok((day, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            })?
            .map(|r| {
                r.map(|(d, v)| {
                    let date: jiff::civil::Date = d.parse().unwrap();
                    (keen_kestrel::analytics::day_of(date), v)
                })
            })
            .collect()
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn daily_weekly_and_monthly_users_are_exact() {
    let app = App::new().await;
    let base = day_number(DAY) - 45;
    // Who uses the app on which day: a pattern with churn and newcomers.
    let mut active: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    let users_on = |d: i64| -> Vec<i64> {
        let mut out: Vec<i64> = (0..40)
            .filter(|u| (u * 7 + d * 3) % 11 < 3 && !(u % 5 == 0 && d > 20))
            .collect();
        if d >= 25 {
            out.push(100 + d);
        }
        out
    };
    for d in 0..45 {
        let day = base + d;
        let users = users_on(d);
        let mut signals = Vec::new();
        for &u in &users {
            signals.push(signal(day, 3600 * (u % 20), u));
            signals.push(signal(day, 3600 * (u % 20) + 60, u));
        }
        insert(&app, signals).await;
        active.entry(day).or_default().extend(users);
        rollup_at(&app, day * 86_400 + 12 * 3600).await;
        // Now and then a signal for yesterday arrives late, after today's
        // row was made: it counts on yesterday and in today's windows.
        if d % 7 == 3 {
            let late = 500 + d;
            insert(&app, vec![signal(day - 1, 80_000, late)]).await;
            active.entry(day - 1).or_default().insert(late);
            rollup_at(&app, day * 86_400 + 13 * 3600).await;
        }
    }
    // An older day gets a signal late: it is rolled up again from the raw
    // signals, with the windows of the days after it.
    let today = base + 44;
    insert(&app, vec![signal(today - 5, 50_000, 900)]).await;
    active.entry(today - 5).or_default().insert(900);
    let report = rollup_at(&app, today * 86_400 + 20 * 3600).await;
    assert_eq!(report.late_days, 1);

    let rows = day_rows(&app).await;
    let mut checked = 0;
    for (day, (dau, wau, mau, sessions, signals)) in &rows {
        let window = |len: i64| -> usize {
            active
                .range(day - len + 1..=*day)
                .flat_map(|(_, u)| u.iter())
                .collect::<BTreeSet<_>>()
                .len()
        };
        let users = active.get(day).map_or(0, BTreeSet::len) as i64;
        assert_eq!(*dau, users, "dau of {day}");
        assert_eq!(*wau, window(7) as i64, "wau of {day}");
        assert_eq!(*mau, window(30) as i64, "mau of {day}");
        assert_eq!(*sessions, users, "one session per user and day");
        assert!(*signals >= users);
        checked += 1;
    }
    assert_eq!(checked, 45, "{rows:?}");
    // Only a month of daily users is kept in the store.
    assert!(
        app.store_count("SELECT COUNT(DISTINCT day) FROM app_users")
            .await
            <= 31
    );
}

#[tokio::test]
async fn rollups_count_hours_breakdowns_and_bounded_payloads() {
    let app = App::new().await;
    let day = day_number(DAY);
    let mut signals = Vec::new();
    for i in 0..60 {
        let mut s = signal(day, 3600 + i, i % 3);
        s.kind = "Picker.used".into();
        s.payload = Some(json!({"choice": format!("v{i:02}"), "kind": "fixed"}).to_string());
        signals.push(s);
    }
    // Test signals stay raw.
    let mut debug = signal(day, 7200, 77);
    debug.flags = app::FLAG_DEBUG;
    signals.push(debug);
    let mut tf = signal(day, 7200, 78);
    tf.flags = app::FLAG_TESTFLIGHT;
    signals.push(tf);
    insert(&app, signals).await;
    let report = rollup_at(&app, DAY + 12 * 3600).await;
    assert_eq!(report.events, 62);
    let rows = day_rows(&app).await;
    assert_eq!(rows[&day].0, 3, "test signals aren't counted");
    assert_eq!(rows[&day].4, 60);
    assert_eq!(
        app.count("SELECT SUM(events) FROM analytics_hours WHERE source = 'app'")
            .await,
        60
    );
    assert_eq!(
        app.count("SELECT visitors FROM analytics_hours WHERE source = 'app' AND hour = '2026-10-22 01:00:00'").await,
        3
    );
    let a = &app;
    let value = |dim: &'static str| async move {
        a.kestrel
            .core
            .with_conn(move |c| {
                c.prepare(
                    "SELECT value, count, visitors FROM analytics_breakdown
                     WHERE source = 'app' AND dim = ?1 ORDER BY value",
                )?
                .query_map([dim], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .unwrap()
    };
    assert_eq!(value("type").await, [("Picker.used".into(), 60, 3)]);
    assert_eq!(value("version").await, [("1.4.0".into(), 60, 3)]);
    assert_eq!(value("build").await, [("1.4.0 (57)".into(), 60, 3)]);
    assert_eq!(
        value("release").await,
        [("com.example.app@1.4.0+57".into(), 60, 3)]
    );
    assert_eq!(value("os").await, [("iOS 18.2".into(), 60, 3)]);
    let payload = value("payload").await;
    let choices: Vec<_> = payload
        .iter()
        .filter(|(v, _, _)| v.starts_with("Picker.used\u{1f}choice\u{1f}"))
        .collect();
    assert_eq!(choices.len(), 51, "50 values and the rest");
    let other = choices
        .iter()
        .find(|(v, _, _)| v.ends_with(app::rollup::OTHER))
        .unwrap();
    assert_eq!(other.1, 10);
    assert!(payload.contains(&("Picker.used\u{1f}kind\u{1f}fixed".into(), 60, 0)));
    // A second rollup adds nothing; new values of a full key go to the rest.
    let mut s = signal(day, 9000, 5);
    s.kind = "Picker.used".into();
    s.payload = Some(json!({"choice": "v99"}).to_string());
    insert(&app, vec![s]).await;
    rollup_at(&app, DAY + 13 * 3600).await;
    let payload = value("payload").await;
    assert!(
        payload
            .iter()
            .any(|(v, n, _)| v.ends_with(app::rollup::OTHER) && *n == 11)
    );
    // The day rolled up again from the raw signals gives the same numbers.
    let before = (
        value("payload").await,
        value("version").await,
        day_rows(&app).await,
    );
    app.kestrel
        .ingest
        .writer()
        .run(move |c| app::rollup::recompute_day(c, 1, day, day))
        .await
        .unwrap();
    let after = (
        value("payload").await,
        value("version").await,
        day_rows(&app).await,
    );
    assert_eq!(before.1, after.1);
    assert_eq!(before.2, after.2);
    let count = |p: &Vec<(String, i64, i64)>| p.iter().map(|x| x.1).sum::<i64>();
    assert_eq!(count(&before.0), count(&after.0));
}

#[tokio::test]
async fn retention_removes_raw_signals_old_rollups_and_deleted_projects() {
    let app = App::new().await;
    let day = day_number(DAY);
    insert(
        &app,
        vec![
            signal(day - 40, 100, 1),
            signal(day - 2, 100, 2),
            signal(day, 100, 3),
        ],
    )
    .await;
    rollup_at(&app, DAY + 12 * 3600).await;
    app.exec(
        "INSERT INTO analytics_days (project_id, source, day, events, visitors)
         SELECT id, 'app', '2025-01-01', 5, 5 FROM projects"
            .into(),
    )
    .await;
    let now = "2026-10-22 12:00:00".to_string();
    let report = app
        .kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::retention::run_blocking(c, &now))
        .await
        .unwrap();
    assert_eq!(report.analytics_events_deleted, 1);
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM analytics_events WHERE source = 'app'")
            .await,
        2
    );
    assert_eq!(
        app.count(
            "SELECT COUNT(*) FROM analytics_days WHERE source = 'app' AND day = '2025-01-01'"
        )
        .await,
        0
    );
    assert!(
        app.count("SELECT COUNT(*) FROM analytics_days WHERE source = 'app'")
            .await
            >= 1
    );
    let pid = app.project_id;
    app.kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::db::triage::delete_project(c, pid))
        .await
        .unwrap();
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM analytics_events")
            .await,
        0
    );
    assert_eq!(app.store_count("SELECT COUNT(*) FROM app_users").await, 0);
    assert_eq!(app.count("SELECT COUNT(*) FROM analytics_days").await, 0);
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn unescape(html: &str) -> String {
    html.replace("&#38;", "&")
        .replace("&amp;", "&")
        .replace("&#34;", "\"")
        .replace("&quot;", "\"")
        .replace("&#60;", "<")
        .replace("&lt;", "<")
        .replace("&#62;", ">")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
}

/// A few users on two versions, with sessions, a paywall with a payload, and
/// a TestFlight user.
async fn some_signals(app: &App) {
    let mut all = Vec::new();
    for (i, version) in ["1.4.0", "1.4.0", "1.3.0"].into_iter().enumerate() {
        let install = format!("install-{i}");
        let mut launched = Sig::new("App.launched", &install);
        launched.version = version;
        let mut paywall = Sig::new("Paywall.shown", &install);
        paywall.version = version;
        paywall.payload =
            json!({"plan": if i == 2 { "free" } else { "pro" }, "source": "onboarding"});
        all.push(launched);
        all.push(paywall);
    }
    let mut tester = Sig::new("App.launched", "install-tf");
    tester.version = "1.5.0";
    tester.flags = (false, true, false);
    all.push(tester);
    send(app, &all).await;
    roll(app).await;
}

#[tokio::test]
async fn the_app_tab_shows_setup_then_numbers_filters_and_the_explorer() {
    let app = App::new().await;
    let p = app.project_id;
    let mut web = Web::new(&app, "vera", "viewer").await;
    // Empty: how to add KestrelKit, with this project's address and key.
    let res = web.get(&format!("/projects/{p}/app")).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let html = unescape(&res.body);
    assert!(html.contains("https://github.com/Ferdinand99/KestrelKit"));
    assert!(html.contains(&format!(
        "Kestrel.configure(url: URL(string: \"http://kestrel.test:3002\")!, key: \"{}\")",
        app.key
    )));
    assert!(html.contains("Kestrel.signal(\"Paywall.shown\""));
    assert!(html.contains("KestrelKit#readme"));
    assert!(
        html.contains(&format!("href=\"/projects/{p}/app\"")),
        "the tab"
    );
    let res = web.get(&format!("/projects/{p}/app/signals")).await;
    assert_eq!(res.status, StatusCode::OK);
    let overview = web.get(&format!("/projects/{p}")).await;
    assert!(!overview.body.contains("Open app analytics"));

    some_signals(&app).await;
    let res = web.get(&format!("/projects/{p}/app")).await;
    assert_eq!(res.status, StatusCode::OK);
    let html = unescape(&res.body);
    for want in [
        "Daily active users",
        "Weekly active users",
        "Monthly active users",
        "Paywall.shown",
        "App.launched",
        "1.4.0",
        "1.3.0",
        "1.4.0 (57)",
        "iOS 18.2",
        "iPhone16,2",
        "nb_NO",
        "Version adoption",
        "1 test signal in this period isn’t counted.",
        "app/live?",
    ] {
        assert!(html.contains(want), "{want} missing");
    }
    assert!(
        !html.contains("1.5.0"),
        "the TestFlight version is a test signal"
    );
    for period in [
        "today",
        "30d",
        "custom&from=2026-10-01&to=2026-10-22",
        "nonsense",
    ] {
        let res = web.get(&format!("/projects/{p}/app?period={period}")).await;
        assert_eq!(res.status, StatusCode::OK, "{period}");
    }
    // Filters read the raw signals; the test toggle includes TestFlight.
    let res = web
        .get(&format!("/projects/{p}/app?period=7d&version=1.3.0"))
        .await;
    let html = unescape(&res.body);
    assert!(html.contains("Version: 1.3.0"), "a chip");
    assert!(!html.contains("1.4.0 (57)"));
    let res = web
        .get(&format!("/projects/{p}/app?period=7d&test=1"))
        .await;
    let html = unescape(&res.body);
    assert!(html.contains("1.5.0"));
    assert!(html.contains("With test signals"));
    let res = web
        .get(&format!(
            "/projects/{p}/app?period=7d&platform=iOS&type=Paywall.shown"
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK);

    // The explorer: every type, then one type's payload.
    let res = web.get(&format!("/projects/{p}/app/signals")).await;
    let html = unescape(&res.body);
    assert!(html.contains(&format!(
        "/projects/{p}/app/signals?period=7d&type=Paywall.shown"
    )));
    let res = web
        .get(&format!("/projects/{p}/app/signals?type=Paywall.shown"))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let html = unescape(&res.body);
    for want in ["Payload", "plan", "pro", "free", "source", "onboarding"] {
        assert!(html.contains(want), "{want} missing");
    }
    let res = web
        .get(&format!(
            "/projects/{p}/app/signals?type=Paywall.shown&version=1.3.0"
        ))
        .await;
    let html = unescape(&res.body);
    assert!(html.contains("free") && !html.contains(">pro<"), "filtered");
    let res = web
        .get(&format!("/projects/{p}/app/signals?type=App.launched"))
        .await;
    assert!(unescape(&res.body).contains("No payload"));

    // The overview's app summary.
    let overview = web.get(&format!("/projects/{p}")).await;
    assert!(overview.body.contains("Open app analytics"));
    assert_eq!(
        web.get("/projects/999/app").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn editors_switch_app_analytics_and_strict_mode() {
    let app = App::new().await;
    let p = app.project_id;
    let mut viewer = Web::new(&app, "vic", "viewer").await;
    let res = viewer
        .post(
            &format!("/projects/{p}/app/settings"),
            &[("enabled", "true")],
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let settings = viewer.get(&format!("/projects/{p}/settings")).await;
    assert!(settings.body.contains("Strict mode"));
    let mut editor = Web::new(&app, "eda", "editor").await;
    let res = editor
        .post(
            &format!("/projects/{p}/app/settings"),
            &[("enabled", "true"), ("strict", "true")],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        app.count("SELECT app_strict FROM projects WHERE id = 1")
            .await,
        1
    );
    editor
        .post(&format!("/projects/{p}/app/settings"), &[])
        .await;
    assert_eq!(
        app.count("SELECT app_analytics + app_strict FROM projects WHERE id = 1")
            .await,
        0
    );
    let page = editor.get(&format!("/projects/{p}/app")).await;
    assert!(page.body.contains("App analytics is off"));
    let res = app
        .send(request(Some(&app.key), GOLDEN.as_bytes().to_vec(), None))
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Releases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn releases_show_their_share_of_the_apps_users() {
    let app = App::new().await;
    let p = app.project_id;
    let now = keen_kestrel::issues::now();
    let ids: Vec<i64> = app
        .kestrel
        .core
        .with_conn(move |c| {
            let mut ids = Vec::new();
            for v in [
                "com.example.app@1.4.0+57",
                "com.example.app@1.3.0",
                "server@2.0.0",
            ] {
                ids.push(keen_kestrel::db::releases::ensure(c, p, v, &now, None)?);
            }
            Ok(ids)
        })
        .await
        .unwrap();
    let mut signals = Vec::new();
    for (i, (version, build)) in [("1.4.0", "57"), ("1.4.0", "57"), ("1.3.0", "50")]
        .into_iter()
        .enumerate()
    {
        let mut s = Sig::new("App.launched", &format!("install-{i}"));
        s.version = version;
        s.build = build;
        signals.push(s);
    }
    send(&app, &signals).await;
    roll(&app).await;
    let uid = add_user(&app, "ana", "viewer").await;
    let token = api_token(&app, uid, TokenScope::Read).await;
    let res = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/releases"),
        &token,
        None,
    )
    .await;
    let list = res.json();
    let of = |version: &str| {
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["version"] == version)
            .unwrap()["adoption_users_7d"]
            .clone()
    };
    assert_eq!(of("com.example.app@1.4.0+57"), json!(66.67));
    assert_eq!(
        of("com.example.app@1.3.0"),
        json!(33.33),
        "every build of a version"
    );
    assert_eq!(of("server@2.0.0"), Value::Null);
    let mut web = Web::new(&app, "vera", "viewer").await;
    let page = web.get(&format!("/projects/{p}/releases/{}", ids[0])).await;
    assert!(
        page.body
            .contains("of the app’s daily users in the last 7 days")
    );
    assert!(page.body.contains("66"));
    let page = web.get(&format!("/projects/{p}/releases/{}", ids[2])).await;
    assert!(page.body.contains("of the sessions in the last 7 days"));
    let list = web.get(&format!("/projects/{p}/releases")).await;
    assert!(list.body.contains("33"));
}

// ---------------------------------------------------------------------------
// API and MCP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_api_and_mcp_answer_app_analytics_with_counts_only() {
    let app = App::new().await;
    let p = app.project_id;
    some_signals(&app).await;
    let uid = add_user(&app, "ana", "viewer").await;
    let token = api_token(&app, uid, TokenScope::Read).await;
    let get = |path: String| {
        let (app, token) = (&app, token.clone());
        async move { bearer(app, "GET", &path, &token, None).await }
    };
    let ts = get(format!(
        "/api/v1/projects/{p}/analytics/app/timeseries?period=today"
    ))
    .await;
    assert_eq!(ts.status, StatusCode::OK, "{}", ts.body);
    let v = ts.json();
    assert_eq!(v["source"], "app");
    assert_eq!(v["interval"], "hour");
    assert_eq!(v["data"].as_array().unwrap().len(), 24);
    assert_eq!(v["totals"]["dau"], 3);
    assert_eq!(v["totals"]["wau"], 3);
    assert_eq!(v["totals"]["mau"], 3);
    assert_eq!(v["totals"]["signals"], 6);
    assert_eq!(v["totals"]["sessions"], 3);
    assert_eq!(v["current_users"], 3);
    let week = get(format!("/api/v1/projects/{p}/analytics/app/timeseries"))
        .await
        .json();
    assert_eq!(week["interval"], "day");
    assert_eq!(week["data"].as_array().unwrap().last().unwrap()["wau"], 3);
    let test = get(format!(
        "/api/v1/projects/{p}/analytics/app/timeseries?test=1"
    ))
    .await
    .json();
    assert_eq!(test["totals"]["dau"], 4);
    assert_eq!(test["filters"]["include_test"], true);
    let b = get(format!(
        "/api/v1/projects/{p}/analytics/app/breakdown?dimension=version"
    ))
    .await;
    assert_eq!(b.status, StatusCode::OK, "{}", b.body);
    assert_eq!(
        b.json()["data"][0],
        json!({"value": "1.4.0", "label": "1.4.0", "users": 2, "count": 4})
    );
    let f = get(format!(
        "/api/v1/projects/{p}/analytics/app/breakdown?dimension=type&version=1.3.0"
    ))
    .await
    .json();
    assert_eq!(f["filters"]["version"], "1.3.0");
    assert_eq!(f["data"].as_array().unwrap().len(), 2);
    let types = get(format!("/api/v1/projects/{p}/analytics/app/signal-types"))
        .await
        .json();
    assert_eq!(types["data"].as_array().unwrap().len(), 2);
    let payload = get(format!(
        "/api/v1/projects/{p}/analytics/app/payload?type=Paywall.shown"
    ))
    .await;
    assert_eq!(payload.status, StatusCode::OK, "{}", payload.body);
    let v = payload.json();
    assert_eq!(v["data"][0]["key"], "plan");
    assert_eq!(
        v["data"][0]["values"][0],
        json!({"value": "pro", "label": "pro", "count": 2})
    );
    let adoption = get(format!(
        "/api/v1/projects/{p}/analytics/app/adoption?period=today"
    ))
    .await
    .json();
    assert_eq!(adoption["versions"][0]["version"], "1.4.0");
    assert_eq!(adoption["versions"][0]["latest"], 66.67);
    for bad in [
        "breakdown?dimension=nope",
        "breakdown?dimension=version&limit=0",
        "timeseries?period=week",
        "payload",
        "payload?type=bad%20type",
        "adoption?limit=99",
    ] {
        let res = get(format!("/api/v1/projects/{p}/analytics/app/{bad}")).await;
        assert_eq!(
            res.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            res.body
        );
    }
    assert_eq!(
        get("/api/v1/projects/999/analytics/app/timeseries".into())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // The reference documents the routes.
    let docs = get("/api/v1/openapi.json".into()).await;
    for path in [
        "/analytics/app/timeseries",
        "/analytics/app/breakdown",
        "/analytics/app/signal-types",
        "/analytics/app/payload",
        "/analytics/app/adoption",
        "/ingest/v1/signals",
    ] {
        assert!(docs.body.contains(path), "{path} undocumented");
    }

    let mcp = |name: &'static str, args: Value| {
        let (app, token) = (&app, token.clone());
        async move {
            let res = bearer(app, "POST", "/mcp", &token, Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}))).await;
            assert_eq!(res.status, StatusCode::OK, "{}", res.body);
            res.json()["result"].clone()
        }
    };
    // The project only has app analytics: the summary is the app's.
    let summary = mcp("analytics_summary", json!({"project": "fixtures"})).await;
    let text = summary["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("App analytics of Fixtures"), "{text}");
    assert!(text.contains("3 today"), "{text}");
    assert!(text.contains("Paywall.shown"), "{text}");
    assert_eq!(
        summary["structuredContent"]["top"]["version"][0]["value"],
        "1.4.0"
    );
    let web_summary = mcp("analytics_summary", json!({"project": p, "source": "web"})).await;
    assert!(
        web_summary["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Web analytics")
    );
    let breakdown = mcp(
        "analytics_breakdown",
        json!({"project": p, "dimension": "model", "filters": {"platform": "iOS"}}),
    )
    .await;
    assert_eq!(
        breakdown["structuredContent"]["data"][0]["value"],
        "iPhone16,2"
    );
    let breakdown = mcp(
        "analytics_breakdown",
        json!({"project": p, "source": "app", "dimension": "os", "include_test": true}),
    )
    .await;
    assert_eq!(breakdown["structuredContent"]["data"][0]["users"], 4);
    let bad = mcp(
        "analytics_breakdown",
        json!({"project": p, "source": "app", "dimension": "page"}),
    )
    .await;
    assert_eq!(bad["isError"], true);
    let bad = mcp("analytics_summary", json!({"project": p, "source": "tv"})).await;
    assert_eq!(bad["isError"], true);

    // No user hash anywhere.
    let users: Vec<i64> = raw_rows(&app)
        .await
        .iter()
        .flat_map(|r| [r["user"].as_i64().unwrap(), r["session"].as_i64().unwrap()])
        .collect();
    for body in [
        ts.body,
        b.body,
        payload.body,
        summary.to_string(),
        breakdown.to_string(),
    ] {
        for u in &users {
            assert!(!body.contains(&u.to_string()), "a user hash leaked: {body}");
        }
    }
}
