//! The MCP server (`POST /mcp`, docs/design.md §6): lets an AI assistant
//! read and triage errors with an API token. The kit does the protocol,
//! authentication and rate limit; Kestrel supplies the tools:
//!
//! * `list_projects`;
//! * `list_issues`: the issue list's search syntax;
//! * `get_issue`: the issue and its latest event, the stack trace formatted
//!   for a language model (in-app frames with file:line, the exception
//!   chain, the last breadcrumbs);
//! * `release_health`: crash-free sessions and users of a release;
//! * `resolve_issue` and `ignore_issue`, offered only to a read-and-write
//!   token of an editor or admin.
//!
//! Nothing a tool returns holds a secret or user data: event payloads were
//! scrubbed at ingest, and the user section (keyed hashes) is left out.

use std::fmt::Write as _;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{Map, Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::mcp::{Handler, RpcResult};
use vole_kit::state::Core;

use crate::api::issues::{issue_json, search_issues, set_status};
use crate::db::projects::{self as db, Project};
use crate::db::triage::{self, IssueRow};
use crate::ingest::Ingest;
use crate::issues::view::{self, FrameGroup, FrameView};
use crate::issues::{Action, IgnoreFor};

/// Breadcrumbs `get_issue` shows at most (the newest).
const CRUMBS: usize = 10;
/// In-app frames shown per stack at most (the innermost).
const FRAMES: usize = 30;

struct Kestrel {
    core: Core,
    ingest: Ingest,
    user: CurrentUser,
    can_write: bool,
}

/// MCP's text content for a JSON value.
fn text_result(value: &Value) -> Value {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    json!({ "content": [{ "type": "text", "text": text }], "structuredContent": value })
}

/// Text for the model with the same data as structured content.
fn text_and_data(text: String, value: &Value) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "structuredContent": value })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": message.into() }], "isError": true })
}

fn project_arg() -> Value {
    json!({"type": ["integer", "string"], "description": "The project's id (the number in its DSN) or slug."})
}

fn issue_arg() -> Value {
    json!({"type": "integer", "description": "The issue's id (from list_issues)."})
}

fn str_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn int_arg(args: &Map<String, Value>, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

impl Kestrel {
    async fn project(&self, args: &Map<String, Value>) -> Result<Project, Value> {
        let key = match args.get("project") {
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(s)) => s.trim().to_string(),
            _ => {
                return Err(tool_error(
                    "Name the project: its id or slug (see list_projects).",
                ));
            }
        };
        let k = key.clone();
        let found = self
            .core
            .with_conn(move |c| match k.parse::<i64>() {
                Ok(id) => db::get(c, id),
                Err(_) => db::by_slug(c, &k),
            })
            .await
            .map_err(|e| tool_error(format!("Could not read the projects: {e}")))?;
        found.ok_or_else(|| tool_error(format!("No project {key}. Use list_projects.")))
    }

    async fn issue(&self, project: &Project, args: &Map<String, Value>) -> Result<IssueRow, Value> {
        let Some(issue_id) = int_arg(args, "issue_id") else {
            return Err(tool_error("Name the issue: issue_id (from list_issues)."));
        };
        let pid = project.id;
        self.core
            .with_conn(move |c| triage::get(c, pid, issue_id))
            .await
            .map_err(|e| tool_error(format!("Could not read the issue: {e}")))?
            .ok_or_else(|| tool_error(format!("No issue {issue_id} in {}.", project.name)))
    }

    async fn list_issues(&self, args: Map<String, Value>) -> Value {
        let project = match self.project(&args).await {
            Ok(p) => p,
            Err(e) => return e,
        };
        let limit = int_arg(&args, "limit").unwrap_or(25).clamp(1, 100);
        let offset = int_arg(&args, "cursor").unwrap_or(0).max(0);
        let page = match search_issues(
            &self.core,
            project.id,
            str_arg(&args, "query").unwrap_or(""),
            str_arg(&args, "status"),
            str_arg(&args, "sort"),
            limit,
            offset,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => return tool_error(format!("Could not search: {e}")),
        };
        let base = self.core.base_url().await;
        let issues: Vec<Value> = page
            .issues
            .iter()
            .map(|i| {
                let mut v = issue_json(i);
                if let Some(b) = &base {
                    let path = v["path"].as_str().unwrap_or("").to_string();
                    v["url"] = Value::from(format!("{}{path}", b.trim_end_matches('/')));
                }
                v
            })
            .collect();
        text_result(&json!({
            "project": {"id": project.id, "name": project.name, "slug": project.slug},
            "total": page.total,
            "issues": issues,
            "next_cursor": page.next_offset,
            "unknown_terms": page.unknown,
        }))
    }

    async fn get_issue(&self, args: Map<String, Value>) -> Value {
        let project = match self.project(&args).await {
            Ok(p) => p,
            Err(e) => return e,
        };
        let issue = match self.issue(&project, &args).await {
            Ok(i) => i,
            Err(e) => return e,
        };
        let issue_id = issue.id;
        let latest = self
            .core
            .with_conn(move |c| triage::latest_event(c, issue_id))
            .await
            .ok()
            .flatten();
        let payload = match latest.as_ref().and_then(|e| e.payload_id) {
            Some(p) => crate::issues::load_payload(&self.ingest, p).await,
            None => None,
        };
        let base = self.core.base_url().await;
        let text = format_issue(
            &project,
            &issue,
            latest.as_ref(),
            payload.as_ref(),
            base.as_deref(),
        );
        let mut data = issue_json(&issue);
        if let Some(e) = &latest {
            data["latest_event"] = json!({
                "event_id": e.event_id,
                "timestamp": e.timestamp,
                "release": e.release,
                "environment": e.environment,
                "has_details": payload.is_some(),
            });
        }
        text_and_data(text, &data)
    }

    async fn change(&self, args: Map<String, Value>, action: Action) -> Value {
        if !self.can_write {
            return tool_error(
                "This token can only read. Changing issues needs a read-and-write token of an editor or admin.",
            );
        }
        let project = match self.project(&args).await {
            Ok(p) => p,
            Err(e) => return e,
        };
        let issue = match self.issue(&project, &args).await {
            Ok(i) => i,
            Err(e) => return e,
        };
        match set_status(
            &self.core,
            &self.ingest,
            &self.user,
            project.id,
            issue.id,
            action,
        )
        .await
        {
            Ok(after) => text_result(&json!({ "issue": issue_json(&after) })),
            Err(e) => tool_error(format!("Could not change the issue: {e}")),
        }
    }

    async fn release_health(&self, args: Map<String, Value>) -> Value {
        let project = match self.project(&args).await {
            Ok(p) => p,
            Err(e) => return e,
        };
        let pid = project.id;
        let wanted = args.get("release").map(|v| match v {
            Value::String(s) => s.trim().to_string(),
            other => other.to_string(),
        });
        let release_id = self
            .core
            .with_conn(move |c| {
                use rusqlite::OptionalExtension as _;
                match wanted.filter(|w| !w.is_empty()) {
                    None => c
                        .query_row(
                            "SELECT id FROM releases WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
                            [pid],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional(),
                    Some(w) => {
                        if let Some(id) = crate::db::releases::id_of(c, pid, &w)? {
                            return Ok(Some(id));
                        }
                        match w.parse::<i64>() {
                            Ok(id) => c
                                .query_row(
                                    "SELECT id FROM releases WHERE project_id = ?1 AND id = ?2",
                                    [pid, id],
                                    |r| r.get::<_, i64>(0),
                                )
                                .optional(),
                            Err(_) => Ok(None),
                        }
                    }
                }
            })
            .await;
        let release_id = match release_id {
            Ok(Some(id)) => id,
            Ok(None) => {
                return tool_error(format!(
                    "No such release in {}. Releases are named by their version, e.g. shop@1.4.1.",
                    project.name
                ));
            }
            Err(e) => return tool_error(format!("Could not read the releases: {e}")),
        };
        let days = int_arg(&args, "days").unwrap_or(30);
        let environment = str_arg(&args, "environment").map(str::to_string);
        match crate::api::releases::release_health(&self.core, pid, release_id, days, environment)
            .await
        {
            Ok(v) => text_result(&v),
            Err(e) => tool_error(format!("Could not read the release health: {e}")),
        }
    }
}

/// `file:line` of a frame, else its module or package.
fn place(f: &FrameView) -> String {
    if !f.location.is_empty() {
        f.location.clone()
    } else {
        f.package.clone()
    }
}

fn flush_hidden(out: &mut String, hidden: &mut usize) {
    if *hidden > 0 {
        let _ = writeln!(out, "    … {hidden} frame(s) outside the app");
        *hidden = 0;
    }
}

/// A stack, innermost frame first: the in-app frames (every frame when
/// none is), runs of others counted.
fn write_stack(out: &mut String, groups: &[FrameGroup]) {
    let mut shown = 0;
    let mut hidden = 0;
    for g in groups {
        match g {
            FrameGroup::Frame(f) => {
                if shown >= FRAMES {
                    hidden += 1;
                    continue;
                }
                flush_hidden(out, &mut hidden);
                let mut line = format!("    at {}", f.function);
                let at = place(f);
                if !at.is_empty() {
                    let _ = write!(line, " ({at})");
                }
                if f.inlined {
                    line.push_str(" [inlined]");
                }
                if !f.missing_dsym.is_empty() {
                    let _ = write!(line, " [no dSYM for {}]", f.missing_dsym);
                }
                let _ = writeln!(out, "{line}");
                if let Some(current) = f.context.iter().find(|l| l.current) {
                    let _ = writeln!(out, "        {}: {}", current.no, current.code.trim_end());
                }
                shown += 1;
            }
            FrameGroup::System(frames) => hidden += frames.len(),
        }
    }
    flush_hidden(out, &mut hidden);
}

/// The issue and its latest event as text for a language model.
fn format_issue(
    project: &Project,
    issue: &IssueRow,
    latest: Option<&triage::EventRow>,
    payload: Option<&Value>,
    base: Option<&str>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", issue.title);
    let _ = writeln!(
        out,
        "Issue {} in project {} (id {}) · status {} ({}) · level {} · platform {} · grouping {}",
        issue.id,
        project.name,
        project.id,
        issue.status,
        issue.substatus,
        issue.level,
        issue.platform,
        issue.grouping_config
    );
    if !issue.culprit.is_empty() {
        let _ = writeln!(out, "Where: {}", issue.culprit);
    }
    let in_release =
        |r: &Option<String>| r.as_deref().map(|r| format!(" in {r}")).unwrap_or_default();
    let _ = writeln!(
        out,
        "Events: {} · users: {} · first seen {} UTC{} · last seen {} UTC{}",
        issue.times_seen,
        issue.users_seen,
        issue.first_seen,
        in_release(&issue.first_release),
        issue.last_seen,
        in_release(&issue.last_release),
    );
    if issue.unsymbolicated {
        out.push_str(
            "Native frames without symbols: upload the build's dSYM to get functions and lines.\n",
        );
    }
    if let Some(b) = base {
        let _ = writeln!(
            out,
            "Link: {}/projects/{}/issues/{}",
            b.trim_end_matches('/'),
            project.id,
            issue.id
        );
    }
    let Some(e) = latest else {
        out.push_str("\nNo stored event.\n");
        return out;
    };
    let _ = writeln!(
        out,
        "\n## Latest event {} at {} UTC{}{}",
        e.event_id,
        e.timestamp,
        e.release
            .as_deref()
            .map(|r| format!(" · release {r}"))
            .unwrap_or_default(),
        e.environment
            .as_deref()
            .map(|r| format!(" · environment {r}"))
            .unwrap_or_default(),
    );
    let Some(payload) = payload else {
        out.push_str("Its details are gone (retention).\n");
        return out;
    };
    let v = view::build(payload, false);
    if !v.message.is_empty() {
        let _ = writeln!(out, "Message: {}", v.message);
    }
    if !v.exceptions.is_empty() {
        out.push_str("\n## Exceptions (the one raised last first, then what caused it)\n");
        for (n, ex) in v.exceptions.iter().enumerate() {
            let head = match (ex.ty.is_empty(), ex.value.is_empty()) {
                (false, false) => format!("{}: {}", ex.ty, ex.value),
                (false, true) => ex.ty.clone(),
                _ => ex.value.clone(),
            };
            let how = match (ex.mechanism.is_empty(), ex.handled) {
                (false, Some(false)) => format!(" [{}, unhandled]", ex.mechanism),
                (false, _) => format!(" [{}]", ex.mechanism),
                (true, Some(false)) => " [unhandled]".to_string(),
                _ => String::new(),
            };
            let caused = if n == 0 { "" } else { "Caused by: " };
            let _ = writeln!(out, "{caused}{head}{how}");
            if !ex.frames.is_empty() {
                out.push_str("  Stack (innermost first):\n");
                write_stack(&mut out, &ex.frames);
            }
        }
    }
    if let Some(t) = &v.thread {
        let _ = writeln!(
            out,
            "\n## {} thread{} (innermost first)",
            if t.crashed { "Crashed" } else { "Current" },
            if t.name.is_empty() {
                String::new()
            } else {
                format!(" {}", t.name)
            }
        );
        write_stack(&mut out, &t.frames);
    }
    if !v.breadcrumbs.is_empty() {
        let _ = writeln!(
            out,
            "\n## Breadcrumbs (the last {} of {}, newest first)",
            v.breadcrumbs.len().min(CRUMBS),
            v.breadcrumbs.len()
        );
        for c in v.breadcrumbs.iter().take(CRUMBS) {
            let mut line = format!("  {}", c.time);
            if !c.category.is_empty() {
                let _ = write!(line, " [{}]", c.category);
            }
            if !c.level.is_empty() && c.level != "info" {
                let _ = write!(line, " {}", c.level);
            }
            let what = if c.message.is_empty() {
                &c.data
            } else {
                &c.message
            };
            if !what.is_empty() {
                let _ = write!(line, " {}", what.chars().take(300).collect::<String>());
            }
            let _ = writeln!(out, "{line}");
        }
    }
    if let Some(r) = &v.request {
        let _ = writeln!(out, "\n## Request\n  {} {}", r.method, r.url);
    }
    if !v.tags.is_empty() {
        out.push_str("\n## Tags\n");
        let tags: Vec<String> = v.tags.iter().map(|(k, val)| format!("{k}={val}")).collect();
        let _ = writeln!(out, "  {}", tags.join(", "));
    }
    if !v.sdk.is_empty() {
        let _ = writeln!(out, "\nSDK: {}", v.sdk);
    }
    out
}

impl Handler for Kestrel {
    fn can_write(&self) -> bool {
        self.can_write
    }

    fn instructions(&self) -> String {
        let mut s = String::from(
            "Keen Kestrel is a self-hosted, Sentry-compatible error tracker. Each project is one app or \
             website with its own DSN. Use list_projects to find a project, list_issues to search its \
             issues (search syntax: is:unresolved|resolved|ignored|all|new|regressed, level:error, \
             release:1.4, env:production, os:iOS, any tag key:value, free text), get_issue for an \
             issue's latest event with its stack trace (in-app frames, file:line, the exception chain, \
             breadcrumbs), and release_health for crash-free sessions and users of a release.",
        );
        if self.can_write {
            s.push_str(
                " resolve_issue and ignore_issue change an issue's status; tell the user what you changed.",
            );
        }
        s
    }

    fn tools(&self) -> Vec<Value> {
        let mut tools = vec![
            json!({
                "name": "list_projects",
                "title": "List projects",
                "description": "List every project: id (the number in its DSN), name, slug (for sentry-cli), platform and when it was created.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
                "annotations": { "readOnlyHint": true, "openWorldHint": false },
            }),
            json!({
                "name": "list_issues",
                "title": "Search issues",
                "description": "Search a project's issues, newest activity first. The query uses the issue list's syntax: is:unresolved|resolved|ignored|all|new|regressed|ongoing|unsymbolicated, level:fatal|error|warning|info|debug, release:<version or name@version, * wildcard>, env:<environment>, os:/browser:/device:/runtime:<name>, any other tag as key:value, ! to negate, quotes for spaces; other words match titles and culprits. Unresolved issues unless the query or status says otherwise.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "query": {"type": "string", "description": "Search terms, e.g. \"is:unresolved level:error release:1.4 checkout\"."},
                        "status": {"type": "string", "enum": ["unresolved", "resolved", "ignored", "all"], "description": "When the query has no is: status. Default unresolved."},
                        "sort": {"type": "string", "enum": ["last_seen", "first_seen", "events", "users"], "description": "Default last_seen."},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Issues per page (default 25)."},
                        "cursor": {"type": "integer", "minimum": 0, "description": "next_cursor of the previous page."}
                    },
                    "required": ["project"],
                    "additionalProperties": false
                },
                "annotations": { "readOnlyHint": true, "openWorldHint": false },
            }),
            json!({
                "name": "get_issue",
                "title": "Get an issue",
                "description": "An issue with its latest event: the exception chain with the stack trace (in-app frames with file:line, innermost first), the crashed thread, the last breadcrumbs, the request line and tags. User data is never included.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "project": project_arg(), "issue_id": issue_arg() },
                    "required": ["project", "issue_id"],
                    "additionalProperties": false
                },
                "annotations": { "readOnlyHint": true, "openWorldHint": false },
            }),
            json!({
                "name": "release_health",
                "title": "Release health",
                "description": "Crash-free sessions and crash-free users of a release (default: the newest), in total and per day, with sessions, errored, crashed and abnormal counts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "release": {"type": "string", "description": "The release's version as sent (e.g. shop@1.4.1) or its id. Default: the newest release."},
                        "days": {"type": "integer", "minimum": 1, "maximum": 400, "description": "Days up to today (default 30)."},
                        "environment": {"type": "string", "description": "One environment only."}
                    },
                    "required": ["project"],
                    "additionalProperties": false
                },
                "annotations": { "readOnlyHint": true, "openWorldHint": false },
            }),
        ];
        if self.can_write {
            tools.push(json!({
                "name": "resolve_issue",
                "title": "Resolve an issue",
                "description": "Mark an issue resolved. With in_next_release, only events from a release newer than the project's newest one bring it back; otherwise any new event makes it regressed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "issue_id": issue_arg(),
                        "in_next_release": {"type": "boolean", "description": "Resolve in the next release (default false)."}
                    },
                    "required": ["project", "issue_id"],
                    "additionalProperties": false
                },
                "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            }));
            tools.push(json!({
                "name": "ignore_issue",
                "title": "Ignore an issue",
                "description": "Ignore an issue: forever, for some hours, or until it has some more events. It comes back as unresolved when the condition is met.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": project_arg(),
                        "issue_id": issue_arg(),
                        "for_hours": {"type": "integer", "minimum": 1, "maximum": 8760, "description": "Ignore for this many hours."},
                        "until_events": {"type": "integer", "minimum": 1, "maximum": 1000000, "description": "Ignore until this many more events."}
                    },
                    "required": ["project", "issue_id"],
                    "additionalProperties": false
                },
                "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            }));
        }
        tools
    }

    async fn call_tool(&self, name: &str, args: Map<String, Value>) -> RpcResult {
        match name {
            "list_projects" => {
                let projects = self
                    .core
                    .with_conn(|c| db::list(c))
                    .await
                    .map_err(|e| (-32603, format!("Could not read the projects: {e}")))?;
                let list: Vec<Value> = projects.iter().map(crate::api::project_json).collect();
                Ok(text_result(&json!({ "projects": list })))
            }
            "list_issues" => Ok(self.list_issues(args).await),
            "get_issue" => Ok(self.get_issue(args).await),
            "release_health" => Ok(self.release_health(args).await),
            "resolve_issue" => {
                let action = if args.get("in_next_release").and_then(Value::as_bool) == Some(true) {
                    Action::ResolveNextRelease
                } else {
                    Action::Resolve
                };
                Ok(self.change(args, action).await)
            }
            "ignore_issue" => {
                let how = match (int_arg(&args, "for_hours"), int_arg(&args, "until_events")) {
                    (Some(_), Some(_)) => {
                        return Ok(tool_error("Give for_hours or until_events, not both."));
                    }
                    (Some(h), None) if (1..=8760).contains(&h) => IgnoreFor::Hours(h),
                    (None, Some(n)) if (1..=1_000_000).contains(&n) => IgnoreFor::Events(n),
                    (None, None) => IgnoreFor::Forever,
                    _ => {
                        return Ok(tool_error(
                            "for_hours is 1–8760 and until_events 1–1000000.",
                        ));
                    }
                };
                Ok(self.change(args, Action::Ignore(how)).await)
            }
            other => Err((-32602, format!("Unknown tool {other}."))),
        }
    }

    async fn list_resources(&self) -> RpcResult {
        Ok(json!({ "resources": [] }))
    }

    fn resource_templates(&self) -> Value {
        json!({ "resourceTemplates": [] })
    }

    async fn read_resource(&self, uri: &str) -> RpcResult {
        Err((-32002, format!("Unknown resource {uri}.")))
    }
}

/// `POST /mcp`.
pub async fn endpoint(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > vole_kit::mcp::MAX_BODY {
        return vole_kit::mcp::http_error(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "The request body is too large.",
        );
    }
    let caller = match vole_kit::mcp::authenticate(&core, &headers, 0).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let handler = Kestrel {
        core,
        ingest,
        can_write: caller.can_write,
        user: caller.user,
    };
    vole_kit::mcp::serve(&handler, &body).await
}
