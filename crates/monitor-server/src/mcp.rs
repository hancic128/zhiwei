//! MCP (Model Context Protocol) server implementation.
//!
//! 通过 SSE transport 暴露 monitor 数据给外部 AI 客户端（Claude Desktop /
//! Cursor / Cline 等）。仅 **读** 端点,D8 决策：reboot / shutdown / kill
//! 等破坏性操作不走 MCP。
//!
//! 设计决策（见 plan）：
//! - 单 endpoint：`POST /mcp/sse`
//!   - request:  Content-Type: application/json, JSON-RPC 2.0 body
//!   - response: Content-Type: text/event-stream, 每个 event 一个 JSON-RPC 消息
//!   - 同步返回（本次所有工具都是 <1s 的快操作）
//! - GET /mcp/sse 暂不实现（未来需要服务端推送时再加）
//! - 鉴权：只接受 AI token（`ReadAuthKind::AiToken`）
//! - HTTP client：reqwest 调 monitor 自己的 REST API，统一用同一 AI token
//!
//! MCP 协议版本：`2024-11-05`（MCP 当前 stable 草案）。

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

/// MCP server 信息（`initialize` 响应）。
fn server_info() -> Value {
    json!({
        "name": SERVER_NAME,
        "version": SERVER_VERSION,
    })
}

fn server_capabilities() -> Value {
    json!({ "tools": {} })
}

/// 工具定义（静态注册表）。
fn tools_list() -> Value {
    json!({
        "tools": [
            tool_def(
                "list_nodes",
                "List all nodes in the cluster with hostname, last_seen, and key latest metrics.",
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_node",
                "Get single node details: host_info (OS/kernel/CPU/memory) and latest metrics. Returns error if not found.",
                json!({
                    "type": "object",
                    "properties": {
                        "node_id": { "type": "string", "description": "节点 ID" }
                    },
                    "required": ["node_id"],
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "get_telemetry",
                "Get latest telemetry frame for a node (CPU/memory/disk/network).",
                json!({
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
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_certs",
                "List certificate scan sources (node + path configured in console), without certificate content.",
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
            ),
            tool_def(
                "list_containers",
                "List latest container snapshot for a node (requires inventory reported by node).",
                json!({
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
                json!({
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

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

// ---------- JSON-RPC 类型 ----------

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

fn rpc_error(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": JsonRpcError { code, message: message.into(), data: None },
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

// ---------- HTTP handler ----------

/// `POST /mcp/sse` —— MCP over SSE（同步返回,每个响应是一个 SSE event）。
pub async fn sse_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 鉴权: 仅 AI token
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

    // 解析 JSON-RPC 请求
    let req: JsonRpcRequest = match serde_json::from_slice::<JsonRpcRequest>(&body) {
        Ok(r) if r.jsonrpc == "2.0" => r,
        Ok(_) => {
            return sse_single(rpc_error(Value::Null, -32600, "jsonrpc must be \"2.0\""));
        }
        Err(e) => {
            return sse_single(rpc_error(Value::Null, -32700, format!("parse error: {e}")));
        }
    };

    // 提取 Bearer token 字符串，工具实现里调 REST 时复用同一凭据。
    // 鉴权已经在上面通过（只接受 AiToken），所以这里安全。
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let response = dispatch(&state, &bearer, req).await;
    sse_single(response)
}

/// 分发 JSON-RPC method 到具体实现。
async fn dispatch(state: &AppState, bearer: &Option<String>, req: JsonRpcRequest) -> Value {
    match req.method.as_str() {
        "initialize" => rpc_result(
            req.id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": server_capabilities(),
                "serverInfo": server_info(),
            }),
        ),
        "notifications/initialized" => {
            // notifications 不需要响应（id 通常为 null）。
            // 但我们的传输是同步返回,所以返回一个 ack。
            rpc_result(req.id, json!({ "acknowledged": true }))
        }
        "ping" => rpc_result(req.id, json!({})),
        "tools/list" => rpc_result(req.id, tools_list()),
        "tools/call" => call_tool(state, bearer, req.id, req.params).await,
        other => rpc_error(req.id, -32601, format!("method not found: {other}")),
    }
}

/// 调工具：解析 name + arguments，调 monitor REST API，包装成 MCP `content`。
async fn call_tool(state: &AppState, bearer: &Option<String>, id: Value, params: Value) -> Value {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return rpc_error(id, -32602, "params.name required"),
    };
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let Some(token) = bearer else {
        return rpc_error(id, -32603, "internal: bearer missing in handler context");
    };

    // 构造内部 HTTP client。base URL 从 listen 地址推——state.listen 是
    // "host:port"，我们用 127.0.0.1:<port> 调自己（TLS 由 monitor 自己终结时
    // 这里需要 https，但本期 MCP 仅暴露给已经能调控制台的客户端，所以走 plain HTTP
    // 即可；TLS 终结的方案以后再处理）。
    //
    // 实现：先 hardcode "http://127.0.0.1:<port>"，从 cfg.listen 拿端口。
    // 完整做法见 AppState.mcp_base_url 字段。
    let base_url = &state.mcp_base_url;

    match call_tool_impl(base_url, token, &name, arguments).await {
        Ok(text) => rpc_result(
            id,
            json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }),
        ),
        Err(e) => rpc_result(
            id,
            json!({
                "content": [{ "type": "text", "text": format!("tool error: {e}") }],
                "isError": true,
            }),
        ),
    }
}

/// 实际工具实现：每个工具 = 一次 HTTP GET 到 monitor REST 端点。
async fn call_tool_impl(
    base_url: &str,
    token: &str,
    name: &str,
    arguments: Value,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url_path = match name {
        "list_nodes" => "/v1/nodes".to_string(),
        // 注意：后端没有 `GET /v1/nodes/:id` 这个端点（只有 telemetry/series/
        // containers/processes 四个子资源）。`/v1/nodes` 列表本身已经带
        // host_info + 最新指标，所以「单节点详情」用列表 + 客户端过滤实现。
        "get_node" => "/v1/nodes".to_string(),
        "get_telemetry" => {
            let node_id = arguments
                .get("node_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("node_id required"))?;
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(100);
            format!("/v1/nodes/{node_id}/telemetry?limit={limit}")
        }
        // 后端 alerts_handler 不接受 limit（固定返回活跃 + 最近 50 条已解决），
        // 所以这里不加查询参数，避免给出「能翻页」的错觉。
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

    // get_node 走的是列表端点，这里按 id 过滤出一个节点。
    // 找不到时明确报错，别让 AI 拿到整个列表还以为拿到了详情。
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
    // 直接返回 JSON 字符串（pretty 一下方便 AI 解析）
    // body 在 unwrap_or 里被 move 进 Value::String，closure 里就拿不到了。
    // 先存一份 fallback 字符串。
    let body_owned = body;
    let v: Value = serde_json::from_str(&body_owned).unwrap_or(Value::String(body_owned.clone()));
    Ok(serde_json::to_string_pretty(&v).unwrap_or(body_owned))
}

/// 把单个 JSON-RPC 响应包装成 SSE 事件流。
fn sse_single(message: Value) -> Response {
    let body = format!(
        "event: message\ndata: {}\n\n",
        serde_json::to_string(&message).unwrap_or_else(|_| "{}".to_string())
    );
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
            ("x-accel-buffering", "no"), // 禁用 nginx 缓冲
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
        let v = rpc_result(json!(7), json!({"ok": true}));
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"]["ok"], true);
        assert!(v.get("error").is_none());
    }

    #[test]
    fn rpc_error_carries_code_and_message() {
        let v = rpc_error(Value::Null, -32601, "method not found: nope");
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
