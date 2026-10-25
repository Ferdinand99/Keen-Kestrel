//! The REST API v1 and the MCP tools for errors (docs/design.md §6): issues
//! with the search syntax, status changes, merge and delete, events,
//! releases, alert rules; who may do what (read and write tokens, viewers
//! and editors); the API reference with the ingest endpoints; and what the
//! MCP tools show (never user data).

mod common;

use axum::http::StatusCode;
use common::{App, Res, add_user, api_token, bearer, envelope_request, envelope_with, new_id};
use serde_json::{Value, json};
use vole_kit::auth::TokenScope;

fn event(ty: &str, level: &str, env: &str) -> Value {
    json!({
        "event_id": new_id(),
        "platform": "python",
        "level": level,
        "release": "shop@1.0",
        "environment": env,
        "user": {"id": "user-ada-42", "email": "ada.lovelace@example.com"},
        "breadcrumbs": {"values": [{"category": "http", "message": "GET /cart 500", "timestamp": 1761120000}]},
        "exception": {"values": [{"type": ty, "value": "boom", "stacktrace": {"frames": [
            {"function": "dispatch", "module": "flask.app", "abs_path": "/venv/lib/python3.13/site-packages/flask/app.py", "lineno": 10},
            {"function": "checkout", "module": "shop.views", "abs_path": "/srv/shop/views.py", "filename": "shop/views.py", "lineno": 42,
             "context_line": "    total = cart.total()"}]}}]},
    })
}

async fn send(app: &App, e: &Value) {
    let res = app
        .send(envelope_request(app.project_id, &app.key, envelope_with(e)))
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
}

struct Tokens {
    viewer: String,
    editor_read: String,
    editor_write: String,
    viewer_write: String,
}

async fn tokens(app: &App) -> Tokens {
    let viewer = add_user(app, "vera", "viewer").await;
    let editor = add_user(app, "edda", "editor").await;
    Tokens {
        viewer: api_token(app, viewer, TokenScope::Read).await,
        viewer_write: api_token(app, viewer, TokenScope::Write).await,
        editor_read: api_token(app, editor, TokenScope::Read).await,
        editor_write: api_token(app, editor, TokenScope::Write).await,
    }
}

async fn seed(app: &App) {
    send(app, &event("ValueError", "error", "production")).await;
    send(app, &event("ValueError", "error", "production")).await;
    send(app, &event("KeyError", "warning", "staging")).await;
    send(app, &event("TypeError", "fatal", "production")).await;
    app.flush().await;
}

fn titles(res: &Res) -> Vec<String> {
    res.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn issues_are_searched_read_and_triaged_through_the_api() {
    let app = App::new().await;
    let t = tokens(&app).await;
    seed(&app).await;
    let p = app.project_id;
    let base = format!("/api/v1/projects/{p}/issues");

    let all = bearer(&app, "GET", &base, &t.viewer, None).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);
    assert_eq!(all.json()["total"], 3);
    assert!(all.header("etag").starts_with("W/"));
    // The search syntax, as on the issue list.
    let res = bearer(
        &app,
        "GET",
        &format!("{base}?query=level:error"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(titles(&res), ["ValueError: boom"]);
    let res = bearer(
        &app,
        "GET",
        &format!("{base}?query=env:staging"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(titles(&res), ["KeyError: boom"]);
    let res = bearer(
        &app,
        "GET",
        &format!("{base}?q=!level:error%20boom&sort=events"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(res.json()["total"], 2);
    let res = bearer(
        &app,
        "GET",
        &format!("{base}?query=nonsense:"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let res = bearer(
        &app,
        "GET",
        &format!("{base}?status=bogus"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    // Paging.
    let page1 = bearer(&app, "GET", &format!("{base}?limit=2"), &t.viewer, None).await;
    assert_eq!(page1.json()["data"].as_array().unwrap().len(), 2);
    let cursor = page1.json()["next_cursor"].as_str().unwrap().to_string();
    let page2 = bearer(
        &app,
        "GET",
        &format!("{base}?limit=2&cursor={cursor}"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(page2.json()["data"].as_array().unwrap().len(), 1);
    assert_eq!(page2.json()["next_cursor"], Value::Null);

    let value_error = all.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "ValueError: boom")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let one = bearer(
        &app,
        "GET",
        &format!("{base}/{value_error}"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(one.json()["events"], 2);
    assert_eq!(one.json()["grouping"][0]["config"], "kestrel:v2");

    // Events: the list, the latest with its payload, one by id.
    let events = bearer(
        &app,
        "GET",
        &format!("{base}/{value_error}/events"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(events.json()["total"], 2);
    let latest = bearer(
        &app,
        "GET",
        &format!("{base}/{value_error}/events/latest"),
        &t.viewer,
        None,
    )
    .await;
    let latest = latest.json();
    assert_eq!(
        latest["payload"]["exception"]["values"][0]["type"],
        "ValueError"
    );
    assert!(!latest.to_string().contains("ada.lovelace@example.com"));
    let event_id = latest["event_id"].as_str().unwrap();
    let by_id = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/events/{event_id}"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(by_id.json()["issue_id"], value_error);
    assert_eq!(
        bearer(
            &app,
            "GET",
            &format!("/api/v1/projects/{p}/events/{}", "0".repeat(32)),
            &t.viewer,
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    // Changing needs a write token of an editor.
    let patch = |status: Value| Some(status);
    for token in [&t.viewer, &t.viewer_write, &t.editor_read] {
        let res = bearer(
            &app,
            "PATCH",
            &format!("{base}/{value_error}"),
            token,
            patch(json!({"status": "resolved"})),
        )
        .await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    }
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{value_error}"),
        &t.editor_write,
        patch(json!({"status": "resolved"})),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["status"], "resolved");
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{value_error}"),
        &t.editor_write,
        patch(json!({"status": "ignored", "ignore_hours": 24})),
    )
    .await;
    assert_eq!(res.json()["status"], "ignored");
    assert!(res.json()["ignore_until"].is_string());
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{value_error}"),
        &t.editor_write,
        patch(json!({"status": "ignored", "ignore_events": 100})),
    )
    .await;
    assert_eq!(res.json()["ignore_until_count"], 102);
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{value_error}"),
        &t.editor_write,
        patch(json!({"status": "ignored", "ignore_hours": 0})),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.json()["error"]["fields"]["ignore_hours"].is_string());
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{value_error}"),
        &t.editor_write,
        patch(json!({"status": "unresolved"})),
    )
    .await;
    assert_eq!(res.json()["status"], "unresolved");
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{value_error}"),
        &t.editor_write,
        patch(json!({"status": "resolved", "in_next_release": true})),
    )
    .await;
    assert_eq!(res.json()["resolved_in_release"], "shop@1.0");
    // The activity names the token's owner.
    assert!(
        app.count("SELECT COUNT(*) FROM issue_activity WHERE actor = 'edda'")
            .await
            >= 5
    );

    // Merge two, then delete what's left.
    let ids: Vec<i64> = all.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["title"] != "ValueError: boom")
        .map(|i| i["id"].as_i64().unwrap())
        .collect();
    let res = bearer(
        &app,
        "POST",
        &format!("{base}/merge"),
        &t.editor_write,
        Some(json!({"ids": [ids[0]]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    let res = bearer(
        &app,
        "POST",
        &format!("{base}/merge"),
        &t.editor_write,
        Some(json!({"ids": ids})),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let left = res.json()["id"].as_i64().unwrap();
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 2);
    let res = bearer(&app, "DELETE", &format!("{base}/{left}"), &t.viewer, None).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = bearer(
        &app,
        "DELETE",
        &format!("{base}/{left}"),
        &t.editor_write,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    assert_eq!(app.count("SELECT COUNT(*) FROM issues").await, 1);
    assert_eq!(
        bearer(&app, "GET", &format!("{base}/{left}"), &t.viewer, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn alert_rules_are_managed_through_the_api() {
    let app = App::new().await;
    let t = tokens(&app).await;
    let p = app.project_id;
    let channel = app
        .kestrel
        .core
        .with_conn(|c| {
            keen_kestrel::db::alerts::insert_channel(
                c,
                "Ops",
                "webhook",
                r#"{"url":"https://hooks.example/secret-hook-token"}"#,
                "",
            )
        })
        .await
        .unwrap();
    let base = format!("/api/v1/projects/{p}/alert-rules");
    let body = json!({"name": "Spikes", "kind": "spike", "threshold": 50, "multiplier": 3, "channel_ids": [channel]});
    let res = bearer(&app, "POST", &base, &t.viewer, Some(body.clone())).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = bearer(&app, "POST", &base, &t.editor_write, Some(body)).await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let rule = res.json();
    assert_eq!(rule["kind"], "spike");
    assert_eq!(rule["threshold"], 50);
    assert_eq!(rule["cooldown_minutes"], 60);
    assert_eq!(rule["channel_ids"], json!([channel]));
    let id = rule["id"].as_i64().unwrap();
    assert_eq!(res.header("location"), format!("{base}/{id}"));
    // Validation: every problem by field.
    let res = bearer(
        &app,
        "POST",
        &base,
        &t.editor_write,
        Some(json!({"name": "", "kind": "new_issue", "cooldown_minutes": -1, "channel_ids": []})),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    let fields = res.json()["error"]["fields"].clone();
    for f in ["name", "cooldown_minutes", "channels"] {
        assert!(fields[f].is_string(), "{f}: {fields}");
    }
    let res = bearer(
        &app,
        "POST",
        &base,
        &t.editor_write,
        Some(json!({"name": "X", "kind": "sometimes", "channel_ids": [channel]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    let res = bearer(
        &app,
        "POST",
        &base,
        &t.editor_write,
        Some(json!({"name": "X", "channel_ids": [9999]})),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    // Update, read, list, delete.
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{id}"),
        &t.editor_write,
        Some(json!({"enabled": false, "digest_minutes": 15})),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["enabled"], false);
    assert_eq!(res.json()["digest_minutes"], 15);
    assert_eq!(res.json()["threshold"], 50, "unchanged");
    let res = bearer(
        &app,
        "PATCH",
        &format!("{base}/{id}"),
        &t.editor_write,
        Some(json!({"digest_minutes": null})),
    )
    .await;
    assert_eq!(res.json()["digest_minutes"], Value::Null);
    let list = bearer(&app, "GET", &base, &t.viewer, None).await;
    assert_eq!(list.json()["data"].as_array().unwrap().len(), 1);
    // Channels: names and kinds for editors, never the settings. Read
    // tokens act as viewers.
    for token in [&t.viewer, &t.editor_read] {
        let res = bearer(&app, "GET", "/api/v1/notification-channels", token, None).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN);
    }
    let res = bearer(
        &app,
        "GET",
        "/api/v1/notification-channels",
        &t.editor_write,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["data"][0]["kind"], "webhook");
    assert!(!res.body.contains("secret-hook-token"));
    let res = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{p}/alerts"),
        &t.viewer,
        None,
    )
    .await;
    assert_eq!(res.json()["total"], 0);
    let res = bearer(
        &app,
        "DELETE",
        &format!("{base}/{id}"),
        &t.editor_write,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    assert_eq!(
        bearer(&app, "GET", &format!("{base}/{id}"), &t.viewer, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_reference_documents_errors_and_ingest() {
    let app = App::new().await;
    let t = tokens(&app).await;
    let doc = bearer(&app, "GET", "/api/v1/openapi.json", &t.viewer, None).await;
    let doc = doc.json();
    for (path, method) in [
        ("/api/v1/projects/{id}/issues", "get"),
        ("/api/v1/projects/{id}/issues/{issue_id}", "patch"),
        ("/api/v1/projects/{id}/issues/merge", "post"),
        ("/api/v1/projects/{id}/issues/{issue_id}", "delete"),
        ("/api/v1/projects/{id}/issues/{issue_id}/events", "get"),
        ("/api/v1/projects/{id}/events/{event_id}", "get"),
        ("/api/v1/projects/{id}/releases", "get"),
        ("/api/v1/projects/{id}/releases/{release_id}/health", "get"),
        ("/api/v1/projects/{id}/alert-rules", "post"),
        ("/api/v1/projects/{id}/alert-rules/{rule_id}", "patch"),
        ("/api/{project_id}/envelope/", "post"),
        ("/api/{project_id}/store/", "post"),
    ] {
        assert!(doc["paths"][path][method].is_object(), "{method} {path}");
    }
    // Ingest takes a DSN key in a header of its own, not an API token.
    let envelope = &doc["paths"]["/api/{project_id}/envelope/"]["post"];
    assert_eq!(envelope["security"], json!([{"sentryAuth": []}]));
    assert_eq!(
        doc["components"]["securitySchemes"]["sentryAuth"],
        json!({"type": "apiKey", "in": "header", "name": "X-Sentry-Auth", "description": doc["components"]["securitySchemes"]["sentryAuth"]["description"]})
    );
    assert!(
        envelope["requestBody"]["content"]["application/x-sentry-envelope"]["example"]
            .as_str()
            .unwrap()
            .contains("\n{\"type\":\"event\"}\n")
    );
    let event = &doc["paths"]["/ingest/v1/event"]["post"];
    assert_eq!(event["security"], json!([{"kestrelKey": []}]));
    assert!(event["requestBody"]["content"]["text/plain"].is_object());
    assert_eq!(
        doc["paths"]["/ingest/v1/signals"]["post"]["security"],
        json!([{"kestrelKey": []}])
    );
    // Path parameters that aren't numbers say so.
    let param = |path: &str, method: &str, name: &str| {
        doc["paths"][path][method]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()["schema"]["type"]
            .clone()
    };
    assert_eq!(
        param("/api/v1/projects/{id}/events/{event_id}", "get", "event_id"),
        "string"
    );
    assert_eq!(
        param("/api/v1/projects/{id}/events/{event_id}", "get", "id"),
        "integer"
    );
    assert_eq!(
        param(
            "/api/0/organizations/{org}/releases/{version}/",
            "put",
            "org"
        ),
        "string"
    );
    assert_eq!(
        param(
            "/api/0/organizations/{org}/releases/{version}/",
            "put",
            "version"
        ),
        "string"
    );
    assert_eq!(
        doc["paths"]["/mcp"]["post"]["security"],
        json!([{"apiToken": []}])
    );
    let mut web = common::Web::new(&app, "reader", "viewer").await;
    let md = web.get("/api-docs.md").await;
    assert_eq!(md.status, StatusCode::OK);
    assert!(md.body.contains("## Endpoints: Ingest"));
    assert!(md.body.contains("### POST /api/{project_id}/envelope/"));
    assert!(md.body.contains("DSN key"));
    // Its curl example sends the key in its header and the envelope as is.
    let env = &md.body[md
        .body
        .find("### POST /api/{project_id}/envelope/")
        .unwrap()..];
    let env = &env[..env[4..].find("### ").map_or(env.len(), |i| i + 4)];
    assert!(
        env.contains("X-Sentry-Auth: Sentry sentry_key=$KK_DSN_KEY, sentry_version=7"),
        "{env}"
    );
    assert!(env.contains("application/x-sentry-envelope"), "{env}");
    assert!(md.body.contains("## Endpoints: Alerts"));
    assert!(md.body.contains("## Endpoints: sentry-cli"));
    // MCP and sentry-cli take an API token (not "public").
    let mcp = &md.body[md.body.find("### POST /mcp").unwrap()..];
    assert!(mcp.contains("- Access: any API token"), "{mcp}");
    let llms = web.get("/llms.txt").await;
    assert!(llms.body.contains("/api/v1/projects/{id}/alert-rules"));
}

async fn rpc(app: &App, token: &str, name: &str, args: Value) -> Value {
    let res = bearer(
        app,
        "POST",
        "/mcp",
        token,
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}})),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    res.json()["result"].clone()
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn mcp_tools_read_and_triage_errors() {
    let app = App::new().await;
    let t = tokens(&app).await;
    seed(&app).await;
    let p = app.project_id;

    let list = |token: &str| {
        let token = token.to_string();
        let app = &app;
        async move {
            let res = bearer(
                app,
                "POST",
                "/mcp",
                &token,
                Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})),
            )
            .await;
            res.json()["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        list(&t.viewer).await,
        [
            "list_projects",
            "list_project_groups",
            "list_issues",
            "get_issue",
            "release_health",
            "analytics_summary",
            "analytics_breakdown"
        ]
    );
    assert_eq!(
        list(&t.editor_write).await,
        [
            "list_projects",
            "list_project_groups",
            "list_issues",
            "get_issue",
            "release_health",
            "analytics_summary",
            "analytics_breakdown",
            "resolve_issue",
            "ignore_issue",
            "create_project_group",
            "rename_project_group",
            "delete_project_group",
            "reorder_project_groups",
            "set_project_group"
        ]
    );

    let found = rpc(
        &app,
        &t.viewer,
        "list_issues",
        json!({"project": "fixtures", "query": "level:fatal"}),
    )
    .await;
    let issues = found["structuredContent"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["title"], "TypeError: boom");
    let id = issues[0]["id"].as_i64().unwrap();
    let by_id = rpc(&app, &t.viewer, "list_issues", json!({"project": p})).await;
    assert_eq!(by_id["structuredContent"]["total"], 3);

    let issue = rpc(
        &app,
        &t.viewer,
        "get_issue",
        json!({"project": p, "issue_id": id}),
    )
    .await;
    let txt = text(&issue);
    assert!(txt.starts_with("# TypeError: boom"), "{txt}");
    assert!(txt.contains("## Exceptions"), "{txt}");
    assert!(txt.contains("at checkout (shop/views.py:42"), "{txt}");
    assert!(txt.contains("total = cart.total()"), "{txt}");
    assert!(txt.contains("frame(s) outside the app"), "{txt}");
    assert!(txt.contains("## Breadcrumbs"), "{txt}");
    assert!(txt.contains("GET /cart 500"), "{txt}");
    // No user data, hashed or not.
    let all = issue.to_string();
    assert!(!all.contains("ada.lovelace"), "{all}");
    assert!(!all.contains("user-ada-42"));
    assert!(!all.contains("hash:"), "{all}");

    // Read tokens can't change anything.
    let res = rpc(
        &app,
        &t.viewer,
        "resolve_issue",
        json!({"project": p, "issue_id": id}),
    )
    .await;
    assert_eq!(res["isError"], true);
    assert_eq!(
        app.count("SELECT COUNT(*) FROM issues WHERE status = 'resolved'")
            .await,
        0
    );
    let res = rpc(
        &app,
        &t.editor_write,
        "resolve_issue",
        json!({"project": p, "issue_id": id}),
    )
    .await;
    assert_eq!(
        res["structuredContent"]["issue"]["status"], "resolved",
        "{res}"
    );
    let res = rpc(
        &app,
        &t.editor_write,
        "ignore_issue",
        json!({"project": p, "issue_id": id, "for_hours": 2}),
    )
    .await;
    assert_eq!(res["structuredContent"]["issue"]["status"], "ignored");
    let res = rpc(
        &app,
        &t.editor_write,
        "ignore_issue",
        json!({"project": p, "issue_id": id, "for_hours": 2, "until_events": 3}),
    )
    .await;
    assert_eq!(res["isError"], true);
    let res = rpc(
        &app,
        &t.viewer,
        "get_issue",
        json!({"project": "nope", "issue_id": id}),
    )
    .await;
    assert_eq!(res["isError"], true);

    // Release health of the newest release (no sessions yet: no rates).
    let res = rpc(&app, &t.viewer, "release_health", json!({"project": p})).await;
    assert_eq!(
        res["structuredContent"]["release"]["version"], "shop@1.0",
        "{res}"
    );
    assert_eq!(
        res["structuredContent"]["totals"]["crash_free_sessions"],
        Value::Null
    );
    let res = rpc(
        &app,
        &t.viewer,
        "release_health",
        json!({"project": p, "release": "none@1"}),
    )
    .await;
    assert_eq!(res["isError"], true);
}
