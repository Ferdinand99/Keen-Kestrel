//! The Hedgerow app channel end to end: a channel, a device registered with
//! a read-only token, a new issue that fires an alert rule, the push the
//! (fake) relay gets and the text the app fetches with its ref.
//!
//! This is the only test in its binary: the kit's push client is installed
//! once per process (by `start_with`), so a second Kestrel started in the
//! same process would take it over.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use common::{App, add_user, api_token, bearer, envelope_request, envelope_with, new_id};
use keen_kestrel::db::alerts::{self as db, RuleInput, RuleKind};
use serde_json::{Value, json};
use vole_kit::auth::TokenScope;

/// What the fake relay saw.
#[derive(Default)]
struct Seen {
    registrations: usize,
    pushes: Vec<Value>,
}

/// A relay that takes every registration (difficulty 0) and accepts every
/// handle. Returns its URL.
async fn fake_relay() -> (String, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let app = Router::new()
        .route(
            "/v1/challenge",
            get(|| async {
                Json(json!({
                    "challenge": "challenge-1",
                    "difficulty": 0,
                    "expires_at": "2099-01-01T00:00:00Z",
                }))
            }),
        )
        .route(
            "/v1/instances",
            post(|State(seen): State<Arc<Mutex<Seen>>>| async move {
                seen.lock().unwrap().registrations += 1;
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "instance_credential": "hrk_registered0",
                        "tier": "new",
                        "limits": {"sends_per_hour": 600, "handles_per_request": 25, "unknown_handles_per_hour": 20},
                    })),
                )
            }),
        )
        .route(
            "/v1/push",
            post(
                |State(seen): State<Arc<Mutex<Seen>>>, Json(body): Json<Value>| async move {
                    let n = body["handles"].as_array().map_or(0, Vec::len);
                    seen.lock().unwrap().pushes.push(body);
                    let results: Vec<Value> =
                        (0..n).map(|_| json!({"status": "accepted"})).collect();
                    Json(json!({ "results": results }))
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, seen)
}

const HANDLE: &str = "hrh_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[tokio::test]
async fn a_new_issue_pushes_a_ref_and_the_ref_opens_the_text() {
    let app = App::new().await;
    let (relay_url, seen) = fake_relay().await;
    // Point the push client at the fake relay, and give the instance an
    // address, so a deep link would exist if the text kept it.
    app.kestrel
        .core
        .with_conn(move |c| {
            vole_kit::db::settings::set(c, vole_kit::push::relay::SETTING_RELAY_URL, &relay_url)?;
            vole_kit::db::settings::set(c, vole_kit::db::settings::BASE_URL, "https://kestrel.test")
        })
        .await
        .unwrap();
    let user = add_user(&app, "ada", "viewer").await;
    let read = api_token(&app, user, TokenScope::Read).await;

    // Push is off until there is a Hedgerow app channel.
    let register = json!({ "handle": HANDLE });
    let res = bearer(
        &app,
        "POST",
        "/api/v1/push/devices",
        &read,
        Some(register.clone()),
    )
    .await;
    assert_eq!(res.status, StatusCode::CONFLICT, "{}", res.body);
    assert_eq!(res.json()["error"]["code"], "push_disabled");

    // The channel has no settings of its own; a rule sends to it.
    let channel = app
        .kestrel
        .core
        .with_conn(|c| db::insert_channel(c, "Phones", "hedgerow", "{}", ""))
        .await
        .unwrap();
    let mut rule = RuleInput::new(RuleKind::NewIssue);
    rule.name = "New issues".into();
    rule.channel_ids = vec![channel];
    let project = app.project_id;
    app.kestrel
        .core
        .with_conn(move |c| db::insert_rule(c, project, &rule))
        .await
        .unwrap();

    // A read-only token registers its own device.
    let res = bearer(&app, "POST", "/api/v1/push/devices", &read, Some(register)).await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);

    // A new issue: one alert.
    let event = json!({
        "event_id": new_id(),
        "platform": "python",
        "level": "error",
        "release": "shop@1.0",
        "environment": "production",
        "exception": {"values": [{"type": "PaymentError", "value": "card declined"}]},
    });
    let res = app
        .send(envelope_request(
            app.project_id,
            &app.key,
            envelope_with(&event),
        ))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    for _ in 0..100 {
        if !seen.lock().unwrap().pushes.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Give a wrong second push time to show up.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let (registrations, mut pushes) = {
        let seen = seen.lock().unwrap();
        (seen.registrations, seen.pushes.clone())
    };
    assert_eq!(registrations, 1, "the server registered itself once");
    assert_eq!(pushes.len(), 1, "one push for one alert");
    let push = pushes.remove(0);
    assert_eq!(push["kind"], "alert");
    assert_eq!(push["handles"], json!([HANDLE]));
    let reference = push["ref"].as_str().expect("a ref").to_string();
    assert!(!reference.is_empty());
    // The relay and Apple get a ref and nothing else of the alert.
    let wire = push.to_string();
    for secret in ["PaymentError", "card declined", "Fixtures", "kestrel.test"] {
        assert!(!wire.contains(secret), "{secret} went to the relay: {wire}");
    }

    // The alert queue counts it as sent, once.
    let sent: i64 = app
        .count("SELECT COUNT(*) FROM alert_log WHERE status = 'sent' AND channel_id IS NOT NULL")
        .await;
    assert_eq!(sent, 1);

    // The app fetches the text from this server, with a read-only token.
    let res = bearer(
        &app,
        "GET",
        &format!("/api/v1/push/events/{reference}"),
        &read,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let text = res.json();
    let title = text["title"].as_str().unwrap();
    let body = text["body"].as_str().unwrap_or_default();
    assert!(
        title.contains("PaymentError") || body.contains("PaymentError"),
        "{text}"
    );
    assert!(
        !title.contains("kestrel.test") && !body.contains("kestrel.test"),
        "the deep link isn't stored: {text}"
    );
    // Not without a token.
    let res = app
        .send(
            axum::http::Request::get(format!("/api/v1/push/events/{reference}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    // An unknown ref is a plain 404.
    let res = bearer(&app, "GET", "/api/v1/push/events/nope", &read, None).await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}
