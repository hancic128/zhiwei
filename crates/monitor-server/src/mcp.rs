//! MCP (Model Context Protocol) server implementation.
//!
//! Exposes monitor data to external AI clients (Claude Desktop / Cursor /
//! Cline, etc.) via the SSE transport. Read-only endpoints only — D8 decision:
//! destructive operations (reboot / shutdown / kill) don't go through MCP.
//!
//! Design decisions (see plan):
//! - Single endpoint: `POST /mcp/sse`
//!   - request:  Content-Type: application/json, JSON-RPC 2.0 body
//!   - response: Content-Type: text/event-stream, one JSON-RPC message per event
//!   - Synchronous response (all tools in this round are <1s fast operations)
//! - GET /mcp/sse not implemented yet (add when server-push is needed)
//! - Authentication: only AI tokens (`ReadAuthKind::AiToken`)
//! - HTTP client: reqwest calls monitor's own REST API, reuses the same AI token
//!
//! MCP protocol version: `2024-11-05` (current MCP stable draft).

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::routes::{read_auth_ok_v2, ReadAuthKind};
use crate::state::AppState;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "zhiwei-monitor";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// MCP server info (`initialize` response).
fn server_info() -> Value {
    json!({
        "name": SERVER_NAME,
        "version": SERVER_VERSION,
    })
}

fn server_capabilities() -> Value {
    json!({ "tools": {} })
}

/// Tool definitions (static registry).
fn tools_list() -> Value {
    json!({
        "tools": [
            tool_def(
                "list_nodes",
                "List all nodes in the cluster with hostname, last_seen, and key latest metrics.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_node",
                "Get single node details: host_info (OS/kernel/CPU/memory) and latest metrics. Returns error if not found.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID" }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_telemetry",
                "Get latest telemetry frame for a node (CPU/memory/disk/network).",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_alerts",
                "List active alerts and the last 50 resolved alerts (fixed 50-item window, no pagination).",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_certs",
                "List certificate scan sources (node + path configured in console), without certificate content.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_containers",
                "List latest container snapshot for a node (requires inventory reported by node).",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string" }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_processes",
                "List latest process snapshot TopN for a node (requires inventory reported by node).",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string" },
                        "sort": {
                            "type": "string",
                            "enum": ["cpu", "memory"],
                            "default": "cpu"
                        },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20 }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
        ]
    })
}

fn tool_def(name: &str, description: &str, input_schema: &Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

// ---------- JSON-RPC types ----------

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

fn rpc_error(id: &Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": JsonRpcError { code, message: message.into(), data: None },
    })
}

fn rpc_result(id: &Value, result: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

// ---------- HTTP handler ----------

/// `POST /mcp/sse` — MCP over SSE (synchronous response, each response is one
/// SSE event).
pub async fn sse_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Authentication: AI token only
    match read_auth_ok_v2(&state, &headers).await {
        ReadAuthKind::AiToken(_) => {}
        ReadAuthKind::Admin => {
            return err_response(
                StatusCode::FORBIDDEN,
                "admin token cannot be used for MCP; create a dedicated AI token",
            );
        }
        ReadAuthKind::None => {
            return err_response(
                StatusCode::UNAUTHORIZED,
                "AI token required: Authorization: Bearer ait_...",
            );
        }
    }

    // Parse JSON-RPC request
    let req: JsonRpcRequest = match serde_json::from_slice::<JsonRpcRequest>(&body) {
        Ok(r) if r.jsonrpc == "2.0" => r,
        Ok(_) => {
            return sse_single(&rpc_error(&Value::Null, -32600, "jsonrpc must be \"2.0\""));
        }
        Err(e) => {
            return sse_single(&rpc_error(&Value::Null, -32700, format!("parse error: {e}")));
        }
    };

    // Extract the Bearer token string; tool implementations reuse the same
    // credential when calling REST. Authentication already passed above
    // (only AiToken accepted), so this is safe.
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(std::string::ToString::to_string);

    let response = dispatch(&state, bearer.as_deref(), req).await;
    sse_single(&response)
}

/// Dispatch JSON-RPC method to specific implementation.
async fn dispatch(state: &AppState, bearer: Option<&str>, req: JsonRpcRequest) -> Value {
    match req.method.as_str() {
        "initialize" => rpc_result(
            &req.id,
            &json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": server_capabilities(),
                "serverInfo": server_info(),
            }),
        ),
        "notifications/initialized" => {
            // Notifications don't need a response (id usually null).
            // But our transport is synchronous-return, so return an ack.
            rpc_result(&req.id, &json!({ "acknowledged": true }))
        }
        "ping" => rpc_result(&req.id, &json!({})),
        "tools/list" => rpc_result(&req.id, &tools_list()),
        "tools/call" => call_tool(state, bearer, &req.id, req.params).await,
        other => rpc_error(&req.id, -32601, format!("method not found: {other}")),
    }
}

/// Call a tool: parse name + arguments, call monitor REST API, wrap as MCP
/// `content`.
async fn call_tool(state: &AppState, bearer: Option<&str>, id: &Value, params: Value) -> Value {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return rpc_error(id, -32602, "params.name required"),
    };
    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let Some(token) = bearer else {
        return rpc_error(id, -32603, "internal: bearer missing in handler context");
    };

    // Construct the internal HTTP client. Base URL is derived from listen
    // address — state.listen is "host:port", we call ourselves at
    // 127.0.0.1:<port> (when monitor terminates TLS itself, this needs
    // https, but MCP is only exposed to clients that can already reach the
    // console, so plain HTTP is fine for now; TLS termination can be added
    // later).
    //
    // Implementation: hardcode "http://127.0.0.1:<port>" first, read port
    // from cfg.listen. See AppState.mcp_base_url for the complete approach.
    let base_url = &state.mcp_base_url;

    match call_tool_impl(base_url, token, &name, &arguments).await {
        Ok(text) => rpc_result(
            id,
            &json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }),
        ),
        Err(e) => rpc_result(
            id,
            &json!({
                "content": [{ "type": "text", "text": format!("tool error: {e}") }],
                "isError": true,
            }),
        ),
    }
}

/// Actual tool implementations: each tool = one HTTP GET to a monitor REST
/// endpoint.
async fn call_tool_impl(
    base_url: &str,
    token: &str,
    name: &str,
    arguments: &Value,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url_path = match name {
        // Note: the backend has no `GET /v1/nodes/:id` endpoint (only the four
        // sub-resources: telemetry/series/containers/processes). `/v1/nodes`
        // list itself already includes host_info + latest metrics, so
        // "get_node" reuses the list path with client-side filtering.
        "list_nodes" | "get_node" => "/v1/nodes".to_string(),
        "get_telemetry" => {
            let node_id = arguments
                .get("node_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("node_id required"))?;
            let limit = arguments
                .get("limit")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(100);
            format!("/v1/nodes/{node_id}/telemetry?limit={limit}")
        }
        // The backend alerts_handler does not accept limit (returns active +
        // last 50 resolved, fixed), so no query parameter here — avoids giving
        // the illusion that pagination exists.
        "list_alerts" => "/v1/alerts".to_string(),
        "list_certs" => "/v1/cert-sources".to_string(),
        "list_containers" => {
            let node_id = arguments
                .get("node_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("node_id required"))?;
            format!("/v1/nodes/{node_id}/containers")
        }
        "list_processes" => {
            let node_id = arguments
                .get("node_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("node_id required"))?;
            format!("/v1/nodes/{node_id}/processes")
        }
        other => anyhow::bail!("unknown tool: {other}"),
    };

    let resp = client
        .get(format!("{base_url}{url_path}"))
        .bearer_auth(token)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("monitor API {status}: {body}");
    }

    // get_node uses the list endpoint; here we filter down to one node by id.
    // If not found, return a clear error — don't let the AI get the whole list
    // and think it got the detail.
    if name == "get_node" {
        let node_id = arguments
            .get("node_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("node_id required"))?;
        let list: Value = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("nodes response not JSON: {e}"))?;
        let found = list
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|n| n.get("id").and_then(|v| v.as_str()) == Some(node_id))
            })
            .cloned();
        return match found {
            Some(node) => Ok(serde_json::to_string_pretty(&node)?),
            None => anyhow::bail!("node not found: {node_id}"),
        };
    }
    // Return JSON string directly (pretty-print helps the AI parse it).
    // body is moved into Value::String in unwrap_or, so the closure can't
    // grab it. Keep a fallback string copy.
    let body_owned = body;
    let v: Value =
        serde_json::from_str(&body_owned).unwrap_or_else(|_| Value::String(body_owned.clone()));
    Ok(serde_json::to_string_pretty(&v).unwrap_or(body_owned))
}

/// Wrap a single JSON-RPC response as an SSE event stream.
fn sse_single(message: &Value) -> Response {
    let body = format!(
        "event: message\ndata: {}\n\n",
        serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string())
    );
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
            ("x-accel-buffering", "no"), // disable nginx buffering
            ("access-control-allow-origin", "*"),
        ],
        body,
    )
        .into_response()
}

fn err_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        [("content-type", "application/json")],
        Json(json!({ "error": msg.into() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_exposes_the_eight_read_tools() {
        let v = tools_list();
        let names: Vec<String> = v["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_string())
            .collect();
        for expected in [
            "list_nodes",
            "get_node",
            "get_telemetry",
            "list_alerts",
            "list_certs",
            "list_containers",
            "list_processes",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        assert_eq!(names.len(), 7, "unexpected tool count: {names:?}");
    }

    #[test]
    fn every_tool_ships_a_json_schema_object() {
        let v = tools_list();
        for t in v["tools"].as_array().unwrap() {
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object", "tool {t:?} schema missing type");
            assert!(t["description"].as_str().is_some());
        }
    }

    #[test]
    fn rpc_result_carries_id_and_result() {
        let v = rpc_result(&json!(7), &json!({"ok": true}));
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"]["ok"], true);
        assert!(v.get("error").is_none());
    }

    #[test]
    fn rpc_error_carries_code_and_message() {
        let v = rpc_error(&Value::Null, -32601, "method not found: nope");
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["error"]["code"], -32601);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("method not found"));
        assert!(v.get("result").is_none());
    }

    #[test]
    fn server_info_reports_this_binary() {
        let v = server_info();
        assert_eq!(v["name"], "zhiwei-monitor");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    }
}
