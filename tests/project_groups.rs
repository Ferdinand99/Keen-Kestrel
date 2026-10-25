//! Project groups: the migration, the grouped project list, the "Groups"
//! page, the project settings' Group select, permissions, API v1 and MCP.

mod common;

use axum::http::StatusCode;
use common::{App, Web, add_user, api_token, bearer};
use serde_json::{Value, json};
use vole_kit::auth::TokenScope;

/// Adds a project named `name` (slug from the name) and returns its id.
async fn project(app: &App, name: &'static str) -> i64 {
    app.kestrel
        .core
        .with_conn(move |c| {
            let slug = keen_kestrel::project::slugify(name);
            keen_kestrel::db::projects::insert(
                c,
                name,
                &slug,
                keen_kestrel::project::Platform::Other,
                "Default",
            )
        })
        .await
        .unwrap()
}

async fn group(app: &App, name: &'static str) -> i64 {
    app.kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::groups::insert(c, name))
        .await
        .unwrap()
}

async fn put_in(app: &App, project_id: i64, group_id: Option<i64>) {
    app.kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::projects::set_group(c, project_id, group_id))
        .await
        .unwrap();
}

async fn group_of(app: &App, project_id: i64) -> Option<i64> {
    app.kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::projects::get(c, project_id))
        .await
        .unwrap()
        .unwrap()
        .group_id
}

async fn group_names(app: &App) -> Vec<String> {
    app.kestrel
        .core
        .with_conn(|c| keen_kestrel::db::groups::list(c))
        .await
        .unwrap()
        .into_iter()
        .map(|g| g.name)
        .collect()
}

/// Where `needle` first appears in `html` (panics when it doesn't).
fn at(html: &str, needle: &str) -> usize {
    html.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not on the page"))
}

#[tokio::test]
async fn the_migration_adds_groups_and_upgrades_existing_projects() {
    let app = App::new().await;
    // A database as 0.6 left it: no project_groups, no projects.group_id.
    app.exec(
        "DROP INDEX idx_projects_group;
         ALTER TABLE projects DROP COLUMN group_id;
         DROP TABLE project_groups;
         DELETE FROM schema_migrations WHERE name = '0009_project_groups.sql';"
            .into(),
    )
    .await;
    let upgraded = keen_kestrel::start(&app.dir, None).await.unwrap();
    let (columns, project) = upgraded
        .core
        .with_conn(|c| {
            let columns: Vec<String> = c
                .prepare("SELECT name FROM pragma_table_info('project_groups') ORDER BY cid")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((columns, keen_kestrel::db::projects::get(c, 1)?))
        })
        .await
        .unwrap();
    assert_eq!(columns, ["id", "name", "sort_order", "created_at"]);
    let project = project.expect("the project survives the upgrade");
    assert_eq!(
        (project.name.as_str(), project.group_id),
        ("Fixtures", None)
    );
    let backups: Vec<String> = std::fs::read_dir(app.dir.join("backups"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        backups.iter().any(|b| b.contains("0009_project_groups")),
        "{backups:?}"
    );
}

#[tokio::test]
async fn the_list_has_a_section_per_group_and_not_in_a_group_last() {
    let app = App::new().await;
    let mut web = Web::new(&app, "vic", "viewer").await;

    // Without groups the list looks as before: no sections.
    let page = web.get("/projects").await.body;
    assert!(!page.contains("data-project-group"), "{page}");
    assert!(!page.contains("Not in a group"));
    assert!(!page.contains("/static/project-groups.js"));

    let ios = project(&app, "Sylo iOS").await;
    let dash = project(&app, "Sylo bot/dash").await;
    project(&app, "hedgerow.tools").await;
    let sylo = group(&app, "Sylo").await;
    let docs = group(&app, "Docs").await;
    put_in(&app, ios, Some(sylo)).await;
    put_in(&app, dash, Some(sylo)).await;
    app.exec(format!(
        "INSERT INTO issues (project_id, title, culprit, level, platform, status, substatus, first_seen, last_seen, times_seen, grouping_config)
         SELECT {ios}, 'Crash', '', 'fatal', 'cocoa', 'unresolved', 'new', datetime('now'), datetime('now'), 1, 'kestrel:v2'"
    ))
    .await;

    let page = web.get("/projects").await.body;
    // Groups by sort order, then "Not in a group" last.
    let (s, d, n) = (
        at(&page, &format!("id=\"group-{sylo}-title\">Sylo<")),
        at(&page, &format!("id=\"group-{docs}-title\">Docs<")),
        at(&page, "Not in a group"),
    );
    assert!(s < d && d < n, "order: {s} {d} {n}");
    assert!(page.contains("2 projects · 1 unresolved issue"), "{page}");
    assert!(page.contains("0 projects · No unresolved issues"));
    assert!(page.contains("No projects in this group yet."));
    // The projects land in their sections: Sylo's two before Docs, the
    // rest (and the fixtures' project) after "Not in a group".
    assert!(at(&page, ">Sylo bot/dash<") < d && at(&page, ">Sylo iOS<") < d);
    assert!(at(&page, ">hedgerow.tools<") > n && at(&page, ">Fixtures<") > n);
    // Collapsible, remembered in the browser.
    assert!(page.contains("data-group-toggle"));
    assert!(page.contains(&format!("aria-controls=\"group-{sylo}-list\"")));
    assert!(page.contains("aria-label=\"Hide projects in Sylo\""));
    assert!(page.contains("<script src=\"/static/project-groups.js\" defer></script>"));
    let js = web.get("/static/project-groups.js").await;
    assert_eq!(js.status, StatusCode::OK);
    assert!(js.body.contains("localStorage") && js.body.contains("try {"));
}

#[tokio::test]
async fn editors_create_rename_reorder_and_delete_groups() {
    let app = App::new().await;
    let mut web = Web::new(&app, "eve", "editor").await;
    let fixtures = app.project_id;

    let page = web.get("/projects/groups").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Create group"), "{}", page.body);

    for name in ["Sylo", "Websites", "Docs"] {
        let res = web.post("/projects/groups", &[("name", name)]).await;
        assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    }
    assert_eq!(group_names(&app).await, ["Sylo", "Websites", "Docs"]);

    // Names: required, unique regardless of case.
    let res = web.post("/projects/groups", &[("name", "  ")]).await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.body.contains("Enter a name."));
    let res = web.post("/projects/groups", &[("name", "sylo")]).await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.body.contains("Another group already has this name."));

    let ids: Vec<i64> = app
        .kestrel
        .core
        .with_conn(|c| keen_kestrel::db::groups::list(c))
        .await
        .unwrap()
        .iter()
        .map(|g| g.id)
        .collect();
    let (sylo, websites, docs) = (ids[0], ids[1], ids[2]);

    let res = web
        .post(
            &format!("/projects/groups/{websites}"),
            &[("name", "Sites")],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    let res = web
        .post(
            &format!("/projects/groups/{docs}/move"),
            &[("direction", "up")],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(group_names(&app).await, ["Sylo", "Docs", "Sites"]);
    web.post(
        &format!("/projects/groups/{sylo}/move"),
        &[("direction", "down")],
    )
    .await;
    assert_eq!(group_names(&app).await, ["Docs", "Sylo", "Sites"]);
    // The list page follows the order.
    put_in(&app, fixtures, Some(sylo)).await;
    let page = web.get("/projects").await.body;
    assert!(
        at(&page, ">Docs<") < at(&page, ">Sylo<") && at(&page, ">Sylo<") < at(&page, ">Sites<")
    );

    // Deleting a group keeps its projects; they move to "Not in a group".
    let res = web
        .post(&format!("/projects/groups/{sylo}/delete"), &[])
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(group_names(&app).await, ["Docs", "Sites"]);
    assert_eq!(group_of(&app, fixtures).await, None);
    let page = web.get("/projects").await.body;
    assert!(at(&page, ">Fixtures<") > at(&page, "Not in a group"));
    assert_eq!(
        web.post("/projects/groups/999/delete", &[]).await.status,
        StatusCode::NOT_FOUND
    );
}

/// The project settings form for the fixtures' project with `group`.
fn form(group: &str) -> [(&'static str, &str); 4] {
    [
        ("name", "Fixtures"),
        ("slug", "fixtures"),
        ("platform", "other"),
        ("group_id", group),
    ]
}

#[tokio::test]
async fn the_project_settings_have_a_group_select() {
    let app = App::new().await;
    let id = app.project_id;
    let mut web = Web::new(&app, "eve", "editor").await;
    let settings = format!("/projects/{id}/settings");

    // No groups: no select.
    assert!(!web.get(&settings).await.body.contains("name=\"group_id\""));

    let sylo = group(&app, "Sylo").await;
    let page = web.get(&settings).await.body;
    assert!(
        page.contains("<select id=\"f-group\" name=\"group_id\""),
        "{page}"
    );
    assert!(page.contains("<option value=\"\" selected>None</option>"));
    assert!(page.contains(&format!("<option value=\"{sylo}\">Sylo</option>")));

    let sylo_text = sylo.to_string();
    let res = web.post(&settings, &form(&sylo_text)).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    assert_eq!(group_of(&app, id).await, Some(sylo));
    assert!(
        web.get(&settings)
            .await
            .body
            .contains(&format!("<option value=\"{sylo}\" selected>Sylo</option>"))
    );

    let res = web.post(&settings, &form("999")).await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.body.contains("Choose one of the groups."));
    assert_eq!(group_of(&app, id).await, Some(sylo));

    // A form without the field (older pages) keeps the group.
    let res = web
        .post(
            &settings,
            &[
                ("name", "Fixtures"),
                ("slug", "fixtures"),
                ("platform", "other"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(group_of(&app, id).await, Some(sylo));

    let res = web.post(&settings, &form("")).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    assert_eq!(group_of(&app, id).await, None);
}

#[tokio::test]
async fn viewers_see_groups_but_cannot_change_them() {
    let app = App::new().await;
    let id = app.project_id;
    let sylo = group(&app, "Sylo").await;
    put_in(&app, id, Some(sylo)).await;
    let mut web = Web::new(&app, "vic", "viewer").await;

    let page = web.get("/projects/groups").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(">Sylo<") && page.body.contains("1 project"));
    assert!(!page.body.contains("/projects/groups/1/delete"));
    assert!(!page.body.contains("New group"));
    let settings = web.get(&format!("/projects/{id}/settings")).await.body;
    assert!(!settings.contains("name=\"group_id\""));
    assert!(
        settings.contains("<dt>Group</dt><dd>Sylo</dd>"),
        "{settings}"
    );

    for (path, form) in [
        ("/projects/groups".to_string(), vec![("name", "Mine")]),
        (format!("/projects/groups/{sylo}"), vec![("name", "Mine")]),
        (
            format!("/projects/groups/{sylo}/move"),
            vec![("direction", "up")],
        ),
        (format!("/projects/groups/{sylo}/delete"), vec![]),
        (
            format!("/projects/{id}/settings"),
            vec![("name", "Fixtures"), ("slug", "fixtures"), ("group_id", "")],
        ),
    ] {
        let res = web.post(&path, &form).await;
        assert_eq!(res.status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(group_names(&app).await, ["Sylo"]);
    assert_eq!(group_of(&app, id).await, Some(sylo));

    // Read tokens read; they can't write.
    let viewer = add_user(&app, "rita", "viewer").await;
    let token = api_token(&app, viewer, TokenScope::Write).await;
    let res = bearer(&app, "GET", "/api/v1/project-groups", &token, None).await;
    assert_eq!(res.status, StatusCode::OK);
    let res = bearer(
        &app,
        "POST",
        "/api/v1/project-groups",
        &token,
        Some(json!({"name": "Nope"})),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = bearer(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{id}"),
        &token,
        Some(json!({"group_id": null})),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_api_manages_groups_and_project_group_ids() {
    let app = App::new().await;
    let editor = add_user(&app, "eve", "editor").await;
    let token = api_token(&app, editor, TokenScope::Write).await;
    let id = app.project_id;

    let create = |name: &'static str| {
        let token = token.clone();
        let app = &app;
        async move {
            bearer(
                app,
                "POST",
                "/api/v1/project-groups",
                &token,
                Some(json!({ "name": name })),
            )
            .await
        }
    };
    let res = create("Sylo").await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let sylo = res.json()["id"].as_i64().unwrap();
    assert_eq!(
        res.header("location"),
        format!("/api/v1/project-groups/{sylo}")
    );
    assert_eq!(res.json()["projects"], 0);
    let web_group = create("Websites").await.json()["id"].as_i64().unwrap();
    let res = create("SYLO").await;
    assert_eq!(res.status, StatusCode::CONFLICT, "{}", res.body);
    let res = create("").await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(res.json()["error"]["fields"]["name"].is_string());

    // Projects carry group_id; null takes them out.
    let res = bearer(&app, "GET", &format!("/api/v1/projects/{id}"), &token, None).await;
    assert_eq!(res.json()["group_id"], Value::Null);
    let res = bearer(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{id}"),
        &token,
        Some(json!({ "group_id": sylo })),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["group_id"], sylo);
    let res = bearer(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{id}"),
        &token,
        Some(json!({ "name": "Fixtures 2" })),
    )
    .await;
    assert_eq!(res.json()["group_id"], sylo, "left out: the group stays");
    let res = bearer(
        &app,
        "PATCH",
        &format!("/api/v1/projects/{id}"),
        &token,
        Some(json!({ "group_id": 999 })),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        res.json()["error"]["fields"]["group_id"],
        "Choose one of the groups."
    );
    let res = bearer(
        &app,
        "POST",
        "/api/v1/projects",
        &token,
        Some(json!({ "name": "Sylo iOS", "group_id": web_group })),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let ios = res.json()["id"].as_i64().unwrap();
    assert_eq!(res.json()["group_id"], web_group);

    let res = bearer(&app, "GET", "/api/v1/project-groups", &token, None).await;
    let data = res.json()["data"].clone();
    assert_eq!(data[0]["name"], "Sylo");
    assert_eq!(data[0]["projects"], 1);
    assert_eq!(data[1]["projects"], 1);

    // Rename, reorder.
    let res = bearer(
        &app,
        "PATCH",
        &format!("/api/v1/project-groups/{web_group}"),
        &token,
        Some(json!({ "name": "Sites" })),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["name"], "Sites");
    let res = bearer(
        &app,
        "PUT",
        "/api/v1/project-groups/order",
        &token,
        Some(json!({ "ids": [web_group, sylo] })),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()["data"][0]["name"], "Sites");
    assert_eq!(group_names(&app).await, ["Sites", "Sylo"]);
    let res = bearer(
        &app,
        "PUT",
        "/api/v1/project-groups/order",
        &token,
        Some(json!({ "ids": [sylo] })),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Delete keeps the projects.
    let res = bearer(
        &app,
        "DELETE",
        &format!("/api/v1/project-groups/{web_group}"),
        &token,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);
    assert_eq!(group_of(&app, ios).await, None);
    let res = bearer(
        &app,
        "GET",
        &format!("/api/v1/project-groups/{web_group}"),
        &token,
        None,
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);

    // Documented.
    let doc = bearer(&app, "GET", "/api/v1/openapi.json", &token, None)
        .await
        .json();
    for (path, method) in [
        ("/api/v1/project-groups", "get"),
        ("/api/v1/project-groups", "post"),
        ("/api/v1/project-groups/order", "put"),
        ("/api/v1/project-groups/{id}", "patch"),
        ("/api/v1/project-groups/{id}", "delete"),
    ] {
        assert!(doc["paths"][path][method].is_object(), "{method} {path}");
    }
}

#[tokio::test]
async fn mcp_tools_list_and_change_groups() {
    let app = App::new().await;
    let editor = add_user(&app, "eve", "editor").await;
    let write = api_token(&app, editor, TokenScope::Write).await;
    let read = api_token(&app, editor, TokenScope::Read).await;
    let call = |token: String, name: &'static str, args: Value| {
        let app = &app;
        async move {
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
            let res = bearer(app, "POST", "/mcp", &token, Some(body)).await;
            assert_eq!(res.status, StatusCode::OK, "{}", res.body);
            res.json()["result"].clone()
        }
    };
    let tools = |token: String| {
        let app = &app;
        async move {
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
            let res = bearer(app, "POST", "/mcp", &token, Some(body)).await;
            res.json()["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    let read_tools = tools(read.clone()).await;
    assert!(read_tools.contains(&"list_project_groups".to_string()));
    assert!(!read_tools.contains(&"create_project_group".to_string()));
    let write_tools = tools(write.clone()).await;
    for t in [
        "create_project_group",
        "rename_project_group",
        "delete_project_group",
        "reorder_project_groups",
        "set_project_group",
    ] {
        assert!(write_tools.contains(&t.to_string()), "{t}");
    }

    let made = call(
        write.clone(),
        "create_project_group",
        json!({"name": "Sylo"}),
    )
    .await;
    let sylo = made["structuredContent"]["group"]["id"].as_i64().unwrap();
    let other = call(
        write.clone(),
        "create_project_group",
        json!({"name": "Docs"}),
    )
    .await;
    let docs = other["structuredContent"]["group"]["id"].as_i64().unwrap();
    let dup = call(
        write.clone(),
        "create_project_group",
        json!({"name": "sylo"}),
    )
    .await;
    assert_eq!(dup["isError"], true);

    let set = call(
        write.clone(),
        "set_project_group",
        json!({"project": "fixtures", "group_id": sylo}),
    )
    .await;
    assert_eq!(
        set["structuredContent"]["project"]["group_id"], sylo,
        "{set}"
    );
    let listed = call(read.clone(), "list_projects", json!({})).await;
    assert_eq!(listed["structuredContent"]["projects"][0]["group_id"], sylo);
    assert_eq!(listed["structuredContent"]["groups"][0]["name"], "Sylo");

    call(
        write.clone(),
        "rename_project_group",
        json!({"group_id": sylo, "name": "Sylo apps"}),
    )
    .await;
    let order = call(
        write.clone(),
        "reorder_project_groups",
        json!({"ids": [docs, sylo]}),
    )
    .await;
    assert_eq!(order["structuredContent"]["groups"][1]["name"], "Sylo apps");
    let groups = call(read.clone(), "list_project_groups", json!({})).await;
    assert_eq!(groups["structuredContent"]["groups"][0]["name"], "Docs");
    assert_eq!(groups["structuredContent"]["groups"][1]["projects"], 1);

    // A read token can't change anything.
    let denied = call(
        read.clone(),
        "delete_project_group",
        json!({"group_id": sylo}),
    )
    .await;
    assert_eq!(denied["isError"], true, "{denied}");
    assert_eq!(group_names(&app).await, ["Docs", "Sylo apps"]);

    let gone = call(
        write.clone(),
        "delete_project_group",
        json!({"group_id": sylo}),
    )
    .await;
    assert_eq!(gone["structuredContent"]["deleted"], sylo);
    assert_eq!(group_of(&app, app.project_id).await, None);
    let out = call(
        write.clone(),
        "set_project_group",
        json!({"project": app.project_id, "group_id": null}),
    )
    .await;
    assert_eq!(out["structuredContent"]["project"]["group_id"], Value::Null);
}
