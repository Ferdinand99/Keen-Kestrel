//! Alerts (docs/design.md §4.7): rules for new issues, regressions, spikes
//! and a release's first crash, cooldowns and digests, delivery through a
//! webhook channel to a receiver in the test (never a real service), off
//! ingest's path, with retries and a log; the channel and rule pages.

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum_extra::extract::cookie::Key;
use common::{App, Hook, Web, envelope_request, envelope_with, new_id};
use keen_kestrel::db::alerts::{self as db, RuleInput, RuleKind};
use serde_json::{Value, json};

/// Kestrel with spikes checked on every batch (tests don't wait a minute).
async fn app() -> App {
    let mut settings = keen_kestrel::ingest::Settings::default();
    settings.alerts.spike_check_every = Duration::ZERO;
    App::with(settings).await
}

/// A webhook channel to `hook`, in `language` ("" for the instance's).
async fn channel(app: &App, hook: &Hook, name: &'static str, language: &'static str) -> i64 {
    let config = json!({"url": hook.url, "method": "POST", "content_type": "json"}).to_string();
    app.kestrel
        .core
        .with_conn(move |c| db::insert_channel(c, name, "webhook", &config, language))
        .await
        .unwrap()
}

/// A rule of `kind` sending to `channels`, changed by `f`.
async fn rule(
    app: &App,
    kind: RuleKind,
    channels: Vec<i64>,
    f: impl FnOnce(&mut RuleInput),
) -> i64 {
    let mut r = RuleInput::new(kind);
    r.name = format!("{} rule", kind.as_str());
    r.channel_ids = channels;
    f(&mut r);
    let project = app.project_id;
    app.kestrel
        .core
        .with_conn(move |c| db::insert_rule(c, project, &r))
        .await
        .unwrap()
}

/// An error event of type `ty` (one issue per type).
fn event(ty: &str) -> Value {
    json!({
        "event_id": new_id(),
        "platform": "python",
        "level": "error",
        "release": "shop@1.0",
        "environment": "production",
        "exception": {"values": [{"type": ty, "value": "it broke"}]},
    })
}

async fn send(app: &App, e: &Value) {
    let res = app
        .send(envelope_request(app.project_id, &app.key, envelope_with(e)))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
}

/// `(kind, status, attempts)` of every log entry, oldest first.
async fn log(app: &App) -> Vec<(String, String, i64)> {
    app.kestrel
        .core
        .with_conn(|c| {
            c.prepare("SELECT kind, status, attempts FROM alert_log ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect()
        })
        .await
        .unwrap()
}

async fn issue_id(app: &App, ty: &'static str) -> i64 {
    app.kestrel
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT id FROM issues WHERE title LIKE ?1",
                [format!("{ty}:%")],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

async fn resolve(app: &App, issue: i64) {
    let project = app.project_id;
    app.kestrel
        .ingest
        .writer()
        .run(move |c| keen_kestrel::db::triage::resolve(c, project, &[issue], false, "test"))
        .await
        .unwrap();
}

fn summary(v: &Value) -> String {
    v["summary"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn new_issues_and_regressions_alert() {
    let app = app().await;
    let hook = Hook::start().await;
    let ch = channel(&app, &hook, "Ops hook", "").await;
    rule(&app, RuleKind::NewIssue, vec![ch], |_| {}).await;
    rule(&app, RuleKind::Regression, vec![ch], |r| {
        r.cooldown_minutes = 0
    })
    .await;
    // Only staging: production events don't set it off.
    rule(&app, RuleKind::NewIssue, vec![ch], |r| {
        r.environment = "staging".into()
    })
    .await;

    send(&app, &event("ValueError")).await;
    app.flush().await;
    // (The sender's own task may get there first.)
    app.kestrel.alerts.deliver_now().await;
    let got = hook.received();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["event"], "new_issue");
    assert_eq!(
        summary(&got[0]),
        "\u{1f534} New issue in Fixtures — ValueError: it broke"
    );
    // The webhook speaks of an issue (KitConfig::webhook), not a monitor.
    assert_eq!(got[0]["issue"], "ValueError: it broke");
    assert!(got[0].get("monitor").is_none(), "{:#?}", got[0]);

    // The same issue again: nothing new.
    send(&app, &event("ValueError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert_eq!(hook.received().len(), 1);

    // Resolved, then seen again: a regression, each time (no cooldown).
    let id = issue_id(&app, "ValueError").await;
    for n in 1..=2 {
        resolve(&app, id).await;
        send(&app, &event("ValueError")).await;
        app.flush().await;
        app.kestrel.alerts.deliver_now().await;
        let got = hook.received();
        assert_eq!(got.len(), 1 + n, "{got:#?}");
        assert_eq!(got[n]["event"], "regression");
        assert!(summary(&got[n]).contains("Issue regressed in Fixtures"));
    }
    let entries = log(&app).await;
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|(_, s, a)| s == "sent" && *a == 1));
}

#[tokio::test]
async fn spikes_need_the_threshold_and_the_baseline_and_respect_the_cooldown() {
    let app = app().await;
    let hook = Hook::start().await;
    let ch = channel(&app, &hook, "Spikes", "").await;
    rule(&app, RuleKind::Spike, vec![ch], |r| {
        r.threshold = 3;
        r.multiplier = 5.0;
        r.cooldown_minutes = 60;
    })
    .await;

    // Three events: not more than the threshold.
    for _ in 0..3 {
        send(&app, &event("SpikeError")).await;
    }
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert!(hook.received().is_empty());
    // A fourth: above it.
    send(&app, &event("SpikeError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    let got = hook.received();
    assert_eq!(got.len(), 1, "{got:#?}");
    assert_eq!(got[0]["event"], "spike");
    assert!(
        summary(&got[0]).contains("Spike in Fixtures: 4 events in the last hour"),
        "{}",
        summary(&got[0])
    );

    // More events within the cooldown: no second alert …
    send(&app, &event("SpikeError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert_eq!(hook.received().len(), 1);
    // … until it is over.
    app.exec("UPDATE alert_cooldowns SET fired_at = '2000-01-01 00:00:00'".into())
        .await;
    send(&app, &event("SpikeError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    let got = hook.received();
    assert_eq!(got.len(), 2);
    assert!(
        summary(&got[1]).contains("6 events"),
        "{}",
        summary(&got[1])
    );

    // A busy issue: 10 events an hour for the past week make the limit
    // 5 × 10 = 50, so 8 events in the last hour are no spike.
    send(&app, &event("BusyError")).await;
    app.flush().await;
    let busy = issue_id(&app, "BusyError").await;
    let now = keen_kestrel::issues::now();
    let this_hour = keen_kestrel::db::issues::hour_of(&now);
    let project = app.project_id;
    app.kestrel
        .core
        .with_conn(move |c| {
            for h in 2..=169 {
                let hour = keen_kestrel::alerts::shift(&this_hour, -h * 3600);
                c.execute(
                    "INSERT INTO issue_hours (issue_id, hour, project_id, events) VALUES (?1, ?2, ?3, 10)",
                    rusqlite::params![busy, hour, project],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    for _ in 0..7 {
        send(&app, &event("BusyError")).await;
    }
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert_eq!(hook.received().len(), 2, "no spike for the busy issue");
    // The numbers the alert would use.
    let r = app
        .kestrel
        .core
        .with_conn(move |c| {
            let rules = db::rules(c, project)?;
            keen_kestrel::alerts::check_spike(c, busy, &rules[0], &now)
        })
        .await
        .unwrap();
    assert_eq!(r.baseline, 10.0);
    assert_eq!(r.limit, 50.0);
    assert_eq!(r.last_hour, 8);
    assert!(!r.fires());
}

#[tokio::test]
async fn the_first_crash_of_a_release_alerts_once() {
    let app = app().await;
    let hook = Hook::start().await;
    let ch = channel(&app, &hook, "Crashes", "").await;
    rule(&app, RuleKind::FirstCrash, vec![ch], |_| {}).await;
    let crash = |ty: &str, release: &str| {
        let mut e = event(ty);
        e["level"] = json!("fatal");
        e["release"] = json!(release);
        e
    };
    send(&app, &crash("CrashOne", "shop@2.0")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    let got = hook.received();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["event"], "first_crash");
    assert!(
        summary(&got[0]).contains("First crash in shop@2.0 (Fixtures)"),
        "{}",
        summary(&got[0])
    );
    // Another crash in the same release, and a plain error in a new one.
    send(&app, &crash("CrashTwo", "shop@2.0")).await;
    send(&app, &event("NotACrash")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert_eq!(hook.received().len(), 1);
    // An unhandled exception in a new release is a crash too.
    let mut unhandled = event("Unhandled");
    unhandled["release"] = json!("shop@2.1");
    unhandled["exception"]["values"][0]["mechanism"] = json!({"type": "generic", "handled": false});
    send(&app, &unhandled).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    let got = hook.received();
    assert_eq!(got.len(), 2);
    assert!(summary(&got[1]).contains("shop@2.1"));
}

#[tokio::test]
async fn digests_gather_a_channels_alerts_into_one_message() {
    let app = app().await;
    let hook = Hook::start().await;
    let ch = channel(&app, &hook, "Digest", "").await;
    rule(&app, RuleKind::NewIssue, vec![ch], |r| {
        r.digest_minutes = Some(15)
    })
    .await;
    send(&app, &event("FirstError")).await;
    send(&app, &event("SecondError")).await;
    app.flush().await;
    assert_eq!(app.kestrel.alerts.deliver_now().await, 0);
    assert!(hook.received().is_empty());
    let entries = log(&app).await;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|(_, s, _)| s == "digest"));
    // Both wait for the same message.
    assert_eq!(
        app.count("SELECT COUNT(DISTINCT due_at) FROM alert_log")
            .await,
        1
    );
    app.exec("UPDATE alert_log SET due_at = '2000-01-01 00:00:00'".into())
        .await;
    app.kestrel.alerts.deliver_now().await;
    let got = hook.received();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["event"], "digest");
    assert!(
        summary(&got[0]).contains("2 alerts from Keen Kestrel"),
        "{}",
        summary(&got[0])
    );
    assert!(log(&app).await.iter().all(|(_, s, _)| s == "sent"));
}

#[tokio::test]
async fn delivery_never_holds_up_ingest() {
    let app = app().await;
    let hook = Hook::start().await;
    hook.state()
        .delay_ms
        .store(3000, std::sync::atomic::Ordering::SeqCst);
    let ch = channel(&app, &hook, "Slow", "").await;
    rule(&app, RuleKind::NewIssue, vec![ch], |_| {}).await;
    let started = Instant::now();
    send(&app, &event("SlowReceiver")).await;
    app.flush().await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    // Ingest goes on while the receiver takes its time.
    for n in 0..20 {
        send(&app, &event(&format!("Busy{n}Error"))).await;
    }
    app.flush().await;
    assert!(started.elapsed() < Duration::from_secs(3));
    // The sender's own task delivers it.
    let got = hook.wait_for(1, 30).await;
    assert!(!got.is_empty());
    assert_eq!(got[0]["event"], "new_issue");
}

#[tokio::test]
async fn failed_deliveries_are_retried_then_given_up() {
    let app = app().await;
    let hook = Hook::start().await;
    hook.state()
        .fail_next
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let ch = channel(&app, &hook, "Flaky", "").await;
    rule(&app, RuleKind::NewIssue, vec![ch], |_| {}).await;
    send(&app, &event("FlakyError")).await;
    app.flush().await;
    // The sender's task may get there first; wait for the attempt.
    app.kestrel.alerts.deliver_now().await;
    let (error, next): (Option<String>, Option<String>) = app
        .kestrel
        .core
        .with_conn(|c| {
            c.query_row("SELECT error, next_attempt_at FROM alert_log", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
        })
        .await
        .unwrap();
    assert_eq!(log(&app).await, [("new_issue".into(), "pending".into(), 1)]);
    assert!(error.unwrap().contains("500"));
    // Tried again a minute later.
    assert!(next.unwrap() > keen_kestrel::issues::now());
    app.exec("UPDATE alert_log SET next_attempt_at = '2000-01-01 00:00:00'".into())
        .await;
    app.kestrel.alerts.deliver_now().await;
    assert_eq!(log(&app).await, [("new_issue".into(), "sent".into(), 2)]);

    // A receiver that keeps failing: after the last retry, failed.
    hook.state()
        .fail_next
        .store(100, std::sync::atomic::Ordering::SeqCst);
    send(&app, &event("AlwaysFailing")).await;
    app.flush().await;
    for _ in 0..4 {
        app.kestrel.alerts.deliver_now().await;
        app.exec(
            "UPDATE alert_log SET next_attempt_at = '2000-01-01 00:00:00' WHERE status = 'pending'"
                .into(),
        )
        .await;
    }
    let entries = log(&app).await;
    assert_eq!(entries[1], ("new_issue".into(), "failed".into(), 4));
    assert_eq!(app.kestrel.alerts.deliver_now().await, 0);
}

#[tokio::test]
async fn paused_channels_are_skipped() {
    let app = app().await;
    let hook = Hook::start().await;
    let ch = channel(&app, &hook, "Paused", "").await;
    rule(&app, RuleKind::NewIssue, vec![ch], |_| {}).await;
    app.kestrel
        .core
        .with_conn(move |c| db::set_channel_active(c, ch, false))
        .await
        .unwrap();
    send(&app, &event("QuietError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert!(hook.received().is_empty());
    assert!(
        log(&app).await.is_empty(),
        "nothing queued for a paused channel"
    );
}

/// The channel pages: editors and admins only; the provider form, secrets
/// never shown again, test messages in the channel's language.
#[tokio::test]
async fn channels_are_managed_with_their_secrets_kept() {
    let app = app().await;
    let hook = Hook::start().await;
    let mut viewer = Web::new(&app, "vera", "viewer").await;
    assert_eq!(
        viewer.get("/settings/notifications").await.status,
        StatusCode::FORBIDDEN
    );
    let mut ed = Web::new(&app, "edda", "editor").await;
    let page = ed.get("/settings/notifications").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("No channels yet"), "{}", page.body);
    let picker = ed.get("/settings/notifications/providers").await;
    assert!(picker.body.contains("ntfy"));
    let form = ed.get("/settings/notifications/form?kind=webhook").await;
    assert!(form.body.contains("name=\"c_url\""), "{}", form.body);
    assert!(
        form.body.contains("https://example.com/hooks/kestrel…"),
        "{}",
        form.body
    );
    // The kit's field partial, with the kit's words for the headers.
    assert!(form.body.contains("Add header"), "{}", form.body);
    assert!(form.body.contains("name=\"language\""));

    // Test before saving.
    let fields = [
        ("kind", "webhook"),
        ("name", "Ops hook"),
        ("c_url", hook.url.as_str()),
        ("c_method", "POST"),
        ("c_content_type", "json"),
        ("c_headers__k", "Authorization"),
        ("c_headers__v", "Bearer hook-secret-xyz"),
        ("language", "nb"),
    ];
    let res = ed
        .post_with("/settings/notifications/test", &fields, true)
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(
        res.header("hx-trigger").contains("success"),
        "{}",
        res.header("hx-trigger")
    );
    let got = hook.wait_for(1, 10).await;
    assert_eq!(got[0]["event"], "test");
    // Norwegian, not English.
    assert!(
        !summary(&got[0]).contains("Test alert from"),
        "{}",
        summary(&got[0])
    );

    // Save it.
    let res = ed.post_with("/settings/notifications", &fields, true).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert!(res.body.contains("Ops hook"));
    let id: i64 = app
        .count("SELECT id FROM notification_channels WHERE name = 'Ops hook'")
        .await;
    let edit = ed.get(&format!("/settings/notifications/{id}/edit")).await;
    assert_eq!(edit.status, StatusCode::OK);
    assert!(
        !edit.body.contains("hook-secret-xyz"),
        "the header is a secret"
    );
    assert!(!edit.body.contains(&hook.url), "the URL is a secret");
    assert!(edit.body.contains("Message language"));

    // A saved channel's test, and an edit that keeps the secrets.
    let res = ed
        .post_with(&format!("/settings/notifications/{id}/test"), &[], true)
        .await;
    assert!(res.header("hx-trigger").contains("success"));
    assert_eq!(hook.wait_for(2, 10).await.len(), 2);
    let res = ed
        .post(
            &format!("/settings/notifications/{id}"),
            &[
                ("kind", "webhook"),
                ("name", "Ops hook 2"),
                ("c_url", ""),
                ("c_method", "POST"),
                ("c_content_type", "json"),
                ("language", ""),
                ("active", "true"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    let config: String = app
        .kestrel
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT config FROM notification_channels WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    // Kept, and sealed at rest (vole-kit 0.5).
    let config: Value = serde_json::from_str(&config).unwrap();
    assert!(
        config["url"].as_str().unwrap().starts_with("sealed:v1:"),
        "{config}"
    );
    let key = &app.kestrel.core.session_key;
    let opened = vole_kit::notify::sealed::open_config(key, "webhook", &config);
    assert_eq!(opened.config["url"], hook.url.as_str());

    // Viewers can't test or delete; editors delete.
    assert_eq!(
        viewer
            .post_with(&format!("/settings/notifications/{id}/test"), &[], true)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let res = ed.delete(&format!("/settings/notifications/{id}")).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        app.count("SELECT COUNT(*) FROM notification_channels")
            .await,
        0
    );
}

/// Channel `id`'s config as stored.
async fn stored_config(app: &App, id: i64) -> Value {
    let raw: String = app
        .kestrel
        .core
        .with_conn(move |c| {
            c.query_row(
                "SELECT config FROM notification_channels WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// vole-kit 0.5: plaintext from an older Kestrel is sealed at the next
/// start, once; alerts still reach the real URL; a secret sealed with
/// another instance key asks to be entered again and sends nothing.
#[tokio::test]
async fn channel_secrets_are_sealed_at_start_and_opened_to_send() {
    let app = app().await;
    let hook = Hook::start().await;
    let ch = channel(&app, &hook, "Ops hook", "").await;
    rule(&app, RuleKind::NewIssue, vec![ch], |_| {}).await;
    drop(keen_kestrel::start(&app.dir, None).await.unwrap());
    let sealed = stored_config(&app, ch).await;
    assert!(
        sealed["url"].as_str().unwrap().starts_with("sealed:v1:"),
        "{sealed}"
    );
    assert_eq!(sealed["method"], "POST", "other settings stay readable");
    drop(keen_kestrel::start(&app.dir, None).await.unwrap());
    assert_eq!(
        stored_config(&app, ch).await,
        sealed,
        "a second start changes nothing"
    );

    send(&app, &event("SealedError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    let delivered = hook.wait_for(1, 10).await.len();
    assert!(delivered >= 1);

    // session.key lost: the secret was sealed with a key that is gone.
    let plain = json!({"url": hook.url, "method": "POST", "content_type": "json"});
    let lost = vole_kit::notify::sealed::seal_config(&Key::from(&[7u8; 64]), "webhook", &plain);
    app.exec(format!(
        "UPDATE notification_channels SET config = '{lost}' WHERE id = {ch}"
    ))
    .await;
    let mut ed = Web::new(&app, "edda", "editor").await;
    let edit = ed.get(&format!("/settings/notifications/{ch}/edit")).await;
    assert_eq!(edit.status, StatusCode::OK);
    assert!(edit.body.contains("instance key changed"), "{}", edit.body);
    let res = ed
        .post_with(&format!("/settings/notifications/{ch}/test"), &[], true)
        .await;
    assert!(res.header("hx-trigger").contains("instance key changed"));
    send(&app, &event("LostKeyError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    assert_eq!(hook.received().len(), delivered, "nothing sent");
    assert!(
        app.count("SELECT COUNT(*) FROM alert_log WHERE error LIKE '%instance key changed%'")
            .await
            >= 1
    );
}

/// The Alerts tab: rules and what they sent; editors add and change rules.
#[tokio::test]
async fn the_alerts_tab_shows_rules_and_history() {
    let app = app().await;
    let hook = Hook::start().await;
    let p = app.project_id;
    let mut ed = Web::new(&app, "edda", "editor").await;
    let page = ed.get(&format!("/projects/{p}/alerts")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("No notification channels yet"));
    let ch = channel(&app, &hook, "Ops hook", "").await;
    let new = ed
        .get(&format!("/projects/{p}/alerts/new?kind=spike"))
        .await;
    assert_eq!(new.status, StatusCode::OK);
    assert!(new.body.contains("Ops hook"));
    // Without a channel: refused with the reason.
    let res = ed
        .post(
            &format!("/projects/{p}/alerts"),
            &[
                ("kind", "new_issue"),
                ("name", "New issues"),
                ("cooldown_minutes", "60"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.body.contains("Choose at least one channel"));
    let ch_s = ch.to_string();
    let res = ed
        .post(
            &format!("/projects/{p}/alerts"),
            &[
                ("kind", "new_issue"),
                ("name", "New issues"),
                ("cooldown_minutes", "60"),
                ("channels", ch_s.as_str()),
                ("enabled", "true"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    send(&app, &event("PageError")).await;
    app.flush().await;
    app.kestrel.alerts.deliver_now().await;
    let page = ed.get(&format!("/projects/{p}/alerts")).await;
    assert!(page.body.contains("New issues"));
    assert!(page.body.contains("New issue in Fixtures"));
    assert!(page.body.contains("PageError: it broke"));
    assert!(page.body.contains("Sent"));
    // A viewer reads it but can't add rules.
    let mut viewer = Web::new(&app, "vera", "viewer").await;
    assert_eq!(
        viewer.get(&format!("/projects/{p}/alerts")).await.status,
        StatusCode::OK
    );
    assert_eq!(
        viewer
            .get(&format!("/projects/{p}/alerts/new"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Edit and delete.
    let rule_id: i64 = app.count("SELECT id FROM alert_rules").await;
    let edit = ed.get(&format!("/projects/{p}/alerts/{rule_id}")).await;
    assert!(edit.body.contains("value=\"New issues\""));
    let res = ed
        .post(&format!("/projects/{p}/alerts/{rule_id}/delete"), &[])
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(app.count("SELECT COUNT(*) FROM alert_rules").await, 0);
    // The history stays.
    assert_eq!(app.count("SELECT COUNT(*) FROM alert_log").await, 1);
}

/// The issue page and navigation have the new tabs.
#[tokio::test]
async fn project_pages_have_the_releases_and_alerts_tabs() {
    let app = app().await;
    let p = app.project_id;
    let mut ed = Web::new(&app, "edda", "editor").await;
    let page = ed.get(&format!("/projects/{p}")).await;
    for tab in [
        "/issues",
        "/releases",
        "/alerts",
        "/debug-files",
        "/settings",
    ] {
        assert!(
            page.body.contains(&format!("href=\"/projects/{p}{tab}\"")),
            "{tab}"
        );
    }
    let settings = ed.get("/settings/security").await;
    assert!(settings.body.contains("href=\"/settings/notifications\""));
    let mut viewer = Web::new(&app, "vera", "viewer").await;
    let settings = viewer.get("/settings/security").await;
    assert!(!settings.body.contains("href=\"/settings/notifications\""));
}
