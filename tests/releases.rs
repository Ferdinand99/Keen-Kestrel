//! Release health and releases (docs/design.md §4.9, §6): the SDKs'
//! recorded sessions rolled up per release, environment and day, crash-free
//! sessions and users, retention of the rollups, the releases pages, the
//! links from issues, and sentry-cli's release commands against the stub.

mod common;

use axum::http::StatusCode;
use common::{App, Web, add_user, api_token, bearer, envelope_request, envelope_with, new_id};
use serde_json::{Value, json};
use vole_kit::auth::TokenScope;

fn fixture(sdk: &str, name: &str) -> common::Fixture {
    common::fixtures()
        .into_iter()
        .find(|f| f.sdk == sdk && f.name == name)
        .unwrap_or_else(|| panic!("{sdk}/{name}"))
}

/// An envelope with one `session` item.
fn session_request(app: &App, session: &Value) -> axum::http::Request<axum::body::Body> {
    let body = format!("{{}}\n{{\"type\":\"session\"}}\n{session}\n");
    envelope_request(app.project_id, &app.key, body.into_bytes())
}

async fn rollup(app: &App) -> usize {
    let now = keen_kestrel::issues::now();
    app.kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::releases::health::rollup_all(c, &now))
        .await
        .unwrap()
        .items
}

/// `(sessions, errored, crashed, abnormal, users, crashed_users)` of a release.
async fn health(app: &App, version: &'static str) -> (i64, i64, i64, i64, i64, i64) {
    app.kestrel
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT coalesce(SUM(h.sessions), 0), coalesce(SUM(h.errored), 0), coalesce(SUM(h.crashed), 0),
                        coalesce(SUM(h.abnormal), 0), coalesce(SUM(h.users), 0), coalesce(SUM(h.crashed_users), 0)
                 FROM releases r LEFT JOIN release_health h ON h.release_id = r.id WHERE r.version = ?1",
                [version],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
        })
        .await
        .unwrap()
}

const COCOA: &str = "com.example.kestrelspike@1.0+1";
const COCOA_SID: &str = "F307F1CD-7605-4BA3-B306-5D597207FF30";
const COCOA_DID: &str = "90712615-8B72-4805-9A69-013EFE8FB8E4";

#[tokio::test]
async fn sessions_roll_up_into_crash_free_rates() {
    let app = App::new().await;
    for (sdk, name) in [
        ("sentry-cocoa", "session"),
        ("sentry-javascript-node", "session-start"),
        ("sentry-javascript-node", "session-end"),
        ("sentry-javascript-node", "sessions"),
        ("sentry-python", "sessions"),
    ] {
        let res = app.send_fixture(&fixture(sdk, name)).await;
        assert_eq!(res.status, StatusCode::OK, "{sdk}/{name}: {}", res.body);
    }
    app.flush().await;
    assert_eq!(rollup(&app).await, 5);
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM session_items").await,
        0,
        "rolled-up items are gone"
    );

    // sentry-cocoa: one healthy session of one device.
    assert_eq!(health(&app, COCOA).await, (1, 0, 0, 0, 1, 0));
    // @sentry/node: a session with an error (started and ended: counted
    // once) and an aggregate with one crashed request session; no device ids.
    assert_eq!(health(&app, "fixture-node@1.0.0").await, (2, 2, 1, 0, 0, 0));
    // sentry-python: one crashed session in an aggregate.
    assert_eq!(
        health(&app, "fixture-python@1.0.0").await,
        (1, 1, 1, 0, 0, 0)
    );

    // The cocoa session crashes (its next launch reports it), and a second
    // device has a clean session.
    let crashed = json!({"sid": COCOA_SID, "did": COCOA_DID, "init": false, "status": "crashed", "errors": 1, "seq": 2,
        "started": "2026-10-21T20:02:56.330Z", "attrs": {"release": COCOA, "environment": "production"}});
    let other = json!({"sid": new_id(), "did": "other-device", "init": true, "status": "exited", "errors": 0,
        "attrs": {"release": COCOA, "environment": "production"}});
    // The same crash reported twice counts once.
    for s in [&crashed, &crashed, &other] {
        assert_eq!(
            app.send(session_request(&app, s)).await.status,
            StatusCode::OK
        );
    }
    app.flush().await;
    rollup(&app).await;
    let (sessions, errored, crashed_n, abnormal, users, crashed_users) = health(&app, COCOA).await;
    assert_eq!((sessions, crashed_n, crashed_users), (2, 1, 1));
    assert_eq!((errored, abnormal), (1, 0));
    // The cocoa session's day may differ from the other's (it started
    // yesterday), so the users of both days add up to two.
    assert_eq!(users, 2);
    let h = keen_kestrel::releases::health::Health {
        sessions,
        errored,
        crashed: crashed_n,
        abnormal,
        users,
        crashed_users,
    };
    assert_eq!(h.crash_free_sessions(), Some(50.0));
    assert_eq!(h.crash_free_users(), Some(50.0));
    // Device ids are kept only as the day's hashes.
    assert_eq!(
        app.count(
            "SELECT COUNT(*) FROM release_health_seen WHERE hash LIKE '%90712615%' OR hash = 'other-device'"
        )
        .await,
        0
    );

    // The API and the pages.
    let editor = add_user(&app, "ed", "editor").await;
    let token = api_token(&app, editor, TokenScope::Read).await;
    let p = app.project_id;
    let list = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/releases"),
        &token,
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    let data = list.json()["data"].as_array().unwrap().clone();
    let cocoa = data.iter().find(|r| r["version"] == COCOA).unwrap();
    assert_eq!(cocoa["health"]["crash_free_sessions"], json!(50.0));
    assert_eq!(cocoa["health"]["crash_free_users"], json!(50.0));
    let python = data
        .iter()
        .find(|r| r["version"] == "fixture-python@1.0.0")
        .unwrap();
    assert_eq!(python["health"]["crash_free_sessions"], json!(0.0));
    let node = data
        .iter()
        .find(|r| r["version"] == "fixture-node@1.0.0")
        .unwrap();
    assert_eq!(node["health"]["crash_free_users"], Value::Null);
    let id = cocoa["id"].as_i64().unwrap();
    let h = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/releases/{id}/health?days=7"),
        &token,
        None,
    )
    .await;
    assert_eq!(h.status, StatusCode::OK, "{}", h.body);
    let h = h.json();
    assert_eq!(h["days"].as_array().unwrap().len(), 7);
    assert_eq!(h["totals"]["sessions"], 2);
    assert_eq!(h["environments"], json!(["production"]));

    let mut web = Web::new(&app, "vera", "viewer").await;
    let page = web.get(&format!("/projects/{p}/releases")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(COCOA));
    assert!(page.body.contains("Crash-free sessions"));
    assert!(page.body.contains("50.00%"), "{}", page.body);
    let page = web.get(&format!("/projects/{p}/releases/{id}")).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(page.body.contains("Health"));
    assert!(page.body.contains("Users with a crash"));
    assert!(page.body.contains("chart-line"));
}

#[tokio::test]
async fn release_pages_list_new_issues_and_regressions_and_issues_link_to_them() {
    let app = App::new().await;
    let p = app.project_id;
    let e = |id: String| {
        json!({"event_id": id, "platform": "python", "release": "shop@3.0",
               "exception": {"values": [{"type": "CheckoutError", "value": "card declined"}]}})
    };
    app.send(envelope_request(p, &app.key, envelope_with(&e(new_id()))))
        .await;
    app.flush().await;
    let issue: i64 = app.count("SELECT id FROM issues").await;
    let release: i64 = app
        .count("SELECT id FROM releases WHERE version = 'shop@3.0'")
        .await;
    // Resolved, then back in the same release: a regression of it.
    app.kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::db::triage::resolve(c, p, &[issue], false, "t"))
        .await
        .unwrap();
    app.send(envelope_request(p, &app.key, envelope_with(&e(new_id()))))
        .await;
    app.flush().await;
    assert_eq!(
        app.count("SELECT events FROM releases WHERE version = 'shop@3.0'")
            .await,
        2
    );
    let mut web = Web::new(&app, "vera", "viewer").await;
    let page = web.get(&format!("/projects/{p}/releases/{release}")).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(page.body.contains("New issues"));
    assert_eq!(
        page.body.matches("CheckoutError: card declined").count(),
        2,
        "a new issue and a regression"
    );
    assert!(page.body.contains("No sessions in the last 30 days"));
    // The issue page links its releases; the link finds the release.
    let issue_page = web.get(&format!("/projects/{p}/issues/{issue}")).await;
    let href = format!("/projects/{p}/releases/find?version=shop%403.0");
    assert!(issue_page.body.contains(&href), "{}", issue_page.body);
    let found = web.get(&href).await;
    assert_eq!(found.status, StatusCode::SEE_OTHER);
    assert_eq!(
        found.header("location"),
        format!("/projects/{p}/releases/{release}")
    );
    assert_eq!(
        web.get(&format!("/projects/{p}/releases/find?version=nope"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // The event page too.
    let event_id: String = app
        .kestrel
        .core
        .with_conn(|c| c.query_row("SELECT event_id FROM events LIMIT 1", [], |r| r.get(0)))
        .await
        .unwrap();
    let event_page = web
        .get(&format!("/projects/{p}/issues/{issue}/events/{event_id}"))
        .await;
    assert!(event_page.body.contains(&href));
}

#[tokio::test]
async fn rollups_older_than_400_days_go() {
    let app = App::new().await;
    let p = app.project_id;
    app.exec(format!(
        "INSERT INTO releases (project_id, version, first_seen, last_seen) VALUES ({p}, 'old@1', '2000-01-01 00:00:00', '2000-01-01 00:00:00');
         INSERT INTO release_health (release_id, environment, day, project_id, sessions)
             SELECT id, 'production', '2000-01-01', {p}, 5 FROM releases WHERE version = 'old@1';
         INSERT INTO release_health (release_id, environment, day, project_id, sessions)
             SELECT id, 'production', date('now'), {p}, 5 FROM releases WHERE version = 'old@1';"
    ))
    .await;
    let now = keen_kestrel::issues::now();
    let report = app
        .kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::retention::run_blocking(c, &now))
        .await
        .unwrap();
    assert_eq!(report.rollups_deleted, 1);
    assert_eq!(app.count("SELECT COUNT(*) FROM release_health").await, 1);
}

#[tokio::test]
async fn sentry_cli_release_commands_work_against_the_stub() {
    let app = App::new().await;
    let editor = add_user(&app, "builder", "editor").await;
    let write = api_token(&app, editor, TokenScope::Write).await;
    let read = api_token(&app, editor, TokenScope::Read).await;

    // `sentry-cli info` shows the token's user.
    let info = bearer(&app, "GET", "/api/0/", &write, None).await;
    assert_eq!(info.status, StatusCode::OK);
    assert_eq!(info.json()["user"]["email"], "builder");
    assert_eq!(info.json()["user"]["username"], "builder");

    // releases new
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any/releases/",
        &write,
        Some(json!({"version": "shop@4.0", "projects": ["fixtures"]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let v = res.json();
    assert_eq!(v["version"], "shop@4.0");
    assert_eq!(v["projects"][0]["slug"], "fixtures");
    assert_eq!(v["dateReleased"], Value::Null);
    assert!(v["dateCreated"].as_str().unwrap().ends_with('Z'));
    // set-commits (accepted, not kept) and finalize
    let res = bearer(
        &app,
        "PUT",
        "/api/0/organizations/any/releases/shop@4.0/",
        &write,
        Some(json!({"refs": [{"repository": "acme/shop", "commit": "abc123"}], "projects": ["fixtures"]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let res = bearer(
        &app,
        "PUT",
        "/api/0/organizations/any/releases/shop%404.0/",
        &write,
        Some(json!({"dateReleased": "2026-10-22T08:05:00Z"})),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["dateReleased"], "2026-10-22T08:05:00Z");
    // info and list, with a read token too
    let res = bearer(
        &app,
        "GET",
        "/api/0/organizations/any/releases/shop@4.0/",
        &read,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let res = bearer(
        &app,
        "GET",
        "/api/0/projects/any/fixtures/releases/",
        &read,
        None,
    )
    .await;
    assert_eq!(res.json()[0]["version"], "shop@4.0");
    let res = bearer(&app, "GET", "/api/0/organizations/any/repos/", &read, None).await;
    assert_eq!(res.json(), json!([]));
    // A read-only token can't create; unknown projects and releases are refused.
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any/releases/",
        &read,
        Some(json!({"version": "shop@5.0", "projects": ["fixtures"]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any/releases/",
        &write,
        Some(json!({"version": "shop@5.0", "projects": ["nope"]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    let res = bearer(
        &app,
        "GET",
        "/api/0/organizations/any/releases/none@1/",
        &read,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
    // The release is Kestrel's like any other.
    assert_eq!(
        app.count(
            "SELECT COUNT(*) FROM releases WHERE version = 'shop@4.0' AND date_released IS NOT NULL"
        )
        .await,
        1
    );
}
