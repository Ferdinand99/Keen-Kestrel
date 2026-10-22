//! Web analytics (docs/design.md §5): the ingest contract of
//! `POST /ingest/v1/event` (beacons, JSON, keys, origins, bots, limits),
//! what is stored of a pageview, the rollups (incremental, late events,
//! retention), the Web tab, the REST API and MCP, k.js, and the privacy
//! scan: no IP address, user agent or personal data in either database.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::{App, CLIENT_IP, Res, add_user, api_token, bearer};
use keen_kestrel::analytics::{self, day_number, rollup};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use vole_kit::auth::TokenScope;

const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
const SITE: &str = "https://shop.example";

/// A beacon as k.js sends it: `text/plain` JSON, from `SITE`.
fn beacon(body: &Value, ua: &str) -> Request<Body> {
    Request::post("/ingest/v1/event")
        .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
        .header(header::USER_AGENT, ua)
        .header(header::ORIGIN, SITE)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn pageview(key: &str, path: &str) -> Value {
    json!({"k": key, "n": "pageview", "u": format!("{SITE}{path}"), "r": null, "w": 1440, "h": 0})
}

async fn send(app: &App, body: &Value, ua: &str) -> Res {
    app.send(beacon(body, ua)).await
}

async fn accepted(app: &App, body: &Value, ua: &str) {
    let res = send(app, body, ua).await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.body);
}

async fn raw_count(app: &App) -> i64 {
    app.flush().await;
    app.store_count("SELECT COUNT(*) FROM analytics_events")
        .await
}

/// Writes what was accepted and rolls it up.
async fn roll(app: &App) -> rollup::Report {
    app.flush().await;
    rollup::run(app.kestrel.ingest.writer()).await.unwrap()
}

async fn set_key(app: &App, sql: &str) {
    app.exec(format!(
        "UPDATE project_keys SET {sql} WHERE project_id = {}",
        app.project_id
    ))
    .await;
}

#[tokio::test]
async fn beacons_and_json_are_accepted_with_202_and_no_cookie() {
    let app = App::new().await;
    // text/plain from sendBeacon or k.js's fetch: no preflight needed.
    let res = send(&app, &pageview(&app.key, "/"), FIREFOX).await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    assert!(res.body.is_empty());
    assert!(res.headers.get(header::SET_COOKIE).is_none());
    assert_eq!(res.header("access-control-allow-origin"), "*");
    // application/json with the key in a header, and the long field names.
    let req = Request::post("/ingest/v1/event")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, FIREFOX)
        .header("x-kestrel-key", &app.key)
        .body(Body::from(
            json!({"name": "pageview", "url": format!("{SITE}/about"), "screen_width": 390})
                .to_string(),
        ))
        .unwrap();
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::ACCEPTED, "{}", res.body);
    // The key in the query string; no content type at all.
    let req = Request::post(format!("/ingest/v1/event?key={}", app.key))
        .header(header::USER_AGENT, FIREFOX)
        .body(Body::from(json!({"u": format!("{SITE}/q")}).to_string()))
        .unwrap();
    assert_eq!(app.send(req).await.status, StatusCode::ACCEPTED);
    assert_eq!(raw_count(&app).await, 3);
    // The preflight for application/json and X-Kestrel-Key.
    let req = Request::options("/ingest/v1/event")
        .header(header::ORIGIN, SITE)
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "content-type, x-kestrel-key",
        )
        .body(Body::empty())
        .unwrap();
    let res = app.send(req).await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    assert!(
        res.header("access-control-allow-headers")
            .contains("x-kestrel-key")
    );
    assert!(res.header("access-control-allow-methods").contains("POST"));
    assert!(res.headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn keys_projects_and_bodies_are_checked() {
    let app = App::new().await;
    let check = |body: Value, status: StatusCode| {
        let app = &app;
        async move {
            let res = send(app, &body, FIREFOX).await;
            assert_eq!(res.status, status, "{body} → {}", res.body);
            assert!(res.headers.get(header::SET_COOKIE).is_none());
            res
        }
    };
    check(json!({"u": format!("{SITE}/")}), StatusCode::UNAUTHORIZED).await;
    check(
        json!({"k": "nope", "u": format!("{SITE}/")}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    check(
        json!({"k": "ffffffffffffffffffffffffffffffff", "u": format!("{SITE}/")}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    check(json!({"k": app.key}), StatusCode::BAD_REQUEST).await;
    check(
        json!({"k": app.key, "u": "ftp://x/"}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    check(
        json!({"k": app.key, "u": format!("{SITE}/"), "n": "   "}),
        StatusCode::ACCEPTED,
    )
    .await;
    // Not JSON, the wrong media type, too large.
    let req = |ct: &str, body: Vec<u8>| {
        Request::post("/ingest/v1/event")
            .header(header::CONTENT_TYPE, ct)
            .header(header::USER_AGENT, FIREFOX)
            .body(Body::from(body))
            .unwrap()
    };
    assert_eq!(
        app.send(req("text/plain", b"not json".to_vec()))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.send(req("application/x-www-form-urlencoded", b"k=1".to_vec()))
            .await
            .status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let big = format!(
        "{{\"k\":\"{}\",\"u\":\"{SITE}/{}\"}}",
        app.key,
        "x".repeat(70 * 1024)
    );
    assert_eq!(
        app.send(req("text/plain", big.into_bytes())).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    // A disabled key, then a project with web analytics off.
    set_key(&app, "enabled = 0").await;
    check(pageview(&app.key, "/"), StatusCode::FORBIDDEN).await;
    set_key(&app, "enabled = 1").await;
    app.exec(format!(
        "UPDATE projects SET web_analytics = 0 WHERE id = {}",
        app.project_id
    ))
    .await;
    let res = check(pageview(&app.key, "/"), StatusCode::FORBIDDEN).await;
    assert!(res.body.contains("web analytics is off"), "{}", res.body);
    assert_eq!(raw_count(&app).await, 1);
}

#[tokio::test]
async fn origins_are_enforced_through_origin_or_referer() {
    let app = App::new().await;
    set_key(&app, "allowed_origins = 'https://shop.example'").await;
    let res = send(&app, &pageview(&app.key, "/"), FIREFOX).await;
    assert_eq!(res.status, StatusCode::ACCEPTED);
    assert_eq!(res.header("access-control-allow-origin"), SITE);
    let from = |origin: Option<&str>, referer: Option<&str>| {
        let mut r = Request::post("/ingest/v1/event")
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::USER_AGENT, FIREFOX);
        if let Some(o) = origin {
            r = r.header(header::ORIGIN, o);
        }
        if let Some(rf) = referer {
            r = r.header(header::REFERER, rf);
        }
        r.body(Body::from(pageview(&app.key, "/").to_string()))
            .unwrap()
    };
    assert_eq!(
        app.send(from(Some("https://evil.example"), None))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(from(None, Some("https://shop.example/pricing?x=1")))
            .await
            .status,
        StatusCode::ACCEPTED,
        "the referer's origin counts"
    );
    assert_eq!(
        app.send(from(None, Some("https://evil.example/")))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(from(None, None)).await.status,
        StatusCode::FORBIDDEN,
        "a key limited to origins needs one"
    );
    assert_eq!(raw_count(&app).await, 2);
}

#[tokio::test]
async fn bots_and_headless_browsers_are_answered_but_not_counted() {
    let app = App::new().await;
    for ua in [
        "",
        "curl/8.4.0",
        "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/141.0.0.0 Safari/537.36",
    ] {
        let res = send(&app, &pageview(&app.key, "/"), ua).await;
        assert_eq!(res.status, StatusCode::ACCEPTED, "{ua}");
    }
    let mut req = beacon(&pageview(&app.key, "/"), FIREFOX);
    req.headers_mut()
        .insert("sec-ch-ua", "\"HeadlessChrome\";v=\"141\"".parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::ACCEPTED);
    let mut req = beacon(&pageview(&app.key, "/"), FIREFOX);
    req.headers_mut()
        .insert("sec-purpose", "prefetch".parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::ACCEPTED);
    assert_eq!(raw_count(&app).await, 0);
}

#[tokio::test]
async fn rate_limits_answer_429() {
    let app = App::with(keen_kestrel::ingest::Settings {
        ip_rate_limit_per_minute: 4,
        ..keen_kestrel::ingest::Settings::default()
    })
    .await;
    set_key(&app, "rate_limit_per_minute = 2").await;
    assert_eq!(
        send(&app, &pageview(&app.key, "/"), FIREFOX).await.status,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(&app, &pageview(&app.key, "/"), FIREFOX).await.status,
        StatusCode::ACCEPTED
    );
    let res = send(&app, &pageview(&app.key, "/"), FIREFOX).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(res.header("retry-after").parse::<u64>().unwrap() >= 1);
    // The per-IP bucket (4 a minute) is next.
    set_key(&app, "rate_limit_per_minute = NULL").await;
    assert_eq!(
        send(&app, &pageview(&app.key, "/"), FIREFOX).await.status,
        StatusCode::ACCEPTED
    );
    let res = send(&app, &pageview(&app.key, "/"), FIREFOX).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers.get(header::SET_COOKIE).is_none());
    assert_eq!(raw_count(&app).await, 3);
}

/// The stored row of the last event.
async fn last_row(app: &App) -> Value {
    app.flush().await;
    let conn = app.kestrel.ingest.store().get().await.unwrap();
    conn.interact(|c| {
        c.query_row(
            "SELECT name, hostname, path, referrer, utm_source, utm_medium, utm_campaign, utm_term,
                    utm_content, browser, os, device, screen, props, visitor
             FROM analytics_events ORDER BY id DESC LIMIT 1",
            [],
            |r| {
                Ok(json!({
                    "name": r.get::<_, String>(0)?, "hostname": r.get::<_, String>(1)?,
                    "path": r.get::<_, String>(2)?, "referrer": r.get::<_, String>(3)?,
                    "utm_source": r.get::<_, String>(4)?, "utm_medium": r.get::<_, String>(5)?,
                    "utm_campaign": r.get::<_, String>(6)?, "utm_term": r.get::<_, String>(7)?,
                    "utm_content": r.get::<_, String>(8)?, "browser": r.get::<_, String>(9)?,
                    "os": r.get::<_, String>(10)?, "device": r.get::<_, String>(11)?,
                    "screen": r.get::<_, String>(12)?, "props": r.get::<_, Option<String>>(13)?,
                    "visitor": r.get::<_, i64>(14)?,
                }))
            },
        )
    })
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn what_a_pageview_keeps() {
    let app = App::new().await;
    let body = json!({"k": app.key, "n": "pageview",
        "u": "https://Shop.Example/pricing/?plan=pro&utm_source=newsletter&utm_medium=email&utm_campaign=autumn&gclid=xyz#plans",
        "r": "https://www.news.example/story?id=1", "w": 1440});
    accepted(&app, &body, FIREFOX).await;
    let row = last_row(&app).await;
    assert_eq!(row["name"], "pageview");
    assert_eq!(row["hostname"], "shop.example");
    assert_eq!(row["path"], "/pricing/");
    assert_eq!(row["referrer"], "news.example");
    assert_eq!(
        (&row["utm_source"], &row["utm_medium"], &row["utm_campaign"]),
        (&json!("newsletter"), &json!("email"), &json!("autumn"))
    );
    assert_eq!(
        (&row["browser"], &row["os"], &row["device"], &row["screen"]),
        (
            &json!("Firefox"),
            &json!("Linux"),
            &json!("desktop"),
            &json!("1440-1919")
        )
    );
    assert_eq!(row["props"], Value::Null);
    // A self-referral is no referrer; an iPhone; a hash route.
    let body = json!({"k": app.key, "u": format!("{SITE}/#/cart?step=2"), "r": "https://www.shop.example/", "w": 390, "h": 1});
    accepted(&app, &body, IPHONE).await;
    let row = last_row(&app).await;
    assert_eq!(row["referrer"], "");
    assert_eq!(row["path"], "/#/cart");
    assert_eq!(
        (&row["browser"], &row["os"], &row["device"], &row["screen"]),
        (
            &json!("Safari"),
            &json!("iOS"),
            &json!("mobile"),
            &json!("0-575")
        )
    );
    // A custom event with properties; personal data is scrubbed.
    let body = json!({"k": app.key, "n": "Signup", "u": format!("{SITE}/users/ada@example.com"),
        "p": {"plan": "pro", "contact": "ada@example.com", "phone": "+47 912 34 567", "password": "hunter2"}});
    accepted(&app, &body, FIREFOX).await;
    let row = last_row(&app).await;
    assert_eq!(row["name"], "Signup");
    assert_eq!(row["path"], "/users/[Filtered]");
    let props: Value = serde_json::from_str(row["props"].as_str().unwrap()).unwrap();
    assert_eq!(
        props,
        json!({"plan": "pro", "contact": "[Filtered]", "phone": "[Filtered]"})
    );
}

#[tokio::test]
async fn visitors_are_daily_hashes_of_ip_browser_and_host() {
    let app = App::new().await;
    accepted(&app, &pageview(&app.key, "/a"), FIREFOX).await;
    let a = last_row(&app).await["visitor"].as_i64().unwrap();
    accepted(&app, &pageview(&app.key, "/b"), FIREFOX).await;
    assert_eq!(last_row(&app).await["visitor"], a, "same browser, same day");
    accepted(&app, &pageview(&app.key, "/a"), IPHONE).await;
    assert_ne!(last_row(&app).await["visitor"], a, "another browser");
    let other_host = json!({"k": app.key, "u": "https://blog.example/"});
    accepted(&app, &other_host, FIREFOX).await;
    assert_ne!(last_row(&app).await["visitor"], a, "another host");
    // The salt lives in memory only: another instance hashes differently.
    let other = App::new().await;
    accepted(&other, &pageview(&other.key, "/a"), FIREFOX).await;
    assert_ne!(last_row(&other).await["visitor"], a);
}

/// Adds raw events directly: `(ts, visitor, name, path, referrer)`.
async fn insert_raw(app: &App, rows: Vec<(i64, i64, &'static str, &'static str, &'static str)>) {
    let pid = app.project_id;
    app.kestrel
        .ingest
        .writer()
        .run(move |c| {
            for (ts, visitor, name, path, referrer) in rows {
                c.execute(
                    "INSERT INTO store.analytics_events (project_id, ts, visitor, name, hostname, path,
                         referrer, browser, os, device, screen)
                     VALUES (?1, ?2, ?3, ?4, 'shop.example', ?5, ?6, 'Firefox', 'Linux', 'desktop', '1440-1919')",
                    rusqlite::params![pid, ts, visitor, name, path, referrer],
                )?;
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

async fn days(app: &App) -> Vec<(String, i64, i64, i64)> {
    app.kestrel
        .core
        .with_conn(|c| {
            c.prepare("SELECT day, pageviews, events, visitors FROM analytics_days ORDER BY day")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect()
        })
        .await
        .unwrap()
}

async fn breakdown(app: &App, dim: &'static str) -> Vec<(String, String, i64, i64)> {
    app.kestrel
        .core
        .with_conn(move |c| {
            c.prepare(
                "SELECT day, value, count, visitors FROM analytics_breakdown WHERE dim = ?1
                 ORDER BY day, value",
            )?
            .query_map([dim], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
        })
        .await
        .unwrap()
}

/// 2026-10-22 00:00:00 UTC.
const DAY: i64 = 1_792_627_200;

#[tokio::test]
async fn rollups_count_daily_uniques_incrementally_and_add_up_ranges() {
    let app = App::new().await;
    let now = DAY + 12 * 3600;
    // Yesterday: visitor 1 twice, visitor 2 once. Today: visitor 1 (a new
    // id on a new day, but here the same number on purpose) and 3.
    insert_raw(
        &app,
        vec![
            (DAY - 3600, 1, "pageview", "/", ""),
            (DAY - 3000, 1, "pageview", "/pricing", "news.example"),
            (DAY - 2000, 2, "pageview", "/pricing", ""),
            (DAY + 600, 1, "pageview", "/", ""),
            (DAY + 4000, 3, "Signup", "/pricing", ""),
        ],
    )
    .await;
    let r = rollup_at(&app, now).await;
    assert_eq!((r.events, r.late_days), (5, 0));
    assert_eq!(r.projects, [app.project_id]);
    assert_eq!(
        days(&app).await,
        [
            ("2026-10-21".to_string(), 3, 0, 2),
            ("2026-10-22".to_string(), 1, 1, 2)
        ]
    );
    // Today's visitor 1 comes back and visitor 4 arrives: one more unique.
    insert_raw(
        &app,
        vec![
            (DAY + 5000, 1, "pageview", "/pricing", ""),
            (DAY + 5100, 4, "pageview", "/", "news.example"),
        ],
    )
    .await;
    let r = rollup_at(&app, now).await;
    assert_eq!(r.events, 2, "only what is new is read");
    assert_eq!(days(&app).await[1], ("2026-10-22".to_string(), 3, 1, 3));
    // Pages: counts and distinct visitors per day.
    assert_eq!(
        breakdown(&app, "page").await,
        [
            ("2026-10-21".to_string(), "/".to_string(), 1, 1),
            ("2026-10-21".to_string(), "/pricing".to_string(), 2, 2),
            ("2026-10-22".to_string(), "/".to_string(), 2, 2),
            ("2026-10-22".to_string(), "/pricing".to_string(), 1, 1),
        ]
    );
    // Entry pages: each visitor's first page of the day.
    assert_eq!(
        breakdown(&app, "entry").await,
        [
            ("2026-10-21".to_string(), "/".to_string(), 1, 1),
            ("2026-10-21".to_string(), "/pricing".to_string(), 1, 1),
            ("2026-10-22".to_string(), "/".to_string(), 2, 2),
        ]
    );
    assert_eq!(
        breakdown(&app, "event").await,
        [("2026-10-22".to_string(), "Signup".to_string(), 1, 1)]
    );
    assert_eq!(
        breakdown(&app, "referrer").await,
        [
            ("2026-10-21".to_string(), String::new(), 2, 2),
            ("2026-10-21".to_string(), "news.example".to_string(), 1, 1),
            ("2026-10-22".to_string(), String::new(), 2, 1),
            ("2026-10-22".to_string(), "news.example".to_string(), 1, 1),
        ]
    );
    // Hours: today's 00:00 hour had visitor 1, the 01:00 hour 1, 3 and 4.
    let hours: Vec<(String, i64, i64, i64)> = app
        .kestrel
        .core
        .with_conn(|c| {
            c.prepare(
                "SELECT hour, pageviews, events, visitors FROM analytics_hours
                 WHERE hour >= '2026-10-22' ORDER BY hour",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
        })
        .await
        .unwrap();
    assert_eq!(
        hours,
        [
            ("2026-10-22 00:00:00".to_string(), 1, 0, 1),
            ("2026-10-22 01:00:00".to_string(), 2, 1, 3)
        ]
    );
    // A range's unique visitors are the sum of the days'.
    let p = analytics::query::Period {
        from: day_number(DAY) - 1,
        to: day_number(DAY),
    };
    let pid = app.project_id;
    let (totals, series, top) = app
        .kestrel
        .core
        .with_conn(move |c| {
            Ok((
                analytics::query::totals(c, pid, p)?,
                analytics::query::series(c, pid, p)?,
                analytics::query::breakdown(c, pid, p, analytics::Dim::Page, 10)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(
        (totals.visitors, totals.pageviews, totals.events),
        (5, 6, 1)
    );
    assert_eq!(series.len(), 2);
    assert_eq!(top.len(), 2);
    assert!(top.iter().all(|r| (r.count, r.visitors) == (3, 3)));
    // The same with a filter, from the raw events.
    let f = analytics::query::Filters::from_pairs([("referrer", "news.example")]);
    let conn = app.kestrel.ingest.store().get().await.unwrap();
    let (raw, raw_top) = conn
        .interact(move |c| {
            Ok::<_, rusqlite::Error>((
                analytics::query::raw_totals(c, pid, p, &f)?,
                analytics::query::raw_breakdown(c, pid, p, &f, analytics::Dim::Page, 10)?,
            ))
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!((raw.visitors, raw.pageviews), (2, 2));
    assert_eq!(raw_top.len(), 2);
}

#[tokio::test]
async fn late_events_roll_their_day_up_again_within_the_raw_retention() {
    let app = App::new().await;
    let now = DAY + 12 * 3600;
    let three_days_ago = DAY - 3 * 86_400;
    insert_raw(
        &app,
        vec![
            (three_days_ago + 100, 7, "pageview", "/a", ""),
            (DAY + 100, 7, "pageview", "/a", ""),
        ],
    )
    .await;
    let r = rollup_at(&app, now).await;
    assert_eq!(r.late_days, 1);
    // A late event for that day (same visitor) and one far too old.
    insert_raw(
        &app,
        vec![
            (three_days_ago + 200, 7, "pageview", "/b", ""),
            (three_days_ago + 300, 8, "pageview", "/b", ""),
            (DAY - 60 * 86_400, 9, "pageview", "/old", ""),
        ],
    )
    .await;
    let r = rollup_at(&app, now).await;
    assert_eq!((r.events, r.late_days), (3, 1));
    assert_eq!(
        days(&app).await,
        [
            ("2026-10-19".to_string(), 3, 0, 2),
            ("2026-10-22".to_string(), 1, 0, 1)
        ]
    );
    let entries = breakdown(&app, "entry").await;
    assert!(entries.contains(&("2026-10-19".to_string(), "/a".to_string(), 1, 1)));
    assert!(entries.contains(&("2026-10-19".to_string(), "/b".to_string(), 1, 1)));
    assert!(!breakdown(&app, "page").await.iter().any(|r| r.1 == "/old"));
    // Yesterday's seen-set is kept, older ones go.
    let seen_days = app
        .store_count("SELECT COUNT(DISTINCT day) FROM analytics_seen")
        .await;
    assert_eq!(seen_days, 1, "only today's");
}

#[tokio::test]
async fn retention_removes_raw_events_and_old_rollups() {
    let app = App::new().await;
    let now_ts = DAY + 12 * 3600;
    insert_raw(
        &app,
        vec![
            (DAY - 40 * 86_400, 1, "pageview", "/old", ""),
            (DAY - 2 * 86_400, 2, "pageview", "/recent", ""),
        ],
    )
    .await;
    rollup_at(&app, now_ts).await;
    app.exec(
        "INSERT INTO analytics_days (project_id, source, day, pageviews, visitors)
              SELECT id, 'web', '2025-01-01', 5, 5 FROM projects"
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
    assert!(report.analytics_rollups_deleted >= 1);
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM analytics_events")
            .await,
        1
    );
    assert_eq!(
        app.count("SELECT COUNT(*) FROM analytics_days WHERE day = '2025-01-01'")
            .await,
        0
    );
    assert_eq!(
        app.count("SELECT COUNT(*) FROM analytics_days WHERE day = '2026-10-20'")
            .await,
        1
    );
    // Per project: a 1-day raw retention removes the recent one too.
    app.exec("UPDATE projects SET retention_analytics_raw_days = 1".into())
        .await;
    let now = "2026-10-22 12:00:00".to_string();
    app.kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::retention::run_blocking(c, &now))
        .await
        .unwrap();
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM analytics_events")
            .await,
        0
    );
    // Deleting the project takes everything along.
    insert_raw(&app, vec![(DAY, 3, "pageview", "/x", "")]).await;
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
    assert_eq!(app.count("SELECT COUNT(*) FROM analytics_days").await, 0);
}

/// HTML as text: the entities askama writes undone.
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

#[tokio::test]
async fn the_web_tab_shows_setup_then_numbers_and_filters() {
    let app = App::new().await;
    let p = app.project_id;
    let mut web = common::Web::new(&app, "wanda", "editor").await;
    let res = web.get(&format!("/projects/{p}/web")).await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.body.contains("/js/k.js"), "the snippet");
    assert!(unescape(&res.body).contains(&format!("data-key=\"{}\"", app.key)));
    assert!(res.body.contains("/static/analytics.css"));
    let overview = web.get(&format!("/projects/{p}")).await;
    assert!(!overview.body.contains("Open web analytics"));

    let ref_body = json!({"k": app.key, "u": format!("{SITE}/pricing?utm_source=newsletter"), "r": "https://news.example/a", "w": 390});
    accepted(&app, &ref_body, IPHONE).await;
    accepted(&app, &pageview(&app.key, "/"), FIREFOX).await;
    accepted(
        &app,
        &json!({"k": app.key, "n": "Signup", "u": format!("{SITE}/pricing")}),
        FIREFOX,
    )
    .await;
    roll(&app).await;
    let res = web.get(&format!("/projects/{p}/web")).await;
    assert_eq!(res.status, StatusCode::OK);
    for text in [
        "/pricing",
        "news.example",
        "newsletter",
        "Firefox",
        "Safari",
        "Signup",
        "Top pages",
        "Entry pages",
        "sse-connect",
        "web-live",
        "576 px",
    ] {
        assert!(res.body.contains(text), "{text}");
    }
    let html = unescape(&res.body);
    assert!(html.contains(&format!("/projects/{p}/web?period=7d&browser=Firefox")));
    assert!(html.contains("< 576 px"));
    for period in [
        "today",
        "30d",
        "custom&from=2026-10-01&to=2026-10-22",
        "nonsense",
    ] {
        let res = web.get(&format!("/projects/{p}/web?period={period}")).await;
        assert_eq!(res.status, StatusCode::OK, "{period}");
    }
    // A filter: counted from the raw events, with a chip to remove it.
    let res = web
        .get(&format!("/projects/{p}/web?period=7d&browser=Safari"))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.body.contains("Browser: Safari"));
    assert!(
        res.body
            .contains(&format!("href=\"/projects/{p}/web?period=7d\""))
    );
    // The overview has the summary now; the settings have the switch.
    let overview = web.get(&format!("/projects/{p}")).await;
    assert!(overview.body.contains("Open web analytics"));
    assert!(overview.body.contains("Current visitors"));
    let settings = web.get(&format!("/projects/{p}/settings")).await;
    assert!(
        settings
            .body
            .contains(&format!("action=\"/projects/{p}/web/settings\""))
    );
    assert!(settings.body.contains("analytics_raw_days"));
    // The live stream stays open (an error or a redirect would end at once).
    let live = tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        web.get(&format!("/projects/{p}/web/live?period=today")),
    )
    .await;
    assert!(live.is_err(), "the stream ended");
    // Turning web analytics off refuses pageviews, and the tab says so.
    let res = web.post(&format!("/projects/{p}/web/settings"), &[]).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        send(&app, &pageview(&app.key, "/"), FIREFOX).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(
        web.get(&format!("/projects/{p}/web"))
            .await
            .body
            .contains("Web analytics is off")
    );
    let res = web
        .post(
            &format!("/projects/{p}/web/settings"),
            &[("enabled", "true")],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        send(&app, &pageview(&app.key, "/"), FIREFOX).await.status,
        StatusCode::ACCEPTED
    );
    // Retention fields save.
    let res = web
        .post(
            &format!("/projects/{p}/retention"),
            &[
                ("event_days", "30"),
                ("issue_days", "90"),
                ("per_day", "1000"),
                ("analytics_raw_days", "14"),
                ("analytics_days", "200"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(
        app.count(
            "SELECT retention_analytics_raw_days * 1000 + retention_analytics_days FROM projects"
        )
        .await,
        14_200
    );
}

#[tokio::test]
async fn viewers_read_the_tab_but_cant_switch_it_off() {
    let app = App::new().await;
    let p = app.project_id;
    let mut web = common::Web::new(&app, "vince", "viewer").await;
    assert_eq!(
        web.get(&format!("/projects/{p}/web")).await.status,
        StatusCode::OK
    );
    let res = web.post(&format!("/projects/{p}/web/settings"), &[]).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(
        web.get("/projects/999/web").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_api_and_mcp_answer_with_counts_only() {
    let app = App::new().await;
    let p = app.project_id;
    accepted(
        &app,
        &json!({"k": app.key, "u": format!("{SITE}/pricing"), "r": "https://news.example/"}),
        FIREFOX,
    )
    .await;
    accepted(&app, &pageview(&app.key, "/"), IPHONE).await;
    accepted(&app, &pageview(&app.key, "/pricing"), IPHONE).await;
    roll(&app).await;
    let uid = add_user(&app, "ana", "viewer").await;
    let token = api_token(&app, uid, TokenScope::Read).await;
    let ts = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/analytics/timeseries?period=today"),
        &token,
        None,
    )
    .await;
    assert_eq!(ts.status, StatusCode::OK, "{}", ts.body);
    let v = ts.json();
    assert_eq!(v["interval"], "hour");
    assert_eq!(v["data"].as_array().unwrap().len(), 24);
    assert_eq!(
        v["totals"],
        json!({"visitors": 2, "pageviews": 3, "events": 0})
    );
    assert_eq!(v["current_visitors"], 2);
    let b = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/analytics/breakdown?dimension=page&period=7d"),
        &token,
        None,
    )
    .await;
    assert_eq!(b.status, StatusCode::OK, "{}", b.body);
    let v = b.json();
    assert_eq!(v["metric"], "pageviews");
    assert_eq!(
        v["data"][0],
        json!({"value": "/pricing", "label": "/pricing", "visitors": 2, "count": 2})
    );
    let f = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/analytics/breakdown?dimension=browser&page=%2Fpricing"),
        &token,
        None,
    )
    .await;
    let v = f.json();
    assert_eq!(v["filters"], json!({"page": "/pricing"}));
    assert_eq!(v["data"].as_array().unwrap().len(), 2);
    let r = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/analytics/breakdown?dimension=referrer"),
        &token,
        None,
    )
    .await;
    assert_eq!(r.json()["data"][0]["label"], "Direct / none");
    for bad in [
        "dimension=nope",
        "dimension=page&limit=0",
        "dimension=page&period=week",
        "dimension=page&period=custom&from=2026-10-10&to=2026-10-01",
    ] {
        let res = bearer(
            &app,
            "GET",
            &format!("/api/v1/projects/{p}/analytics/breakdown?{bad}"),
            &token,
            None,
        )
        .await;
        assert_eq!(
            res.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            res.body
        );
    }
    assert_eq!(
        bearer(
            &app,
            "GET",
            "/api/v1/projects/999/analytics/timeseries",
            &token,
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    // No visitor id anywhere.
    let visitors: Vec<i64> = {
        let conn = app.kestrel.ingest.store().get().await.unwrap();
        conn.interact(|c| -> rusqlite::Result<Vec<i64>> {
            c.prepare("SELECT DISTINCT visitor FROM analytics_events")?
                .query_map([], |r| r.get(0))?
                .collect()
        })
        .await
        .unwrap()
        .unwrap()
    };
    let mcp = |name: &'static str, args: Value| {
        let (app, token) = (&app, token.clone());
        async move {
            let res = bearer(app, "POST", "/mcp", &token, Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}))).await;
            assert_eq!(res.status, StatusCode::OK, "{}", res.body);
            res.json()["result"].clone()
        }
    };
    let summary = mcp(
        "analytics_summary",
        json!({"project": "fixtures", "period": "7d"}),
    )
    .await;
    let text = summary["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("Visitors: 2"), "{text}");
    assert!(text.contains("/pricing"), "{text}");
    assert_eq!(
        summary["structuredContent"]["top"]["referrer"][0]["value"],
        ""
    );
    let breakdown = mcp(
        "analytics_breakdown",
        json!({"project": p, "dimension": "device", "filters": {"browser": "Safari"}}),
    )
    .await;
    assert_eq!(breakdown["structuredContent"]["data"][0]["value"], "mobile");
    let bad = mcp(
        "analytics_breakdown",
        json!({"project": p, "dimension": "nope"}),
    )
    .await;
    assert_eq!(bad["isError"], true);
    for body in [
        ts.body,
        b.body,
        f.body,
        r.body,
        summary.to_string(),
        breakdown.to_string(),
    ] {
        for v in &visitors {
            assert!(
                !body.contains(&v.to_string()),
                "a visitor id leaked: {body}"
            );
        }
    }
}

#[tokio::test]
async fn k_js_is_small_cached_and_cookie_free() {
    let app = App::new().await;
    let get = |headers: &[(&str, &str)]| {
        let mut r = Request::get("/js/k.js");
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        app.send(r.body(Body::empty()).unwrap())
    };
    let plain = get(&[]).await;
    assert_eq!(plain.status, StatusCode::OK);
    assert!(plain.header("content-type").starts_with("text/javascript"));
    assert!(plain.body.contains("/ingest/v1/event"));
    assert!(!plain.body.contains("// "), "comments are stripped");
    assert!(plain.header("cache-control").contains("max-age"));
    assert!(plain.headers.get(header::SET_COOKIE).is_none());
    let etag = plain.header("etag").to_string();
    assert!(etag.starts_with('"'));
    let mut req = Request::get("/js/k.js")
        .header(header::ACCEPT_ENCODING, "gzip, br")
        .body(Body::empty())
        .unwrap();
    req.headers_mut()
        .insert(header::HOST, "kestrel.test".parse().unwrap());
    let router = keen_kestrel::router(app.kestrel.clone());
    let gz = tower::ServiceExt::oneshot(router, req).await.unwrap();
    assert_eq!(gz.headers()[header::CONTENT_ENCODING], "gzip");
    let bytes = axum::body::to_bytes(gz.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        bytes.len() < 2048,
        "k.js is {} bytes gzip-compressed",
        bytes.len()
    );
    let fresh = get(&[("if-none-match", &etag)]).await;
    assert_eq!(fresh.status, StatusCode::NOT_MODIFIED);
    // The ingest port serves the snippet and its endpoint, nothing else.
    let ingest = keen_kestrel::ingest_router(app.kestrel.clone());
    let mut req = Request::get("/js/k.js").body(Body::empty()).unwrap();
    assert_eq!(
        common::send_to(&ingest, &mut req).await.status,
        StatusCode::OK
    );
    let mut req = beacon(&pageview(&app.key, "/via-ingest-port"), FIREFOX);
    assert_eq!(
        common::send_to(&ingest, &mut req).await.status,
        StatusCode::ACCEPTED
    );
    let mut req = Request::get(format!("/projects/{}/web", app.project_id))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        common::send_to(&ingest, &mut req).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn nothing_personal_is_stored_from_web_events() {
    let app = App::new().await;
    const UA: &str =
        "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/131.0 PrivacyProbe/7.7";
    const EMAIL: &str = "ada.lovelace@example.com";
    let bodies = [
        json!({"k": app.key, "u": format!("{SITE}/account/{EMAIL}?email={EMAIL}&utm_source={EMAIL}&utm_campaign=call%20%2B4791234567"), "r": format!("https://mail.example/?to={EMAIL}"), "w": 1280}),
        json!({"k": app.key, "u": format!("{SITE}/pay/4111111111111111"), "w": 1280}),
        json!({"k": app.key, "n": format!("Contact {EMAIL}"), "u": format!("{SITE}/"), "p": {"who": EMAIL, "ip": CLIENT_IP, "user_agent": UA, "card": "4111 1111 1111 1111", "note": format!("from {CLIENT_IP}")}}),
    ];
    for b in &bodies {
        accepted(&app, b, UA).await;
    }
    roll(&app).await;
    assert_eq!(
        app.store_count("SELECT COUNT(*) FROM analytics_events")
            .await,
        3
    );
    assert!(app.count("SELECT COUNT(*) FROM analytics_breakdown").await > 5);
    let needles = [
        CLIENT_IP,
        UA,
        "PrivacyProbe",
        EMAIL,
        "ada.lovelace",
        "4791234567",
        "4111111111111111",
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
                    for needle in needles {
                        assert!(!text.contains(needle), "{needle} in {file} {table}: {text}");
                    }
                }
            }
        }
    }
    assert!(scanned > 50, "{scanned}");
    // And the answers set no cookie (checked above for each ingest answer).
}
