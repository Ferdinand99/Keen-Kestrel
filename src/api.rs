//! REST API v1 (`/api/v1/**`): projects and their DSN keys, issues and
//! events ([`issues`]), releases and their health ([`releases`]), alert
//! rules and the alerts they sent ([`rules`]), debug files, web analytics
//! ([`analytics`]), app analytics ([`app`]), and the route
//! table ([`DOCS`]) the kit turns into `/api-docs`, the OpenAPI document,
//! `/api-docs.md` and `/llms.txt`. The table also documents the ingest
//! endpoints the SDKs call (with a DSN key, not an API token) and
//! sentry-cli's.
//!
//! Conventions are the kit's (`vole_kit::http::rest`): bearer tokens (read
//! tokens act as viewers), `{"error": {"code", "message", "fields"}}`, lists
//! as `{"data": [...], "next_cursor": ...}` with `?limit=&cursor=`, weak
//! ETags, `201 Created` with `Location`, `204 No Content` on delete.

use std::fmt::Write as _;

use axum::Router;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::docs::{BearerScheme, Credential, DocsSpec, ErrorCode, HeaderScheme, RouteDoc, r};
use vole_kit::http::rest::{
    Body, Id, Params, V1Error, V1Result, created, json_ok, limit, no_content, paginate,
    parse_cursor,
};
use vole_kit::i18n;
use vole_kit::state::Core;

use crate::db::projects::{self as db, KeyChanges, Project, ProjectKey};
use crate::dsn;
use crate::project::{self, Platform};

pub mod analytics;
pub mod app;
pub mod groups;
pub mod issues;
pub mod releases;
pub mod rules;

// ---------------------------------------------------------------------------
// Reference
// ---------------------------------------------------------------------------

const LIMIT_Q: &str = "limit: page size (1–500, default 50)";
const CURSOR_Q: &str = "cursor: next_cursor from the previous page";

const PROJECT_EXAMPLE: &str = r#"{
  "id": 1,
  "name": "Sylo for iOS",
  "slug": "sylo-ios",
  "platform": "apple",
  "group_id": 2,
  "created_at": "2026-10-22 08:00:00"
}"#;

const GROUP_EXAMPLE: &str = r#"{
  "id": 2,
  "name": "Sylo",
  "sort_order": 0,
  "projects": 3,
  "created_at": "2026-10-22 08:00:00"
}"#;

const KEY_EXAMPLE: &str = r#"{
  "id": 3,
  "project_id": 1,
  "name": "Default",
  "public_key": "6f1b0c2d9e8a4b7c8d1e2f3a4b5c6d7e",
  "dsn": "https://6f1b0c2d9e8a4b7c8d1e2f3a4b5c6d7e@kestrel.example.com/1",
  "enabled": true,
  "rate_limit_per_minute": null,
  "allowed_origins": [],
  "created_at": "2026-10-22 08:00:00"
}"#;

const DIF_UPLOAD_EXAMPLE: &str = r#"{
  "data": [{
    "id": 4,
    "project_id": 1,
    "debug_id": "74b62df4-771f-3338-8a76-efff43e57038",
    "code_id": "74b62df4771f33388a76efff43e57038",
    "object_name": "Sylo",
    "arch": "arm64",
    "kind": "macho",
    "features": ["debug", "symtab", "unwind"],
    "size": 3482880,
    "sha1": "1d8303e249d133f5d0fa0590edb7efaaf6f27bad",
    "release": null,
    "uploaded_at": "2026-10-22 08:00:00",
    "last_used_at": null
  }],
  "skipped": ["Sylo.app.dSYM/Contents/Info.plist"]
}"#;

const ISSUE_EXAMPLE: &str = r#"{
  "id": 42,
  "project_id": 1,
  "title": "ValueError: invalid literal for int() with base 10: 'abc'",
  "culprit": "shop.views in checkout",
  "level": "error",
  "platform": "python",
  "status": "unresolved",
  "substatus": "new",
  "first_seen": "2026-10-22 08:00:00",
  "last_seen": "2026-10-22 09:15:00",
  "events": 17,
  "users": 4,
  "first_release": "shop@1.4.0",
  "last_release": "shop@1.4.1",
  "resolved_in_release": null,
  "ignore_until": null,
  "ignore_until_count": null,
  "grouping_config": "kestrel:v2",
  "unsymbolicated": false,
  "path": "/projects/1/issues/42"
}"#;

const EVENT_EXAMPLE: &str = r#"{
  "event_id": "9ec79c33ec9942ab8353589fcb2e04dc",
  "issue_id": 42,
  "project_id": 1,
  "timestamp": "2026-10-22 09:15:00",
  "received_at": "2026-10-22 09:15:01",
  "level": "error",
  "release": "shop@1.4.1",
  "environment": "production",
  "has_details": true,
  "path": "/projects/1/issues/42/events/9ec79c33ec9942ab8353589fcb2e04dc",
  "payload": {"event_id": "9ec79c33ec9942ab8353589fcb2e04dc", "exception": {"values": [ … ]}, "…": "…"}
}"#;

const RELEASE_EXAMPLE: &str = r#"{
  "id": 7,
  "project_id": 1,
  "version": "shop@1.4.1",
  "first_seen": "2026-10-20 12:00:00",
  "last_seen": "2026-10-22 09:15:00",
  "date_released": "2026-10-20 11:58:00",
  "first_crash_at": null,
  "events": 120,
  "new_issues": 2,
  "health": {"sessions": 5400, "errored": 61, "crashed": 4, "abnormal": 1, "users": 812, "crashed_users": 3,
             "crash_free_sessions": 99.93, "crash_free_users": 99.63},
  "adoption_7d": 64.5,
  "adoption_users_7d": 71.2,
  "path": "/projects/1/releases/7"
}"#;

const HEALTH_EXAMPLE: &str = r#"{
  "release": {"id": 7, "version": "shop@1.4.1"},
  "from": "2026-10-16",
  "to": "2026-10-22",
  "environments": ["production"],
  "totals": {"sessions": 5400, "errored": 61, "crashed": 4, "abnormal": 1, "users": 812, "crashed_users": 3,
             "crash_free_sessions": 99.93, "crash_free_users": 99.63},
  "days": [{"day": "2026-10-16", "sessions": 0, "errored": 0, "crashed": 0, "abnormal": 0, "users": 0,
            "crashed_users": 0, "crash_free_sessions": null, "crash_free_users": null}, "…"]
}"#;

const RULE_EXAMPLE: &str = r#"{
  "id": 3,
  "project_id": 1,
  "name": "Spikes in production",
  "kind": "spike",
  "enabled": true,
  "environment": "",
  "threshold": 100,
  "multiplier": 5.0,
  "cooldown_minutes": 60,
  "digest_minutes": null,
  "channel_ids": [2],
  "created_at": "2026-10-22 08:00:00",
  "updated_at": "2026-10-22 08:00:00"
}"#;

const TIMESERIES_EXAMPLE: &str = r#"{
  "source": "web",
  "period": "7d",
  "from": "2026-10-16",
  "to": "2026-10-22",
  "interval": "day",
  "filters": {"browser": "Firefox"},
  "totals": {"visitors": 812, "pageviews": 2140, "events": 37},
  "current_visitors": 4,
  "data": [{"start": "2026-10-16", "visitors": 101, "pageviews": 288, "events": 5}, "…"]
}"#;

const BREAKDOWN_EXAMPLE: &str = r#"{
  "source": "web",
  "dimension": "referrer",
  "metric": "pageviews",
  "period": "30d",
  "from": "2026-09-23",
  "to": "2026-10-22",
  "filters": {},
  "data": [
    {"value": "", "label": "Direct / none", "visitors": 1204, "count": 3012},
    {"value": "news.ycombinator.com", "label": "news.ycombinator.com", "visitors": 388, "count": 452}
  ]
}"#;

const APP_TIMESERIES_EXAMPLE: &str = r#"{
  "source": "app",
  "period": "7d",
  "from": "2026-10-16",
  "to": "2026-10-22",
  "filters": {"version": null, "platform": "iOS", "type": null, "include_test": false},
  "interval": "day",
  "totals": {"signals": 18240, "sessions": 3920, "user_days": 2610, "dau": 402, "wau": 1310, "mau": 2875},
  "current_users": 12,
  "data": [{"start": "2026-10-16", "users": 371, "wau": 1288, "mau": 2840, "sessions": 552, "signals": 2580}, "…"]
}"#;

const APP_BREAKDOWN_EXAMPLE: &str = r#"{
  "source": "app",
  "dimension": "version",
  "metric": "signals",
  "period": "30d",
  "from": "2026-09-23",
  "to": "2026-10-22",
  "filters": {"version": null, "platform": null, "type": null, "include_test": false},
  "data": [
    {"value": "1.4.0", "label": "1.4.0", "users": 9120, "count": 61230},
    {"value": "1.3.2", "label": "1.3.2", "users": 3010, "count": 18800}
  ]
}"#;

const APP_PAYLOAD_EXAMPLE: &str = r#"{
  "source": "app",
  "type": "Paywall.shown",
  "period": "7d",
  "from": "2026-10-16",
  "to": "2026-10-22",
  "filters": {"version": null, "platform": null, "type": null, "include_test": false},
  "data": [
    {"key": "plan", "values": [{"value": "pro", "label": "pro", "count": 640}, {"value": "free", "label": "free", "count": 212}]},
    {"key": "source", "values": [{"value": "onboarding", "label": "onboarding", "count": 590}, {"value": null, "label": "Other values", "count": 14}]}
  ]
}"#;

const APP_FILTER_Q: &str = "version, platform, type: only signals with this value; test=1: include test signals (debug builds, TestFlight, the simulator). Filters and test signals are counted from the raw signals, which are kept 30 days";

const PERIOD_Q: &str =
    "period: today, yesterday, 7d (default), 30d, any 1d–400d, or custom with from and to";
const FROM_Q: &str = "from, to: YYYY-MM-DD (UTC days, both included; at most 400 days)";
const FILTER_Q: &str = "page, referrer, utm_source, utm_medium, utm_campaign, browser, os, device, screen: only events with this value (counted from the raw events, which are kept 30 days)";

const ERROR_EXAMPLE: &str = r#"{
  "error": {
    "code": "validation_failed",
    "message": "Enter a name.",
    "fields": {"name": "Enter a name."}
  }
}"#;

/// Kestrel's own documented endpoints, in display order.
const OWN_ROUTES: &[RouteDoc] = &[
    // -- Projects --
    r("GET", "/api/v1/projects", "viewer", "Projects", "List projects, oldest first.")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …project… ], "next_cursor": null}"#),
    r("POST", "/api/v1/projects", "editor", "Projects", "Create a project. It gets one enabled key (\"Default\"). The slug is made from the name when you leave it out; platform is apple, javascript, rust, python or other (default); group_id puts it in a project group (default none).")
        .req(r#"{"name": "Sylo for iOS", "platform": "apple", "group_id": 2}"#)
        .res(PROJECT_EXAMPLE),
    r("GET", "/api/v1/projects/{id}", "viewer", "Projects", "Get one project.").res(PROJECT_EXAMPLE),
    r("PATCH", "/api/v1/projects/{id}", "editor", "Projects", "Change a project's name, slug, platform or group (group_id; null takes it out of its group); fields you leave out stay.")
        .req(r#"{"name": "Sylo", "slug": "sylo"}"#)
        .res(PROJECT_EXAMPLE),
    r("DELETE", "/api/v1/projects/{id}", "editor", "Projects", "Delete a project and its keys. Its id is never reused."),
    // -- Project groups --
    r("GET", "/api/v1/project-groups", "viewer", "Project groups", "List the project groups in the project list's order (sort_order, then name), each with how many projects it has.")
        .res(r#"{"data": [ …group… ], "next_cursor": null}"#),
    r("POST", "/api/v1/project-groups", "editor", "Project groups", "Create a group after the others. Names are 1–80 characters and unique regardless of case; an instance has at most 100 groups.")
        .req(r#"{"name": "Sylo"}"#)
        .res(GROUP_EXAMPLE),
    r("PUT", "/api/v1/project-groups/order", "editor", "Project groups", "Put the groups in this order: ids lists every group exactly once. Answers the groups in their new order.")
        .req(r#"{"ids": [4, 2, 7]}"#)
        .res(r#"{"data": [ …group… ], "next_cursor": null}"#),
    r("GET", "/api/v1/project-groups/{id}", "viewer", "Project groups", "Get one group.").res(GROUP_EXAMPLE),
    r("PATCH", "/api/v1/project-groups/{id}", "editor", "Project groups", "Rename a group.")
        .req(r#"{"name": "Sylo apps"}"#)
        .res(GROUP_EXAMPLE),
    r("DELETE", "/api/v1/project-groups/{id}", "editor", "Project groups", "Delete a group. Its projects stay, in no group."),
    // -- Keys --
    r("GET", "/api/v1/projects/{id}/keys", "viewer", "Keys", "List a project's DSN keys, oldest first, each with its DSN.")
        .res(r#"{"data": [ …key… ], "next_cursor": null}"#),
    r("POST", "/api/v1/projects/{id}/keys", "editor", "Keys", "Create a key with a new random public key. A project has at most 20 keys.")
        .req(r#"{"name": "Production", "rate_limit_per_minute": 600, "allowed_origins": ["https://sylo.app"]}"#)
        .res(KEY_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/keys/{key_id}", "viewer", "Keys", "Get one key.").res(KEY_EXAMPLE),
    r("PATCH", "/api/v1/projects/{id}/keys/{key_id}", "editor", "Keys", "Change a key: name, enabled, rate_limit_per_minute (null: the default of 1000 items a minute) or allowed_origins (an empty list allows any origin).")
        .req(r#"{"enabled": false, "rate_limit_per_minute": null}"#)
        .res(KEY_EXAMPLE),
    r("DELETE", "/api/v1/projects/{id}/keys/{key_id}", "editor", "Keys", "Delete a key. SDKs that use its DSN can no longer send."),
    // -- Debug files --
    r("POST", "/api/v1/projects/{id}/dsyms", "editor", "Debug files", "Upload debug files: a zip of dSYMs (or one object file) as the body (Content-Type: application/zip) or as multipart field `file`. Each object (one per architecture) is stored with a SymCache; files in the zip that aren't debug files are listed in `skipped`. Needs a token with the difs:write scope (bound to this project, or to all projects) or a read-and-write token of an editor; a token bound to another project is refused with 403. At most 1 GB per upload and per file.")
        .req("<zip archive>")
        .res(DIF_UPLOAD_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/debug-files", "viewer", "Debug files", "List a project's debug files, oldest first.")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …debug file… ], "next_cursor": null}"#),
    r("DELETE", "/api/v1/projects/{id}/debug-files/{dif_id}", "editor", "Debug files", "Delete a debug file and its SymCache. Events that arrive later for its build stay unsymbolicated."),
    // -- Issues --
    r("GET", "/api/v1/projects/{id}/issues", "viewer", "Issues", "Search a project's issues with the issue list's syntax, newest activity first. `query` takes terms like `is:unresolved level:error release:1.4 env:production os:iOS checkout` (an `is:` in it wins over `status`); terms it can't read are listed in `unknown_terms`.")
        .q(&["query: search terms (also `q`)", "status: unresolved (default), resolved, ignored or all", "sort: last_seen (default), first_seen, events or users", LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …issue… ], "next_cursor": "50", "total": 132, "unknown_terms": []}"#),
    r("GET", "/api/v1/projects/{id}/issues/{issue_id}", "viewer", "Issues", "Get one issue, with its grouping hashes and what went into them (`grouping`).").res(ISSUE_EXAMPLE),
    r("PATCH", "/api/v1/projects/{id}/issues/{issue_id}", "editor", "Issues", "Change an issue's status: `resolved` (with `in_next_release: true`, events from releases first seen after the project's newest one regress it), `unresolved`, or `ignored` (forever, or with `ignore_events` 1–1,000,000 more events or `ignore_hours` 1–8760). Noted in the issue's activity under the token owner's name.")
        .req(r#"{"status": "ignored", "ignore_hours": 24}"#)
        .res(ISSUE_EXAMPLE),
    r("POST", "/api/v1/projects/{id}/issues/merge", "editor", "Issues", "Merge 2–500 issues into the one with the most events: hashes, events, tag values, users and hourly counts move over; the others are deleted. Answers the issue that is left.")
        .req(r#"{"ids": [42, 57]}"#)
        .res(ISSUE_EXAMPLE),
    r("DELETE", "/api/v1/projects/{id}/issues/{issue_id}", "editor", "Issues", "Delete an issue with its events and their details. New events like it start a new issue."),
    r("GET", "/api/v1/projects/{id}/issues/{issue_id}/events", "viewer", "Issues", "An issue's stored events, newest first (without their payloads).")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …event… ], "next_cursor": null, "total": 17}"#),
    r("GET", "/api/v1/projects/{id}/issues/{issue_id}/events/latest", "viewer", "Issues", "The issue's newest event that still has its details, with the normalised payload as stored (already scrubbed at ingest).").res(EVENT_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/events/{event_id}", "viewer", "Issues", "One event by its id (32 hex characters, dashes allowed), with its issue and payload.").res(EVENT_EXAMPLE),
    // -- Releases --
    r("GET", "/api/v1/projects/{id}/releases", "viewer", "Releases", "A project's releases, newest first, each with its events, new issues, all-time health (sessions, crash-free sessions and users), its share of the last 7 days' sessions (`adoption_7d`) and, when KestrelKit sends signals for it (`bundle@version+build` matches the app's bundle id, version and build), its share of the app's daily active users in the last 7 days (`adoption_users_7d`, else `null`).")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …release… ], "next_cursor": null, "total": 12}"#),
    r("GET", "/api/v1/projects/{id}/releases/{release_id}", "viewer", "Releases", "Get one release.").res(RELEASE_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/releases/{release_id}/health", "viewer", "Releases", "A release's health per UTC day and in total: sessions, errored (crashed and abnormal included), crashed, abnormal, distinct users and crashed users, crash-free sessions and users in percent (`null` without sessions).")
        .q(&["days: days up to today (1–400, default 30)", "environment: one environment only"])
        .res(HEALTH_EXAMPLE),
    // -- Analytics --
    r("GET", "/api/v1/projects/{id}/analytics/timeseries", "viewer", "Analytics", "A project's web analytics over a period: totals (unique visitors, the sum of each day's; pageviews; custom events), visitors seen in the last five minutes, and the series by hour (one-day periods) or by UTC day. Without filters it reads the rollups (kept 400 days); with filters the raw events.")
        .q(&[PERIOD_Q, FROM_Q, FILTER_Q])
        .res(TIMESERIES_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/analytics/breakdown", "viewer", "Analytics", "The top values of one dimension over a period, most visitors first: `page`, `entry` (a visitor's first page of the day), `referrer` (the referring domain; \"\" is direct or none), `utm_source`, `utm_medium`, `utm_campaign`, `browser`, `os`, `device` (desktop, mobile, tablet), `screen` (a width bucket) or `event` (custom events by name). `count` is pageviews, entries for `entry`, events for `event` (`metric` says which).")
        .q(&["dimension: which values to count (required)", "limit: values (1–100, default 10)", PERIOD_Q, FROM_Q, FILTER_Q])
        .res(BREAKDOWN_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/analytics/app/timeseries", "viewer", "Analytics", "A project's app analytics (KestrelKit signals) over a period: totals (signals; sessions and users counted per day and added up as `sessions` and `user_days`; daily, weekly and monthly active users on the period's last day as `dau`, `wau`, `mau`), users seen in the last five minutes, and the series by hour (one-day periods) or by UTC day, with each day's `wau` and `mau`. Test signals (debug builds, TestFlight, the simulator) are left out unless `test=1`. Without filters it reads the rollups (kept 400 days); with filters the raw signals (30 days).")
        .q(&[PERIOD_Q, FROM_Q, APP_FILTER_Q])
        .res(APP_TIMESERIES_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/analytics/app/breakdown", "viewer", "Analytics", "The top values of one dimension of app signals over a period, most users first (users counted per day and added up): `type` (the signal type), `version`, `build` (version and build), `release` (`bundle@version+build`, the name Sentry's Apple SDK gives the same release), `platform`, `os` (platform and version), `model` (hardware model) or `locale`. `count` is signals.")
        .q(&["dimension: which values to count (required)", "limit: values (1–100, default 10)", PERIOD_Q, FROM_Q, APP_FILTER_Q])
        .res(APP_BREAKDOWN_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/analytics/app/signal-types", "viewer", "Analytics", "The signal types of a period with their signals and users, most users first.")
        .q(&["limit: types (1–500, default 50)", PERIOD_Q, FROM_Q, APP_FILTER_Q])
        .res(r#"{"source": "app", "period": "7d", "from": "2026-10-16", "to": "2026-10-22", "filters": {"version": null, "platform": null, "type": null, "include_test": false}, "metric": "signals", "data": [{"value": "App.launched", "label": "App.launched", "users": 1310, "count": 4022}, "…"]}"#),
    r("GET", "/api/v1/projects/{id}/analytics/app/payload", "viewer", "Analytics", "A signal type's payload over a period: each key (most signals first) with its top values and their signals. The rollups keep at most 50 values per type, key and day; the rest count as one value with `value: null` (\"Other values\").")
        .q(&["type: the signal type (required)", "limit: values per key (1–100, default 10)", PERIOD_Q, FROM_Q, APP_FILTER_Q])
        .res(APP_PAYLOAD_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/analytics/app/adoption", "viewer", "Analytics", "Version adoption: the versions with the most users over the period, each with its share (percent) of the daily active users on every day of the period and on the last one.")
        .q(&["limit: versions (1–20, default 5)", PERIOD_Q, FROM_Q, APP_FILTER_Q])
        .res(r#"{"source": "app", "period": "30d", "from": "2026-09-23", "to": "2026-10-22", "filters": {"version": null, "platform": null, "type": null, "include_test": false}, "days": ["2026-09-23", "…"], "versions": [{"version": "1.4.0", "users": 21480, "shares": [0.0, "…", 71.2], "latest": 71.2}, "…"]}"#),
    // -- Alerts --
    r("GET", "/api/v1/projects/{id}/alert-rules", "viewer", "Alerts", "A project's alert rules, oldest first.")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [ …alert rule… ], "next_cursor": null}"#),
    r("POST", "/api/v1/projects/{id}/alert-rules", "editor", "Alerts", "Create an alert rule. `kind`: new_issue, regression, spike (more than `threshold` events in an hour and more than `multiplier` times the hourly average of the 7 days before) or first_crash (the first crash of a release). `environment` limits new_issue, regression and first_crash to one environment. An issue alerts a rule at most once per `cooldown_minutes` (0–525600, default 60); with `digest_minutes` (1–1440) its channels get one message for everything in that many minutes. `channel_ids`: at least one notification channel.")
        .req(r#"{"name": "Spikes in production", "kind": "spike", "threshold": 100, "multiplier": 5, "channel_ids": [2]}"#)
        .res(RULE_EXAMPLE),
    r("GET", "/api/v1/projects/{id}/alert-rules/{rule_id}", "viewer", "Alerts", "Get one alert rule.").res(RULE_EXAMPLE),
    r("PATCH", "/api/v1/projects/{id}/alert-rules/{rule_id}", "editor", "Alerts", "Change an alert rule; fields you leave out stay (`digest_minutes: null` turns the digest off).")
        .req(r#"{"enabled": false}"#)
        .res(RULE_EXAMPLE),
    r("DELETE", "/api/v1/projects/{id}/alert-rules/{rule_id}", "editor", "Alerts", "Delete an alert rule. Alerts it queued and didn't send yet are skipped."),
    r("GET", "/api/v1/projects/{id}/alerts", "viewer", "Alerts", "What the project's rules sent, newest first: one entry per alert and channel, with its result (`pending`, `digest`, `sent`, `failed`, `skipped`), attempts and error.")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [{"id": 9, "rule_id": 3, "rule_name": "New issues", "channel_id": 2, "channel_name": "Ops Slack", "issue_id": 42, "kind": "new_issue", "status": "sent", "attempts": 1, "error": null, "created_at": "2026-10-22 08:00:01", "sent_at": "2026-10-22 08:00:02", "summary": "New issue in Shop", "issue_title": "ValueError: bad input"}], "next_cursor": null, "total": 1}"#),
    r("GET", "/api/v1/notification-channels", "editor", "Alerts", "The notification channels rules can send to: id, name, kind, active and language. Their settings (with secrets) are only on the Notifications page.")
        .q(&[LIMIT_Q, CURSOR_Q])
        .res(r#"{"data": [{"id": 2, "name": "Ops Slack", "kind": "slack", "active": true, "language": "", "created_at": "2026-10-22 08:00:00"}], "next_cursor": null}"#),
    // -- Ingest --
    r("POST", "/api/{project_id}/envelope/", "dsn key", "Ingest", "Where the Sentry SDKs send events, sessions, user feedback and attachments: a Sentry envelope (newline-delimited JSON), optionally gzip, deflate, br or zstd encoded (20 MB at most, 1 MB per event, 10 MB per attachment). Authenticated with the project's DSN key in `X-Sentry-Auth: Sentry sentry_key=<public key>, sentry_version=7`, `?sentry_key=` or the envelope header's `dsn`; API tokens don't work here. Answers `{\"id\": \"<event id>\"}` at once; 429 with `Retry-After` and `X-Sentry-Rate-Limits` when a limit or the queue is full. Point an SDK at it with the DSN from the project's settings.")
        .body_type("application/x-sentry-envelope")
        .req("{\"event_id\":\"9ec79c33ec9942ab8353589fcb2e04dc\",\"dsn\":\"https://<public key>@kestrel.example.com/1\"}\n{\"type\":\"event\"}\n{\"message\":\"Hello\",\"level\":\"warning\"}")
        .res(r#"{"id": "9ec79c33ec9942ab8353589fcb2e04dc"}"#),
    r("POST", "/api/{project_id}/store/", "dsn key", "Ingest", "The older endpoint for one event as JSON (old SDKs), with the same authentication and limits as the envelope endpoint.")
        .req(r#"{"message": "Hello", "level": "warning"}"#)
        .res(r#"{"id": "9ec79c33ec9942ab8353589fcb2e04dc"}"#),
    r("POST", "/ingest/v1/event", "public key", "Ingest", "Where the k.js snippet sends pageviews and custom events: one JSON object, as `text/plain` (no CORS preflight) or `application/json` (64 KB at most). `k` (or `key`, `X-Kestrel-Key`, `?key=`) is the project's public key; `n` the event (`pageview` or a custom event's name); `u` the page's URL; `r` the referrer; `w` the screen width; `h: 1` keeps a `#/route` in the path; `p` up to 20 properties of a custom event. The key's allowed origins are checked against `Origin` (else `Referer`); the project must have web analytics on. Bots are answered like everyone else and not counted. Answers `202 Accepted` with an empty body and never sets a cookie; 429 with `Retry-After` when a limit or the queue is full.")
        .body_type("text/plain")
        .req(r#"{"k": "<public key>", "n": "pageview", "u": "https://shop.example/pricing?utm_source=newsletter", "r": "https://news.example/", "w": 1440}"#),
    r("POST", "/ingest/v1/signals", "public key", "Ingest", "Where KestrelKit (and any app SDK that speaks its protocol, v1) sends app signals: `{\"sent_at\", \"sdk\", \"signals\": [...]}` with at most 100 signals and 256 KB (also once gzip is undone), the project's public key in `X-Kestrel-Key`, optionally `Content-Encoding: gzip`. A signal has `type` (1–100 characters from `[A-Za-z0-9._-]`), `time`, `install_id`, `session_id`, `app` (`bundle_id`, `version`, `build`), `device` (`platform`, `os_version`, `model`, `locale`), `flags` (`debug`, `testflight`, `simulator`) and `payload` (at most 20 keys of 40 characters, string values of 200). Signals that break a limit are cut or dropped one by one; `time` is corrected by `sent_at` for the device's clock. Kestrel keeps a keyed hash of the install id, never the id, the IP address or the user agent, and scrubs email addresses, phone and card numbers in payloads (control characters are removed first) and drops payload keys that look like secrets (`password`, `token`, `key`, `card` and the like). Answers `202 {\"accepted\": n}` (and `dropped` when signals were dropped); 400 for a malformed batch, 401 for a missing or unknown key, 403 for a disabled key or app analytics turned off, 413 for too large, 429 (rate limit) and 503 (queue full) with `Retry-After` in seconds.")
        .req(r#"{"sent_at": "2026-10-22T19:00:00.000Z", "sdk": {"name": "kestrelkit", "version": "0.1.0"}, "signals": [{"type": "Paywall.shown", "time": "2026-10-22T18:59:58.123Z", "install_id": "8F3C2B1A-0D4E-4F5A-9B6C-7D8E9F0A1B2C", "session_id": "1B2C3D4E-5F60-4718-8293-A4B5C6D7E8F9", "app": {"bundle_id": "com.example.app", "version": "1.4.0", "build": "57"}, "device": {"platform": "iOS", "os_version": "18.2", "model": "iPhone16,2", "locale": "nb_NO"}, "flags": {"debug": false, "testflight": false, "simulator": false}, "payload": {"plan": "pro"}}]}"#)
        .res(r#"{"accepted": 1}"#),
    r("GET", "/js/k.js", "public", "Ingest", "The web analytics snippet (under 2 KB gzip-compressed): `<script defer src=\"https://kestrel.example.com/js/k.js\" data-key=\"<public key>\"></script>`. Cached for a day, with an ETag."),
    // -- sentry-cli --
    r("GET", "/api/0/", "api token", "sentry-cli", "What sentry-cli's `info` and upload commands ask first: the token's user and scopes (`project:releases` when it may upload debug files and manage releases).")
        .res(r#"{"version": "0", "auth": {"scopes": ["org:read", "project:read", "project:write", "project:releases"]}, "user": {"id": "1", "name": "Ada", "username": "ada", "email": "ada"}}"#),
    r("POST", "/api/0/organizations/{org}/releases/", "api token", "sentry-cli", "`sentry-cli releases new`: creates the release in each project of `projects` (slugs or ids; any organization slug works). Needs a token that may upload debug files.")
        .req(r#"{"version": "shop@1.4.1", "projects": ["shop"]}"#)
        .res(r#"{"version": "shop@1.4.1", "dateCreated": "2026-10-22T08:00:00Z", "dateReleased": null, "newGroups": 0, "projects": [{"id": 1, "slug": "shop", "name": "Shop"}], "…": "…"}"#),
    r("PUT", "/api/0/organizations/{org}/releases/{version}/", "api token", "sentry-cli", "`sentry-cli releases finalize` (sets `dateReleased`) and `set-commits` (commits and refs are accepted and not kept: Kestrel has no repository integration).")
        .req(r#"{"projects": ["shop"], "dateReleased": "2026-10-22T08:05:00Z"}"#),
    r("GET", "/api/0/organizations/{org}/releases/{version}/", "api token", "sentry-cli", "`sentry-cli releases info`: the release across the projects that have it."),
    // -- System --
    r("GET", "/api/v1/openapi.json", "viewer", "System", "This reference as an OpenAPI 3.1 document."),
    r("POST", "/mcp", "api token", "System", "Model Context Protocol server (Streamable HTTP, JSON-RPC 2.0) for AI assistants. Tools: list_projects, list_issues, get_issue, release_health, analytics_summary, analytics_breakdown (web or app analytics); with a read-and-write token of an editor also resolve_issue and ignore_issue. Bearer API token; GET answers 405.")
        .req(r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_issues", "arguments": {"project": "shop", "query": "is:unresolved level:error"}}}"#),
    r("GET", "/healthz", "public", "System", "Liveness probe: always \"ok\"."),
];

/// The three routes of the Hedgerow app, from the kit.
const PUSH_ROUTES: [RouteDoc; 3] = vole_kit::push::api::ROUTE_DOCS;

const ROUTE_COUNT: usize = OWN_ROUTES.len() + PUSH_ROUTES.len();

/// Our routes, then the kit's push routes (the "Push" group comes last).
const fn all_routes() -> [RouteDoc; ROUTE_COUNT] {
    let mut all = [OWN_ROUTES[0]; ROUTE_COUNT];
    let mut i = 0;
    while i < ROUTE_COUNT {
        all[i] = if i < OWN_ROUTES.len() {
            OWN_ROUTES[i]
        } else {
            PUSH_ROUTES[i - OWN_ROUTES.len()]
        };
        i += 1;
    }
    all
}

static ALL_ROUTES: [RouteDoc; ROUTE_COUNT] = all_routes();

/// Every documented route: drives the reference page, OpenAPI and `llms.txt`.
pub static ROUTES: &[RouteDoc] = &ALL_ROUTES;

/// Machine credentials besides API tokens and sessions.
const CREDENTIALS: &[Credential] = &[
    Credential {
        access: "dsn key",
        words: "a project's DSN key (`X-Sentry-Auth: Sentry sentry_key=<public key>`, `?sentry_key=` or the envelope's `dsn` header), not an API token",
        bearer: None,
        header: Some(HeaderScheme {
            name: "sentryAuth",
            header: "X-Sentry-Auth",
            description: "A project's DSN key: `Sentry sentry_key=<public key>, sentry_version=7` (also `?sentry_key=` or the envelope's `dsn` header)",
            example: "Sentry sentry_key=$KK_DSN_KEY, sentry_version=7",
        }),
    },
    Credential {
        access: "public key",
        words: "a project's public key (the DSN key) in `X-Kestrel-Key` (k.js also sends it as `k` in the body), not an API token",
        bearer: None,
        header: Some(HeaderScheme {
            name: "kestrelKey",
            header: "X-Kestrel-Key",
            description: "A project's public key (the DSN key)",
            example: "$KK_DSN_KEY",
        }),
    },
    Credential {
        access: "api token",
        words: "any API token (`Authorization: Bearer kk_…`); a session doesn't work",
        bearer: Some(BearerScheme {
            name: "apiToken",
            description: "API token (kk_…), sessions not accepted",
            variable: "TOKEN",
        }),
        header: None,
    },
];

fn curl_path_param(_route: &RouteDoc, segment: &str) -> Option<String> {
    match segment {
        "{id}" | "{project_id}" => Some("1".into()),
        "{key_id}" => Some("3".into()),
        "{dif_id}" => Some("4".into()),
        "{issue_id}" => Some("42".into()),
        "{event_id}" => Some("9ec79c33ec9942ab8353589fcb2e04dc".into()),
        "{release_id}" => Some("7".into()),
        "{rule_id}" => Some("3".into()),
        "{ref}" => Some("$PUSH_REF".into()),
        "{org}" => Some("any".into()),
        "{version}" => Some("shop@1.4.1".into()),
        _ => None,
    }
}

fn markdown_intro(out: &mut String, base: &str) {
    let version = crate::VERSION;
    let _ = write!(
        out,
        "# Keen Kestrel API\n\n\
         > REST API of a Keen Kestrel instance (version {version}), generated from the server's own route table. \
         Keen Kestrel is a self-hosted, Sentry-compatible error tracker with privacy-friendly web and app analytics: \
         projects and their DSN keys, issues and events, releases and their health, alert rules, debug files, web and app \
         analytics. The official Sentry SDKs, the k.js snippet and KestrelKit send to it (see Ingest).\n\n\
         - Instance: {base}\n\
         - API base URL: {base}/api/v1\n\
         - Browsable reference: {base}/api-docs (sign in) · OpenAPI 3.1: {base}/api/v1/openapi.json\n\
         - The web interface is translated; this document, field names, values and error codes are English.\n\n\
         Examples use two shell variables; never paste a real token into a chat or commit it:\n\n\
         ```bash\n\
         export KK_URL=\"{base}\"\n\
         export KK_TOKEN=\"<API token from Settings → API tokens>\"\n\
         curl -H \"Authorization: Bearer $KK_TOKEN\" \"$KK_URL/api/v1/projects\"\n\
         ```\n\n\
         The ingest examples send a project's public key (its DSN key) as `$KK_DSN_KEY`.\n\n\
         ## Authentication\n\n\
         - Create a token in the web app under **Settings → API tokens** (`/settings/tokens`). Tokens start with `kk_`, \
         are shown once and can expire (30, 90, 365 days or never).\n\
         - Send it on every request: `Authorization: Bearer $KK_TOKEN`.\n\
         - Scope `read` acts as a **viewer** (every GET); scope `write` acts as the token's owner with their role.\n\
         - Roles: **viewer** reads everything; **editor** and **admin** also create, change and delete projects, keys, issues and alert rules.\n\
         - API tokens work only under `/api/` and for `/mcp`; they can't open the web interface.\n\
         - A project's DSN (`https://<public key>@<host>/<project id>`) is not an API token: it can only send events.\n\n\
         ## Conventions\n\n\
         - JSON in and out; send bodies with `Content-Type: application/json`.\n\
         - Lists return `{{\"data\": [...], \"next_cursor\": \"42\" | null}}`. Pass `?limit=` (1–500, default 50) and \
         `?cursor=<next_cursor>` for the next page; stop when `next_cursor` is `null`. Treat the cursor as opaque.\n\
         - GETs send a weak `ETag` and answer `If-None-Match` with `304 Not Modified`.\n\
         - Times are UTC strings `YYYY-MM-DD HH:MM:SS`.\n\
         - `POST` that creates returns `201 Created` with a `Location` header; `DELETE` returns `204 No Content`.\n\
         - `PATCH` changes only the fields you send.\n\
         - Cookie-authenticated writes must come from the app's own pages; token requests are exempt.\n\n"
    );
}

fn markdown_middle(out: &mut String) {
    out.push_str(
        "## Projects and keys\n\n\
         - A project is one app or website. Its integer `id` is part of every DSN and is never reused.\n\
         - `slug`: lowercase letters, digits and dashes (at most 50), unique; `sentry-cli` can use it as the project.\n\
         - `platform`: `apple`, `javascript`, `rust`, `python` or `other`. It only picks the setup guide shown first.\n\
         - `group_id`: the project group it's in (`null`: none). Groups are sections of the project list, in their `sort_order` \
         (then name); a project is in at most one, groups don't nest, and deleting a group leaves its projects in none.\n\
         - A key has a random 32-character hex `public_key` and a `dsn` built from the instance's public URL.\n\
         - `enabled: false` makes the key's DSN refused; `rate_limit_per_minute` (1–1,000,000 or `null` for the default of 1000) caps the items (events, sessions, pageviews, signals) it may send per minute; \
         `allowed_origins` lists the browser origins (`https://example.com`, or `*`) that may use it, and an empty list allows any.\n\n\
         ## Debug files\n\n\
         - Debug files (dSYMs) let Kestrel symbolicate native crashes: function, file and line instead of image and offset. \
         Each object (one per debug id and architecture) is kept with a SymCache made from it.\n\
         - `sentry-cli debug-files upload` works unchanged: `sentry-cli --url <this instance> debug-files upload --org any \
         --project <slug or id> <path>`, with `SENTRY_AUTH_TOKEN` (or `--auth-token`) set to an API token. Its endpoints are \
         `GET /api/0/organizations/{org}/chunk-upload/`, `POST` to the same path, `POST /api/0/projects/{org}/{project}/files/difs/assemble/` \
         and the older `POST /api/0/projects/{org}/{project}/files/dsyms/`; any organization slug is accepted.\n\
         - Uploading needs a token with the scope `difs:write` (made on the project's Debug files page, where it is bound to one project; it only reads otherwise) or a \
         read-and-write token, and the token's owner must be an editor or admin. A bound token uploads to its own project and creates and changes releases there only; \
         any other project answers 403. Administrators can also make a token for all projects (discouraged); tokens made before binding existed work for all projects, \
         and a `difs:write` token without a project (made under Settings → API tokens) is refused.\n\
         - A debug file applies to events that arrive after it; stored events are not symbolicated again.\n\n\
         ## Issues, events and releases\n\n\
         - An issue is a group of events that share a grouping hash (configuration `kestrel:v2`; issues made by `kestrel:v1` keep it). \
         `status` is `unresolved`, `resolved` or `ignored`; `substatus` `new`, `regressed` or `ongoing`. \
         `events` and `users` count every event, stored or not; at most 1000 events per issue and day keep their details.\n\
         - The search syntax: `is:unresolved|resolved|ignored|all|new|regressed|ongoing|unsymbolicated`, `level:`, `release:` \
         (as sent, or only the version; `*` is a wildcard), `env:`, `os:`, `browser:`, `device:`, `runtime:`, any other `key:value` tag, \
         `!` to negate, double quotes for values with spaces; other words search titles and culprits; an event id finds its issue.\n\
         - Event payloads are the normalised events as stored: IP addresses are never kept, secrets and card numbers are `[Filtered]`, \
         user ids, emails and names are keyed hashes unless the project keeps them.\n\
         - Releases are created on first sight (an event or a session names one) or by `sentry-cli releases new`. Health comes from the \
         SDKs' sessions, rolled up per release, environment and day: crash-free sessions are `1 − crashed / sessions`, crash-free users \
         `1 − crashed users / users` (users are distinct device ids per day, so over several days they add up).\n\n\
         ## Web analytics\n\n\
         - Sites send pageviews and custom events with the k.js snippet (`GET /js/k.js`, with `data-key` set to a public key \
         of the project) to `POST /ingest/v1/event`. No cookies; IP addresses and user agents are never stored.\n\
         - A visitor is a hash of a salt that changes at midnight UTC, the project, the IP address, the user agent and the \
         host, so unique visitors are counted per UTC day; over a period they are the sum of the days.\n\
         - Paths are kept without their query (UTM parameters apart), referrers as their domain; email addresses, phone \
         numbers and card numbers in paths, UTM values and custom event names and properties are `[Filtered]`.\n\
         - Raw events are kept 30 days and rolled up every minute into hourly and daily counts kept 400 days (per project: \
         Settings → Retention). Queries without filters read the rollups; with filters, the raw events.\n\n\
         ## App analytics\n\n\
         - Apps send signals with the KestrelKit Swift package (`Kestrel.configure(url:key:)` with a public key of the \
         project) to `POST /ingest/v1/signals`. Kestrel keeps `HMAC(project secret, install id)`, cut to 64 bits, never the \
         install id, the IP address or the user agent; in a project's strict mode the key changes every UTC day.\n\
         - `dau`, `wau` and `mau` are distinct users on a day and in the 7 and 30 days up to it (in strict mode, the days \
         added up). Sessions are distinct session ids per day.\n\
         - Test signals (debug builds, TestFlight, the simulator) are kept raw but left out of the rollups; `test=1` \
         counts them from the raw signals.\n\
         - A release `bundle@version+build` matches the app's bundle id, version and build, so the releases show their \
         share of the app's users (`adoption_users_7d`).\n\n\
         ## Alerts\n\n\
         - Alert rules belong to a project and send to the instance's notification channels (Settings → Notifications: Slack, Discord, \
         Teams, Telegram, ntfy, email, webhooks, PagerDuty and about 30 more).\n\
         - Rules are evaluated after events are written and messages are sent by a task of their own, retried after 1, 5 and 15 minutes; \
         `GET …/alerts` shows each result. Messages link to the issue when the instance's public URL is set (Settings → General, or `KK_PUBLIC_URL`).\n\n",
    );
}

fn nothing(_: &mut String) {}

/// What Kestrel contributes to the kit's API reference.
pub static DOCS: DocsSpec = DocsSpec {
    routes: ROUTES,
    openapi_title: "Keen Kestrel API",
    openapi_description: "REST API for Keen Kestrel. Authenticate with `Authorization: Bearer kk_…` (create tokens under Settings → API tokens). Read-only tokens act as viewers. Errors use `{\"error\": {\"code\", \"message\", \"fields\"}}`; lists use `?limit=&cursor=` and return `next_cursor`.",
    bearer_description: "API token (kk_…)",
    // DSN keys only ingest; sentry-cli's routes and MCP take API tokens
    // without sessions.
    credentials: CREDENTIALS,
    error_example: ERROR_EXAMPLE,
    error_codes: ERROR_CODES,
    config_document: None,
    curl_path_param,
    markdown_intro,
    markdown_middle,
    markdown_outro: nothing,
    page_subtitle: "api.docs_subtitle",
    openapi_path: "/api/v1/openapi.json",
    api_base: "/api/v1",
    example_path: "/api/v1/projects",
    metrics_path: None,
    fallback_host: Some("localhost:3002"),
    // Path parameters that aren't integers; the others (`{id}`,
    // `{issue_id}` …) are.
    param_types: &[
        ("event_id", "string"),
        ("org", "string"),
        ("version", "string"),
        ("debug_id", "string"),
        ("project", "string"),
    ],
    // Every route says who may call it: a role, `public`, `dsn key`,
    // `public key` or `api token`.
    strict_access: true,
};

/// Kestrel's wording of the kit's `conflict` row (what clashes here), and
/// the codes of the Hedgerow app's routes.
const ERROR_CODES: &[ErrorCode] = &[
    ErrorCode {
        code: "conflict",
        status: 409,
        when: "another project already uses the slug, the project already has 20 keys, another project group has the name, or there are already 100 groups",
        on_page: true,
    },
    ErrorCode {
        code: "push_disabled",
        status: 409,
        when: "registering a Hedgerow app device while no active Hedgerow app notification channel exists",
        on_page: true,
    },
    ErrorCode {
        code: "too_many_devices",
        status: 409,
        when: "registering a Hedgerow app device when the user (16) or the server (256) has the most allowed",
        on_page: false,
    },
    ErrorCode {
        code: "too_many_pending",
        status: 409,
        when: "registering a Hedgerow app device while 4 of the user's devices (20 on the server) have not received a push yet",
        on_page: false,
    },
    ErrorCode {
        code: "rate_limited",
        status: 429,
        when: "registering more than 4 new Hedgerow app devices in an hour",
        on_page: false,
    },
];

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Protected routes (behind `require_auth`), nested under `/api/v1`.
pub fn router() -> Router<Core> {
    let api = Router::new()
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/projects/{id}",
            get(show_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route("/project-groups", get(groups::list).post(groups::create))
        .route("/project-groups/order", axum::routing::put(groups::reorder))
        .route(
            "/project-groups/{id}",
            get(groups::show)
                .patch(groups::rename)
                .delete(groups::delete),
        )
        .route("/projects/{id}/keys", get(list_keys).post(create_key))
        .route(
            "/projects/{id}/keys/{key_id}",
            get(show_key).patch(update_key).delete(delete_key),
        )
        .route(
            "/projects/{id}/dsyms",
            axum::routing::post(crate::difs::upload::v1_upload)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/projects/{id}/debug-files",
            get(crate::difs::upload::v1_list),
        )
        .route(
            "/projects/{id}/debug-files/{dif_id}",
            axum::routing::delete(crate::difs::upload::v1_delete),
        )
        .route("/projects/{id}/issues", get(issues::list))
        .route("/projects/{id}/issues/merge", post(issues::merge))
        .route(
            "/projects/{id}/issues/{issue_id}",
            get(issues::show)
                .patch(issues::update)
                .delete(issues::delete),
        )
        .route(
            "/projects/{id}/issues/{issue_id}/events",
            get(issues::events),
        )
        .route(
            "/projects/{id}/issues/{issue_id}/events/latest",
            get(issues::latest_event),
        )
        .route("/projects/{id}/events/{event_id}", get(issues::event))
        .route("/projects/{id}/releases", get(releases::list))
        .route("/projects/{id}/releases/{release_id}", get(releases::show))
        .route(
            "/projects/{id}/releases/{release_id}/health",
            get(releases::show_health),
        )
        .route(
            "/projects/{id}/analytics/timeseries",
            get(analytics::timeseries),
        )
        .route(
            "/projects/{id}/analytics/breakdown",
            get(analytics::breakdown),
        )
        .route(
            "/projects/{id}/analytics/app/timeseries",
            get(app::timeseries),
        )
        .route(
            "/projects/{id}/analytics/app/breakdown",
            get(app::breakdown),
        )
        .route(
            "/projects/{id}/analytics/app/signal-types",
            get(app::signal_types),
        )
        .route("/projects/{id}/analytics/app/payload", get(app::payload))
        .route("/projects/{id}/analytics/app/adoption", get(app::adoption))
        .route(
            "/projects/{id}/alert-rules",
            get(rules::list).post(rules::create),
        )
        .route(
            "/projects/{id}/alert-rules/{rule_id}",
            get(rules::show).patch(rules::update).delete(rules::delete),
        )
        .route("/projects/{id}/alerts", get(rules::history))
        .route("/notification-channels", get(rules::channels))
        .fallback(fallback)
        .route_layer(middleware::from_fn(
            vole_kit::http::rest::session_write_guard,
        ));
    // The Hedgerow app's three routes (devices and push texts), with their
    // full paths. The kit's handlers serve every signed-in caller, read-only
    // tokens included; `ROUTES` documents them. A session needs to come
    // from our own pages for the writes, as for every other route here.
    let push = vole_kit::push::api::router::<Core>().route_layer(middleware::from_fn(
        vole_kit::http::rest::session_write_guard,
    ));
    Router::new().nest("/api/v1", api).merge(push)
}

async fn fallback() -> V1Error {
    V1Error::not_found()
}

pub(crate) fn require_editor(user: &CurrentUser) -> V1Result<()> {
    user.require_editor().map_err(V1Error::from)
}

/// A validation error on `field`, in the request's language.
fn invalid(field: &str, key: &'static str) -> V1Error {
    V1Error::field(field, i18n::t(key))
}

pub fn project_json(p: &Project) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "slug": p.slug,
        "platform": p.platform.as_str(),
        "group_id": p.group_id,
        "created_at": p.created_at,
    })
}

fn key_json(k: &ProjectKey, base: &str) -> Value {
    json!({
        "id": k.id,
        "project_id": k.project_id,
        "name": k.name,
        "public_key": k.public_key,
        "dsn": dsn::dsn(base, &k.public_key, k.project_id),
        "enabled": k.enabled,
        "rate_limit_per_minute": k.rate_limit_per_minute,
        "allowed_origins": k.allowed_origins,
        "created_at": k.created_at,
    })
}

/// The `id` of a JSON object (for paging).
fn json_id(v: &Value) -> i64 {
    v["id"].as_i64().unwrap_or_default()
}

async fn load(core: &Core, id: i64) -> V1Result<Project> {
    core.with_conn(move |c| db::get(c, id))
        .await?
        .ok_or_else(V1Error::not_found)
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_projects(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Params(q): Params<ListQuery>,
) -> V1Result<Response> {
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let projects = core.with_conn(|c| db::list(c)).await?;
    let data: Vec<Value> = projects.iter().map(project_json).collect();
    let page = paginate(data, json_id, cursor, limit(q.limit));
    Ok(json_ok(&headers, &page))
}

async fn show_project(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
) -> V1Result<Response> {
    let project = load(&core, id).await?;
    Ok(json_ok(&headers, &project_json(&project)))
}

/// `null` and a missing field differ: `Some(None)` clears.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectInput {
    name: Option<String>,
    slug: Option<String>,
    platform: Option<String>,
    /// `null` takes the project out of its group; missing keeps it.
    #[serde(default, deserialize_with = "double_option")]
    group_id: Option<Option<i64>>,
}

fn parse_platform(raw: Option<&str>) -> V1Result<Option<Platform>> {
    raw.map(|p| {
        Platform::parse(p).ok_or_else(|| {
            V1Error::field(
                "platform",
                "Use one of: apple, javascript, rust, python, other.",
            )
        })
    })
    .transpose()
}

async fn create_project(
    State(core): State<Core>,
    user: CurrentUser,
    Body(input): Body<ProjectInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let name = project::check_name(input.name.as_deref().unwrap_or_default())
        .map_err(|k| invalid("name", k))?;
    let platform = parse_platform(input.platform.as_deref())?.unwrap_or(Platform::Other);
    let slug = input
        .slug
        .as_deref()
        .map(|s| project::check_slug(s).map_err(|k| invalid("slug", k)))
        .transpose()?;
    let group_id = groups::check_group(&core, input.group_id.flatten()).await?;
    let key_name = i18n::t("keys.default_name");
    let stored = name.clone();
    let created_project = core
        .with_conn(move |c| {
            let slug = match slug {
                Some(slug) if db::slug_taken(c, &slug, None)? => return Ok(None),
                Some(slug) => slug,
                None => db::free_slug(c, &project::slugify(&stored))?,
            };
            let id = db::insert(c, &stored, &slug, platform, key_name)?;
            db::set_group(c, id, group_id)?;
            db::get(c, id)
        })
        .await?;
    let Some(p) = created_project else {
        return Err(V1Error::conflict(i18n::t("projects.err_slug_taken")));
    };
    tracing::info!(user = %user.username, project = %name, id = p.id, "project created (API)");
    Ok(created(
        format!("/api/v1/projects/{}", p.id),
        &project_json(&p),
    ))
}

async fn update_project(
    State(core): State<Core>,
    user: CurrentUser,
    Id(id): Id<i64>,
    Body(input): Body<ProjectInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let current = load(&core, id).await?;
    let name = match input.name.as_deref() {
        Some(n) => project::check_name(n).map_err(|k| invalid("name", k))?,
        None => current.name,
    };
    let slug = match input.slug.as_deref() {
        Some(s) => project::check_slug(s).map_err(|k| invalid("slug", k))?,
        None => current.slug,
    };
    let platform = parse_platform(input.platform.as_deref())?.unwrap_or(current.platform);
    let group_id = match input.group_id {
        Some(g) => groups::check_group(&core, g).await?,
        None => current.group_id,
    };
    let updated = core
        .with_conn(move |c| {
            if db::slug_taken(c, &slug, Some(id))? {
                return Ok(None);
            }
            db::update(c, id, &name, &slug, platform)?;
            db::set_group(c, id, group_id)?;
            db::get(c, id)
        })
        .await?;
    let Some(p) = updated else {
        return Err(V1Error::conflict(i18n::t("projects.err_slug_taken")));
    };
    Ok(axum::Json(project_json(&p)).into_response())
}

async fn delete_project(
    State(_core): State<Core>,
    Extension(ingest): Extension<crate::ingest::Ingest>,
    user: CurrentUser,
    Id(id): Id<i64>,
) -> V1Result<Response> {
    require_editor(&user)?;
    // On the writer's connection, so the store's rows go too.
    let found = ingest
        .writer()
        .run(move |c| {
            let tx = c.transaction()?;
            let found = crate::db::triage::delete_project(&tx, id)?;
            tx.commit()?;
            Ok(found)
        })
        .await
        .map_err(vole_kit::http::ApiError::internal)?;
    if !found {
        return Err(V1Error::not_found());
    }
    ingest.difs().forget_project(id);
    tracing::info!(user = %user.username, id, "project deleted (API)");
    Ok(no_content())
}

async fn list_keys(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Params(q): Params<ListQuery>,
) -> V1Result<Response> {
    load(&core, id).await?;
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let keys = core.with_conn(move |c| db::keys(c, id)).await?;
    let base = dsn::public_base(core.base_url().await, &headers);
    let data: Vec<Value> = keys.iter().map(|k| key_json(k, &base)).collect();
    let page = paginate(data, json_id, cursor, limit(q.limit));
    Ok(json_ok(&headers, &page))
}

async fn show_key(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    Id((id, key_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    let key = core
        .with_conn(move |c| db::get_key(c, id, key_id))
        .await?
        .ok_or_else(V1Error::not_found)?;
    let base = dsn::public_base(core.base_url().await, &headers);
    Ok(json_ok(&headers, &key_json(&key, &base)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInput {
    name: Option<String>,
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    rate_limit_per_minute: Option<Option<i64>>,
    allowed_origins: Option<Vec<String>>,
}

fn key_changes(input: KeyInput) -> V1Result<KeyChanges> {
    Ok(KeyChanges {
        name: input
            .name
            .as_deref()
            .map(|n| project::check_name(n).map_err(|k| invalid("name", k)))
            .transpose()?,
        enabled: input.enabled,
        rate_limit_per_minute: input
            .rate_limit_per_minute
            .map(|l| project::check_rate_limit(l).map_err(|k| invalid("rate_limit_per_minute", k)))
            .transpose()?,
        allowed_origins: input
            .allowed_origins
            .as_ref()
            .map(|o| {
                project::check_origins(o.iter().map(String::as_str))
                    .map_err(|k| invalid("allowed_origins", k))
            })
            .transpose()?,
    })
}

async fn create_key(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Body(input): Body<KeyInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    load(&core, id).await?;
    let mut changes = key_changes(input)?;
    let name = changes
        .name
        .take()
        .unwrap_or_else(|| i18n::t("keys.default_name").to_string());
    let key = core
        .with_conn(move |c| {
            if db::keys(c, id)?.len() >= project::MAX_KEYS {
                return Ok(None);
            }
            let key_id = db::insert_key(c, id, &name)?;
            db::update_key(c, id, key_id, &changes)?;
            db::get_key(c, id, key_id)
        })
        .await?;
    let Some(key) = key else {
        return Err(V1Error::conflict(i18n::t("keys.err_too_many")));
    };
    let base = dsn::public_base(core.base_url().await, &headers);
    Ok(created(
        format!("/api/v1/projects/{id}/keys/{}", key.id),
        &key_json(&key, &base),
    ))
}

async fn update_key(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    Id((id, key_id)): Id<(i64, i64)>,
    Body(input): Body<KeyInput>,
) -> V1Result<Response> {
    require_editor(&user)?;
    let changes = key_changes(input)?;
    let key = core
        .with_conn(move |c| {
            if !db::update_key(c, id, key_id, &changes)? {
                return Ok(None);
            }
            db::get_key(c, id, key_id)
        })
        .await?
        .ok_or_else(V1Error::not_found)?;
    let base = dsn::public_base(core.base_url().await, &headers);
    Ok(axum::Json(key_json(&key, &base)).into_response())
}

async fn delete_key(
    State(core): State<Core>,
    user: CurrentUser,
    Id((id, key_id)): Id<(i64, i64)>,
) -> V1Result<Response> {
    require_editor(&user)?;
    if !core
        .with_conn(move |c| db::delete_key(c, id, key_id))
        .await?
    {
        return Err(V1Error::not_found());
    }
    Ok(no_content())
}
