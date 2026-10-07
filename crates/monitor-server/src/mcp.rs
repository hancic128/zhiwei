//! MCP (Model Context Protocol) server implementation.
//!
//! Exposes monitor data and management operations to external AI clients
//! (Claude Desktop / Cursor / Cline, etc.) via the SSE transport. Most tools are
//! read-only; management tools (rules / channels / probes / cert sources /
//! node delete / whitelisted control commands) are also exposed so an AI client
//! can do what the console can. Arbitrary shell is never possible: commands go
//! through the ops-signed whitelist in `proto/control.proto`.
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
                "List active alerts and resolved alerts (supports filtering by time range, status, and source).",
                &json!({
                    "type": "object",
                    "properties": {
                        "since": { "type": "integer", "description": "Start time in Unix milliseconds (optional)" },
                        "until": { "type": "integer", "description": "End time in Unix milliseconds (optional)" },
                        "status": { "type": "string", "enum": ["open", "resolved", "all"], "description": "Filter by alert status (default: all)" },
                        "sources": { "type": "string", "description": "Comma-separated source types: rule,probe,cert,node_offline,container" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50, "description": "Maximum number of resolved alerts to return" },
                        "offset": { "type": "integer", "minimum": 0, "description": "Pagination offset for resolved alerts" }
                    },
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
                        "node_id": { "type": "string" }
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
                "Permanently delete a node and all data referencing it (telemetry / inventory / probe results / cert sources / command history / alerts). Fails with 409 if the node still has unsent commands unless force=true, in which case those commands are voided and the delete is idempotent.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID to delete" },
                        "force": { "type": "boolean", "default": false, "description": "Void pending commands and delete even if the node has unsent commands (also makes delete idempotent)" }
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
                "Create a new alert rule: fire when `metric` `op` `threshold` holds for `duration_seconds`. Severity must be warning or critical.",
                &json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Rule name" },
                        "metric": { "type": "string", "description": "Metric name, e.g. host.cpu.usage / host.mem.usage / host.disk.usage / host.net.rx_bytes" },
                        "op": { "type": "string", "enum": ["gt", "gte", "lt", "lte", "eq"], "description": "Comparison operator" },
                        "threshold": { "type": "number", "description": "Threshold value the metric is compared against" },
                        "duration_seconds": { "type": "integer", "minimum": 0, "default": 0, "description": "How long the condition must hold before firing (0 = fire immediately)" },
                        "severity": { "type": "string", "enum": ["warning", "critical"], "default": "warning", "description": "Alert severity" }
                    },
                    "required": ["name", "metric", "op", "threshold"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_rule",
                "Update an existing alert rule. Only the provided fields are changed.",
                &json!({
                    "type": "object",
                    "properties": {
                        "rule_id": { "type": "integer", "description": "Numeric rule ID to update" },
                        "name": { "type": "string", "description": "New rule name" },
                        "metric": { "type": "string", "description": "New metric name" },
                        "op": { "type": "string", "enum": ["gt", "gte", "lt", "lte", "eq"] },
                        "threshold": { "type": "number" },
                        "duration_seconds": { "type": "integer", "minimum": 0 },
                        "severity": { "type": "string", "enum": ["warning", "critical"] },
                        "enabled": { "type": "boolean", "description": "Enable or disable the rule" }
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
                        "rule_id": { "type": "integer", "description": "Numeric rule ID to delete" }
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
                "Enable/disable a built-in rule or tune its threshold and duration.",
                &json!({
                    "type": "object",
                    "properties": {
                        "rule_id": { "type": "string", "description": "Built-in rule ID (e.g. node_offline, node_online)" },
                        "enabled": { "type": "boolean", "description": "Enable or disable the rule" },
                        "threshold": { "type": "number", "description": "New threshold value" },
                        "duration_seconds": { "type": "integer", "minimum": 0, "description": "Seconds the condition must hold before firing" }
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
                "Create a certificate scan source: a path on a node (empty node_id = all nodes). Optionally configure expiry notifications.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID where the cert path resides; empty string = apply to all nodes" },
                        "path": { "type": "string", "description": "Path to a certificate file or directory on the node (non-recursive)" },
                        "notify_enabled": { "type": "boolean", "default": true, "description": "Send expiry notifications for matched certificates" },
                        "notify_days_before": { "type": "integer", "minimum": 1, "maximum": 365, "default": 30, "description": "Notify this many days before expiry" }
                    },
                    "required": ["path"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "test_cert_source",
                "Have the node scan a certificate path immediately (without saving it). Returns a command_id to poll via get_command.",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Node ID to scan on" },
                        "path": { "type": "string", "description": "Certificate file or directory path on that node" }
                    },
                    "required": ["node_id", "path"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_cert_source",
                "Update a certificate source's scope, path, enabled flag, or notification settings. Only the provided fields change.",
                &json!({
                    "type": "object",
                    "properties": {
                        "source_id": { "type": "string", "description": "Certificate source ID" },
                        "node_id": { "type": "string", "description": "New owning node ID; empty string = all nodes" },
                        "path": { "type": "string", "description": "New certificate file or directory path" },
                        "enabled": { "type": "boolean", "description": "Enable or disable the source" },
                        "notify_enabled": { "type": "boolean", "description": "Toggle expiry notifications" },
                        "notify_days_before": { "type": "integer", "minimum": 1, "maximum": 365, "description": "Notify this many days before expiry" }
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
                "Create a notification channel. `kind` selects the required fields: feishu needs app_id + secret (App Secret) + receive_id; slack/webhook/bluebird need url (secret is an optional signing token).",
                &json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Channel display name" },
                        "kind": { "type": "string", "enum": ["feishu", "slack", "bluebird", "webhook"], "default": "webhook", "description": "Notification channel type" },
                        "url": { "type": "string", "description": "Webhook URL (slack / webhook / bluebird; feishu leaves this empty)" },
                        "secret": { "type": "string", "description": "Feishu App Secret, or optional HMAC / Bearer token for webhook delivery" },
                        "app_id": { "type": "string", "description": "Feishu App ID (required for feishu)" },
                        "receive_id": { "type": "string", "description": "Feishu receive ID: group chat_id or user open_id (required for feishu)" },
                        "receive_id_type": { "type": "string", "enum": ["chat_id", "open_id", "user_id", "union_id", "email"], "default": "chat_id", "description": "Feishu receive ID type" },
                        "min_severity": { "type": "string", "enum": ["info", "warning", "critical"], "default": "warning", "description": "Only notify for alerts at or above this severity" }
                    },
                    "required": ["name"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "test_channel",
                "Send a test notification using the given channel parameters (does not require the channel to be saved yet).",
                &json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["feishu", "slack", "bluebird", "webhook"], "description": "Notification channel type" },
                        "url": { "type": "string", "description": "Webhook URL (slack / webhook / bluebird)" },
                        "secret": { "type": "string", "description": "Feishu App Secret, or optional webhook token" },
                        "app_id": { "type": "string", "description": "Feishu App ID" },
                        "receive_id": { "type": "string", "description": "Feishu receive ID (chat_id / open_id)" },
                        "receive_id_type": { "type": "string", "enum": ["chat_id", "open_id", "user_id", "union_id", "email"], "description": "Feishu receive ID type" }
                    },
                    "required": ["kind"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_channel",
                "Update a notification channel. Only the provided fields change; an empty `secret` keeps the existing value.",
                &json!({
                    "type": "object",
                    "properties": {
                        "channel_id": { "type": "integer", "description": "Numeric channel ID to update" },
                        "name": { "type": "string", "description": "New channel name" },
                        "url": { "type": "string", "description": "New webhook URL" },
                        "secret": { "type": "string", "description": "New credential (empty string keeps the existing value)" },
                        "app_id": { "type": "string", "description": "New Feishu App ID" },
                        "receive_id": { "type": "string", "description": "New Feishu receive ID" },
                        "receive_id_type": { "type": "string", "enum": ["chat_id", "open_id", "user_id", "union_id", "email"] },
                        "min_severity": { "type": "string", "enum": ["info", "warning", "critical"] },
                        "enabled": { "type": "boolean", "description": "Enable or disable the channel" }
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
                        "channel_id": { "type": "integer", "description": "Numeric channel ID to delete" }
                    },
                    "required": ["channel_id"],
                    "additionalProperties": false,
                }),
            ),
            // ========== Services (timeline) ==========
            tool_def(
                "get_services_timeline",
                "Get service/probe health over time as bucketed up/degraded/down counts. level=service groups by service name, level=probe by individual probe.",
                &json!({
                    "type": "object",
                    "properties": {
                        "from": { "type": "integer", "description": "Start time in Unix milliseconds (default: 1 hour ago)" },
                        "to": { "type": "integer", "description": "End time in Unix milliseconds (default: now)" },
                        "buckets": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Number of time buckets" },
                        "level": { "type": "string", "enum": ["service", "probe"], "default": "service", "description": "Aggregate by service or by probe" }
                    },
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
                "Create a new probe (top-level entity). kind=http requires target_json {\"url\":\"https://...\"}; kind=tcp/tls requires {\"host\":\"...\",\"port\":N}. expect_json is optional assertions (e.g. status code / body contains).",
                &json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Probe name" },
                        "description": { "type": "string", "description": "Optional description" },
                        "kind": { "type": "string", "enum": ["http", "tcp", "tls"], "description": "Probe type" },
                        "target_json": { "type": "string", "description": "JSON string of the target, e.g. {\"url\":\"https://example.com\"} or {\"host\":\"1.2.3.4\",\"port\":443}" },
                        "expect_json": { "type": "string", "description": "JSON string of expectations/assertions (optional)" },
                        "interval_seconds": { "type": "integer", "minimum": 10, "maximum": 86400, "default": 60, "description": "Check interval in seconds" },
                        "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 60000, "default": 5000 },
                        "failure_threshold": { "type": "integer", "minimum": 1, "maximum": 100, "default": 3, "description": "Consecutive failures before the probe is marked down" },
                        "node_ids": { "type": "array", "items": { "type": "string" }, "description": "Node IDs that should execute this probe; empty/omitted = any node" },
                        "enabled": { "type": "boolean", "default": true }
                    },
                    "required": ["name", "kind", "target_json"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "test_probe",
                "Run a probe configuration once without saving it. Same target/expect validation as create_probe.",
                &json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["http", "tcp", "tls"], "description": "Probe type" },
                        "target_json": { "type": "string", "description": "JSON string of the target to test" },
                        "expect_json": { "type": "string", "description": "JSON string of expectations (optional)" },
                        "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 60000, "default": 5000 }
                    },
                    "required": ["kind", "target_json"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "update_probe",
                "Update a probe configuration. Only the provided fields change; changing target_json requires kind to be given as well.",
                &json!({
                    "type": "object",
                    "properties": {
                        "probe_id": { "type": "string", "description": "Probe ID to update" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "kind": { "type": "string", "enum": ["http", "tcp", "tls"] },
                        "target_json": { "type": "string", "description": "New target JSON (requires kind)" },
                        "expect_json": { "type": "string" },
                        "interval_seconds": { "type": "integer", "minimum": 10, "maximum": 86400 },
                        "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 60000 },
                        "failure_threshold": { "type": "integer", "minimum": 1, "maximum": 100 },
                        "node_ids": { "type": "array", "items": { "type": "string" }, "description": "Replacement node binding; empty array = any node" },
                        "enabled": { "type": "boolean" }
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
            //
            // There is no arbitrary-shell action on purpose: nodes only run a fixed, signed
            // whitelist (see proto/control.proto `Action`). Exposing `action` + `params` keeps
            // the MCP surface identical to what the console can do — nothing more.
            tool_def(
                "exec_command",
                "Issue a control command to a node (whitelisted actions only; runs on a later poll, returns a command_id to poll via get_command)",
                &json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "Target node ID" },
                        "action": {
                            "type": "string",
                            "enum": [
                                "noop", "fetch_logs", "kill_process", "restart_host", "shutdown_host",
                                "container_start", "container_stop", "container_restart", "container_remove",
                                "refresh_inventory", "scan_certs", "upgrade_agent", "rollback_agent"
                            ],
                            "description": "Whitelisted action to run on the node"
                        },
                        "params": {
                            "type": "object",
                            "description": "Action parameters (e.g. fetch_logs: {container|path, tail}; container_*: {container, force}; kill_process: {pid, signal}; scan_certs: {path}; upgrade_agent: {version, download_url, sha256})"
                        }
                    },
                    "required": ["node_id", "action"],
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
                "Get the current todo list and task status (supports filtering by time range, status, and source).",
                &json!({
                    "type": "object",
                    "properties": {
                        "since": { "type": "integer", "description": "Start time in Unix milliseconds (optional)" },
                        "until": { "type": "integer", "description": "End time in Unix milliseconds (optional)" },
                        "status": { "type": "string", "enum": ["open", "resolved", "all"], "description": "Filter by alert status (default: all)" },
                        "sources": { "type": "string", "description": "Comma-separated source types: rule,probe,cert,node_offline,container" },
                        "page": { "type": "integer", "minimum": 0, "default": 0, "description": "Page number (0-indexed)" },
                        "page_size": { "type": "integer", "minimum": 1, "maximum": 100, "default": 5, "description": "Items per page" },
                        "offset": { "type": "integer", "minimum": 0, "description": "Row offset (alternative to page)" }
                    },
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

/// Extract a required identifier that may arrive as a JSON string or number, normalized to
/// the string the REST path expects. Numeric IDs (rule / channel / alert) are advertised as
/// `integer`, but clients sometimes send them quoted; both must work.
fn req_id(arguments: &Value, key: &str) -> anyhow::Result<String> {
    match arguments.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => anyhow::bail!("{key} is required"),
    }
}

/// Build the `/v1/exec` request body from MCP tool arguments.
///
/// Kept separate from the dispatch so the "arguments → server body" mapping is unit-testable:
/// the previous bug was exactly a mismatch here (args used `command`, server expects `action`).
fn exec_body(arguments: &Value) -> anyhow::Result<Value> {
    Ok(json!({
        "node_id": req_str(arguments, "node_id")?,
        "action": req_str(arguments, "action")?,
        "params": arguments.get("params").cloned().unwrap_or_else(|| json!({})),
    }))
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
            // Build query params
            let mut params = Vec::new();
            if let Some(since) = opt_i64(arguments, "since") {
                params.push(format!("since={since}"));
            }
            if let Some(until) = opt_i64(arguments, "until") {
                params.push(format!("until={until}"));
            }
            if let Some(status) = opt_str(arguments, "status") {
                params.push(format!("status={status}"));
            }
            if let Some(sources) = opt_str(arguments, "sources") {
                params.push(format!("sources={sources}"));
            }
            if let Some(limit) = opt_i64(arguments, "limit") {
                params.push(format!("limit={limit}"));
            }
            if let Some(offset) = opt_i64(arguments, "offset") {
                params.push(format!("offset={offset}"));
            }
            let query = if params.is_empty() {
                String::new()
            } else {
                format!("?{}", params.join("&"))
            };
            let body = http_get(&client, base_url, token, &format!("/v1/alerts{query}")).await?;
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
            let force = arguments
                .get("force")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let path = if force {
                format!("/v1/nodes/{node_id}?force=1")
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
            let rule_id = req_id(arguments, "rule_id")?;
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
            let rule_id = req_id(arguments, "rule_id")?;
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
            let rule_id = req_id(arguments, "rule_id")?;
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
            let alert_id = req_id(arguments, "alert_id")?;
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
            let alert_id = req_id(arguments, "alert_id")?;
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
            // node_id is optional at the MCP surface (empty = all nodes) but the REST handler
            // requires the key, so always send it rather than 400-ing on an all-nodes source.
            let mut body = arguments.clone();
            if let Some(map) = body.as_object_mut() {
                map.insert(
                    "node_id".to_string(),
                    json!(opt_str(arguments, "node_id").unwrap_or_default()),
                );
            }
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/cert-sources",
                Some(&body),
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
            // Params, not a saved channel id: the server's /v1/channels/test takes the same
            // kind/url/secret/app_id/receive_id fields and sends one message through the real
            // delivery path, so a passing test means real alerts will go out too.
            let body = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/channels/test",
                Some(arguments),
            )
            .await?;
            pretty_json(body)
        }
        "update_channel" => {
            let channel_id = req_id(arguments, "channel_id")?;
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
            let channel_id = req_id(arguments, "channel_id")?;
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

        // ========== Services (timeline) ==========
        "get_services_timeline" => {
            let mut params = Vec::new();
            if let Some(from) = opt_i64(arguments, "from") {
                params.push(format!("from={from}"));
            }
            if let Some(to) = opt_i64(arguments, "to") {
                params.push(format!("to={to}"));
            }
            if let Some(buckets) = opt_i64(arguments, "buckets") {
                params.push(format!("buckets={buckets}"));
            }
            if let Some(level) = opt_str(arguments, "level") {
                params.push(format!("level={level}"));
            }
            let query = if params.is_empty() {
                String::new()
            } else {
                format!("?{}", params.join("&"))
            };
            let body = http_get(
                &client,
                base_url,
                token,
                &format!("/v1/services/timeline{query}"),
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
            // Forward the whitelisted action + params verbatim; the server validates and the
            // node re-validates before running, so an unknown action surfaces as an error here.
            let body = exec_body(arguments)?;
            let out = http_request(
                &client,
                base_url,
                token,
                reqwest::Method::POST,
                "/v1/exec",
                Some(&body),
            )
            .await?;
            pretty_json(out)
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
            // Build query params
            let mut params = Vec::new();
            if let Some(since) = opt_i64(arguments, "since") {
                params.push(format!("since={since}"));
            }
            if let Some(until) = opt_i64(arguments, "until") {
                params.push(format!("until={until}"));
            }
            if let Some(status) = opt_str(arguments, "status") {
                params.push(format!("status={status}"));
            }
            if let Some(sources) = opt_str(arguments, "sources") {
                params.push(format!("sources={sources}"));
            }
            if let Some(page) = opt_i64(arguments, "page") {
                params.push(format!("page={page}"));
            }
            if let Some(page_size) = opt_i64(arguments, "page_size") {
                params.push(format!("page_size={page_size}"));
            }
            if let Some(offset) = opt_i64(arguments, "offset") {
                params.push(format!("offset={offset}"));
            }
            let query = if params.is_empty() {
                String::new()
            } else {
                format!("?{}", params.join("&"))
            };
            let body = http_get(&client, base_url, token, &format!("/v1/todo{query}")).await?;
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
        check_timeline_tools(&names);
        check_probe_tools(&names);
        check_command_tools(&names);
        check_other_tools(&names);
        // 7 read-only + 2 node + 6 rule + 2 alert + 4 cert + 5 channel + 1 timeline
        // + 6 probe + 3 command + 2 other = 38 tools (service CRUD dropped with the service layer)
        assert_eq!(names.len(), 38, "unexpected tool count: {}", names.len());
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

    fn check_timeline_tools(names: &[String]) {
        assert!(
            names.contains(&"get_services_timeline".to_string()),
            "missing get_services_timeline"
        );
        // The service CRUD layer was removed (design spec 2026-10-04); probes are top-level.
        for gone in [
            "list_services",
            "create_service",
            "update_service",
            "delete_service",
        ] {
            assert!(
                !names.contains(&gone.to_string()),
                "stale service tool {gone}"
            );
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

    /// `exec_command` must speak the server's vocabulary (`action` + `params`), not a shell
    /// `command` string — the mismatch 400'd every call. Guard the schema and the body shape.
    #[test]
    fn exec_command_uses_whitelisted_action_not_shell() {
        let v = tools_list();
        let tool = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "exec_command")
            .expect("exec_command tool missing");
        let props = &tool["inputSchema"]["properties"];
        assert!(props.get("action").is_some(), "exec_command needs `action`");
        assert!(
            props.get("command").is_none(),
            "exec_command must not advertise a raw shell `command`"
        );
        let required = tool["inputSchema"]["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r == "node_id"));
        assert!(required.iter().any(|r| r == "action"));

        let body = exec_body(&json!({
            "node_id": "n1",
            "action": "fetch_logs",
            "params": {"container": "web", "tail": 50}
        }))
        .unwrap();
        assert_eq!(body["node_id"], "n1");
        assert_eq!(body["action"], "fetch_logs");
        assert_eq!(body["params"]["tail"], 50);

        // params is optional → defaults to an empty object the server accepts
        let body = exec_body(&json!({"node_id": "n1", "action": "noop"})).unwrap();
        assert_eq!(body["params"], json!({}));

        // missing action is a hard error, not a silently-empty command
        assert!(exec_body(&json!({"node_id": "n1"})).is_err());
    }

    /// Rules are metric/op/threshold based on the server — not a `PromQL` `expr`. Guard the schema
    /// against a regression to the old MCP-only vocabulary.
    #[test]
    fn create_rule_speaks_metric_op_threshold_not_promql() {
        let v = tools_list();
        let tool = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "create_rule")
            .expect("create_rule tool missing");
        let props = &tool["inputSchema"]["properties"];
        for expected in ["metric", "op", "threshold", "duration_seconds"] {
            assert!(
                props.get(expected).is_some(),
                "create_rule needs `{expected}`"
            );
        }
        assert!(
            props.get("expr").is_none(),
            "create_rule must not advertise `expr`"
        );
        let required = tool["inputSchema"]["required"].as_array().unwrap();
        for r in ["name", "metric", "op", "threshold"] {
            assert!(
                required.iter().any(|v| v == r),
                "create_rule must require `{r}`"
            );
        }
    }

    /// Channels are discriminated by `kind` (feishu/slack/bluebird/webhook) with a
    /// `min_severity` filter — not the old `type: webhook` + url.
    #[test]
    fn channel_tools_use_kind_and_min_severity() {
        let v = tools_list();
        let create = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "create_channel")
            .expect("create_channel missing");
        let props = &create["inputSchema"]["properties"];
        assert!(props.get("kind").is_some(), "create_channel needs `kind`");
        assert!(
            props.get("min_severity").is_some(),
            "create_channel needs `min_severity`"
        );
        assert!(
            props.get("type").is_none(),
            "create_channel must not advertise `type`"
        );
        let kinds = props["kind"]["enum"].as_array().unwrap();
        for k in ["feishu", "slack", "bluebird", "webhook"] {
            assert!(kinds.iter().any(|v| v == k), "kind enum missing {k}");
        }

        // test_channel takes params, not a saved channel_id
        let test = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "test_channel")
            .expect("test_channel missing");
        let tprops = &test["inputSchema"]["properties"];
        assert!(tprops.get("kind").is_some(), "test_channel needs `kind`");
        assert!(
            tprops.get("channel_id").is_none(),
            "test_channel must not take `channel_id` (server sends by params)"
        );
    }

    /// Probes are top-level with a JSON target, not the old flat `type`/`target` string.
    #[test]
    fn probe_tools_use_kind_and_json_target() {
        let v = tools_list();
        let create = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "create_probe")
            .expect("create_probe missing");
        let props = &create["inputSchema"]["properties"];
        assert!(
            props.get("target_json").is_some(),
            "create_probe needs `target_json`"
        );
        assert!(
            props.get("service_id").is_none(),
            "create_probe must not reference `service_id`"
        );
        let kinds = props["kind"]["enum"].as_array().unwrap();
        for k in ["http", "tcp", "tls"] {
            assert!(kinds.iter().any(|v| v == k), "probe kind enum missing {k}");
        }
    }

    /// Numeric ids are advertised as integers but clients may send them quoted; both must
    /// resolve to the string the REST path needs.
    #[test]
    fn req_id_accepts_numbers_and_strings() {
        assert_eq!(req_id(&json!({"rule_id": 3}), "rule_id").unwrap(), "3");
        assert_eq!(req_id(&json!({"rule_id": "3"}), "rule_id").unwrap(), "3");
        assert!(req_id(&json!({}), "rule_id").is_err());
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
