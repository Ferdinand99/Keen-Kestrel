//! Grouping snapshots: every event in every recorded SDK fixture, with the
//! title, culprit, tags and the grouping components and hash it gets
//! (`kestrel:v2`). A changed snapshot means changed grouping: that needs a
//! new grouping configuration, not an updated snapshot (docs/design.md §4.4).
//! Review with `cargo insta review`.
//!
//! Moving from `kestrel:v1` to `kestrel:v2` (SDK frames never in-app)
//! changed every snapshot's `config` and `hash` (the configuration's name
//! is part of the hash) and nothing else: no fixture has an SDK frame marked
//! in-app (the sentry-cocoa crashes are unsymbolicated here, the Rust SDK's
//! frames were already left out by v1's Rust rules, and the JavaScript and
//! Python SDK frames are in `node_modules` and `site-packages`). The rule
//! itself is tested in `src/event/grouping.rs`.

mod common;

use std::fmt::Write as _;

use keen_kestrel::event::{self, Context};
use serde_json::Value;

const KEY: [u8; 32] = [7; 32];

fn context() -> Context<'static> {
    Context {
        project_id: 1,
        keep_user_details: false,
        hash_key: &KEY,
        received: "2026-10-22T14:00:00Z".parse().unwrap(),
    }
}

#[test]
fn every_fixture_event_groups_as_recorded() {
    keen_kestrel::install();
    let mut seen = 0;
    for f in common::fixtures() {
        for (n, (payload, envelope_id)) in f.events().into_iter().enumerate() {
            let r = event::prepare(&payload, &context(), envelope_id.as_deref()).unwrap();
            let components: Value = serde_json::from_str(&r.components).unwrap();
            let mut text = String::new();
            let _ = writeln!(text, "title: {}", r.title);
            let _ = writeln!(text, "culprit: {}", r.culprit);
            let _ = writeln!(text, "level: {}", r.level);
            let _ = writeln!(text, "platform: {}", r.platform);
            let _ = writeln!(text, "config: {}", r.grouping_config);
            let _ = writeln!(text, "hash: {}", r.hash);
            let _ = writeln!(text, "unsymbolicated: {}", r.unsymbolicated);
            let tags: Vec<String> = r.tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let _ = writeln!(text, "tags: {}", tags.join(", "));
            let _ = writeln!(
                text,
                "components:\n{}",
                serde_json::to_string_pretty(&components).unwrap()
            );
            let name = format!("{}__{}__{}__{n}", f.sdk, f.version, f.name).replace('.', "_");
            insta::assert_snapshot!(name, text);
            seen += 1;
        }
    }
    assert!(seen >= 20, "{seen} events");
}

/// The hash of the first event in a fixture.
fn hash_of(sdk: &str, name: &str) -> (String, Value) {
    let f = common::fixtures()
        .into_iter()
        .find(|f| f.sdk == sdk && f.name == name)
        .unwrap_or_else(|| panic!("{sdk}/{name}"));
    let (payload, id) = f.events().into_iter().next().unwrap();
    let r = event::prepare(&payload, &context(), id.as_deref()).unwrap();
    (r.hash, serde_json::from_str(&r.components).unwrap())
}

#[test]
fn the_same_error_twice_is_one_issue() {
    keen_kestrel::install();
    for (sdk, a, b) in [
        ("sentry-python", "plain-exception", "plain-exception-again"),
        ("sentry-rust", "plain-error", "plain-error-again"),
        ("sentry-cocoa", "handled-nserror", "handled-nserror-again"),
    ] {
        assert_eq!(hash_of(sdk, a).0, hash_of(sdk, b).0, "{sdk}");
    }
}

#[test]
fn shapes_group_by_the_right_rule() {
    keen_kestrel::install();
    let kind = |sdk, name| hash_of(sdk, name).1["kind"].as_str().unwrap().to_string();
    for sdk in ["sentry-python", "sentry-javascript-node", "sentry-rust"] {
        assert_eq!(kind(sdk, "custom-fingerprint"), "fingerprint", "{sdk}");
        let (_, c) = hash_of(sdk, "custom-fingerprint");
        assert_eq!(c["values"][0], "payment-gateway");
        assert!(c.get("default").is_some(), "{{{{ default }}}} expands");
    }
    // Chained exceptions contribute every exception of the chain.
    let (_, py) = hash_of("sentry-python", "chained-exception");
    assert_eq!(py["values"].as_array().unwrap().len(), 2);
    assert_eq!(py["values"][1]["type"], "RuntimeError");
    let (_, node) = hash_of("sentry-javascript-node", "chained-exception");
    assert_eq!(node["values"].as_array().unwrap().len(), 2);
    // Templates group by the template, not the text.
    assert_eq!(
        hash_of("sentry-python", "message-template").1,
        serde_json::json!({"kind": "message", "template": "Order %s failed after %d retries"})
    );
    // Without a stack: type and normalised value.
    let (_, rust) = hash_of("sentry-rust", "plain-error");
    assert_eq!(rust["kind"], "exception_value");
    assert_eq!(rust["values"][0]["value"], "Invalid order <num>");
    // A Rust panic groups by the app's frames, not the SDK's or std's.
    let (_, panic) = hash_of("sentry-rust", "panic");
    let frames = panic["values"][0]["frames"].to_string();
    assert!(frames.contains("kk_fixtures::main"), "{frames}");
    assert!(
        !frames.contains("sentry") && !frames.contains("core::"),
        "{frames}"
    );
    // Stripped iOS builds: image and offset, unsymbolicated.
    for crash in [
        "crash-fatalerror",
        "crash-force-unwrap",
        "crash-nsexception",
    ] {
        let f = common::fixtures()
            .into_iter()
            .find(|f| f.sdk == "sentry-cocoa" && f.name == crash)
            .unwrap();
        let (payload, id) = f.events().into_iter().next().unwrap();
        let r = event::prepare(&payload, &context(), id.as_deref()).unwrap();
        assert!(r.unsymbolicated, "{crash}");
        assert!(
            r.components.contains("\"image\":\"KestrelSpike\""),
            "{crash}: {}",
            r.components
        );
    }
}
