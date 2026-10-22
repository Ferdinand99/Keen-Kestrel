//! The MCP server (`POST /mcp`): lets an AI assistant read Kestrel with an
//! API token. The kit does the protocol, authentication and rate limit;
//! Kestrel supplies the tools. For now: `list_projects`.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{Map, Value, json};
use vole_kit::mcp::{Handler, RpcResult};
use vole_kit::state::Core;

use crate::db::projects as db;

struct Kestrel {
    core: Core,
    can_write: bool,
}

/// MCP's text content for a JSON value.
fn text_result(value: &Value) -> Value {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    json!({ "content": [{ "type": "text", "text": text }], "structuredContent": value })
}

impl Handler for Kestrel {
    fn can_write(&self) -> bool {
        self.can_write
    }

    fn instructions(&self) -> String {
        "Keen Kestrel is a self-hosted, Sentry-compatible error tracker. Each project is one app or \
         website with its own DSN. Use list_projects to see the projects (id, name, slug, platform). \
         Issues and events are not available yet."
            .into()
    }

    fn tools(&self) -> Vec<Value> {
        vec![json!({
            "name": "list_projects",
            "title": "List projects",
            "description": "List every project: id (the number in its DSN), name, slug (for sentry-cli), platform and when it was created.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        })]
    }

    async fn call_tool(&self, name: &str, _args: Map<String, Value>) -> RpcResult {
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
pub async fn endpoint(State(core): State<Core>, headers: HeaderMap, body: Bytes) -> Response {
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
        can_write: caller.can_write,
    };
    vole_kit::mcp::serve(&handler, &body).await
}
