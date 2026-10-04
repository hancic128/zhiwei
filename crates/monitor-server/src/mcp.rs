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
#[allow(clippy::too_many_lines)]
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
            // ========== Node Management ==========
            tool_def(
                "update_node",
                "Update node alias and/or tags.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID to update" },
                        "alias": { "type": "string", "description": "New display name for the node" },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Replacement tag list"
                        }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "delete_node",
                "Delete a node from the cluster. Use force=true to remove even if it is currently online.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID to delete" },
                        "force": { "type": "boolean", "default": false, "description": "Force deletion even if node is online" }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Alert Rules ==========
            tool_def(
                "list_rules",
                "List all configured alert rules.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "create_rule",
                "Create a new alert rule. Expr is a PromQL-compatible expression.",
                &json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Rule name" },
                        "expr": { "type": "string", "description": "PromQL expression that evaluates to a boolean" },
                        "severity": { "type": "string", "enum": ["info", "warning", "critical"], "description": "Alert severity" },
                        "labels": {
                            "type": "object",
                            "additionalProperties": { "type": "string" },
                            "description": "Optional label key-value pairs"
                        }
                    },
                    "required": ["name", "expr", "severity"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_rule",
                "Update an existing alert rule.",
                &json!({
                    "type": "object",
                    "properties": {
                        "rule_id": { "type": "string", "description": "Rule ID to update" },
                        "name": { "type": "string", "description": "New rule name" },
                        "expr": { "type": "string", "description": "New PromQL expression" },
                        "severity": { "type": "string", "enum": ["info", "warning", "critical"] },
                        "labels": { "type": "object", "additionalProperties": { "type": "string" } }
                    },
                    "required": ["rule_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "delete_rule",
                "Delete an alert rule by ID.",
                &json!({
                    "type": "object",
                    "properties": {
                        "rule_id": { "type": "string", "description": "Rule ID to delete" }
                    },
                    "required": ["rule_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_builtin_rules",
                "List all built-in system alert rules (cannot be deleted, can be enabled/disabled).",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_builtin_rule",
                "Enable/disable a built-in rule or update its labels.",
                &json!({
                    "type": "object",
                    "properties": {
                        "rule_id": { "type": "string", "description": "Built-in rule ID" },
                        "enabled": { "type": "boolean", "description": "Enable or disable the rule" },
                        "labels": { "type": "object", "additionalProperties": { "type": "string" } }
                    },
                    "required": ["rule_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Alert Actions ==========
            tool_def(
                "silence_alert",
                "Silence an alert for a specified duration in minutes.",
                &json!({
                    "type": "object",
                    "properties": {
                        "alert_id": { "type": "string", "description": "Alert ID to silence" },
                        "minutes": { "type": "integer", "minimum": 1, "description": "Silence duration in minutes" }
                    },
                    "required": ["alert_id", "minutes"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "resolve_alert",
                "Manually resolve an alert (mark it as resolved).",
                &json!({
                    "type": "object",
                    "properties": {
                        "alert_id": { "type": "string", "description": "Alert ID to resolve" }
                    },
                    "required": ["alert_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Certificate Sources ==========
            tool_def(
                "create_cert_source",
                "Create a new certificate scan source on a node.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID where the cert file resides" },
                        "path": { "type": "string", "description": "Path to certificate file on the node" },
                        "command": { "type": "string", "description": "Optional command to fetch/renew the certificate" }
                    },
                    "required": ["node_id", "path"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "test_cert_source",
                "Test a certificate source configuration (validates path and optional command).",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID to test against" },
                        "path": { "type": "string", "description": "Certificate file path" },
                        "command": { "type": "string", "description": "Optional fetch command" }
                    },
                    "required": ["node_id", "path"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_cert_source",
                "Update a certificate source path or command.",
                &json!({
                    "type": "object",
                    "properties": {
                        "source_id": { "type": "string", "description": "Certificate source ID" },
                        "path": { "type": "string", "description": "New certificate file path" },
                        "command": { "type": "string", "description": "New fetch command" }
                    },
                    "required": ["source_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "delete_cert_source",
                "Delete a certificate source by ID.",
                &json!({
                    "type": "object",
                    "properties": {
                        "source_id": { "type": "string", "description": "Certificate source ID to delete" }
                    },
                    "required": ["source_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Notification Channels ==========
            tool_def(
                "list_channels",
                "List all notification channels.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "create_channel",
                "Create a new notification channel (webhook).",
                &json!({
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["webhook"], "description": "Channel type" },
                        "name": { "type": "string", "description": "Channel display name" },
                        "url": { "type": "string", "description": "Webhook URL to POST notifications to" },
                        "secret": { "type": "string", "description": "Optional HMAC secret for signing" }
                    },
                    "required": ["type", "name", "url"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "test_channel",
                "Send a test notification to a channel to verify it is working.",
                &json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Channel ID to test" }
                    },
                    "required": ["channel_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_channel",
                "Update a notification channel.",
                &json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Channel ID to update" },
                        "name": { "type": "string", "description": "New channel name" },
                        "url": { "type": "string", "description": "New webhook URL" },
                        "secret": { "type": "string", "description": "New HMAC secret" }
                    },
                    "required": ["channel_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "delete_channel",
                "Delete a notification channel by ID.",
                &json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "string", "description": "Channel ID to delete" }
                    },
                    "required": ["channel_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Services ==========
            tool_def(
                "list_services",
                "List all registered services and their targets.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "create_service",
                "Register a new service with target endpoints.",
                &json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Service name" },
                        "targets": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of target URLs or host:port endpoints"
                        }
                    },
                    "required": ["name", "targets"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_service",
                "Update a service name or targets.",
                &json!({
                    "type": "object",
                    "properties": {
                        "service_id": { "type": "string", "description": "Service ID to update" },
                        "name": { "type": "string", "description": "New service name" },
                        "targets": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["service_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "delete_service",
                "Delete a service by ID.",
                &json!({
                    "type": "object",
                    "properties": {
                        "service_id": { "type": "string", "description": "Service ID to delete" }
                    },
                    "required": ["service_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Probes ==========
            tool_def(
                "list_probes",
                "List all configured probes and their status.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "create_probe",
                "Create a new probe for a service.",
                &json!({
                    "type": "object",
                    "properties": {
                        "service_id": { "type": "string", "description": "Service ID to probe" },
                        "type": { "type": "string", "enum": ["http", "tcp", "icmp"], "description": "Probe type" },
                        "target": { "type": "string", "description": "Target URL or host:port" },
                        "interval": { "type": "integer", "minimum": 10, "description": "Check interval in seconds" }
                    },
                    "required": ["service_id", "type", "target"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "test_probe",
                "Test a probe configuration immediately without saving it.",
                &json!({
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["http", "tcp", "icmp"], "description": "Probe type" },
                        "target": { "type": "string", "description": "Target URL or host:port" }
                    },
                    "required": ["type", "target"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_probe",
                "Update a probe configuration.",
                &json!({
                    "type": "object",
                    "properties": {
                        "probe_id": { "type": "string", "description": "Probe ID to update" },
                        "type": { "type": "string", "enum": ["http", "tcp", "icmp"] },
                        "target": { "type": "string", "description": "New target" },
                        "interval": { "type": "integer", "minimum": 10 }
                    },
                    "required": ["probe_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "delete_probe",
                "Delete a probe by ID.",
                &json!({
                    "type": "object",
                    "properties": {
                        "probe_id": { "type": "string", "description": "Probe ID to delete" }
                    },
                    "required": ["probe_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_probe_results",
                "Get recent probe check results for a probe.",
                &json!({
                    "type": "object",
                    "properties": {
                        "probe_id": { "type": "string", "description": "Probe ID" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 }
                    },
                    "required": ["probe_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Command Execution ==========
            tool_def(
                "exec_command",
                "Execute a shell command on a node and return the output.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Target node ID" },
                        "command": { "type": "string", "description": "Shell command to execute" }
                    },
                    "required": ["node_id", "command"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_command_history",
                "List command execution history, optionally filtered by node.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Filter by node ID (optional)" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 }
                    },
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_command",
                "Get the result of a specific command execution by ID.",
                &json!({
                    "type": "object",
                    "properties": {
                        "command_id": { "type": "string", "description": "Command execution ID" }
                    },
                    "required": ["command_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Other ==========
            tool_def(
                "get_todo",
                "Get the current todo list and task status.",
                &json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_retention",
                "Get data retention policy and current storage usage.",
                &json!({
                    "type": "object",
                    "properties": {},
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

// ---------- HTTP handlers ----------

/// `GET /mcp/sse` — Health check. Returns server info in SSE format.
pub async fn health_handler() -> Response {
    let body = json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": server_capabilities(),
        "serverInfo": server_info(),
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
            ("access-control-allow-origin", "*"),
        ],
        format!("event: message\ndata: {body_str}\n\n"),
    )
        .into_response()
}

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
            return sse_single(&rpc_error(
                &Value::Null,
                -32700,
                format!("parse error: {e}"),
            ));
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
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
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

// ---------- Helper functions for tool implementations ----------

/// Extract optional string field from arguments.
fn opt_str(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract optional i64 field from arguments.
fn opt_i64(arguments: &Value, key: &str) -> Option<i64> {
    arguments.get(key).and_then(serde_json::Value::as_i64)
}

/// Extract required string field from arguments.
fn req_str(arguments: &Value, key: &str) -> anyhow::Result<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

/// HTTP GET request helper.
async fn http_get(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    path: &str,
) -> anyhow::Result<String> {
    let resp = client
        .get(format!("{base_url}{path}"))
        .bearer_auth(token)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("monitor API {status}: {body}");
    }
    Ok(body)
}

/// HTTP request helper that handles any method with optional JSON body.
async fn http_request(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
) -> anyhow::Result<String> {
    let req = client
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(token);
    let req = if let Some(b) = body { req.json(b) } else { req };
    let resp = req.send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("monitor API {status}: {body}");
    }
    Ok(body)
}

/// Pretty-print JSON response, or return raw string if not valid JSON.
fn pretty_json(body: String) -> String {
    serde_json::from_str::<Value>(&body)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| body.clone()))
        .unwrap_or(body)
}

// ---------- Tool implementations ----------

/// Actual tool implementations: each tool = one HTTP call to a monitor REST endpoint.
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
async fn call_tool_impl(
    base_url: &str,
    token: &str,
    name: &str,
    arguments: &Value,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let result = match name {
        // ========== Existing read-only tools ==========
        "list_nodes" | "get_node" => {
            let body = http_get(&client, base_url, token, "/v1/nodes").await?;
            // get_node filters client-side
            if name == "get_node" {
                let node_id = req_str(arguments, "node_id")?;
                let list: Value = serde_json::from_str(&body)
                    .map_err(|e| anyhow::anyhow!("nodes response not JSON: {e}"))?;
                let found = list
                    .as_array()
                    .and_then(|arr| {
                        arr.iter()
                            .find(|n| n.get("id").and_then(|v| v.as_str()) == Some(&node_id))
                    })
                    .cloned();
                match found {
                    Some(node) => serde_json::to_string_pretty(&node)?,
                    None => anyhow::bail!("node not found: {node_id}"),
                }
            } else {
                pretty_json(body)
            }
        }
        "get_telemetry" => {
            let node_id = req_str(arguments, "node_id")?;
            let limit = opt_i64(arguments, "limit").unwrap_or(100);
            let body = http_get(
                &client,
                base_url,
                token,
                &format!("/v1/nodes/{node_id}/telemetry?limit={limit}"),
            )
            .await?;
            pretty_json(body)
        }
        "list_alerts" => {
            let body = http_get(&client, base_url, token, "/v1/alerts").await?;
            pretty_json(body)
        }
        "list_certs" => {
            let body = http_get(&client, base_url, token, "/v1/cert-sources").await?;
            pretty_json(body)
        }
        "list_containers" => {
            let node_id = req_str(arguments, "node_id")?;
            let body = http_get(
                &client,
                base_url,
                token,
                &format!("/v1/nodes/{node_id}/containers"),
            )
            .await?;
            pretty_json(body)
        }
        "list_processes" => {
            let node_id = req_str(arguments, "node_id")?;
            let body = http_get(
                &client,
                base_url,
                token,
                &format!("/v1/nodes/{node_id}/processes"),
            )
            .await?;
            pretty_json(body)
        }

        // ========== Node Management ==========
        "update_node" => {
            let node_id = req_str(arguments, "node_id")?;
            let mut map = serde_json::Map::new();
            if let Some(alias) = opt_str(arguments, "alias") {
                map.insert("alias".to_string(), json!(alias));
            }
            if let Some(tags) = arguments.get("tags") {
                map.insert("tags".to_string(), tags.clone());
            }
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/nodes/{node_id}"),
                Some(&json!(map)),
            )
            .await?;
            pretty_json(body)
        }
        "delete_node" => {
            let node_id = req_str(arguments, "node_id")?;
            let force = opt_str(arguments, "force").unwrap_or_default();
            let path = if force == "true" || force == "1" {
                format!("/v1/nodes/{node_id}?force=true")
            } else {
                format!("/v1/nodes/{node_id}")
            };
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::DELETE,
                &path,
                None,
            )
            .await?;
            pretty_json(body)
        }

        // ========== Alert Rules ==========
        "list_rules" => {
            let body = http_get(&client, base_url, token, "/v1/rules").await?;
            pretty_json(body)
        }
        "create_rule" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/rules",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "update_rule" => {
            let rule_id = req_str(arguments, "rule_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/rules/{rule_id}"),
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "delete_rule" => {
            let rule_id = req_str(arguments, "rule_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::DELETE,
                &format!("/v1/rules/{rule_id}"),
                None,
            )
            .await?;
            pretty_json(body)
        }
        "list_builtin_rules" => {
            let body = http_get(&client, base_url, token, "/v1/builtin-alerts").await?;
            pretty_json(body)
        }
        "update_builtin_rule" => {
            let rule_id = req_str(arguments, "rule_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/builtin-alerts/{rule_id}"),
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }

        // ========== Alert Actions ==========
        "silence_alert" => {
            let alert_id = req_str(arguments, "alert_id")?;
            let minutes = opt_i64(arguments, "minutes").unwrap_or(60);
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                &format!("/v1/alerts/{alert_id}/silence"),
                Some(&json!({ "minutes": minutes })),
            )
            .await?;
            pretty_json(body)
        }
        "resolve_alert" => {
            let alert_id = req_str(arguments, "alert_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                &format!("/v1/alerts/{alert_id}/resolve"),
                None,
            )
            .await?;
            pretty_json(body)
        }

        // ========== Certificate Sources ==========
        "create_cert_source" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/cert-sources",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "test_cert_source" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/cert-sources/test",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "update_cert_source" => {
            let source_id = req_str(arguments, "source_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/cert-sources/{source_id}"),
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "delete_cert_source" => {
            let source_id = req_str(arguments, "source_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::DELETE,
                &format!("/v1/cert-sources/{source_id}"),
                None,
            )
            .await?;
            pretty_json(body)
        }

        // ========== Notification Channels ==========
        "list_channels" => {
            let body = http_get(&client, base_url, token, "/v1/channels").await?;
            pretty_json(body)
        }
        "create_channel" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/channels",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "test_channel" => {
            let channel_id = req_str(arguments, "channel_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                &format!("/v1/channels/{channel_id}/test"),
                None,
            )
            .await?;
            pretty_json(body)
        }
        "update_channel" => {
            let channel_id = req_str(arguments, "channel_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/channels/{channel_id}"),
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "delete_channel" => {
            let channel_id = req_str(arguments, "channel_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::DELETE,
                &format!("/v1/channels/{channel_id}"),
                None,
            )
            .await?;
            pretty_json(body)
        }

        // ========== Services ==========
        "list_services" => {
            let body = http_get(&client, base_url, token, "/v1/services").await?;
            pretty_json(body)
        }
        "create_service" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/services",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "update_service" => {
            let service_id = req_str(arguments, "service_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/services/{service_id}"),
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "delete_service" => {
            let service_id = req_str(arguments, "service_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::DELETE,
                &format!("/v1/services/{service_id}"),
                None,
            )
            .await?;
            pretty_json(body)
        }

        // ========== Probes ==========
        "list_probes" => {
            let body = http_get(&client, base_url, token, "/v1/probes").await?;
            pretty_json(body)
        }
        "create_probe" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/probes",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "test_probe" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/probes/test",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "update_probe" => {
            let probe_id = req_str(arguments, "probe_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::PATCH,
                &format!("/v1/probes/{probe_id}"),
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "delete_probe" => {
            let probe_id = req_str(arguments, "probe_id")?;
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::DELETE,
                &format!("/v1/probes/{probe_id}"),
                None,
            )
            .await?;
            pretty_json(body)
        }
        "get_probe_results" => {
            let probe_id = req_str(arguments, "probe_id")?;
            let limit = opt_i64(arguments, "limit").unwrap_or(100);
            let body = http_get(
                &client,
                base_url,
                token,
                &format!("/v1/probes/{probe_id}/results?limit={limit}"),
            )
            .await?;
            pretty_json(body)
        }

        // ========== Command Execution ==========
        "exec_command" => {
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/exec",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "list_command_history" => {
            let node_id = opt_str(arguments, "node_id");
            let limit = opt_i64(arguments, "limit").unwrap_or(50);
            let path = node_id.map_or_else(
                || format!("/v1/commands/history?limit={limit}"),
                |nid| format!("/v1/commands/history?node_id={nid}&limit={limit}"),
            );
            let body = http_get(&client, base_url, token, &path).await?;
            pretty_json(body)
        }
        "get_command" => {
            let command_id = req_str(arguments, "command_id")?;
            let body = http_get(
                &client,
                base_url,
                token,
                &format!("/v1/commands/{command_id}"),
            )
            .await?;
            pretty_json(body)
        }

        // ========== Other ==========
        "get_todo" => {
            let body = http_get(&client, base_url, token, "/v1/todo").await?;
            pretty_json(body)
        }
        "get_retention" => {
            let body = http_get(&client, base_url, token, "/v1/retention").await?;
            pretty_json(body)
        }

        other => anyhow::bail!("unknown tool: {other}"),
    };

    Ok(result)
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
    fn tools_list_exposes_all_tools() {
        let names = extract_tool_names();
        check_readonly_tools(&names);
        check_node_tools(&names);
        check_rule_tools(&names);
        check_alert_tools(&names);
        check_cert_tools(&names);
        check_channel_tools(&names);
        check_service_tools(&names);
        check_probe_tools(&names);
        check_command_tools(&names);
        check_other_tools(&names);
        // Total: 7 original + 34 new = 41 tools
        assert_eq!(names.len(), 41, "unexpected tool count: {}", names.len());
    }

    fn extract_tool_names() -> Vec<String> {
        let v = tools_list();
        v["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn check_readonly_tools(names: &[String]) {
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
    }

    fn check_node_tools(names: &[String]) {
        assert!(
            names.contains(&"update_node".to_string()),
            "missing update_node"
        );
        assert!(
            names.contains(&"delete_node".to_string()),
            "missing delete_node"
        );
    }

    fn check_rule_tools(names: &[String]) {
        for tool in [
            "list_rules",
            "create_rule",
            "update_rule",
            "delete_rule",
            "list_builtin_rules",
            "update_builtin_rule",
        ] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    fn check_alert_tools(names: &[String]) {
        assert!(
            names.contains(&"silence_alert".to_string()),
            "missing silence_alert"
        );
        assert!(
            names.contains(&"resolve_alert".to_string()),
            "missing resolve_alert"
        );
    }

    fn check_cert_tools(names: &[String]) {
        for tool in [
            "create_cert_source",
            "test_cert_source",
            "update_cert_source",
            "delete_cert_source",
        ] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    fn check_channel_tools(names: &[String]) {
        for tool in [
            "list_channels",
            "create_channel",
            "test_channel",
            "update_channel",
            "delete_channel",
        ] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    fn check_service_tools(names: &[String]) {
        for tool in [
            "list_services",
            "create_service",
            "update_service",
            "delete_service",
        ] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    fn check_probe_tools(names: &[String]) {
        for tool in [
            "list_probes",
            "create_probe",
            "test_probe",
            "update_probe",
            "delete_probe",
            "get_probe_results",
        ] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    fn check_command_tools(names: &[String]) {
        for tool in ["exec_command", "list_command_history", "get_command"] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    fn check_other_tools(names: &[String]) {
        assert!(names.contains(&"get_todo".to_string()), "missing get_todo");
        assert!(
            names.contains(&"get_retention".to_string()),
            "missing get_retention"
        );
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
