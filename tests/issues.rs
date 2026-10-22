//! The issue pages driven in-process, with real ingest behind them: the
//! overview's empty and busy states, the list's filters, search, sorting and
//! paging, the issue and event pages for every recorded SDK fixture, the
//! lifecycle actions and who may take them, merging, ignoring until a count
//! or a time, resolving in the next release, retention and the live list.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use common::{App, CLIENT_IP, envelope_request, envelope_with, new_id};
use futures_util::StreamExt as _;
use keen_kestrel::db::triage;
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// Sign-ins one at a time (see [`Browser::new`]).
static SIGN_IN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
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
}

/// A signed-in browser on an [`App`].
struct Browser<'a> {
    app: &'a App,
    cookies: BTreeMap<String, String>,
}

impl<'a> Browser<'a> {
    /// Adds a user with `role` and signs in as them.
    async fn new(app: &'a App, name: &'static str, role: &'static str) -> Browser<'a> {
        let hash = vole_kit::auth::hash_password(PASSWORD);
        app.kestrel
            .core
            .with_conn(move |c| {
                c.execute(
                    "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
                    rusqlite::params![name, hash, role],
                )
            })
            .await
            .unwrap();
        let mut b = Browser {
            app,
            cookies: BTreeMap::new(),
        };
        // The kit's sign-in limiter is process-wide and keyed by user name, and
        // parallel tests sign in with the same names: one at a time.
        let guard = SIGN_IN.lock().await;
        let res = b
            .post("/login", &[("username", name), ("password", PASSWORD)])
            .await;
        drop(guard);
        assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
        assert!(b.cookies.contains_key("kk_session"));
        b
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
        let peer: SocketAddr = format!("{CLIENT_IP}:40000").parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        let res = self.app.router.clone().oneshot(req).await.unwrap();
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

    /// An htmx request for a fragment (the list's filter changes).
    async fn fragment(&mut self, path: &str) -> Res {
        self.send(
            Request::get(path)
                .header("hx-request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
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

    /// Follows one redirect.
    async fn follow(&mut self, res: &Res) -> Res {
        assert!(res.status.is_redirection(), "{}: {}", res.status, res.body);
        let to = res.location().to_string();
        self.get(&to).await
    }
}

/// Kestrel's name and icons, never Vigilant Vole's words.
fn assert_branded(path: &str, html: &str) {
    assert!(
        html.contains("Keen Kestrel"),
        "{path}: no product name\n{html}"
    );
    assert!(html.contains("/static/favicon.svg"), "{path}: no icon");
    for foreign in [
        "Vigilant", "vigilant", "Vole", "vv_", "VV_", "monitor", "Monitor", "Kit Demo",
    ] {
        assert!(!html.contains(foreign), "{path}: contains {foreign:?}");
    }
}

/// Sends one event and waits until it is written.
async fn send(app: &App, mut event: Value) {
    if event.get("event_id").is_none() {
        event["event_id"] = Value::from(new_id());
    }
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            envelope_with(&event),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    app.flush().await;
}

fn exception(ty: &str, value: &str) -> Value {
    json!({"platform": "python", "exception": {"values": [{"type": ty, "value": value}]}})
}

fn with(mut base: Value, extra: Value) -> Value {
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

/// The issue ids of the project, by title.
async fn issue_ids(app: &App) -> BTreeMap<String, i64> {
    let project = app.project_id;
    app.kestrel
        .core
        .with_conn(move |c| {
            c.prepare("SELECT title, id FROM issues WHERE project_id = ?1")?
                .query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .await
        .unwrap()
}

async fn issue(app: &App, id: i64) -> triage::IssueRow {
    let project = app.project_id;
    app.kestrel
        .core
        .with_conn(move |c| triage::get(c, project, id))
        .await
        .unwrap()
        .unwrap()
}

async fn activity(app: &App, id: i64) -> Vec<String> {
    app.kestrel
        .core
        .with_conn(move |c| triage::activity(c, id))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.kind)
        .collect()
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_overview_is_empty_until_the_first_event() {
    let app = App::new().await;
    let mut b = Browser::new(&app, "ada", "admin").await;
    let path = format!("/projects/{}", app.project_id);
    let empty = b.get(&path).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(empty.body.contains("No events yet"), "{}", empty.body);
    assert!(empty.body.contains("Send your first event"));
    assert!(!empty.body.contains("Unresolved issues"));
    assert!(empty.body.contains("sentry_sdk.init"));
    assert_branded(&path, &empty.body);

    send(&app, exception("KeyError", "missing id")).await;
    send(&app, exception("KeyError", "missing id")).await;
    let busy = b.get(&path).await;
    assert!(!busy.body.contains("No events yet"), "{}", busy.body);
    assert!(busy.body.contains("Unresolved issues"), "{}", busy.body);
    assert!(busy.body.contains("Latest issues"));
    assert!(busy.body.contains("KeyError: missing id"));
    assert!(busy.body.contains(r#"class="chart-line""#));
    // The setup moves into a closed disclosure.
    assert!(busy.body.contains("DSN and setup"));
    assert!(busy.body.contains(r#"<details class="disclosure card">"#));
    assert!(busy.body.contains("sentry_sdk.init"));
    assert_branded(&path, &busy.body);
    let o = app
        .kestrel
        .core
        .with_conn(|c| {
            let now: String = c.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
            triage::overview(c, 1, &now)
        })
        .await
        .unwrap();
    assert_eq!((o.unresolved, o.events_24h, o.events_14d), (1, 2, 2));
    assert_eq!(o.daily.len(), 14);
    assert_eq!(*o.daily.last().unwrap(), 2);
}

#[tokio::test]
async fn every_fixture_has_issue_and_event_pages() {
    let app = App::new().await;
    for f in common::fixtures() {
        app.send_fixture(&f).await;
    }
    app.flush().await;
    let mut b = Browser::new(&app, "ada", "admin").await;
    let p = app.project_id;
    let list = b.get(&format!("/projects/{p}/issues")).await;
    assert_eq!(list.status, StatusCode::OK);
    assert_branded("issues", &list.body);
    assert!(list.body.contains("sse-connect=\"/projects/1/issues/live"));
    let found = b.get(&format!("/projects/{p}/issues?q=RuntimeError")).await;
    assert!(found.body.contains(">RuntimeError: "), "{}", found.body);
    let ids = issue_ids(&app).await;
    assert!(ids.len() >= 15, "{ids:?}");
    let mut stacks = 0;
    for (title, id) in &ids {
        let base = format!("/projects/{p}/issues/{id}");
        for tab in [
            "",
            "?tab=events",
            "?tab=tags",
            "?tab=grouping",
            "?frames=all",
        ] {
            let page = b.get(&format!("{base}{tab}")).await;
            assert_eq!(page.status, StatusCode::OK, "{base}{tab}: {}", page.body);
            assert_branded(&format!("{base}{tab}"), &page.body);
            assert!(!page.body.contains(CLIENT_IP), "{base}{tab}: client IP");
            assert!(
                !page.body.contains("ada.lovelace@example.com"),
                "{base}{tab}: raw email"
            );
        }
        let details = b.get(&base).await;
        assert!(details.body.contains("Latest event"), "{title}");
        assert!(details.body.contains("Raw JSON"), "{title}");
        assert!(details.body.contains("First seen"), "{title}");
        if details.body.contains("Stack trace") {
            stacks += 1;
        }
        let grouping = b.get(&format!("{base}?tab=grouping")).await;
        assert!(grouping.body.contains("kestrel:v1"), "{title}");
        assert!(grouping.body.contains("Why these events are grouped"));
        // The latest event's own page, with paging between events.
        let event_id: String = app
            .kestrel
            .core
            .with_conn({
                let id = *id;
                move |c| {
                    c.query_row(
                        "SELECT event_id FROM events WHERE issue_id = ?1 ORDER BY timestamp DESC LIMIT 1",
                        [id],
                        |r| r.get(0),
                    )
                }
            })
            .await
            .unwrap();
        let event = b.get(&format!("{base}/events/{event_id}")).await;
        assert_eq!(event.status, StatusCode::OK, "{}", event.body);
        assert!(event.body.contains(&event_id), "{title}");
        assert!(event.body.contains("Newer") && event.body.contains("Older"));
        assert_branded("event", &event.body);
    }
    assert!(stacks >= 8, "{stacks} stack traces");

    // Chained exceptions read from the main one to its cause.
    let chained = ids
        .iter()
        .find(|(t, _)| t.starts_with("RuntimeError: "))
        .map(|(_, id)| *id)
        .unwrap();
    let page = b.get(&format!("/projects/{p}/issues/{chained}")).await;
    assert!(page.body.contains("Caused by"), "{}", page.body);
    let first = page
        .body
        .split(r#"<h3 class="card-title break">"#)
        .nth(1)
        .unwrap();
    assert!(first.starts_with("RuntimeError<"), "{first}");
    // A crash from sentry-cocoa: unsymbolicated, frames outside the app fold.
    let crash = ids
        .iter()
        .find(|(t, _)| t.starts_with("EXC_BREAKPOINT"))
        .map(|(_, id)| *id)
        .unwrap();
    let page = b.get(&format!("/projects/{p}/issues/{crash}")).await;
    assert!(page.body.contains("Not symbolicated"));
    assert!(
        page.body.contains("frames outside the app"),
        "{}",
        page.body
    );
    assert!(page.body.contains("Breadcrumbs"));
    assert!(page.body.contains("Operating system"));
    let all = b
        .get(&format!("/projects/{p}/issues/{crash}?frames=all"))
        .await;
    assert!(!all.body.contains("frames outside the app"));
    // Unknown ids are 404s.
    for path in [
        format!("/projects/{p}/issues/999999"),
        format!("/projects/{p}/issues/{crash}/events/0123456789abcdef0123456789abcdef"),
        format!("/projects/{p}/issues/{crash}/events/not-an-id"),
        "/projects/999/issues".to_string(),
    ] {
        assert_eq!(b.get(&path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
}

/// The list's results for `qs` (a fragment) and which of the three test
/// issues it shows.
async fn check(b: &mut Browser<'_>, qs: &str, want: &[&str]) -> Res {
    let res = b.fragment(&format!("/projects/1/issues?{qs}")).await;
    assert_eq!(res.status, StatusCode::OK, "{qs}");
    assert!(!res.body.contains("<html"), "{qs}: a fragment");
    let shown: Vec<&str> = ["CheckoutError", "SyncError", "Disk almost full"]
        .into_iter()
        .filter(|t| res.body.contains(&format!(">{t}")))
        .collect();
    assert_eq!(shown, want, "{qs}");
    res
}

#[tokio::test]
async fn the_list_filters_searches_sorts_and_pages() {
    let app = App::new().await;
    let ios = json!({"contexts": {"os": {"name": "iOS", "version": "26.2"}}});
    send(
        &app,
        with(
            exception("CheckoutError", "card declined"),
            json!({"release": "shop@1.4.0+12", "environment": "production", "contexts": ios["contexts"], "user": {"id": "u1"}}),
        ),
    )
    .await;
    for user in ["u1", "u2", "u3"] {
        send(
            &app,
            with(
                exception("SyncError", "server gone"),
                json!({"release": "shop@1.5.0", "environment": "staging", "tags": {"shop": "north"}, "user": {"id": user}}),
            ),
        )
        .await;
    }
    send(
        &app,
        json!({"message": "Disk almost full", "level": "warning", "release": "shop@1.4.0+12"}),
    )
    .await;
    let ids = issue_ids(&app).await;
    let checkout = ids["CheckoutError: card declined"];
    let mut b = Browser::new(&app, "ada", "admin").await;
    let p = app.project_id;
    check(
        &mut b,
        "",
        &["CheckoutError", "SyncError", "Disk almost full"],
    )
    .await;
    check(&mut b, "q=level:warning", &["Disk almost full"]).await;
    check(&mut b, "level=error", &["CheckoutError", "SyncError"]).await;
    check(
        &mut b,
        "q=release:1.4.0",
        &["CheckoutError", "Disk almost full"],
    )
    .await;
    check(&mut b, "release=shop%401.5.0", &["SyncError"]).await;
    check(&mut b, "q=env:staging", &["SyncError"]).await;
    // Events without an environment are "production".
    check(
        &mut b,
        "env=production",
        &["CheckoutError", "Disk almost full"],
    )
    .await;
    check(
        &mut b,
        "q=%21env:staging",
        &["CheckoutError", "Disk almost full"],
    )
    .await;
    check(&mut b, "q=os:iOS", &["CheckoutError"]).await;
    check(&mut b, "q=shop:north", &["SyncError"]).await;
    check(&mut b, "q=declin", &["CheckoutError"]).await;
    check(&mut b, "q=%22server+gone%22", &["SyncError"]).await;
    check(
        &mut b,
        "q=is:unresolved+level:error+env:staging+serv",
        &["SyncError"],
    )
    .await;
    let res = check(&mut b, "q=is:sleepy+disk", &["Disk almost full"]).await;
    assert!(res.body.contains("Left out of the search: is:sleepy"));
    let none = check(&mut b, "q=nothing-like-this", &[]).await;
    assert!(none.body.contains("No issues match"));
    // The chips come along out of band and follow an is: in the search.
    let res = check(&mut b, "q=is:resolved", &[]).await;
    assert!(res.body.contains(r#"id="issue-chips" hx-swap-oob="true""#));
    assert!(res.body.contains(r#"value="resolved" checked"#));
    assert!(
        res.headers
            .get("hx-push-url")
            .is_some_and(|v| v.to_str().unwrap().contains("q=is%3Aresolved"))
    );
    // A chip picked while the search names a status: the chip wins.
    let res = b
        .fragment(&format!(
            "/projects/{p}/issues?q=is:resolved+disk&status=unresolved&shown_status=resolved"
        ))
        .await;
    assert_eq!(
        res.headers.get("hx-redirect").unwrap(),
        &format!("/projects/{p}/issues?status=unresolved&q=disk")
    );
    // Event ids find their issue.
    let event_id: String = app
        .kestrel
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT event_id FROM events WHERE issue_id = ?1",
                [checkout],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    check(&mut b, &format!("q={event_id}"), &["CheckoutError"]).await;

    // Sorting: events and users put SyncError first; first seen the newest.
    for sort in ["events", "users"] {
        let res = b
            .fragment(&format!("/projects/{p}/issues?sort={sort}"))
            .await;
        let sync = res.body.find(">SyncError").unwrap();
        let check_ = res.body.find(">CheckoutError").unwrap();
        assert!(sync < check_, "{sort}");
    }
    assert_eq!(
        issue(&app, ids["SyncError: server gone"]).await.users_seen,
        3
    );
    let res = b
        .fragment(&format!("/projects/{p}/issues?sort=first_seen"))
        .await;
    assert!(res.body.find(">Disk almost full").unwrap() < res.body.find(">CheckoutError").unwrap());

    // Paging: 30 more issues make two pages of 25.
    for i in 0..30u8 {
        let word = format!("{}{}", char::from(b'a' + i / 26), char::from(b'a' + i % 26));
        send(&app, json!({"message": format!("Paging {word}")})).await;
    }
    let first = b.fragment(&format!("/projects/{p}/issues")).await;
    assert!(first.body.contains("33 issues"), "{}", first.body);
    assert!(first.body.contains("1–25 of 33"));
    assert!(first.body.contains("rel=\"next\""));
    let second = b.fragment(&format!("/projects/{p}/issues?page=2")).await;
    assert!(second.body.contains("26–33 of 33"));
    assert!(second.body.contains("rel=\"prev\""));
    // A page past the end shows the last one.
    let past = b.fragment(&format!("/projects/{p}/issues?page=9")).await;
    assert!(past.body.contains("26–33 of 33"));

    // The full page: filters, sort and select mode for editors.
    let page = b.get(&format!("/projects/{p}/issues")).await;
    assert!(page.body.contains("shop@1.4.0+12"), "release filter");
    assert!(page.body.contains("Select"));
    let select = b.get(&format!("/projects/{p}/issues?select=1")).await;
    assert!(select.body.contains(r#"form="bulk-form""#));
    assert!(!select.body.contains("sse-connect"));
}

#[tokio::test]
async fn lifecycle_actions_and_who_may_take_them() {
    let app = App::new().await;
    send(
        &app,
        with(
            exception("TimeoutError", "slow"),
            json!({"release": "app@1"}),
        ),
    )
    .await;
    let id = issue_ids(&app).await["TimeoutError: slow"];
    let p = app.project_id;
    let action = format!("/projects/{p}/issues/{id}/action");

    // Viewers read but can't act.
    let mut viewer = Browser::new(&app, "vera", "viewer").await;
    assert_eq!(
        viewer
            .get(&format!("/projects/{p}/issues/{id}"))
            .await
            .status,
        StatusCode::OK
    );
    let page = viewer.get(&format!("/projects/{p}/issues/{id}")).await;
    assert!(!page.body.contains("value=\"resolve\""));
    for a in ["resolve", "ignore", "delete", "unresolve"] {
        let res = viewer.post(&action, &[("action", a)]).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{a}");
    }
    let res = viewer
        .post(
            &format!("/projects/{p}/issues/bulk"),
            &[("action", "resolve"), ("ids", &id.to_string())],
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(issue(&app, id).await.status, "unresolved");

    // Editors act.
    let mut editor = Browser::new(&app, "ed", "editor").await;
    let page = editor.get(&format!("/projects/{p}/issues/{id}")).await;
    assert!(page.body.contains("value=\"resolve\""));
    assert!(page.body.contains("value=\"resolve_next_release\""));
    let res = editor.post(&action, &[("action", "resolve")]).await;
    assert_eq!(res.location(), format!("/projects/{p}/issues/{id}"));
    let page = editor.follow(&res).await;
    assert!(page.body.contains("Issue resolved."));
    assert!(page.body.contains("Resolved by ed"), "{}", page.body);
    assert_eq!(issue(&app, id).await.status, "resolved");

    let res = editor.post(&action, &[("action", "unresolve")]).await;
    assert!(
        editor
            .follow(&res)
            .await
            .body
            .contains("Issue marked unresolved.")
    );
    let row = issue(&app, id).await;
    assert_eq!(
        (row.status.as_str(), row.substatus.as_str()),
        ("unresolved", "ongoing")
    );

    let res = editor.post(&action, &[("action", "ignore")]).await;
    let page = editor.follow(&res).await;
    assert!(page.body.contains("Issue ignored."));
    assert!(page.body.contains("Ignored by ed"));
    assert_eq!(issue(&app, id).await.status, "ignored");
    // An ignored issue still counts new events but stays ignored.
    send(
        &app,
        with(
            exception("TimeoutError", "slow"),
            json!({"release": "app@1"}),
        ),
    )
    .await;
    let row = issue(&app, id).await;
    assert_eq!((row.status.as_str(), row.times_seen), ("ignored", 2));

    let bad = editor.post(&action, &[("action", "explode")]).await;
    assert!(
        editor
            .follow(&bad)
            .await
            .body
            .contains("That action isn’t available.")
    );
    assert_eq!(
        activity(&app, id).await,
        ["ignored", "unresolved", "resolved"]
    );

    // Bulk: resolve two, from select mode, back to the list.
    send(&app, exception("OtherError", "two")).await;
    let other = issue_ids(&app).await["OtherError: two"];
    let res = editor
        .post(
            &format!("/projects/{p}/issues/bulk"),
            &[
                ("action", "resolve"),
                ("ids", &id.to_string()),
                ("ids", &other.to_string()),
                (
                    "return_to",
                    &format!("/projects/{p}/issues?status=all&select=1"),
                ),
            ],
        )
        .await;
    assert_eq!(
        res.location(),
        format!("/projects/{p}/issues?status=all&select=1")
    );
    assert!(
        editor
            .follow(&res)
            .await
            .body
            .contains("2 issues resolved.")
    );
    assert_eq!(issue(&app, other).await.status, "resolved");
    // return_to stays on the issue list.
    let res = editor
        .post(
            &format!("/projects/{p}/issues/bulk"),
            &[
                ("action", "unresolve"),
                ("ids", &other.to_string()),
                ("return_to", "https://evil.example/"),
            ],
        )
        .await;
    assert_eq!(res.location(), format!("/projects/{p}/issues"));
    let res = editor
        .post(
            &format!("/projects/{p}/issues/bulk"),
            &[("action", "resolve")],
        )
        .await;
    assert!(
        editor
            .follow(&res)
            .await
            .body
            .contains("Tick at least one issue.")
    );

    // Delete: the issue, its events and their payloads go.
    let payloads_before = app.store_count("SELECT COUNT(*) FROM payloads").await;
    let res = editor.post(&action, &[("action", "delete")]).await;
    assert_eq!(res.location(), format!("/projects/{p}/issues"));
    assert!(editor.follow(&res).await.body.contains("Issue deleted."));
    let project = app.project_id;
    let left = app
        .kestrel
        .core
        .with_conn(move |c| triage::get(c, project, id))
        .await
        .unwrap();
    assert!(left.is_none());
    assert_eq!(
        app.count("SELECT COUNT(*) FROM events WHERE issue_id NOT IN (SELECT id FROM issues)")
            .await,
        0
    );
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM payloads").await,
        payloads_before - 2
    );
    assert_eq!(app.count("SELECT COUNT(*) FROM issue_hashes").await, 1);
    assert_eq!(
        editor
            .get(&format!("/projects/{p}/issues/{id}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // The same error again makes a new issue.
    send(
        &app,
        with(
            exception("TimeoutError", "slow"),
            json!({"release": "app@1"}),
        ),
    )
    .await;
    assert_ne!(issue_ids(&app).await["TimeoutError: slow"], id);
}

#[tokio::test]
async fn merging_moves_events_hashes_counts_and_users() {
    let app = App::new().await;
    let hour_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        - 3600.0;
    for user in ["a", "b"] {
        send(
            &app,
            with(
                exception("ParseError", "bad header"),
                json!({"release": "app@1", "user": {"id": user}, "tags": {"shop": "north"}, "timestamp": hour_ago}),
            ),
        )
        .await;
    }
    for user in ["b", "c", "d"] {
        send(
            &app,
            with(
                exception("FormatError", "bad body"),
                json!({"release": "app@2", "user": {"id": user}, "tags": {"shop": "north"}}),
            ),
        )
        .await;
    }
    send(&app, exception("Unrelated", "x")).await;
    let ids = issue_ids(&app).await;
    let (parse, format, unrelated) = (
        ids["ParseError: bad header"],
        ids["FormatError: bad body"],
        ids["Unrelated: x"],
    );
    let p = app.project_id;
    let mut b = Browser::new(&app, "ed", "editor").await;
    // One issue is not enough.
    let res = b
        .post(
            &format!("/projects/{p}/issues/bulk"),
            &[("action", "merge"), ("ids", &parse.to_string())],
        )
        .await;
    assert!(
        b.follow(&res)
            .await
            .body
            .contains("Tick at least two issues to merge.")
    );

    let res = b
        .post(
            &format!("/projects/{p}/issues/bulk"),
            &[
                ("action", "merge"),
                ("ids", &parse.to_string()),
                ("ids", &format.to_string()),
            ],
        )
        .await;
    // FormatError has more events, so it stays.
    assert_eq!(res.location(), format!("/projects/{p}/issues/{format}"));
    let page = b.follow(&res).await;
    assert!(page.body.contains("2 issues merged."));
    assert!(
        page.body.contains("ed merged 1 issue into this one"),
        "{}",
        page.body
    );
    let ids = issue_ids(&app).await;
    assert_eq!(ids.len(), 2);
    assert!(!ids.contains_key("ParseError: bad header"));
    let merged = issue(&app, format).await;
    assert_eq!(merged.times_seen, 5);
    assert_eq!(merged.users_seen, 4, "a, b, c and d");
    assert_eq!(merged.first_release.as_deref(), Some("app@1"));
    assert_eq!(merged.last_release.as_deref(), Some("app@2"));
    let count = |sql: String| {
        let core = app.kestrel.core.clone();
        async move {
            core.with_conn(move |c| c.query_row(&sql, [], |r| r.get::<_, i64>(0)))
                .await
                .unwrap()
        }
    };
    assert_eq!(
        count(format!(
            "SELECT COUNT(*) FROM events WHERE issue_id = {format}"
        ))
        .await,
        5
    );
    assert_eq!(
        count(format!(
            "SELECT COUNT(*) FROM issue_hashes WHERE issue_id = {format}"
        ))
        .await,
        2
    );
    assert_eq!(
        count(format!(
            "SELECT times_seen FROM issue_tag_values WHERE issue_id = {format} AND key = 'shop'"
        ))
        .await,
        5
    );
    assert_eq!(
        count(format!(
            "SELECT SUM(events) FROM issue_hours WHERE issue_id = {format}"
        ))
        .await,
        5
    );
    assert_eq!(
        count(format!(
            "SELECT COUNT(*) FROM events WHERE issue_id = {unrelated}"
        ))
        .await,
        1
    );
    // The grouping tab shows both hashes; old ParseErrors land here now.
    let grouping = b
        .get(&format!("/projects/{p}/issues/{format}?tab=grouping"))
        .await;
    assert!(grouping.body.contains("other issues were merged into it"));
    send(&app, exception("ParseError", "bad header")).await;
    assert_eq!(issue_ids(&app).await.len(), 2);
    assert_eq!(issue(&app, format).await.times_seen, 6);
    assert_eq!(
        b.get(&format!("/projects/{p}/issues/{parse}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn ignoring_until_a_count_or_a_time() {
    let app = App::new().await;
    send(&app, exception("NoisyError", "again")).await;
    let id = issue_ids(&app).await["NoisyError: again"];
    let p = app.project_id;
    let mut b = Browser::new(&app, "ed", "editor").await;
    let action = format!("/projects/{p}/issues/{id}/action");

    // Until 10 more events: 9 keep it ignored, the 10th brings it back.
    let res = b.post(&action, &[("action", "ignore_events_10")]).await;
    let page = b.follow(&res).await;
    assert!(
        page.body.contains("Ignored until 10 more events arrive."),
        "{}",
        page.body
    );
    assert!(page.body.contains("Ignored by ed until 10 more events"));
    for _ in 0..9 {
        send(&app, exception("NoisyError", "again")).await;
    }
    assert_eq!(issue(&app, id).await.status, "ignored");
    send(&app, exception("NoisyError", "again")).await;
    let row = issue(&app, id).await;
    assert_eq!(
        (row.status.as_str(), row.substatus.as_str(), row.times_seen),
        ("unresolved", "ongoing", 11)
    );
    assert_eq!(activity(&app, id).await[0], "unignored");

    // For an hour: an event within the hour keeps it ignored.
    let res = b.post(&action, &[("action", "ignore_hours_1")]).await;
    assert!(b.follow(&res).await.body.contains("Ignored until "));
    let until = issue(&app, id).await.ignore_until.unwrap();
    assert!(until > keen_kestrel::issues::now());
    send(&app, exception("NoisyError", "again")).await;
    assert_eq!(issue(&app, id).await.status, "ignored");
    // Once the time has passed, the next event brings it back …
    app.exec(format!(
        "UPDATE issues SET ignore_until = '2020-01-01 00:00:00' WHERE id = {id}"
    ))
    .await;
    send(&app, exception("NoisyError", "again")).await;
    assert_eq!(issue(&app, id).await.status, "unresolved");
    // … and so does the sweep, without any event.
    b.post(&action, &[("action", "ignore_hours_24")]).await;
    app.exec(format!(
        "UPDATE issues SET ignore_until = '2020-01-01 00:00:00' WHERE id = {id}"
    ))
    .await;
    let n = app
        .kestrel
        .ingest
        .writer()
        .run(|c| triage::expire_ignored(c, "2026-01-01 00:00:00"))
        .await
        .unwrap();
    assert_eq!(n, 1);
    let row = issue(&app, id).await;
    assert_eq!(
        (row.status.as_str(), row.ignore_until),
        ("unresolved", None)
    );
    assert_eq!(activity(&app, id).await[0], "unignored");
    // Bad conditions are refused.
    let res = b.post(&action, &[("action", "ignore_hours_0")]).await;
    assert!(
        b.follow(&res)
            .await
            .body
            .contains("That action isn’t available.")
    );
}

#[tokio::test]
async fn resolving_in_the_next_release_and_regressions() {
    let app = App::new().await;
    let crash = |release: &str| with(exception("CrashError", "boom"), json!({"release": release}));
    send(&app, crash("app@1.0")).await;
    send(&app, crash("app@1.1")).await;
    let id = issue_ids(&app).await["CrashError: boom"];
    let p = app.project_id;
    let mut b = Browser::new(&app, "ed", "editor").await;
    let action = format!("/projects/{p}/issues/{id}/action");

    // The newest release is app@1.1: it and older ones don't bring it back.
    let res = b.post(&action, &[("action", "resolve_next_release")]).await;
    let page = b.follow(&res).await;
    assert!(page.body.contains("Issue resolved in the next release."));
    assert!(
        page.body.contains("Resolved in the release after app@1.1."),
        "{}",
        page.body
    );
    assert_eq!(
        issue(&app, id).await.resolved_in_release.as_deref(),
        Some("app@1.1")
    );
    send(&app, crash("app@1.1")).await;
    send(&app, crash("app@1.0")).await;
    send(&app, json!({"exception": {"values": [{"type": "CrashError", "value": "boom"}]}, "platform": "python"})).await;
    assert_eq!(issue(&app, id).await.status, "resolved");
    // A newer release does.
    send(&app, crash("app@1.2")).await;
    let row = issue(&app, id).await;
    assert_eq!(
        (row.status.as_str(), row.substatus.as_str()),
        ("unresolved", "regressed")
    );
    let page = b.get(&format!("/projects/{p}/issues/{id}")).await;
    assert!(page.body.contains("Regressed in app@1.2"), "{}", page.body);
    assert!(page.body.contains(">Regressed<"), "the badge");
    assert_eq!(
        activity(&app, id).await,
        ["regressed", "resolved_in_release"]
    );
    // Resolved now: any event regresses it.
    b.post(&action, &[("action", "resolve")]).await;
    send(&app, crash("app@1.0")).await;
    assert_eq!(issue(&app, id).await.substatus, "regressed");
    assert_eq!(activity(&app, id).await[0], "regressed");

    // Without any release, "next release" resolves now.
    let other = App::new().await;
    send(&other, exception("Plain", "no release")).await;
    let pid = issue_ids(&other).await["Plain: no release"];
    let mut b2 = Browser::new(&other, "ed", "editor").await;
    let page = b2.get(&format!("/projects/1/issues/{pid}")).await;
    assert!(!page.body.contains("resolve_next_release"));
    b2.post(
        &format!("/projects/1/issues/{pid}/action"),
        &[("action", "resolve_next_release")],
    )
    .await;
    let row = issue(&other, pid).await;
    assert_eq!(
        (row.status.as_str(), row.resolved_in_release),
        ("resolved", None)
    );
}

#[tokio::test]
async fn retention_removes_old_details_and_old_issues() {
    let app = App::new().await;
    for kind in ["Old", "Recent"] {
        send(&app, exception(kind, "x")).await;
        send(&app, exception(kind, "x")).await;
    }
    let ids = issue_ids(&app).await;
    let (old, recent) = (ids["Old: x"], ids["Recent: x"]);
    // Make the Old issue 100 days old and one Recent event 40 days old.
    app.exec(format!(
        "UPDATE issues SET first_seen = '2000-01-01 00:00:00', last_seen = datetime('now', '-100 days') WHERE id = {old};
         UPDATE events SET received_at = datetime('now', '-100 days') WHERE issue_id = {old};
         UPDATE events SET received_at = datetime('now', '-40 days')
             WHERE id = (SELECT min(id) FROM events WHERE issue_id = {recent});
         INSERT INTO issue_hours (issue_id, hour, project_id, events)
             VALUES ({recent}, strftime('%Y-%m-%d %H:00:00', 'now', '-40 days'), 1, 3);"
    ))
    .await;
    let store = app.kestrel.ingest.store().get().await.unwrap();
    let main_db = app.dir.join("kestrel.sqlite");
    store
        .interact(move |c| {
            c.execute("ATTACH DATABASE ?1 AS main_db", [main_db.to_string_lossy()])?;
            c.execute_batch(
                "UPDATE payloads SET received_at = (SELECT received_at FROM main_db.events e
                     WHERE e.event_id = payloads.event_id);
                 INSERT INTO payloads (project_id, event_id, received_at, data)
                     VALUES (77, 'orphan', '2026-01-01 00:00:00', x'00');
                 DETACH DATABASE main_db;",
            )
        })
        .await
        .unwrap()
        .unwrap();
    drop(store);
    assert_eq!(app.store_count("SELECT COUNT(*) FROM payloads").await, 5);

    let now = keen_kestrel::issues::now();
    let report = app
        .kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::retention::run_blocking(c, &now))
        .await
        .unwrap();
    // Old: deleted with its 2 events (and their payloads). Recent: the
    // 40-day-old event lost its payload but kept its row. The orphan went.
    assert_eq!(report.issues_deleted, 1);
    assert_eq!(
        report.details_deleted, 4,
        "3 payloads and the orphan: {report:?}"
    );
    assert_eq!(report.hours_deleted, 1);
    let ids = issue_ids(&app).await;
    assert_eq!(ids.keys().collect::<Vec<_>>(), ["Recent: x"]);
    assert_eq!(app.count("SELECT COUNT(*) FROM events").await, 2);
    assert_eq!(
        app.count("SELECT COUNT(*) FROM events WHERE payload_id IS NULL")
            .await,
        1
    );
    assert_eq!(app.store_count("SELECT COUNT(*) FROM payloads").await, 1);
    assert_eq!(app.count("SELECT times_seen FROM issues").await, 2);
    // The event without details still has its row and page.
    let mut b = Browser::new(&app, "ada", "admin").await;
    let p = app.project_id;
    let events = b
        .get(&format!("/projects/{p}/issues/{recent}?tab=events"))
        .await;
    assert!(events.body.contains("No details"), "{}", events.body);
    let gone: String = app
        .kestrel
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT event_id FROM events WHERE issue_id = ?1 AND payload_id IS NULL",
                [recent],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    let page = b
        .get(&format!("/projects/{p}/issues/{recent}/events/{gone}"))
        .await;
    assert!(page.body.contains("Details no longer kept"));

    // Retention settings: shown, validated and saved per project.
    let settings = b.get(&format!("/projects/{p}/settings")).await;
    assert!(settings.body.contains("Retention"));
    assert!(settings.body.contains(r#"name="event_days""#));
    assert!(settings.body.contains(r#"value="30""#) && settings.body.contains(r#"value="90""#));
    let bad = b
        .post(
            &format!("/projects/{p}/retention"),
            &[
                ("event_days", "0"),
                ("issue_days", "90"),
                ("per_day", "1000"),
            ],
        )
        .await;
    assert!(
        b.follow(&bad)
            .await
            .body
            .contains("Enter a whole number of days")
    );
    let res = b
        .post(
            &format!("/projects/{p}/retention"),
            &[
                ("event_days", "7"),
                ("issue_days", "1,000"),
                ("per_day", "2"),
            ],
        )
        .await;
    assert_eq!(res.location(), format!("/projects/{p}/settings#retention"));
    assert!(b.follow(&res).await.body.contains("Retention saved."));
    let r = app
        .kestrel
        .core
        .with_conn(move |c| triage::retention(c, p))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (r.event_days, r.issue_days, r.payloads_per_day),
        (7, 1000, 2)
    );
    // The daily cap applies at once: 2 stored today, the rest counted.
    for _ in 0..3 {
        send(&app, exception("Capped", "x")).await;
    }
    let capped = issue_ids(&app).await["Capped: x"];
    assert_eq!(issue(&app, capped).await.times_seen, 3);
    assert_eq!(
        app.count("SELECT COUNT(*) FROM events WHERE issue_id = (SELECT id FROM issues WHERE title = 'Capped: x')")
            .await,
        2
    );
    // Viewers see the settings but can't change them.
    let mut viewer = Browser::new(&app, "vera", "viewer").await;
    let page = viewer.get(&format!("/projects/{p}/settings")).await;
    assert!(page.body.contains("Retention") && !page.body.contains(r#"name="event_days""#));
    let res = viewer
        .post(
            &format!("/projects/{p}/retention"),
            &[("event_days", "1"), ("issue_days", "1"), ("per_day", "1")],
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deleting_a_project_empties_the_store_too() {
    let app = App::new().await;
    send(&app, exception("Gone", "soon")).await;
    assert_eq!(app.store_count("SELECT COUNT(*) FROM payloads").await, 1);
    let mut b = Browser::new(&app, "ed", "editor").await;
    let res = b
        .post(&format!("/projects/{}/delete", app.project_id), &[])
        .await;
    assert_eq!(res.location(), "/projects");
    assert_eq!(app.store_count("SELECT COUNT(*) FROM payloads").await, 0);
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 0);
}

#[tokio::test]
async fn the_live_list_streams_updates() {
    let app = App::new().await;
    send(&app, exception("FirstError", "one")).await;
    let mut b = Browser::new(&app, "ada", "admin").await;
    let mut req = Request::get(format!("/projects/{}/issues/live", app.project_id))
        .header(header::HOST, HOST)
        .header(
            header::COOKIE,
            format!("kk_session={}", b.cookies["kk_session"]),
        )
        .body(Body::empty())
        .unwrap();
    let peer: SocketAddr = format!("{CLIENT_IP}:40000").parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));
    let res = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut body = res.into_body().into_data_stream();
    send(&app, exception("SecondError", "two")).await;
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !seen.contains("SecondError") {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .expect("an update within 10 s")
            .unwrap()
            .unwrap();
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.contains("event: issues-updated"), "{seen}");
    assert!(seen.contains("FirstError"));
    let _ = b.get("/projects").await;
}
