//! Ingest driven in-process: every recorded SDK request (contract), what
//! ends up in both databases (privacy), authentication, size limits, rate
//! limits, CORS, the writer under a burst, regressions and capture mode.

mod common;

use std::io::Write as _;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::{App, CLIENT_IP, envelope_request, envelope_with, new_id};
use keen_kestrel::ingest::Settings;
use keen_kestrel::writer::WriterSettings;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// Every fixture answers 200 with the right id, and everything with an
/// event ends up stored.
#[tokio::test]
async fn every_fixture_is_accepted() {
    let app = App::new().await;
    let fixtures = common::fixtures();
    let mut events = 0;
    for f in &fixtures {
        let res = app.send_fixture(f).await;
        assert_eq!(res.status, StatusCode::OK, "{}: {}", f.id(), res.body);
        match f.expected_id() {
            Some(id) => assert_eq!(res.json(), json!({ "id": id }), "{}", f.id()),
            None => assert_eq!(res.json(), json!({}), "{}", f.id()),
        }
        events += f.events().len() as i64;
    }
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, events);
    assert_eq!(
        app.count("SELECT SUM(times_seen) FROM issues").await,
        events
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM payloads").await,
        events
    );
    // Sessions from cocoa, node and python; client reports are dropped.
    assert!(app.store_count("SELECT COUNT(*) FROM session_items").await >= 4);
    assert_eq!(
        app.kestrel
            .ingest
            .writer()
            .stats()
            .failed
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    // Releases and environments are created on first sight.
    let releases = app.count("SELECT COUNT(*) FROM releases").await;
    assert_eq!(
        releases, 4,
        "fixture-{{python,node,rust}}@1.0.0 and the iOS app"
    );
    assert!(
        app.count("SELECT COUNT(*) FROM environments WHERE name = 'fixtures'")
            .await
            == 1
    );

    // The same events again are duplicates: nothing changes.
    for f in &fixtures {
        assert_eq!(app.send_fixture(f).await.status, StatusCode::OK);
    }
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, events);
    assert_eq!(
        app.count("SELECT SUM(times_seen) FROM issues").await,
        events
    );

    // Issues are searchable by title and culprit.
    let hits = app
        .count("SELECT COUNT(*) FROM issues_fts WHERE issues_fts MATCH 'quantity'")
        .await;
    assert!(hits >= 3, "{hits}");
}

/// After ingest, no text or blob in either database holds the client's IP
/// address, its user agent, a raw email address or any of the secrets the
/// fixtures carry.
#[tokio::test]
async fn nothing_personal_is_stored() {
    let app = App::new().await;
    for f in common::fixtures() {
        let mut req = f.request(app.project_id, &app.key);
        req.headers_mut().insert(
            header::USER_AGENT,
            "Mozilla/5.0 (X11; Linux x86_64) KestrelFixture/1.0"
                .parse()
                .unwrap(),
        );
        assert_eq!(app.send(req).await.status, StatusCode::OK, "{}", f.id());
    }
    // Feedback, a session with an IP and user agent, and an attachment.
    let id = new_id();
    let event =
        json!({"event_id": id, "message": "with feedback", "user": {"ip_address": "{{auto}}"}});
    let report = json!({"event_id": id, "name": "Ada", "email": "ada.lovelace@example.com", "comments": "card 4111111111111111 failed"});
    let session = json!({"sid": new_id(), "did": "ada.lovelace@example.com", "status": "ok", "init": true,
        "attrs": {"release": "web@1", "ip_address": CLIENT_IP, "user_agent": "KestrelFixture/1.0"}});
    let feedback = json!({"event_id": new_id(), "contexts": {"feedback": {"message": "it broke, token=fixture-secret-token-123",
        "contact_email": "ada.lovelace@example.com", "name": "Ada", "url": "https://shop.example/?api_key=fixture-api-key-321"}}});
    let attachment = b"log line from the app";
    let body = format!(
        "{{\"event_id\":\"{id}\"}}\n{{\"type\":\"event\"}}\n{event}\n{{\"type\":\"user_report\"}}\n{report}\n{{\"type\":\"session\"}}\n{session}\n{{\"type\":\"feedback\"}}\n{feedback}\n{{\"type\":\"attachment\",\"length\":{},\"filename\":\"app.log\"}}\n{}\n",
        attachment.len(),
        String::from_utf8_lossy(attachment)
    );
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            body.into_bytes(),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    app.flush().await;
    assert_eq!(app.store_count("SELECT COUNT(*) FROM feedback").await, 2);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM attachments").await, 1);

    // The SDKs' own user ids (sentry-cocoa's is its install id) are hashed
    // like email addresses and user names. An id written into the fixture
    // apps' code (`setUser({ id: "user-42" })`) also arrives as source
    // context, which is kept as code: those ids aren't searched for.
    let payloads: Vec<String> = common::fixtures()
        .iter()
        .flat_map(|f| f.events())
        .map(|(payload, _)| String::from_utf8_lossy(&payload).into_owned())
        .collect();
    let in_code = |id: &str| {
        payloads.iter().any(|p| {
            p.contains(&format!("id: \\\"{id}\\\""))
                || p.contains(&format!("\\\"id\\\": \\\"{id}\\\""))
        })
    };
    let user_ids: Vec<String> = payloads
        .iter()
        .filter_map(|p| {
            let event: serde_json::Value = serde_json::from_str(p).ok()?;
            Some(event["user"]["id"].as_str()?.to_string())
        })
        .filter(|id| !in_code(id))
        .collect();
    assert!(
        user_ids
            .iter()
            .any(|id| id == "90712615-8B72-4805-9A69-013EFE8FB8E4"),
        "{user_ids:?}"
    );
    let mut needles: Vec<&str> = vec![
        CLIENT_IP,
        "KestrelFixture/1.0",
        "ada.lovelace@example.com",
        "fixture-secret-token-123",
        "fixture-cookie-456",
        "fixture-password-789",
        "fixture-api-key-321",
        "fixture-query-token",
        "4111111111111111",
        "4111 1111 1111 1111",
        "4111-1111-1111-1111",
        "{{auto}}",
    ];
    let lower: Vec<String> = user_ids.iter().map(|id| id.to_lowercase()).collect();
    needles.extend(user_ids.iter().chain(&lower).map(String::as_str));
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
    assert!(scanned > 200, "{scanned}");
    // Every stored user id is a hash, those written into code included.
    let conn = app.kestrel.ingest.store().get().await.unwrap();
    let blobs: Vec<Vec<u8>> = conn
        .interact(|c| {
            c.prepare("SELECT data FROM payloads")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()
        })
        .await
        .unwrap()
        .unwrap();
    let mut ids = 0;
    for blob in blobs {
        let event: serde_json::Value =
            serde_json::from_slice(&keen_kestrel::store::decompress(&blob, 64 << 20).unwrap())
                .unwrap();
        if let Some(id) = event["user"]["id"].as_str() {
            assert!(id.starts_with("hash:"), "{id}");
            ids += 1;
        }
    }
    assert!(ids >= 5, "{ids}");
    // The hashes are there instead.
    let conn = app.kestrel.ingest.store().get().await.unwrap();
    let report: String = conn
        .interact(|c| c.query_row("SELECT data FROM feedback WHERE data LIKE '%comments%' OR data LIKE '%Ada%' LIMIT 1", [], |r| r.get(0)))
        .await
        .unwrap()
        .unwrap();
    assert!(report.contains("\"email\":\"hash:"), "{report}");
}

#[tokio::test]
async fn keys_must_exist_be_enabled_and_belong_to_the_project() {
    let app = App::new().await;
    let body = || envelope_with(&json!({"event_id": new_id(), "message": "x"}));
    let unknown = "ffffffffffffffffffffffffffffffff";
    let res = app
        .send(envelope_request(app.project_id, unknown, body()))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
    assert_eq!(res.header("x-sentry-error"), "unknown DSN key");
    let res = app
        .send(envelope_request(app.project_id, "not-a-key", body()))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    // No credentials at all.
    let req = Request::post(format!("/api/{}/envelope/", app.project_id))
        .body(Body::from(body()))
        .unwrap();
    assert_eq!(app.send(req).await.status, StatusCode::UNAUTHORIZED);
    // The key of another project.
    let other = app
        .kestrel
        .core
        .with_conn(|c| {
            keen_kestrel::db::projects::insert(
                c,
                "Other",
                "other",
                keen_kestrel::project::Platform::Other,
                "Default",
            )
        })
        .await
        .unwrap();
    let res = app.send(envelope_request(other, &app.key, body())).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    let res = app.send(envelope_request(999, &app.key, body())).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = app.send(envelope_request(0, &app.key, body())).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    // A disabled key.
    app.exec(format!(
        "UPDATE project_keys SET enabled = 0 WHERE project_id = {}",
        app.project_id
    ))
    .await;
    let res = app
        .send(envelope_request(app.project_id, &app.key, body()))
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(res.header("x-sentry-error"), "the DSN key is disabled");
    app.exec(format!(
        "UPDATE project_keys SET enabled = 1 WHERE project_id = {}",
        app.project_id
    ))
    .await;

    // The key in the query string, and in the envelope header's DSN.
    let req = Request::post(format!(
        "/api/{}/envelope/?sentry_key={}&sentry_version=7",
        app.project_id, app.key
    ))
    .body(Body::from(body()))
    .unwrap();
    assert_eq!(app.send(req).await.status, StatusCode::OK);
    let id = new_id();
    let dsn_body = format!(
        "{{\"event_id\":\"{id}\",\"dsn\":\"http://{}@kestrel.test:3002/{}\"}}\n{{\"type\":\"event\"}}\n{{\"message\":\"via dsn\"}}\n",
        app.key, app.project_id
    );
    let req = Request::post(format!("/api/{}/envelope/", app.project_id))
        .body(Body::from(dsn_body))
        .unwrap();
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["id"], id);

    // The legacy store endpoint.
    let id = new_id();
    let req = Request::post(format!("/api/{}/store/", app.project_id))
        .header(
            "x-sentry-auth",
            format!("Sentry sentry_key={}, sentry_secret=ignored", app.key),
        )
        .body(Body::from(
            json!({"event_id": id, "message": "stored"}).to_string(),
        ))
        .unwrap();
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json(), json!({ "id": id }));
}

#[tokio::test]
async fn sizes_and_encodings_are_bounded() {
    let app = App::new().await;
    let send = |body: Vec<u8>, encoding: Option<&str>| {
        let mut req = envelope_request(app.project_id, &app.key, body);
        if let Some(e) = encoding {
            req.headers_mut()
                .insert(header::CONTENT_ENCODING, e.parse().unwrap());
        }
        app.send(req)
    };
    // A zip bomb: 30 MB of zeros in a few kilobytes.
    let bomb = gzip(&vec![0u8; 30 * 1024 * 1024]);
    assert!(bomb.len() < 100 * 1024);
    assert_eq!(
        send(bomb, Some("gzip")).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    // Over 20 MB as sent.
    let huge = vec![b' '; 21 * 1024 * 1024];
    assert_eq!(send(huge, None).await.status, StatusCode::PAYLOAD_TOO_LARGE);
    // An event over 1 MB.
    let big = envelope_with(&json!({"event_id": new_id(), "message": "x".repeat(1_100_000)}));
    assert_eq!(send(big, None).await.status, StatusCode::PAYLOAD_TOO_LARGE);
    let req = Request::post(format!("/api/{}/store/", app.project_id))
        .header("x-sentry-auth", format!("Sentry sentry_key={}", app.key))
        .body(Body::from(
            json!({"message": "x".repeat(1_100_000)}).to_string(),
        ))
        .unwrap();
    assert_eq!(app.send(req).await.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Broken bodies.
    assert_eq!(
        send(b"not gzip".to_vec(), Some("gzip")).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(b"{}".to_vec(), Some("compress")).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(b"not json\n".to_vec(), None).await.status,
        StatusCode::BAD_REQUEST
    );
    let bad_event = b"{}\n{\"type\":\"event\"}\n[1,2]\n".to_vec();
    assert_eq!(send(bad_event, None).await.status, StatusCode::BAD_REQUEST);
    // Every encoding works.
    let ok = || envelope_with(&json!({"event_id": new_id(), "message": "compressed"}));
    assert_eq!(send(gzip(&ok()), Some("gzip")).await.status, StatusCode::OK);
    assert_eq!(
        send(zstd::bulk::compress(&ok(), 3).unwrap(), Some("zstd"))
            .await
            .status,
        StatusCode::OK
    );
    let mut br = Vec::new();
    brotli::CompressorWriter::new(&mut br, 4096, 5, 22)
        .write_all(&ok())
        .unwrap();
    assert_eq!(send(br, Some("br")).await.status, StatusCode::OK);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&ok()).unwrap();
    assert_eq!(
        send(z.finish().unwrap(), Some("deflate")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn rate_limits_answer_429_with_headers() {
    let app = App::with(Settings {
        ip_rate_limit_per_minute: 5,
        ..Settings::default()
    })
    .await;
    app.exec(format!(
        "UPDATE project_keys SET rate_limit_per_minute = 2 WHERE project_id = {}",
        app.project_id
    ))
    .await;
    let body = || envelope_with(&json!({"event_id": new_id(), "message": "x"}));
    for _ in 0..2 {
        assert_eq!(
            app.send(envelope_request(app.project_id, &app.key, body()))
                .await
                .status,
            StatusCode::OK
        );
    }
    let res = app
        .send(envelope_request(app.project_id, &app.key, body()))
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.header("retry-after"), "30");
    assert_eq!(res.header("x-sentry-rate-limits"), "30::key");
    // Then the IP's own bucket (5 a minute) runs out.
    app.exec(format!(
        "UPDATE project_keys SET rate_limit_per_minute = NULL WHERE project_id = {}",
        app.project_id
    ))
    .await;
    assert_eq!(
        app.send(envelope_request(app.project_id, &app.key, body()))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(envelope_request(app.project_id, &app.key, body()))
            .await
            .status,
        StatusCode::OK
    );
    let res = app
        .send(envelope_request(app.project_id, &app.key, body()))
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.header("retry-after"), "12");
    assert_eq!(res.header("x-sentry-rate-limits"), "12::organization");
}

#[tokio::test]
async fn a_full_queue_answers_429() {
    let app = App::with(Settings {
        writer: WriterSettings {
            queue_capacity: 2,
            ..WriterSettings::default()
        },
        ..Settings::default()
    })
    .await;
    let mut body = Vec::new();
    body.extend_from_slice(b"{}\n");
    for _ in 0..3 {
        let e = json!({"event_id": new_id(), "message": "one of three"}).to_string();
        body.extend_from_slice(format!("{{\"type\":\"event\"}}\n{e}\n").as_bytes());
    }
    let res = app
        .send(envelope_request(app.project_id, &app.key, body))
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);
    assert_eq!(res.header("retry-after"), "1");
    assert!(!res.header("x-sentry-rate-limits").is_empty());
}

/// Many clients at once: every accepted event is written, in batches.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_writer_loses_nothing_under_a_burst() {
    let app = std::sync::Arc::new(
        App::with(Settings {
            ip_rate_limit_per_minute: 0,
            ..Settings::default()
        })
        .await,
    );
    const CLIENTS: usize = 16;
    const EACH: usize = 150;
    let mut tasks = Vec::new();
    for client in 0..CLIENTS {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let mut ok = 0;
            for n in 0..EACH {
                // Five kinds of error, so the events spread over five issues.
                let e = json!({"event_id": new_id(), "exception": {"values": [{"type": format!("Burst{}", n % 5), "value": format!("client {client} event {n}")}]}});
                let res = app.send(envelope_request(app.project_id, &app.key, envelope_with(&e))).await;
                if res.status == StatusCode::OK {
                    ok += 1;
                }
            }
            ok
        }));
    }
    let mut accepted = 0;
    for t in tasks {
        accepted += t.await.unwrap();
    }
    app.flush().await;
    assert_eq!(accepted, CLIENTS * EACH, "every request accepted");
    let total = (CLIENTS * EACH) as i64;
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, total);
    assert_eq!(app.count("SELECT SUM(times_seen) FROM issues").await, total);
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 5);
    let stats = app.kestrel.ingest.writer().stats();
    let batches = stats.batches.load(std::sync::atomic::Ordering::Relaxed);
    assert!(batches > 0 && batches < total as u64, "{batches} batches");
    assert_eq!(stats.failed.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cors_follows_the_keys_origins() {
    let app = App::new().await;
    let with_origin = |origin: &str| {
        let mut req = envelope_request(
            app.project_id,
            &app.key,
            envelope_with(&json!({"event_id": new_id(), "message": "x"})),
        );
        req.headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        req
    };
    // No allowed origins: anyone.
    let res = app.send(with_origin("https://anywhere.example")).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("access-control-allow-origin"), "*");
    assert!(
        res.header("access-control-expose-headers")
            .contains("x-sentry-rate-limits")
    );
    // A list: only those, echoed.
    app.exec(format!(
        "UPDATE project_keys SET allowed_origins = 'https://shop.example' WHERE project_id = {}",
        app.project_id
    ))
    .await;
    let res = app.send(with_origin("https://shop.example")).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.header("access-control-allow-origin"),
        "https://shop.example"
    );
    assert_eq!(res.header("vary"), "Origin");
    let res = app.send(with_origin("https://evil.example")).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    // Server SDKs send no Origin.
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            envelope_with(&json!({"message": "x"})),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("access-control-allow-origin"), "");
    // The preflight.
    let req = Request::options(format!("/api/{}/envelope/", app.project_id))
        .header(header::ORIGIN, "https://shop.example")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.header("access-control-allow-methods"), "POST, OPTIONS");
    assert!(
        res.header("access-control-allow-headers")
            .contains("x-sentry-auth")
    );
    // No session cookie is ever set, and the CSRF check doesn't apply.
    assert!(res.headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn resolved_issues_regress() {
    let app = App::new().await;
    let send = |release: &str| {
        let e = json!({"event_id": new_id(), "release": release, "exception": {"values": [{"type": "Regress", "value": "again"}]}});
        app.send(envelope_request(
            app.project_id,
            &app.key,
            envelope_with(&e),
        ))
    };
    assert_eq!(send("app@1").await.status, StatusCode::OK);
    app.flush().await;
    let issue = |app: &App| {
        let core = app.kestrel.core.clone();
        let project = app.project_id;
        async move {
            core.with_conn(move |c| keen_kestrel::db::issues::list(c, project))
                .await
                .unwrap()[0]
                .clone()
        }
    };
    let first = issue(&app).await;
    assert_eq!(
        (
            first.status.as_str(),
            first.substatus.as_str(),
            first.times_seen
        ),
        ("unresolved", "new", 1)
    );

    // Resolved: the next event regresses it.
    let id = first.id;
    app.kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::issues::resolve(c, id, None))
        .await
        .unwrap();
    send("app@1").await;
    app.flush().await;
    let now = issue(&app).await;
    assert_eq!(
        (now.status.as_str(), now.substatus.as_str(), now.times_seen),
        ("unresolved", "regressed", 2)
    );

    // Resolved in the next release: app@1 doesn't regress it, app@2 does.
    app.kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::issues::resolve(c, id, Some("app@1")))
        .await
        .unwrap();
    send("app@1").await;
    app.flush().await;
    assert_eq!(issue(&app).await.status, "resolved");
    send("app@2").await;
    app.flush().await;
    let now = issue(&app).await;
    assert_eq!(
        (now.status.as_str(), now.substatus.as_str(), now.times_seen),
        ("unresolved", "regressed", 4)
    );
}

#[tokio::test]
async fn capture_mode_records_requests_as_sent() {
    let dir = std::env::temp_dir().join(format!("keen-kestrel-capture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app = App::with(Settings {
        capture_dir: Some(dir.clone()),
        ..Settings::default()
    })
    .await;
    let body = gzip(&envelope_with(
        &json!({"event_id": new_id(), "message": "captured"}),
    ));
    let mut req = envelope_request(app.project_id, &app.key, body.clone());
    req.headers_mut()
        .insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::OK);
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    let body_file = files
        .iter()
        .find(|p| p.extension().is_some_and(|x| x == "body"))
        .unwrap();
    assert_eq!(std::fs::read(body_file).unwrap(), body);
    let meta: Value =
        serde_json::from_slice(&std::fs::read(body_file.with_extension("json")).unwrap()).unwrap();
    assert_eq!(meta["uri"], format!("/api/{}/envelope/", app.project_id));
    assert_eq!(meta["headers"]["content-encoding"], "gzip");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_ingest_port_serves_ingest_only() {
    let app = App::new().await;
    let router = keen_kestrel::ingest_router(app.kestrel.clone());
    let mut req = envelope_request(
        app.project_id,
        &app.key,
        envelope_with(&json!({"message": "via ingest port"})),
    );
    assert_eq!(
        common::send_to(&router, &mut req).await.status,
        StatusCode::OK
    );
    let mut health = Request::get("/healthz").body(Body::empty()).unwrap();
    assert_eq!(
        common::send_to(&router, &mut health).await.status,
        StatusCode::OK
    );
    for path in ["/login", "/projects", "/api/v1/projects", "/setup"] {
        let mut req = Request::get(path).body(Body::empty()).unwrap();
        assert_eq!(
            common::send_to(&router, &mut req).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn transactions_and_other_items_are_accepted_and_dropped() {
    let app = App::new().await;
    let mut body = String::from("{}\n");
    for ty in [
        "transaction",
        "span",
        "profile",
        "replay_event",
        "replay_recording",
        "check_in",
        "statsd",
        "log",
        "client_report",
        "something_new",
    ] {
        body.push_str(&format!("{{\"type\":\"{ty}\"}}\n{{}}\n"));
    }
    body.push_str(
        "{\"type\":\"attachment\",\"attachment_type\":\"event.minidump\",\"length\":4}\nMDMP\n",
    );
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            body.into_bytes(),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json(), json!({}));
    app.flush().await;
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, 0);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM attachments").await, 0);
}
