//! HTTP routes for the monitor server.
//!
//! Two endpoint families:
//!   POST /v1/enroll       — bootstrap token + node Ed25519 public key → node_id
//!   POST /v1/telemetry    — 节点 Ed25519 请求签名，protobuf payload
//!
//! Bootstrap tokens are short-lived (default 10 minutes) and stored in memory.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use parking_lot::Mutex;
use prost::Message as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use zhiwei_common::NodeId;
use zhiwei_proto::common::{EnrollRequest, EnrollResponse};
use zhiwei_proto::control::CommandResult;
use zhiwei_proto::telemetry::{InventoryReport, TelemetryBatch};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1", get(index_handler))
        .route("/v1/enroll", post(enroll_handler))
        .route("/v1/telemetry", post(telemetry_handler))
        .route("/v1/nodes", get(list_nodes_handler))
        .route("/v1/nodes/:id", axum::routing::patch(patch_node_handler))
        .route("/v1/nodes/:id/telemetry", get(node_telemetry_handler))
        .route("/v1/nodes/:id/series", get(node_series_handler))
        .route("/v1/nodes/:id/containers", get(node_containers_handler))
        .route("/v1/nodes/:id/processes", get(node_processes_handler))
        .route("/v1/inventory", post(inventory_handler))
        .route("/v1/containers", get(all_containers_handler))
        .route("/v1/certificates", get(all_certificates_handler))
        .route(
            "/v1/cert-sources",
            get(crate::certs_api::list_cert_sources_handler)
                .post(crate::certs_api::create_cert_source_handler),
        )
        .route(
            "/v1/cert-sources/test",
            post(crate::certs_api::test_cert_source_handler),
        )
        .route(
            "/v1/cert-sources/:id",
            axum::routing::patch(crate::certs_api::patch_cert_source_handler)
                .delete(crate::certs_api::delete_cert_source_handler),
        )
        .route(
            "/v1/cert-config",
            get(crate::certs_api::cert_config_handler),
        )
        .route("/v1/alerts", get(alerts_handler))
        .route("/v1/alerts/:id/silence", post(silence_alert_handler))
        .route(
            "/v1/services/timeline",
            get(crate::probes_api::services_timeline_handler),
        )
        .route(
            "/v1/rules",
            get(list_rules_handler).post(create_rule_handler),
        )
        .route(
            "/v1/rules/:id",
            axum::routing::patch(patch_rule_handler).delete(delete_rule_handler),
        )
        .route(
            "/v1/channels",
            get(list_channels_handler).post(create_channel_handler),
        )
        .route("/v1/channels/test", post(test_channel_handler))
        .route("/v1/admin/token", post(change_admin_token_handler))
        .route(
            "/v1/channels/:id",
            axum::routing::patch(patch_channel_handler).delete(delete_channel_handler),
        )
        .route("/v1/ca", get(ca_handler))
        .route(
            "/v1/services",
            get(crate::probes_api::list_services_handler)
                .post(crate::probes_api::create_service_handler),
        )
        .route(
            "/v1/services/:id",
            axum::routing::patch(crate::probes_api::patch_service_handler)
                .delete(crate::probes_api::delete_service_handler),
        )
        .route(
            "/v1/probes",
            get(crate::probes_api::list_probes_handler)
                .post(crate::probes_api::create_probe_handler),
        )
        .route(
            "/v1/probes/:id",
            axum::routing::patch(crate::probes_api::patch_probe_handler)
                .delete(crate::probes_api::delete_probe_handler),
        )
        .route(
            "/v1/probes/:id/results",
            get(crate::probes_api::probe_results_handler),
        )
        .route(
            "/v1/probe-config",
            get(crate::probes_api::probe_config_handler),
        )
        .route(
            "/v1/probe-results",
            post(crate::probes_api::probe_results_ingest_handler),
        )
        .route("/v1/todo", get(crate::todo_api::todo_handler))
        .route("/v1/retention", get(crate::retention::retention_handler))
        .route("/v1/help", get(help_handler))
        .route("/mcp/sse", axum::routing::post(crate::mcp::sse_handler))
        .route(
            "/v1/ai-tokens",
            get(list_ai_tokens_handler).post(create_ai_token_handler),
        )
        .route(
            "/v1/ai-tokens/:id",
            axum::routing::delete(delete_ai_token_handler),
        )
        .route(
            "/v1/enroll-tokens",
            get(list_enroll_tokens_handler).post(create_enroll_token_handler),
        )
        .route(
            "/v1/enroll-tokens/:id",
            axum::routing::delete(delete_enroll_token_handler),
        )
        .route("/v1/series/nodes", get(all_nodes_series_handler))
        .route("/v1/commands", get(node_commands_handler))
        .route("/v1/commands/:id/result", post(command_result_handler))
        .route("/v1/exec", post(exec_handler))
        .route("/v1/commands/history", get(command_history_handler))
        .route("/v1/commands/:id", get(command_detail_handler))
        .route("/healthz", get(healthz))
        // 节点入网脚本。**故意不鉴权**：目标机器此刻还没有任何凭据，
        // 真正的秘密是 enroll 命令里带过去的 ZHIWEI_BOOTSTRAP_TOKEN。
        // 脚本本身不含任何秘密，公开它等于公开安装方式（同 Tailscale 等做法）。
        .route("/install-node.sh", get(install_node_script_handler))
        // 兜底：控制台静态资源 + SPA 深链（未构建控制台时返回 404）
        .fallback(ui_handler)
        .with_state(state)
}

/// 校验节点请求签名（取代原 mTLS 客户端证书）。
///
/// 依次检查：签名头齐全 → 节点已入网且有公钥 → 时间窗 → nonce 未重放 → 签名。
/// 任一步失败都返回 401，且不透露具体是哪一步（避免给攻击者反馈）。
///
/// 返回 `(节点 id, 节点公钥)`——调用方若需要再验一层载荷签名（如命令回执），
/// 可以直接用这个公钥，不必再查一次库。
pub(crate) async fn verify_node(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
    path_and_query: &str,
    body: &[u8],
) -> Result<(zhiwei_common::NodeId, zhiwei_common::PublicKey), (StatusCode, String)> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };

    let unauthorized = || (StatusCode::UNAUTHORIZED, "节点签名校验失败".to_string());

    let node_id_str = get(zhiwei_common::auth::HEADER_NODE).ok_or_else(unauthorized)?;
    let ts: i64 = get(zhiwei_common::auth::HEADER_TIMESTAMP)
        .and_then(|s| s.parse().ok())
        .ok_or_else(unauthorized)?;
    let nonce = get(zhiwei_common::auth::HEADER_NONCE).ok_or_else(unauthorized)?;
    let signature = get(zhiwei_common::auth::HEADER_SIGNATURE).ok_or_else(unauthorized)?;

    let id = zhiwei_common::NodeId::from_string(node_id_str.clone());
    let record = state
        .storage
        .nodes()
        .find_by_id(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("node lookup: {e}"),
            )
        })?
        .ok_or_else(unauthorized)?;

    if record.public_key.is_empty() {
        // 老节点（mTLS 时代入网、没有公钥）需要重新入网
        return Err((
            StatusCode::UNAUTHORIZED,
            "该节点没有登记公钥，请重新 enroll".to_string(),
        ));
    }

    let pub_bytes = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(record.public_key.as_bytes())
            .map_err(|_| unauthorized())?
    };

    let pub_key = zhiwei_common::PublicKey(pub_bytes);

    let signed = zhiwei_common::SignedHeaders {
        node_id: node_id_str.clone(),
        timestamp: ts,
        nonce: nonce.clone(),
        signature,
    };
    signed
        .verify(&pub_key, method, path_and_query, body)
        .map_err(|_| unauthorized())?;

    // 签名有效后再消耗 nonce：无效请求不该污染缓存
    if !state
        .nonce_cache
        .accept(&nonce, zhiwei_common::Timestamp::now().unix_nano())
    {
        return Err((StatusCode::UNAUTHORIZED, "请求已重放".to_string()));
    }

    Ok((id, pub_key))
}

/// 读端点只认一种凭据：`Authorization: Bearer <admin token>`
/// （浏览器控制台用）。节点走签名鉴权，且不读这些端点。
/// 鉴权层级 v2：不仅返回 bool，还区分凭据类型。
///
/// 当前所有受保护端点都是读端点，所以 AI token 实际权限 = admin token 的全部
/// 权限（去掉 admin 改 admin.token 自身的能力）。后续 manage 类写端点落地时，
/// handler 里加 `matches!(kind, Admin | AiToken)` 即可对 AI token 开放。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadAuthKind {
    /// 控制台 / 浏览器 / 拥有 admin token 的人。
    Admin,
    /// AI token，附带 token id（用于审计 last_used_at）。
    AiToken(String),
    /// 没有合法凭据。
    None,
}

/// 解析 Authorization 头，返回凭据类型。
///
/// 顺序：admin token（内存 ct_eq 比对，最快）→ AI token（SHA-256 后查 SQLite）。
/// AI token 不命中或已撤销都返回 None。
pub(crate) async fn read_auth_ok_v2(state: &AppState, headers: &HeaderMap) -> ReadAuthKind {
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    else {
        return ReadAuthKind::None;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return ReadAuthKind::None;
    };

    // 1. admin token
    // 把 String 取出来就 drop guard，否则 RwLockReadGuard 不是 Send，
    // 后续 await 时整个 handler future 就不是 Send，axum 不认。
    let admin_ok = {
        let current = state.admin_token.read().unwrap_or_else(|e| e.into_inner());
        crate::admin::ct_eq(token.as_bytes(), current.as_bytes())
    };
    if admin_ok {
        return ReadAuthKind::Admin;
    }

    // 2. AI token
    let hash = sha256_hex(token.as_bytes());
    match state.storage.ai_tokens().find_active_by_hash(&hash).await {
        Ok(Some(t)) => {
            // best-effort: 更新 last_used_at。失败不影响主请求。
            let id = t.id.clone();
            let now = zhiwei_common::Timestamp::now().unix_nano();
            let repo = state.storage.ai_tokens();
            tokio::spawn(async move {
                let _ = repo.touch_last_used(&id, now).await;
            });
            ReadAuthKind::AiToken(t.id)
        }
        _ => ReadAuthKind::None,
    }
}

/// 旧接口：保留兼容。新代码请直接用 `read_auth_ok_v2`。
///
/// 等价于 `matches!(v2(...), Admin | AiToken(_))`，但不 unwrap 也不分发 ——
/// 调用方已经在用 bool，没必要为新代码增加心智负担。
pub(crate) async fn read_auth_ok(state: &AppState, headers: &HeaderMap) -> bool {
    !matches!(read_auth_ok_v2(state, headers).await, ReadAuthKind::None)
}

/// SHA-256 → 小写 hex（用 ring，已在依赖里）。AI token 哈希专用。
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// 一个入网令牌的元信息。`id` 给 UI 展示 + 撤销用；`token` 字符串本身只在校验路径用。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BootstrapTokenMeta {
    pub id: String,
    pub label: String,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
    /// 长期有效（来自 ZHIWEI_BOOTSTRAP_TOKEN）时为 true，UI 可单独标识。
    pub permanent: bool,
}

#[derive(Default)]
pub struct BootstrapTokens {
    /// token 字符串 -> 元信息
    inner: Mutex<HashMap<String, BootstrapTokenMeta>>,
}

/// 永不失效的过期时间戳。给 `ZHIWEI_BOOTSTRAP_TOKEN` 用——托管平台
/// （尤其免费层）没有 Shell、也挂不了持久卷，抢「启动日志里 10 分钟有效」
/// 的一次性 token 不现实。
const NEVER_EXPIRES: u64 = u64::MAX;

impl BootstrapTokens {
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
    pub async fn add(&self, token: String, ttl_secs: u64) {
        self.add_with_label(token, ttl_secs, String::new()).await;
    }
    /// 添加一个带 label 的临时 token。`label` 为空也合法。
    /// `id` 自动生成（`boot-<6 hex>`），仅用于 UI 展示和撤销。
    pub async fn add_with_label(&self, token: String, ttl_secs: u64, label: String) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let expires = now + ttl_secs;
        let id = format!("boot-{}", &hex_encode(&rand_bytes_3())[..6]);
        let meta = BootstrapTokenMeta {
            id,
            label,
            created_at_unix: now,
            expires_at_unix: expires,
            permanent: false,
        };
        self.inner.lock().insert(token, meta);
    }
    /// 登记一个长期有效的入网令牌（`ZHIWEI_BOOTSTRAP_TOKEN`）。
    /// 删掉环境变量并重启即等于撤销。
    pub fn add_static(&self, token: String) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = format!("boot-static-{}", &hex_encode(&rand_bytes_3())[..6]);
        let meta = BootstrapTokenMeta {
            id,
            label: "ZHIWEI_BOOTSTRAP_TOKEN".into(),
            created_at_unix: now,
            expires_at_unix: NEVER_EXPIRES,
            permanent: true,
        };
        self.inner.lock().insert(token, meta);
    }
    pub fn mint() -> String {
        let mut buf = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut buf);
        format!("zhi-bt-{}", hex_encode(&buf))
    }
    pub fn check(&self, token: &str) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut guard = self.inner.lock();
        match guard.get(token) {
            Some(meta) if meta.expires_at_unix > now => true,
            Some(_) => {
                guard.remove(token);
                false
            }
            None => false,
        }
    }
    /// 列出当前未过期的 token 元信息。**不含明文 token**。
    pub fn list_active(&self) -> Vec<BootstrapTokenMeta> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.inner
            .lock()
            .values()
            .filter(|m| m.expires_at_unix > now)
            .cloned()
            .collect()
    }
    /// 按 id 撤销（找到第一个匹配的 token 字符串后移除）。
    /// 返回是否真的撤销了什么。
    pub fn revoke_by_id(&self, id: &str) -> bool {
        let mut guard = self.inner.lock();
        let target = guard
            .iter()
            .find(|(_, m)| m.id == id)
            .map(|(t, _)| t.clone());
        if let Some(token) = target {
            guard.remove(&token);
            true
        } else {
            false
        }
    }
}

fn rand_bytes_3() -> [u8; 3] {
    let mut buf = [0u8; 3];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

pub(crate) fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ErrorBody { error: msg.into() })).into_response()
}

/// 控制台静态资源 + SPA 兜底。
///
/// 深链（`/nodes`、`/services`…）必须返回 **200 + index.html**：早先用
/// `ServeDir::not_found_service` 时页面能渲染，但状态码是 404，会被线上探针、
/// 爬虫与代理缓存误判成「页面不存在」。这里自己读文件，顺便给出正确 MIME。
pub(crate) async fn ui_handler(State(state): State<AppState>, uri: axum::http::Uri) -> Response {
    use axum::http::header::CONTENT_TYPE;

    let Some(dir) = state.ui_dir.clone() else {
        return err(StatusCode::NOT_FOUND, "console build not found");
    };
    let rel = uri.path().trim_start_matches('/');

    // 未知 API 路径仍按 JSON 404 返回，避免把 HTML 混进客户端
    if rel.starts_with("v1/") || rel == "v1" {
        return err(StatusCode::NOT_FOUND, "unknown endpoint");
    }

    // 已存在的静态文件优先；拒绝目录穿越
    if !rel.is_empty() && !rel.contains("..") {
        let path = dir.join(rel);
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return (StatusCode::OK, [(CONTENT_TYPE, mime_of(&path))], bytes).into_response();
        }
    }

    match tokio::fs::read(dir.join("index.html")).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(e) => err(
            StatusCode::NOT_FOUND,
            format!("console index.html 读取失败: {e}"),
        ),
    }
}

fn mime_of(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn healthz() -> Response {
    (StatusCode::OK, "ok").into_response()
}

// ---------- /v1/enroll ----------

/// enroll 响应里要不要给节点下发本地 CA。
///
/// 只在「本进程终结 TLS」时下发——那种情况下 monitor 的证书就是这个本地 CA 签的，
/// 节点 pin 它才有意义。
///
/// 部署在 Render / Railway 这类边缘终结 TLS 的平台后面时（`--plain-http`），
/// 边缘用的是正经证书、与本地 CA 毫无关系；若仍然下发，节点会把它当唯一信任根，
/// 表现为 **enroll 成功、之后每个请求都 TLS 校验失败**，排查起来很费劲。
/// 所以这里直接不下发，让节点走系统根。
pub(crate) fn enroll_ca_pem(ca_cert_pem: &str, tls_terminated_locally: bool) -> String {
    if tls_terminated_locally {
        ca_cert_pem.to_string()
    } else {
        String::new()
    }
}

/// ops 签名公钥（base64），随 enroll 下发给节点做 TOFU。
///
/// monitor 启动时就缓存了一份，但**缓存为空时会再读一次盘**：同容器多进程部署时
/// ops-server 可能比 monitor 起得晚（见 `scripts/docker-entrypoint.sh`），
/// 早启动的 monitor 不该因此永久不给新节点下发公钥——那种节点会一直
/// 「未持有 ops 公钥」，命令通道静默失效。
pub(crate) async fn ops_public_key(state: &AppState) -> String {
    if !state.ops_public_key.is_empty() {
        return state.ops_public_key.clone();
    }
    tokio::fs::read_to_string(state.data_dir.join("ops.pub"))
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

async fn enroll_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Auth: Authorization: Bearer <bootstrap-token>
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());
    let token = match auth.and_then(|s| s.strip_prefix("Bearer ")) {
        Some(t) => t,
        None => return err(StatusCode::UNAUTHORIZED, "missing bearer token"),
    };
    if !state.bootstrap_tokens.check(token) {
        return err(
            StatusCode::UNAUTHORIZED,
            "invalid or expired bootstrap token",
        );
    }

    // Content-Type: application/protobuf
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !ct.starts_with("application/protobuf") {
        return err(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/protobuf",
        );
    }

    let req = match EnrollRequest::decode(&body[..]) {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("decode EnrollRequest: {e}"),
            )
        }
    };

    let hostname = req.hostname.clone();
    if hostname.is_empty() {
        return err(StatusCode::BAD_REQUEST, "hostname required");
    }
    // 身份 = 节点自带的 Ed25519 公钥（不再是 CA 签发的客户端证书）
    if req.public_key.len() != 32 {
        return err(
            StatusCode::BAD_REQUEST,
            "public_key 必须是 32 字节 Ed25519 公钥",
        );
    }
    let public_key_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&req.public_key)
    };

    // Allocate NodeId and persist
    let node_id = NodeId::new();
    let labels_map: HashMap<String, String> = req
        .labels
        .iter()
        .map(|l| (l.key.clone(), l.value.clone()))
        .collect();
    let labels_json = serde_json::to_string(&labels_map).unwrap_or_else(|_| "{}".into());

    // 节点侧可选的别名 / 标签（`zhiwei-node --alias/--tags`，install-node.sh 也支持）。
    // 规则与控制台 PATCH 完全一致：越界值直接 400，而不是截断后静默入库。
    let alias = match normalize_alias(&req.alias) {
        Ok(a) => a,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    let tags = match normalize_tags(&req.tags) {
        Ok(t) => t,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };

    let now = zhiwei_common::Timestamp::now().unix_nano();
    let record = zhiwei_storage::node_repo::NodeRecord {
        id: node_id.to_string(),
        hostname: hostname.clone(),
        labels_json,
        client_cert_pem: String::new(), // 历史列，mTLS 已废弃
        enrolled_at_unix_nano: now,
        last_seen_unix_nano: None,
        // 主机信息在第一次 telemetry 上报时才填充
        host_info_json: "{}".to_string(),
        public_key: public_key_b64.clone(),
        // 别名与标签：节点侧可以自带（可选），之后由管理员在控制台维护
        alias,
        tags_json: serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string()),
    };
    if let Err(e) = state.storage.nodes().insert(&record).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("insert node: {e}"),
        );
    }

    let resp = EnrollResponse {
        node_id: node_id.to_string(),
        ca_cert_pem: enroll_ca_pem(&state.ca_cert_pem, state.tls_terminated_locally),
        monitor_cert_pem: String::new(), // nodes pin via the CA bundle returned above
        ops_public_key: ops_public_key(&state).await,
    };
    let mut buf = Vec::new();
    if let Err(e) = prost::Message::encode(&resp, &mut buf) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("encode EnrollResponse: {e}"),
        );
    }

    info!(%node_id, %hostname, "enrolled new node");
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/protobuf")
        .body(axum::body::Body::from(buf))
        .unwrap()
}

// ---------- /v1/telemetry ----------

async fn telemetry_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, _node_pub) = match verify_node(&state, &headers, method.as_str(), pq, &body).await
    {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };

    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !ct.starts_with("application/protobuf") {
        return err(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/protobuf",
        );
    }

    let batch = match TelemetryBatch::decode(&body[..]) {
        Ok(b) => b,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("decode TelemetryBatch: {e}"),
            )
        }
    };
    // 身份已由请求签名确认；这里再校验 body 里的 node_id 与签名主体一致，
    // 防止用 A 节点的签名提交 B 节点的数据
    let node_id_str = batch.node_id.clone();
    if node_id_str.is_empty() || node_id_str != node_id.as_str() {
        return err(StatusCode::BAD_REQUEST, "node_id 与签名主体不一致");
    }

    let ts = batch.ts_unix_nano;
    let interval = batch.interval_seconds;
    if let Err(e) = state
        .storage
        .telemetry()
        .insert(&node_id_str, ts, interval, &body)
        .await
    {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("insert telemetry: {e}"),
        );
    }
    if let Err(e) = state.storage.nodes().touch_last_seen(&node_id).await {
        warn!(error = %e, "touch_last_seen failed");
    }

    debug!(%node_id_str, %ts, interval, "telemetry batch stored");

    // 落库后跑一遍告警规则（有规则才查库，无规则时开销可忽略）。
    // 告警文案用的是**显示名**（有别名用别名），主机名 VM-16-12-opencloudos
    // 这种认不出是哪台机器。
    let hostname = state
        .storage
        .nodes()
        .find_by_id(&node_id)
        .await
        .ok()
        .flatten()
        .map(|n| node_display_name(&n.alias, &n.hostname))
        .unwrap_or_default();
    crate::alerts::evaluate(&state, &node_id, &hostname, &batch).await;

    (StatusCode::NO_CONTENT).into_response()
}

// ---------- read endpoints (Bearer admin token) ----------

#[derive(Serialize)]
struct IndexBody {
    service: &'static str,
    version: &'static str,
    authenticated: bool,
    endpoints: Vec<&'static str>,
    nodes: i64,
    telemetry_batches: i64,
}

async fn index_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let nodes = state
        .storage
        .nodes()
        .list_all()
        .await
        .map(|v| v.len() as i64)
        .unwrap_or(-1);
    let batches = state.storage.telemetry().count_all().await.unwrap_or(-1);
    Json(IndexBody {
        service: "zhiwei-monitor",
        version: env!("CARGO_PKG_VERSION"),
        authenticated: read_auth_ok(&state, &headers).await,
        endpoints: vec![
            "GET  /v1/nodes",
            "GET  /v1/nodes/:id/telemetry?limit=10",
            "POST /v1/enroll",
            "POST /v1/telemetry",
            "GET  /healthz",
        ],
        nodes,
        telemetry_batches: batches,
    })
    .into_response()
}

#[derive(Serialize)]
struct NodeView {
    id: String,
    hostname: String,
    labels: serde_json::Value,
    /// 管理员给的简短别称（≤10 字符），空串表示未设置
    alias: String,
    /// 管理员给的标签（≤10 个）
    tags: Vec<String>,
    enrolled_at_unix_nano: i64,
    last_seen_unix_nano: Option<i64>,
    /// 主机基本信息（操作系统 / 内核 / CPU / IP 等），未见上报时为空对象
    host_info: serde_json::Value,
    /// 节点 Ed25519 公钥（base64）；用于调试/审计，UI 不直接展示
    public_key: String,
    /// 最新一帧里的几个关键指标（列表页的 CPU / 内存 / 磁盘列直接取这里，
    /// 避免每节点再来一次 series 请求）
    latest: Option<NodeLatestView>,
}

#[derive(Serialize)]
struct NodeLatestView {
    ts_unix_nano: i64,
    cpu_percent: Option<f64>,
    mem_used_bytes: Option<f64>,
    mem_total_bytes: Option<f64>,
    disk_usage_percent: Option<f64>,
    disk_used_bytes: Option<f64>,
    disk_total_bytes: Option<f64>,
    /// 全部网卡累计收 / 发字节（列表页只用得到时间戳，速率在详情页算）
    net_rx_bytes: Option<f64>,
    net_tx_bytes: Option<f64>,
}

/// 从一帧 telemetry 里取列表页要用的指标
fn latest_view(ts_unix_nano: i64, batch: &TelemetryBatch) -> NodeLatestView {
    let get = |name: &str| {
        batch
            .metrics
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.value)
    };
    NodeLatestView {
        ts_unix_nano,
        cpu_percent: get("host.cpu.usage"),
        mem_used_bytes: get("host.mem.used_bytes"),
        mem_total_bytes: get("host.mem.total_bytes"),
        disk_usage_percent: get("host.disk.usage"),
        disk_used_bytes: get("host.disk.used_bytes"),
        disk_total_bytes: get("host.disk.total_bytes"),
        net_rx_bytes: sum_if_present(batch.network.iter().map(|n| n.rx_bytes as f64)),
        net_tx_bytes: sum_if_present(batch.network.iter().map(|n| n.tx_bytes as f64)),
    }
}

async fn list_nodes_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let nodes = match state.storage.nodes().list_all().await {
        Ok(v) => v,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list nodes: {e}"),
            )
        }
    };

    // 一次取齐「每节点最新一帧」，列表页不再逐节点发 series 请求
    let mut latest_by_node: HashMap<String, NodeLatestView> = HashMap::new();
    if let Ok(rows) = state.storage.telemetry().latest_per_node().await {
        for (node_id, ts, payload) in rows {
            if let Ok(batch) = TelemetryBatch::decode(&payload[..]) {
                latest_by_node.insert(node_id, latest_view(ts, &batch));
            }
        }
    }

    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        out.push(NodeView {
            labels: serde_json::from_str(&n.labels_json).unwrap_or(serde_json::json!({})),
            alias: n.alias.clone(),
            tags: parse_tags(&n.tags_json),
            latest: latest_by_node.remove(&n.id),
            id: n.id,
            hostname: n.hostname,
            enrolled_at_unix_nano: n.enrolled_at_unix_nano,
            last_seen_unix_nano: n.last_seen_unix_nano,
            host_info: serde_json::from_str(&n.host_info_json)
                .unwrap_or_else(|_| serde_json::json!({})),
            public_key: n.public_key,
        });
    }
    Json(out).into_response()
}

/// `tags_json` 里存的是字符串数组；历史行可能是 `{}` 或损坏内容，
/// 统一降级成空数组，别让一条脏数据把整个列表打成 500。
fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// 别名上限（按字符计，不是字节）：中文别名也该按「几个字」算。
const MAX_ALIAS_CHARS: usize = 10;
/// 标签数量上限
const MAX_TAGS: usize = 10;
/// 单个标签长度上限
const MAX_TAG_CHARS: usize = 24;

/// 归一化别名：去首尾空白并按字符数校验。
fn normalize_alias(raw: &str) -> Result<String, String> {
    let alias = raw.trim().to_string();
    if alias.chars().count() > MAX_ALIAS_CHARS {
        return Err(format!("别名最多 {MAX_ALIAS_CHARS} 个字符"));
    }
    Ok(alias)
}

/// 归一化标签：去空白 / 去空串 / 去重（保持顺序），再校验个数与长度。
///
/// 控制台改元数据（`PATCH /v1/nodes/:id`）与节点入网自报（`EnrollRequest`）共用，
/// 两条路径的规则必须一致——否则节点侧能塞进控制台拒绝的值。
fn normalize_tags(raw: &[String]) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut cleaned = Vec::new();
    for tag in raw {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > MAX_TAG_CHARS {
            return Err(format!("单个标签最多 {MAX_TAG_CHARS} 个字符"));
        }
        if seen.insert(tag.to_string()) {
            cleaned.push(tag.to_string());
        }
    }
    if cleaned.len() > MAX_TAGS {
        return Err(format!("标签最多 {MAX_TAGS} 个"));
    }
    Ok(cleaned)
}

#[derive(Deserialize)]
struct NodeMetaPatch {
    /// 缺省 = 不改（区别于传空串 = 清空别名）
    #[serde(default)]
    alias: Option<String>,
    /// 缺省 = 不改（区别于传空数组 = 清空标签）
    #[serde(default)]
    tags: Option<Vec<String>>,
}

/// `PATCH /v1/nodes/:id`：改管理员维护的别名与标签。
///
/// 只接受 alias / tags 两个字段，节点身份（id、公钥、hostname、上报数据）
/// 一律不可从控制台改。校验失败返回 400 并把原因原样给控制台。
async fn patch_node_handler(
    State(state): State<AppState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let patch: NodeMetaPatch = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if patch.alias.is_none() && patch.tags.is_none() {
        return err(StatusCode::BAD_REQUEST, "alias 与 tags 至少要给一个");
    }

    let node = zhiwei_common::NodeId::from_string(node_id.clone());
    let current = match state.storage.nodes().find_by_id(&node).await {
        Ok(Some(n)) => n,
        Ok(None) => return err(StatusCode::NOT_FOUND, "node not enrolled"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    };

    let alias = match patch.alias {
        Some(a) => match normalize_alias(&a) {
            Ok(a) => a,
            Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
        },
        None => current.alias.clone(),
    };

    let tags = match patch.tags {
        Some(raw) => match normalize_tags(&raw) {
            Ok(t) => t,
            Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
        },
        None => parse_tags(&current.tags_json),
    };

    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into());
    if let Err(e) = state
        .storage
        .nodes()
        .update_meta(&node, &alias, &tags_json)
        .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("update: {e}"));
    }

    Json(serde_json::json!({ "id": node.as_str(), "alias": alias, "tags": tags })).into_response()
}

#[derive(Serialize)]
struct MetricView {
    name: String,
    value: f64,
}

#[derive(Serialize)]
struct BatchView {
    ts_unix_nano: i64,
    interval_seconds: i32,
    bytes: usize,
    metrics: Vec<MetricView>,
    disks: Vec<String>,
    network_interfaces: Vec<String>,
    signature_bytes: usize,
}

#[derive(Serialize)]
struct NodeTelemetryView {
    node_id: String,
    returned: usize,
    batches: Vec<BatchView>,
}

async fn node_telemetry_handler(
    State(state): State<AppState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(10);

    let rows = match state.storage.telemetry().recent(&node_id, limit).await {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query telemetry: {e}"),
            );
        }
    };

    let mut batches = Vec::with_capacity(rows.len());
    for (ts, interval, payload) in rows {
        let decoded = TelemetryBatch::decode(&payload[..]).ok();
        let (metrics, disks, ifaces, sig_len) = match decoded {
            Some(b) => (
                b.metrics
                    .into_iter()
                    .map(|m| MetricView {
                        name: m.name,
                        value: m.value,
                    })
                    .collect(),
                b.disks
                    .into_iter()
                    .map(|d| {
                        format!(
                            "{} {} {}%",
                            d.mountpoint,
                            d.filesystem,
                            if d.total_bytes > 0 {
                                d.used_bytes * 100 / d.total_bytes
                            } else {
                                0
                            }
                        )
                    })
                    .collect(),
                b.network.into_iter().map(|n| n.name).collect(),
                b.signature.len(),
            ),
            None => (Vec::new(), Vec::new(), Vec::new(), 0),
        };
        batches.push(BatchView {
            ts_unix_nano: ts,
            interval_seconds: interval,
            bytes: payload.len(),
            metrics,
            disks,
            network_interfaces: ifaces,
            signature_bytes: sig_len,
        });
    }

    Json(NodeTelemetryView {
        node_id,
        returned: batches.len(),
        batches,
    })
    .into_response()
}

// ---------- series (charts) ----------

#[derive(Serialize)]
struct SeriesPoint {
    /// Unix milliseconds — friendlier for JS charting than nanos.
    t: i64,
    v: f64,
}

#[derive(Serialize)]
struct SeriesView {
    node_id: String,
    metric: String,
    points: Vec<SeriesPoint>,
    /// Latest value across all metrics, useful for KPI tiles.
    latest: Option<f64>,
    /// `raw` = 原始 10 秒数据；`hourly` = 小时聚合（窗口起点早于原始保留期时）。
    /// 前端可据此提示「这段是小时级」——见设计文档 §8。
    resolution: &'static str,
}

/// `GET /v1/nodes/:id/series?metric=<name>&from=<ms>&to=<ms>&limit=N&rate=1`
///
/// Returns one metric's recent points in chronological order. Decoding happens
/// server-side so the browser receives a compact array instead of N protobuf
/// blobs.
///
/// - `from` / `to`（毫秒，闭区间）：详情页的时间范围控件；缺省是最近 1 小时。
/// - `rate=1`：对**累计型**指标（网卡收发字节）做每秒差分，得到 bytes/s；
///   计数器回绕或节点重启导致的负增量按 0 处理，避免图上出现向下的尖刺。
async fn node_series_handler(
    State(state): State<AppState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let metric = q.get("metric").cloned().unwrap_or_default();
    if metric.is_empty() {
        return err(StatusCode::BAD_REQUEST, "query parameter `metric` required");
    }
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(120);
    let now_ms = zhiwei_common::Timestamp::now().unix_nano() / 1_000_000;
    let to_ms = q
        .get("to")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(now_ms);
    let from_ms = q
        .get("from")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(to_ms - 3_600_000);
    if from_ms >= to_ms {
        return err(StatusCode::BAD_REQUEST, "from 必须早于 to");
    }
    let rate = matches!(q.get("rate").map(String::as_str), Some("1") | Some("true"));

    // 窗口起点早于原始保留期 → 走小时聚合；否则走原始数据。
    // 原始数据的保留天数见 retention::RAW_RETENTION_DAYS。
    let raw_floor_ms = now_ms - crate::retention::RAW_RETENTION_DAYS * 24 * 60 * 60 * 1000;
    if from_ms < raw_floor_ms {
        return hourly_series(
            &state,
            node_id,
            metric,
            from_ms * 1_000_000,
            to_ms * 1_000_000,
            limit,
            rate,
        )
        .await;
    }

    let rows = match state
        .storage
        .telemetry()
        .range(&node_id, from_ms * 1_000_000, to_ms * 1_000_000, limit)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query telemetry: {e}"),
            );
        }
    };

    let mut raw: Vec<(i64, f64)> = Vec::new();
    for (ts, _interval, payload) in rows {
        let Ok(batch) = TelemetryBatch::decode(&payload[..]) else {
            continue;
        };
        if let Some(v) = extract_metric(&batch, &metric) {
            raw.push((ts / 1_000_000, v));
        }
    }

    let points: Vec<SeriesPoint> = if rate {
        to_rate(&raw)
    } else {
        raw.iter()
            .map(|(t, v)| SeriesPoint { t: *t, v: *v })
            .collect()
    };

    let latest = points.last().map(|p| p.v);

    Json(SeriesView {
        node_id,
        metric,
        points,
        latest,
        resolution: "raw",
    })
    .into_response()
}

/// `GET /v1/series/nodes?metric=<name>&from=<ms>&to=<ms>&limit=N`
///
/// 全部节点同一个指标的趋势——**一条线一个节点**，前端配图例。
/// 与单节点 `/v1/nodes/:id/series` 同一套取数与抽稀口径（长窗口同样走小时聚合）。
async fn all_nodes_series_handler(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let metric = q.get("metric").cloned().unwrap_or_default();
    if metric.is_empty() {
        return err(StatusCode::BAD_REQUEST, "query parameter `metric` required");
    }
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(300);
    let now_ms = zhiwei_common::Timestamp::now().unix_nano() / 1_000_000;
    let to_ms = q
        .get("to")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(now_ms);
    let from_ms = q
        .get("from")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(to_ms - 3_600_000);
    if from_ms >= to_ms {
        return err(StatusCode::BAD_REQUEST, "from 必须早于 to");
    }

    let raw_floor_ms = now_ms - crate::retention::RAW_RETENTION_DAYS * 24 * 60 * 60 * 1000;
    let hourly = from_ms < raw_floor_ms;

    let nodes = match state.storage.nodes().list_all().await {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list nodes: {e}"),
            )
        }
    };

    let telemetry = state.storage.telemetry();
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(nodes.len());
    for n in &nodes {
        let points: Vec<SeriesPoint> = if hourly {
            match telemetry
                .hourly_range(
                    &n.id,
                    &metric,
                    from_ms * 1_000_000,
                    to_ms * 1_000_000,
                    limit,
                )
                .await
            {
                Ok(rows) => rows
                    .into_iter()
                    .map(|(ts, avg, ..)| SeriesPoint {
                        t: ts / 1_000_000,
                        v: avg,
                    })
                    .collect(),
                Err(e) => {
                    warn!(error = %e, node = %n.hostname, "取小时聚合失败");
                    Vec::new()
                }
            }
        } else {
            match telemetry
                .range(&n.id, from_ms * 1_000_000, to_ms * 1_000_000, limit)
                .await
            {
                Ok(rows) => rows
                    .into_iter()
                    .filter_map(|(ts, _interval, payload)| {
                        let batch = TelemetryBatch::decode(&payload[..]).ok()?;
                        extract_metric(&batch, &metric).map(|v| SeriesPoint {
                            t: ts / 1_000_000,
                            v,
                        })
                    })
                    .collect(),
                Err(e) => {
                    warn!(error = %e, node = %n.hostname, "取遥测失败");
                    Vec::new()
                }
            }
        };
        // 没有数据的节点不画线，图例里也就不会出现无意义的空条目
        if points.is_empty() {
            continue;
        }
        out.push(serde_json::json!({
            "node_id": n.id,
            "hostname": n.hostname,
            "latest": points.last().map(|p| p.v),
            "points": points,
        }));
    }

    Json(serde_json::json!({
        "metric": metric,
        "resolution": if hourly { "hourly" } else { "raw" },
        "nodes": out,
    }))
    .into_response()
}

/// 长窗口走小时聚合表。每桶一条：非速率取平均值；
/// 速率用桶内的 first/last 还原（`(last - first) / 3600`），
/// 所以计数器类指标（网络字节数）在这里依然画得出来。
async fn hourly_series(
    state: &AppState,
    node_id: String,
    metric: String,
    from_ns: i64,
    to_ns: i64,
    limit: i64,
    rate: bool,
) -> Response {
    let rows = match state
        .storage
        .telemetry()
        .hourly_range(&node_id, &metric, from_ns, to_ns, limit)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query hourly telemetry: {e}"),
            )
        }
    };

    let points: Vec<SeriesPoint> = rows
        .iter()
        .map(|(ts, avg, _min, _max, first, last, _samples)| SeriesPoint {
            t: ts / 1_000_000,
            v: if rate {
                ((last - first) / 3600.0).max(0.0)
            } else {
                *avg
            },
        })
        .collect();
    let latest = points.last().map(|p| p.v);

    Json(SeriesView {
        node_id,
        metric,
        points,
        latest,
        resolution: "hourly",
    })
    .into_response()
}

/// 指标抽取：`metrics[]` 是主来源；**派生指标**在这里补齐，让控制台可以像
/// 普通指标一样画 / 告警：
///
/// - `host.net.rx_bytes` / `host.net.tx_bytes`：不在 `metrics[]` 里（每网卡一份），
///   跨网卡求和；
/// - `host.mem.usage`：节点上报的是 used/total 两个字节数，百分比在这里算。
///   这条曾经漏掉过——播种规则「内存使用率过高」用的就是这个指标名，
///   而节点从不上报它、求值也只查 `metrics[]`，**那条规则永远不会触发**。
pub(crate) fn extract_metric(batch: &TelemetryBatch, name: &str) -> Option<f64> {
    if let Some(m) = batch.metrics.iter().find(|m| m.name == name) {
        return Some(m.value);
    }
    match name {
        "host.net.rx_bytes" => sum_if_present(batch.network.iter().map(|n| n.rx_bytes as f64)),
        "host.net.tx_bytes" => sum_if_present(batch.network.iter().map(|n| n.tx_bytes as f64)),
        "host.mem.usage" => {
            let get = |n: &str| batch.metrics.iter().find(|m| m.name == n).map(|m| m.value);
            match (get("host.mem.used_bytes"), get("host.mem.total_bytes")) {
                (Some(used), Some(total)) if total > 0.0 => Some(used * 100.0 / total),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 空集合 → None（该节点没上报网卡），有值 → 求和
fn sum_if_present(it: impl Iterator<Item = f64>) -> Option<f64> {
    let vals: Vec<f64> = it.collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals.iter().sum())
    }
}

/// 累计计数器 → 每秒速率（bytes/s）。相邻两点求差，负增量按 0（计数器回绕或节点重启）。
fn to_rate(raw: &[(i64, f64)]) -> Vec<SeriesPoint> {
    raw.windows(2)
        .map(|w| {
            let dt_s = (w[1].0 - w[0].0) as f64 / 1000.0;
            let delta = w[1].1 - w[0].1;
            let v = if dt_s > 0.0 {
                (delta / dt_s).max(0.0)
            } else {
                0.0
            };
            SeriesPoint { t: w[1].0, v }
        })
        .collect()
}

// ---------- host info ----------

#[derive(Serialize)]
struct IpAddrView {
    addr: String,
    prefix: u32,
}

#[derive(Serialize)]
struct IfaceView {
    name: String,
    addresses: Vec<IpAddrView>,
    loopback: bool,
}

#[derive(Serialize)]
struct HostInfoView {
    hostname: String,
    os_name: String,
    os_version: String,
    long_os_version: String,
    kernel_version: String,
    arch: String,
    cpu_brand: String,
    cpu_cores: u32,
    total_memory_bytes: u64,
    uptime_seconds: u64,
    boot_time_unix_seconds: u64,
    interfaces: Vec<IfaceView>,
    agent_version: String,
}

fn host_info_view(info: &zhiwei_proto::telemetry::HostInfo) -> HostInfoView {
    HostInfoView {
        hostname: info.hostname.clone(),
        os_name: info.os_name.clone(),
        os_version: info.os_version.clone(),
        long_os_version: info.long_os_version.clone(),
        kernel_version: info.kernel_version.clone(),
        arch: info.arch.clone(),
        cpu_brand: info.cpu_brand.clone(),
        cpu_cores: info.cpu_cores,
        total_memory_bytes: info.total_memory_bytes,
        uptime_seconds: info.uptime_seconds,
        boot_time_unix_seconds: info.boot_time_unix_seconds,
        interfaces: info
            .interfaces
            .iter()
            .map(|i| IfaceView {
                name: i.name.clone(),
                addresses: i
                    .addresses
                    .iter()
                    .map(|a| IpAddrView {
                        addr: a.addr.clone(),
                        prefix: a.prefix,
                    })
                    .collect(),
                loopback: i.loopback,
            })
            .collect(),
        agent_version: info.agent_version.clone(),
    }
}

// ---------- inventory（快照：主机信息 + 容器 + 进程） ----------

/// `POST /v1/inventory`
///
/// 低频（默认 5 分钟）上报的「当前状态」：主机信息写回 nodes，
/// 容器与进程写进 node_inventory（每节点只留最新一份）。
/// 这类数据不进 telemetry_batches，避免每 30 秒重复存静态数据。
async fn inventory_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, _node_pub) = match verify_node(&state, &headers, method.as_str(), pq, &body).await
    {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !ct.starts_with("application/protobuf") {
        return err(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/protobuf",
        );
    }

    let report = match InventoryReport::decode(&body[..]) {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("decode InventoryReport: {e}"),
            );
        }
    };

    if report.node_id != node_id.as_str() {
        return err(StatusCode::BAD_REQUEST, "node_id 与签名主体不一致");
    }

    // 证书告警要用主机名，先留一份（host_info 是 Option，可能缺失）
    let hostname = report
        .host_info
        .as_ref()
        .map(|i| i.hostname.clone())
        .unwrap_or_else(|| node_id.as_str().to_string());

    if let Some(info) = report.host_info.as_ref() {
        match serde_json::to_string(&host_info_view(info)) {
            Ok(json) => {
                if let Err(e) = state
                    .storage
                    .nodes()
                    .update_host_info(&node_id, &json)
                    .await
                {
                    warn!(error = %e, "update_host_info failed");
                }
            }
            Err(e) => warn!(error = %e, "serialize host_info failed"),
        }
        // 节点可以自报新名字：入网时拿到的可能只是 bogon 这类占位值，
        // 换用 --node-name 或 LocalHostName 后无需重新入网即可改名
        if let Err(e) = state
            .storage
            .nodes()
            .update_hostname(&node_id, &info.hostname)
            .await
        {
            warn!(error = %e, "update_hostname failed");
        }
    }

    let containers: Vec<ContainerView> = report
        .containers
        .iter()
        .map(|c| ContainerView {
            id: c.id.clone(),
            name: c.name.clone(),
            image: c.image.clone(),
            state: c.state.clone(),
            status: c.status.clone(),
            runtime: c.runtime.clone(),
            created_at_unix_nano: c.created_at_unix_nano,
            started_at_unix_nano: c.started_at_unix_nano,
            finished_at_unix_nano: c.finished_at_unix_nano,
            compose_project: c.compose_project.clone(),
            compose_service: c.compose_service.clone(),
        })
        .collect();

    let processes: Vec<ProcessView> = report
        .processes
        .as_ref()
        .map(|p| {
            p.processes
                .iter()
                .map(|x| ProcessView {
                    pid: x.pid,
                    name: x.name.clone(),
                    cmdline: x.cmdline.clone(),
                    user: x.user.clone(),
                    cpu_percent: x.cpu_percent,
                    memory_bytes: x.memory_bytes,
                })
                .collect()
        })
        .unwrap_or_default();

    let certificates: Vec<CertView> = report
        .certificates
        .iter()
        .map(|c| CertView {
            path: c.path.clone(),
            subject: c.subject.clone(),
            issuer: c.issuer.clone(),
            not_after_unix_nano: c.not_after_unix_nano,
            not_before_unix_nano: c.not_before_unix_nano,
            domains: c.domains.clone(),
            serial: c.serial.clone(),
            parse_error: c.parse_error,
            source_id: c.source_id.clone(),
        })
        .collect();

    let containers_json = serde_json::to_string(&containers).unwrap_or_else(|_| "[]".into());
    let processes_json = serde_json::to_string(&processes).unwrap_or_else(|_| "[]".into());
    let certificates_json = serde_json::to_string(&certificates).unwrap_or_else(|_| "[]".into());

    if let Err(e) = state
        .storage
        .inventory()
        .upsert(
            &node_id,
            report.ts_unix_nano,
            &containers_json,
            &processes_json,
            &certificates_json,
        )
        .await
    {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("upsert inventory: {e}"),
        );
    }

    debug!(
        node_id = %report.node_id,
        containers = containers.len(),
        processes = processes.len(),
        certificates = certificates.len(),
        "inventory stored"
    );

    // 证书到期评估：跟快照同拍，配置改了/证书续签了都能立刻反映到告警。
    // 文案用显示名（有别名用别名），与指标告警一致。
    let display = state
        .storage
        .nodes()
        .find_by_id(&node_id)
        .await
        .ok()
        .flatten()
        .map(|n| node_display_name(&n.alias, &hostname))
        .unwrap_or_else(|| hostname.clone());
    crate::alerts::evaluate_cert_expiry(&state, node_id.as_str(), &display, &certificates_json)
        .await;
    (StatusCode::NO_CONTENT).into_response()
}

#[derive(Serialize, Clone)]
struct CertView {
    path: String,
    subject: String,
    issuer: String,
    not_after_unix_nano: i64,
    not_before_unix_nano: i64,
    domains: Vec<String>,
    serial: String,
    parse_error: bool,
    /// 命中的服务端证书来源 id；空 = 内置 / 命令行 glob 扫到的
    source_id: String,
}

#[derive(Serialize, Clone)]
struct ContainerView {
    id: String,
    name: String,
    image: String,
    state: String,
    status: String,
    runtime: String,
    created_at_unix_nano: i64,
    /// 逐容器 inspect 得到；0 = 未知（老版本节点 agent 未上报）
    started_at_unix_nano: i64,
    finished_at_unix_nano: i64,
    /// compose 项目（「应用」维度）——老版本节点 agent 上报时为空串
    #[serde(default)]
    compose_project: String,
    #[serde(default)]
    compose_service: String,
}

#[derive(Serialize, Clone)]
struct ProcessView {
    pid: i32,
    name: String,
    cmdline: String,
    user: String,
    cpu_percent: f64,
    memory_bytes: u64,
}

#[derive(Serialize)]
struct NodeContainersView {
    node_id: String,
    ts_unix_nano: Option<i64>,
    containers: serde_json::Value,
}

async fn node_containers_handler(
    State(state): State<AppState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let id = zhiwei_common::NodeId::from_string(node_id.clone());
    let row = match state.storage.inventory().find(&id).await {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query inventory: {e}"),
            );
        }
    };
    let (ts, containers) = match row {
        Some(r) => (
            Some(r.ts_unix_nano),
            serde_json::from_str(&r.containers_json).unwrap_or_else(|_| serde_json::json!([])),
        ),
        None => (None, serde_json::json!([])),
    };
    Json(NodeContainersView {
        node_id,
        ts_unix_nano: ts,
        containers,
    })
    .into_response()
}

#[derive(Serialize)]
struct NodeProcessesView {
    node_id: String,
    ts_unix_nano: Option<i64>,
    processes: serde_json::Value,
}

async fn node_processes_handler(
    State(state): State<AppState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let id = zhiwei_common::NodeId::from_string(node_id.clone());
    let row = match state.storage.inventory().find(&id).await {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query inventory: {e}"),
            );
        }
    };
    let (ts, processes) = match row {
        Some(r) => (
            Some(r.ts_unix_nano),
            serde_json::from_str(&r.processes_json).unwrap_or_else(|_| serde_json::json!([])),
        ),
        None => (None, serde_json::json!([])),
    };
    Json(NodeProcessesView {
        node_id,
        ts_unix_nano: ts,
        processes,
    })
    .into_response()
}

#[derive(Serialize)]
struct ContainerGroupView {
    node_id: String,
    hostname: String,
    /// 管理员别名（空串=未设置）；容器页的节点下拉优先显示它
    alias: String,
    ts_unix_nano: Option<i64>,
    containers: serde_json::Value,
}

/// `GET /v1/containers`：所有节点的容器，按节点分组（容器页一次请求拿全）
async fn all_containers_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let nodes = match state.storage.nodes().list_all().await {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list nodes: {e}"),
            )
        }
    };

    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let id = zhiwei_common::NodeId::from_string(n.id.clone());
        let row = state.storage.inventory().find(&id).await.ok().flatten();
        let (ts, containers) = match row {
            Some(r) => (
                Some(r.ts_unix_nano),
                serde_json::from_str(&r.containers_json).unwrap_or_else(|_| serde_json::json!([])),
            ),
            None => (None, serde_json::json!([])),
        };
        out.push(ContainerGroupView {
            node_id: n.id,
            hostname: n.hostname,
            alias: n.alias.clone(),
            ts_unix_nano: ts,
            containers,
        });
    }
    Json(out).into_response()
}

// ---------- 证书 ----------

#[derive(Serialize)]
struct NodeCertsView {
    node_id: String,
    hostname: String,
    ts_unix_nano: Option<i64>,
    certificates: serde_json::Value,
}

/// `GET /v1/certificates`：所有节点的证书（证书页一次拿全，前端按到期日排序）
async fn all_certificates_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let nodes = match state.storage.nodes().list_all().await {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list nodes: {e}"),
            )
        }
    };

    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        let id = zhiwei_common::NodeId::from_string(n.id.clone());
        let row = state.storage.inventory().find(&id).await.ok().flatten();
        let (ts, certificates) = match row {
            Some(r) => (
                Some(r.ts_unix_nano),
                serde_json::from_str(&r.certificates_json)
                    .unwrap_or_else(|_| serde_json::json!([])),
            ),
            None => (None, serde_json::json!([])),
        };
        out.push(NodeCertsView {
            node_id: n.id,
            hostname: n.hostname,
            ts_unix_nano: ts,
            certificates,
        });
    }
    Json(out).into_response()
}

// ---------- 告警与规则 ----------

#[derive(Serialize)]
struct AlertsView {
    open: Vec<zhiwei_storage::alerts_repo::Alert>,
    resolved: Vec<zhiwei_storage::alerts_repo::Alert>,
}

async fn alerts_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let repo = state.storage.alerts();
    let (open, resolved) = match (repo.open_alerts().await, repo.resolved_alerts(50).await) {
        (Ok(o), Ok(r)) => (o, r),
        (Err(e), _) | (_, Err(e)) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query alerts: {e}"),
            );
        }
    };
    Json(AlertsView { open, resolved }).into_response()
}

async fn silence_alert_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        minutes: i64,
    }
    let minutes = if body.is_empty() {
        60
    } else {
        serde_json::from_slice::<Body>(&body)
            .map(|b| b.minutes)
            .unwrap_or(60)
    };
    let until =
        zhiwei_common::Timestamp::now().unix_nano() + minutes.clamp(1, 10_080) * 60 * 1_000_000_000;
    if let Err(e) = state.storage.alerts().silence_alert(id, until).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("silence: {e}"));
    }
    info!(alert_id = id, minutes, "告警已静默");
    (StatusCode::NO_CONTENT).into_response()
}

async fn list_rules_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.alerts().list_rules().await {
        Ok(rules) => Json(rules).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list rules: {e}"),
        ),
    }
}

async fn create_rule_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        name: String,
        metric: String,
        op: String,
        threshold: f64,
        #[serde(default)]
        duration_seconds: i64,
        #[serde(default = "default_severity")]
        severity: String,
    }
    fn default_severity() -> String {
        "warning".into()
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if !["gt", "gte", "lt", "lte", "eq"].contains(&b.op.as_str()) {
        return err(StatusCode::BAD_REQUEST, "op 必须是 gt/gte/lt/lte/eq");
    }
    if !["warning", "critical"].contains(&b.severity.as_str()) {
        return err(StatusCode::BAD_REQUEST, "severity 必须是 warning/critical");
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state
        .storage
        .alerts()
        .create_rule(
            &b.name,
            &b.metric,
            &b.op,
            b.threshold,
            b.duration_seconds.max(0),
            &b.severity,
            now,
        )
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create rule: {e}"),
        ),
    }
}

async fn patch_rule_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        enabled: bool,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state
        .storage
        .alerts()
        .set_rule_enabled(id, b.enabled, now)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("patch rule: {e}"),
        ),
    }
}

async fn delete_rule_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state.storage.alerts().delete_rule(id, now).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("delete rule: {e}"),
        ),
    }
}

async fn list_channels_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.alerts().list_channels().await {
        Ok(c) => Json(c).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list channels: {e}"),
        ),
    }
}

// ---------- CA 信息 ----------

#[derive(Serialize)]
struct CaView {
    subject: String,
    not_before_unix_nano: i64,
    not_after_unix_nano: i64,
    serial: String,
    /// SHA-256 指纹（冒号分隔大写十六进制，便于人工比对）
    fingerprint_sha256: String,
    nodes_enrolled: i64,
    /// 本进程是否自己终结 TLS。
    ///
    /// false 时（托管平台，边缘终结 TLS）这个 CA **不参与任何事**：
    /// 它既不是节点信任的根（节点走系统根），monitor 的证书也是边缘签的。
    /// 前端据此把这一节写成「当前部署用不到」而不是让人以为它是集群身份根。
    tls_terminated_locally: bool,
}

async fn ca_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let pem = state.ca.cert_pem.clone();
    let info = tokio::task::spawn_blocking(move || parse_ca(&pem))
        .await
        .ok()
        .and_then(|r| r.ok());
    let Some((subject, nb, na, serial, fp)) = info else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "解析 CA 证书失败");
    };

    let nodes_enrolled = state
        .storage
        .nodes()
        .list_all()
        .await
        .map(|v| v.len() as i64)
        .unwrap_or(0);

    Json(CaView {
        subject,
        not_before_unix_nano: nb,
        not_after_unix_nano: na,
        serial,
        fingerprint_sha256: fp,
        nodes_enrolled,
        tls_terminated_locally: state.tls_terminated_locally,
    })
    .into_response()
}

/// 解析本地 CA 证书（PEM）取主体 / 有效期 / 序列号 / 指纹
#[allow(clippy::type_complexity)]
fn parse_ca(pem: &str) -> anyhow::Result<(String, i64, i64, String, String)> {
    use x509_parser::prelude::FromDer;
    let der = pem
        .lines()
        .filter(|l| !l.trim_start().starts_with("-----"))
        .collect::<String>();
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(der.trim())?;
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&bytes)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let fingerprint = digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");

    Ok((
        cert.subject().to_string(),
        cert.validity().not_before.timestamp() * 1_000_000_000,
        cert.validity().not_after.timestamp() * 1_000_000_000,
        cert.raw_serial_as_string(),
        fingerprint,
    ))
}

// ---------- 通知渠道 ----------

async fn create_channel_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        name: String,
        #[serde(default = "default_kind")]
        kind: String,
        url: String,
        #[serde(default)]
        secret: String,
        #[serde(default = "default_sev")]
        min_severity: String,
    }
    fn default_kind() -> String {
        "webhook".into()
    }
    fn default_sev() -> String {
        "warning".into()
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if b.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "url 不能为空");
    }
    if !crate::alerts::CHANNEL_KINDS.contains(&b.kind.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "不支持的通知类型 {}（可选：{}）",
                b.kind,
                crate::alerts::CHANNEL_KINDS.join(" / ")
            ),
        );
    }
    if !matches!(b.min_severity.as_str(), "warning" | "critical") {
        return err(
            StatusCode::BAD_REQUEST,
            "min_severity 只能是 warning 或 critical",
        );
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state
        .storage
        .alerts()
        .create_channel(&b.name, &b.kind, &b.url, &b.secret, &b.min_severity, now)
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create channel: {e}"),
        ),
    }
}

/// `POST /v1/channels/test` —— 拿当前填的参数**真发一条**，返回成功或失败原因。
///
/// 用参数而不是渠道 id：新增渠道的对话框里，「测试」要在保存之前就能点。
async fn test_channel_handler(
    State(_state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&_state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        kind: String,
        url: String,
        #[serde(default)]
        secret: String,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if b.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "url 不能为空");
    }
    if !crate::alerts::CHANNEL_KINDS.contains(&b.kind.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("不支持的通知类型 {}", b.kind),
        );
    }

    let now = zhiwei_common::Timestamp::now().unix_nano();
    let payload = crate::alerts::channel_body(
        &b.kind,
        &crate::alerts::test_rule(),
        &crate::alerts::test_facts(),
        now,
    );
    match crate::alerts::post_webhook(&b.url, &b.secret, &payload).await {
        Ok(()) => Json(serde_json::json!({ "ok": true, "detail": "" })).into_response(),
        // 投递失败不是服务端错误——把原因如实回给控制台，用户自己判断
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "detail": e.to_string(),
        }))
        .into_response(),
    }
}

/// `POST /v1/admin/token` —— 改控制台凭据。
///
/// 要求带上当前凭据（等价于确认操作），新凭据写回 `<data-dir>/admin.token`
/// 并立即生效。节点那一侧不受影响——它们走签名，不认这个凭据。
async fn change_admin_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(serde::Deserialize)]
    struct Body {
        current: String,
        new: String,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };

    {
        let current = state.admin_token.read().unwrap_or_else(|e| e.into_inner());
        if !crate::admin::ct_eq(b.current.trim().as_bytes(), current.as_bytes()) {
            return err(StatusCode::UNAUTHORIZED, "当前凭据不正确");
        }
    }
    if let Err(msg) = crate::admin::validate_new_token(b.new.trim()) {
        return err(StatusCode::BAD_REQUEST, msg);
    }

    if let Err(e) = crate::admin::save(&state.data_dir, b.new.trim()).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("写入新凭据失败：{e}"),
        );
    }
    // 先落盘再改内存：反过来的话，落盘失败会让内存与文件不一致
    {
        let mut current = state.admin_token.write().unwrap_or_else(|e| e.into_inner());
        *current = b.new.trim().to_string();
    }
    // headers 参与鉴权只是为了与其它 handler 的形状一致，这里不做前置校验——
    // 改凭据本身要求带 current，等于已确认过身份
    let _ = headers;
    Json(serde_json::json!({ "ok": true })).into_response()
}

async fn patch_channel_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        enabled: bool,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    match state
        .storage
        .alerts()
        .set_channel_enabled(id, b.enabled)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("patch channel: {e}"),
        ),
    }
}

async fn delete_channel_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.alerts().delete_channel(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("delete channel: {e}"),
        ),
    }
}

// ---------- 控制通道 ----------

#[derive(Serialize)]
struct NodeCommandsView {
    /// base64 的 Command protobuf 列表，节点自行验签
    commands: Vec<String>,
}

/// 节点长轮询可挂起的最长时间（秒）。没有命令时把它挂在这里，
/// 一旦有新命令（本机 ops 签发成功）立刻返回——比 10s 定时轮询快一个数量级。
const MAX_COMMAND_WAIT_SECS: u64 = 25;
/// 即便没人唤醒也每这么久查一次库：兜住「命令不是本进程签发」的边角
/// （多实例、运维手工插库），保证最长 2s 也能拿到。
const COMMAND_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);

/// 拉取某节点的待执行命令。`wait_secs > 0` 时为长轮询：
/// 挂起直到有命令或超时（返回空列表）。
///
/// 关键顺序是「先订阅、再查库」：这样查库与 await 之间落库的命令不会漏掉信号，
/// 否则会出现「命令已入库、节点却睡满一个超时」的间隙。
async fn collect_pending(
    state: &AppState,
    node_id: &str,
    wait_secs: u64,
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
    let mut rx = state.command_signal.subscribe();
    loop {
        let rows = state.storage.commands().pending_for(node_id, 10).await?;
        if !rows.is_empty() || wait_secs == 0 {
            return Ok(rows);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Vec::new());
        }
        tokio::select! {
            _ = rx.changed() => {}
            _ = tokio::time::sleep(remaining.min(COMMAND_RECHECK)) => {}
        }
    }
}

/// 节点拉取待执行命令（请求签名）。返回 base64 的 Command protobuf，命令签名由节点校验。
///
/// `wait=<秒>`（可选，上限 [`MAX_COMMAND_WAIT_SECS`]）开启长轮询；不带则立即返回，
/// 老版本节点行为不变。
async fn node_commands_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, _node_pub) = match verify_node(&state, &headers, method.as_str(), pq, &[]).await {
        Ok(v) => v,
        Err((code, msg)) => return err(code, msg),
    };
    let Some(query_node_id) = q.get("node_id").cloned() else {
        return err(StatusCode::BAD_REQUEST, "node_id required");
    };
    if query_node_id != node_id.as_str() {
        return err(StatusCode::BAD_REQUEST, "node_id 与签名主体不一致");
    }
    let wait_secs = q
        .get("wait")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(MAX_COMMAND_WAIT_SECS);

    match collect_pending(&state, node_id.as_str(), wait_secs).await {
        Ok(rows) => {
            use base64::Engine;
            let commands = rows
                .into_iter()
                .map(|(_, payload)| base64::engine::general_purpose::STANDARD.encode(payload))
                .collect();
            let view = NodeCommandsView { commands };
            match serde_json::to_vec(&view) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(json))
                    .unwrap(),
                Err(e) => err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("serialize commands: {e}"),
                ),
            }
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("query commands: {e}"),
        ),
    }
}

/// 节点回执（请求签名）。只有签名主体本人能提交自己的回执。
async fn command_result_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, node_pub) = match verify_node(&state, &headers, method.as_str(), pq, &body).await
    {
        Ok(v) => v,
        Err((code, msg)) => return err(code, msg),
    };

    let result = match CommandResult::decode(&body[..]) {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("decode CommandResult: {e}"),
            );
        }
    };
    if result.command_id != id {
        return err(StatusCode::BAD_REQUEST, "command_id 与路径不一致");
    }
    if result.node_id != node_id.as_str() {
        return err(StatusCode::BAD_REQUEST, "回执 node_id 与签名主体不一致");
    }

    // 回执由节点私钥签名：即使传输层被攻破，也无法伪造别人的执行结果
    {
        let mut unsigned = result.clone();
        let sig = zhiwei_common::Signature(std::mem::take(&mut unsigned.signature));
        let mut preimage = Vec::new();
        if let Err(e) = prost::Message::encode(&unsigned, &mut preimage) {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("encode result: {e}"),
            );
        }
        if zhiwei_common::KeyPair::verify(&node_pub, &preimage, &sig).is_err() {
            return err(StatusCode::UNAUTHORIZED, "回执签名校验失败");
        }
    }

    // 回执只能写回自己名下的命令，避免用 A 的签名覆盖 B 的命令结果
    let owner = match state.storage.commands().owner_of(&id).await {
        Ok(owner) => owner,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("command lookup: {e}"),
            )
        }
    };
    match owner {
        Some(owner) if owner == node_id.as_str() => {}
        Some(_) => return err(StatusCode::FORBIDDEN, "回执主体与命令归属不一致"),
        None => return err(StatusCode::NOT_FOUND, "命令不存在"),
    }

    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state
        .storage
        .commands()
        .submit_result(
            &id,
            result.ok,
            &result.error,
            Some(result.payload.as_slice()),
            now,
        )
        .await
    {
        Ok(()) => {
            debug!(command_id = %id, ok = result.ok, "命令回执已记录");
            (StatusCode::NO_CONTENT).into_response()
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("store result: {e}"),
        ),
    }
}

#[derive(serde::Deserialize)]
struct ExecBody {
    node_id: String,
    action: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// 控制台发起命令：转给 ops-server 签名后落库（见 crates/ops-server）。
async fn exec_handler(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: ExecBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };

    // 节点必须存在，避免给未知节点签发命令
    let node = zhiwei_common::NodeId::from_string(b.node_id.clone());
    match state.storage.nodes().find_by_id(&node).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "node not enrolled"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    }

    // 转给 ops-server 签名（localhost，明文 HTTP；ops 是独立进程、持有签名私钥）
    let payload = serde_json::json!({
        "node_id": b.node_id,
        "action": b.action,
        "params": b.params,
        "actor": "console",
    });
    match sign_command(&state, &payload).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "command_id": id })),
        )
            .into_response(),
        // ops 的 4xx 是「动作/参数被拒」，原因要原样透给控制台——
        // 否则界面上只会看到「服务不可用」，用户改不了参数只会反复重试
        Err(OpsSignError::Rejected { status, message }) => err(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            message,
        ),
        Err(OpsSignError::Unavailable(detail)) => err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("ops-server 不可用：{detail}"),
        ),
    }
}

#[derive(Serialize)]
struct CommandHistoryView {
    id: String,
    node_id: String,
    node_hostname: String,
    action: String,
    params_json: String,
    state: String,
    issued_at_unix_nano: i64,
    ttl_seconds: i64,
    result_ok: Option<bool>,
    result_error: Option<String>,
    /// 产物以 UTF-8 文本返回（日志之类）
    result_text: Option<String>,
    result_received_at_unix_nano: Option<i64>,
}

/// 命令行 → 控制台视图。history 与单条查询共用，字段口径只有这一处。
fn command_view(
    c: zhiwei_storage::commands_repo::CommandRow,
    node_hostname: String,
) -> CommandHistoryView {
    CommandHistoryView {
        node_hostname,
        result_text: c
            .result_payload
            .as_ref()
            .and_then(|p| String::from_utf8(p.clone()).ok()),
        id: c.id,
        node_id: c.node_id,
        action: c.action,
        params_json: c.params_json,
        state: c.state,
        issued_at_unix_nano: c.issued_at_unix_nano,
        ttl_seconds: c.ttl_seconds,
        result_ok: c.result_ok,
        result_error: c.result_error,
        result_received_at_unix_nano: c.result_received_at_unix_nano,
    }
}

async fn command_history_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let rows = match state.storage.commands().recent(100).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("history: {e}")),
    };
    let nodes = state.storage.nodes().list_all().await.unwrap_or_default();
    let name_of = |id: &str| {
        nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.hostname.clone())
            .unwrap_or_default()
    };

    let out: Vec<CommandHistoryView> = rows
        .into_iter()
        .map(|c| {
            let host = name_of(&c.node_id);
            command_view(c, host)
        })
        .collect();
    Json(out).into_response()
}

/// `GET /v1/commands/:id` —— 单条命令的当前状态。
///
/// 控制台等一条命令的回执时只关心这一条：拉整段 history（100 条）既重又容易被
/// 别的动作干扰，单条查询让轮询间隔可以压到几百毫秒，点完动作几秒内就能看到结果。
async fn command_detail_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let row = match state.storage.commands().find(&id).await {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "命令不存在"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    };
    let hostname = state
        .storage
        .nodes()
        .list_all()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|n| n.id == row.node_id)
        .map(|n| n.hostname)
        .unwrap_or_default();
    Json(command_view(row, hostname)).into_response()
}

/// 把签名请求转给 ops-server（localhost）
/// 转发签名请求失败的两类原因：ops 明确拒绝（4xx，可展示给用户）
/// 与链路问题（连不上 / 超时 / 5xx，属于「服务不可用」）。
pub(crate) enum OpsSignError {
    Rejected { status: u16, message: String },
    Unavailable(String),
}

impl OpsSignError {
    /// 只给日志用的短描述（面向用户的文案走上面两类的分支处理）
    pub(crate) fn message(&self) -> String {
        match self {
            OpsSignError::Rejected { message, .. } => message.clone(),
            OpsSignError::Unavailable(detail) => detail.clone(),
        }
    }
}

/// 发起一条命令：转给 ops-server 签名，成功后唤醒等待中的节点长轮询。
///
/// 所有写操作的必须入口——少走这一处，节点就得等下一次兜底查库才拿到命令。
pub(crate) async fn sign_command(
    state: &AppState,
    payload: &serde_json::Value,
) -> Result<String, OpsSignError> {
    let id = ops_sign(&state.ops_endpoint, payload).await?;
    state.notify_command();
    Ok(id)
}

pub(crate) async fn ops_sign(
    endpoint: &str,
    payload: &serde_json::Value,
) -> Result<String, OpsSignError> {
    let authority = endpoint.strip_prefix("http://").ok_or_else(|| {
        OpsSignError::Unavailable("ops endpoint 仅支持 http://（本机回环）".into())
    })?;
    let (host_port, path) = match authority.find('/') {
        Some(i) => (&authority[..i], &authority[i..]),
        None => (authority, "/exec"),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(8444)),
        None => (host_port, 8444),
    };

    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_| {
        OpsSignError::Unavailable(format!(
            "连不上 {endpoint}：3 秒内没有响应。自建部署请确认 zhiwei-ops 在跑；\
             容器 / 托管平台部署请用镜像自带的 entrypoint。"
        ))
    })?
    .map_err(|e| OpsSignError::Unavailable(ops_connect_error_hint(&e, endpoint)))?;

    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| OpsSignError::Unavailable(format!("握手失败：{e}")))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body = payload.to_string();
    let req = hyper::Request::builder()
        .method("POST")
        .uri(path)
        .header("Host", host)
        .header("Content-Type", "application/json")
        .body(http_body_util::Full::new(bytes::Bytes::from(body)))
        .map_err(|e| OpsSignError::Unavailable(format!("构造请求失败：{e}")))?;

    let res = tokio::time::timeout(std::time::Duration::from_secs(3), sender.send_request(req))
        .await
        .map_err(|_| OpsSignError::Unavailable("请求超时".into()))?
        .map_err(|e| OpsSignError::Unavailable(format!("请求失败：{e}")))?;
    let status = res.status();
    use http_body_util::BodyExt;
    let bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| OpsSignError::Unavailable(format!("读取响应失败：{e}")))?
        .to_bytes();
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        if status.is_client_error() {
            // ops 的错误体形如 {"error":"..."}，取出来原样展示
            let message = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or(text);
            return Err(OpsSignError::Rejected {
                status: status.as_u16(),
                message,
            });
        }
        return Err(OpsSignError::Unavailable(format!("HTTP {status}: {text}")));
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| OpsSignError::Unavailable(format!("响应不是 JSON：{e}")))?;
    v.get("command_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| OpsSignError::Unavailable("ops 响应缺少 command_id".into()))
}

#[cfg(test)]
mod series_tests {
    use super::*;
    use zhiwei_proto::telemetry::{Metric, NetworkInterface};

    fn batch_with(metrics: Vec<(&str, f64)>, net: Vec<(u64, u64)>) -> TelemetryBatch {
        TelemetryBatch {
            node_id: "n1".into(),
            ts_unix_nano: 0,
            interval_seconds: 10,
            metrics: metrics
                .into_iter()
                .map(|(name, value)| Metric {
                    name: name.into(),
                    value,
                    labels: Default::default(),
                })
                .collect(),
            network: net
                .into_iter()
                .enumerate()
                .map(|(i, (rx, tx))| NetworkInterface {
                    name: format!("eth{i}"),
                    rx_bytes: rx,
                    tx_bytes: tx,
                    rx_packets: 0,
                    tx_packets: 0,
                })
                .collect(),
            disks: vec![],
            signature: vec![],
        }
    }

    #[test]
    fn extract_metric_prefers_metrics_then_derives_network_sum() {
        let b = batch_with(vec![("host.cpu.usage", 12.5)], vec![(100, 200), (300, 400)]);
        assert_eq!(extract_metric(&b, "host.cpu.usage"), Some(12.5));
        // 网络累计量不在 metrics[] 里，跨网卡求和
        assert_eq!(extract_metric(&b, "host.net.rx_bytes"), Some(400.0));
        assert_eq!(extract_metric(&b, "host.net.tx_bytes"), Some(600.0));
        assert_eq!(extract_metric(&b, "host.disk.total_bytes"), None);

        // 没有网卡的节点：不该返回 0，而是 None（图上留空，而不是画一条零线）
        let empty = batch_with(vec![], vec![]);
        assert_eq!(extract_metric(&empty, "host.net.rx_bytes"), None);
    }

    #[test]
    fn extract_metric_derives_memory_usage_percent() {
        // 播种规则「内存使用率过高」用的就是 host.mem.usage，而节点只上报
        // used/total 两个字节数——不在这里派生，那条规则永远不会触发。
        let b = batch_with(
            vec![
                ("host.mem.used_bytes", 8_000_000_000.0),
                ("host.mem.total_bytes", 16_000_000_000.0),
            ],
            vec![],
        );
        assert_eq!(extract_metric(&b, "host.mem.usage"), Some(50.0));

        // 节点已上报真值时就以真值优先，不覆盖
        let reported = batch_with(
            vec![
                ("host.mem.usage", 42.0),
                ("host.mem.used_bytes", 8_000_000_000.0),
                ("host.mem.total_bytes", 16_000_000_000.0),
            ],
            vec![],
        );
        assert_eq!(extract_metric(&reported, "host.mem.usage"), Some(42.0));

        // total 缺失或为 0：返回 None，而不是编一个数出来
        let only_used = batch_with(vec![("host.mem.used_bytes", 123.0)], vec![]);
        assert_eq!(extract_metric(&only_used, "host.mem.usage"), None);
        let zero_total = batch_with(
            vec![
                ("host.mem.used_bytes", 123.0),
                ("host.mem.total_bytes", 0.0),
            ],
            vec![],
        );
        assert_eq!(extract_metric(&zero_total, "host.mem.usage"), None);
    }

    #[test]
    fn to_rate_is_per_second_and_clamps_counter_reset() {
        let rising = to_rate(&[(0, 100.0), (1_000, 300.0), (2_000, 500.0)]);
        assert_eq!(rising.len(), 2);
        assert_eq!(rising[0].t, 1_000);
        assert_eq!(rising[0].v, 200.0);
        assert_eq!(rising[1].v, 200.0);

        // 计数器回绕 / 节点重启：负增量按 0，不画向下的尖刺
        let reset = to_rate(&[(0, 9_000.0), (1_000, 100.0)]);
        assert_eq!(reset[0].v, 0.0);

        // 单点无法求差 → 空序列（图上是空状态，而不是 0）
        assert!(to_rate(&[(0, 1.0)]).is_empty());
    }

    #[test]
    fn latest_view_reads_key_metrics_from_one_batch() {
        let b = batch_with(
            vec![
                ("host.cpu.usage", 42.0),
                ("host.mem.used_bytes", 1024.0),
                ("host.mem.total_bytes", 4096.0),
                ("host.disk.usage", 88.0),
            ],
            vec![(10, 20)],
        );
        let v = latest_view(123, &b);
        assert_eq!(v.ts_unix_nano, 123);
        assert_eq!(v.cpu_percent, Some(42.0));
        assert_eq!(v.mem_used_bytes, Some(1024.0));
        assert_eq!(v.disk_usage_percent, Some(88.0));
        assert_eq!(v.net_rx_bytes, Some(10.0));
    }
}

#[cfg(test)]
mod enroll_ca_tests {
    use super::*;

    const LOCAL_CA: &str = "-----BEGIN CERTIFICATE-----\nlocal\n-----END CERTIFICATE-----\n";

    #[test]
    fn self_hosted_hands_the_local_ca_to_the_node() {
        // 本进程终结 TLS：monitor 的证书就是这个 CA 签的，节点该 pin 它
        assert_eq!(enroll_ca_pem(LOCAL_CA, true), LOCAL_CA);
    }

    #[test]
    fn edge_terminated_tls_hands_nothing_so_the_node_uses_system_roots() {
        // Render / Railway：边缘用的是正经证书，本地 CA 与它无关。
        // 下发了就会变成「enroll 成功、之后每个请求都 TLS 校验失败」。
        assert_eq!(enroll_ca_pem(LOCAL_CA, false), "");
    }
}

#[cfg(test)]
mod bootstrap_token_tests {
    use super::*;

    #[test]
    fn static_token_never_expires_and_survives_repeated_checks() {
        // ZHIWEI_BOOTSTRAP_TOKEN 走这条路：托管平台免费层没法去抢
        // 「启动日志里 10 分钟」的一次性 token，得有个长期有效的。
        let tokens = BootstrapTokens::default();
        tokens.add_static("zhi-bt-fixed-token-0123456789".into());

        // 反复 check：过期分支会 remove，静态令牌不能因此消失。
        for _ in 0..3 {
            assert!(tokens.check("zhi-bt-fixed-token-0123456789"));
        }
        assert!(!tokens.check("zhi-bt-something-else"));
    }

    #[tokio::test]
    async fn static_token_keeps_the_ephemeral_one_from_being_minted() {
        // 设了固定令牌，启动时就不该再生成/打印一个一次性 token。
        let tokens = BootstrapTokens::default();
        assert!(tokens.is_empty().await);
        tokens.add_static("zhi-bt-fixed-token-0123456789".into());
        assert!(!tokens.is_empty().await);
    }

    #[tokio::test]
    async fn one_shot_token_still_expires() {
        // 回归：一次性 token 的 10 分钟 TTL 不能被上面的改动弄丢。
        let tokens = BootstrapTokens::default();
        tokens.add("zhi-bt-short-lived".into(), 0).await; // 立刻过期
        assert!(!tokens.check("zhi-bt-short-lived"));
    }
}

/// `GET /v1/help` —— 帮助页 markdown 内容。
///
/// 鉴权：admin token 或 AI token 都可读（AI 客户端如果要做 onboarding 也用得到）。
/// 没找到 help 文件时返回空 body，UI 端展示占位文案。
///
/// 正文里的 `{{BASE_URL}}` 会换成**控制台当前的访问地址**（scheme + host，
/// 与 enroll 命令同源），用户照帮助页复制命令就能直接跑，不必手改示例域名。
async fn help_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin or AI token)",
        );
    }
    let mut snap = state.help.snapshot();
    let base = public_base_url(&headers, state.tls_terminated_locally);
    snap.body = snap.body.replace(BASE_URL_PLACEHOLDER, &base);
    Json(snap).into_response()
}

/// 帮助页里代表「控制台地址」的占位符（见 `assets/help.md`）。
const BASE_URL_PLACEHOLDER: &str = "{{BASE_URL}}";

// ---------- AI Tokens ----------

/// `GET /v1/ai-tokens` —— 列出所有 AI token 元信息（不含明文）。
async fn list_ai_tokens_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.ai_tokens().list_all().await {
        Ok(rows) => Json(serde_json::json!({ "tokens": rows })).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list ai tokens: {e}"),
        ),
    }
}

/// `POST /v1/ai-tokens` —— 创建 AI token。返回明文 token（**仅这一次**）。
async fn create_ai_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(Deserialize)]
    struct Body {
        name: String,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let name = b.name.trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name 不能为空");
    }
    if name.chars().count() > 64 {
        return err(StatusCode::BAD_REQUEST, "name 过长（>64 字符）");
    }

    // 生成 32 字节熵的明文 token → base64url 编码。
    // 前缀 `ait_` 与 bootstrap token 的 `zhi-bt-` 区分，便于 grep。
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    use base64::Engine;
    let token = format!(
        "ait_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
    );
    let id = format!("ait-{}", &hex_encode(&rand_bytes_3())[..6]);
    let hash = sha256_hex(token.as_bytes());
    let now_unix_nano = zhiwei_common::Timestamp::now().unix_nano();

    match state
        .storage
        .ai_tokens()
        .create(&id, &hash, name, now_unix_nano)
        .await
    {
        Ok(row) => Json(serde_json::json!({
            "id": row.id,
            "name": row.name,
            "token": token,
            "created_at_unix_nano": row.created_at_unix_nano,
            "warning": "明文 token 仅返回一次，请立即复制保存",
        }))
        .into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create ai token: {e}"),
        ),
    }
}

/// `DELETE /v1/ai-tokens/:id` —— 撤销。立即生效（下次请求 401）。
async fn delete_ai_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let now_unix_nano = zhiwei_common::Timestamp::now().unix_nano();
    match state.storage.ai_tokens().revoke(&id, now_unix_nano).await {
        Ok(true) => Json(serde_json::json!({ "ok": true, "id": id })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "id 不存在或已撤销"),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("revoke ai token: {e}"),
        ),
    }
}

// ---------- Enroll Tokens（运行时入网命令） ----------

/// `GET /v1/enroll-tokens` —— 列出当前所有未过期的入网令牌元信息。
async fn list_enroll_tokens_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    Json(serde_json::json!({ "tokens": state.bootstrap_tokens.list_active() })).into_response()
}

#[derive(Deserialize)]
struct CreateEnrollTokenBody {
    /// TTL 秒数；默认 86400 (24h)。最长 7 天。
    #[serde(default)]
    ttl_secs: Option<u64>,
    /// 可选 label；为空也合法。
    #[serde(default)]
    label: Option<String>,
}

/// 连不上 ops-server 时给控制台看的文案。
///
/// 「没起 ops-server」是部署形态问题，原始 OS 报错（`Connection refused
/// (os error 111)`）对使用者毫无帮助，所以这里直接把「该做什么」写进文案；
/// 调用方已经加了「ops-server 不可用：」前缀，这里别再复述。
fn ops_connect_error_hint(e: &std::io::Error, endpoint: &str) -> String {
    if is_connection_refused(e) {
        format!(
            "连不上 {endpoint}（{e}）：该地址上没有进程在监听。\
             自建部署请启动 zhiwei-ops；容器 / 托管平台部署请用镜像自带的 \
             entrypoint（它会在同一容器里一并拉起 zhiwei-ops）。"
        )
    } else {
        format!(
            "连不上 {endpoint}（{e}）：自建部署请确认 zhiwei-ops 在跑；\
             容器 / 托管平台部署请用镜像自带的 entrypoint。"
        )
    }
}

/// 判断「对端没有监听」。
///
/// 光看 `ErrorKind` 不够：解析主机名 / 经过中间层时 kind 会掉成
/// `Uncategorized`，只剩文案里的 "Connection refused" 还认得出来——线上
/// 报回来的正是 `连接失败：Connection refused (os error 111)` 这种形态，
/// 只认 kind 就漏成了没头没尾的原始报错。errno 111 是 Linux 的 ECONNREFUSED。
fn is_connection_refused(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::ConnectionRefused
        || e.raw_os_error() == Some(111)
        || e.to_string().contains("Connection refused")
}

/// 节点在「给人看」的语境里用的名字：设了别名就用别名，否则退回主机名。
///
/// 告警文案、通知、待办都用它——主机名常常是 `VM-16-12-opencloudos` 这种，
/// 用户看不出是哪台机器；别名是用户自己起的「北京入口」。
pub(crate) fn node_display_name(alias: &str, hostname: &str) -> String {
    let alias = alias.trim();
    if alias.is_empty() {
        hostname.to_string()
    } else {
        alias.to_string()
    }
}

/// 控制台的对外访问地址（scheme + host），从请求头推断。
///
/// enroll 命令与帮助页共用这一处：两处给出的地址必须一致，否则用户照帮助页
/// 粘命令会因为地址不对而连错。挂在 nginx / 托管平台后面时优先
/// `X-Forwarded-Proto`，其余回退见 [`enroll_url_scheme`]。
fn public_base_url(headers: &HeaderMap, tls_terminated_locally: bool) -> String {
    let scheme = enroll_url_scheme(headers, tls_terminated_locally);
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:8443");
    format!("{scheme}://{host}")
}

/// 生成入网命令时该用 http 还是 https。
///
/// 1) `X-Forwarded-Proto` 最权威——只有边缘知道对外那一段是明文还是 TLS；
/// 2) 没有它：本进程自己终结 TLS → https；
/// 3) 本进程是明文 HTTP，且 Host 是回环 / 内网地址 → http。
///
/// 第 3 条专治「内网 IP + 明文」的自建部署：以前一律猜 https，控制台给出的
/// 命令是 `https://10.0.0.5:8443/install-node.sh`，照着执行必然连不上。
/// 公网域名不在第 3 条范围内（仍猜 https）：托管平台边缘几乎总会给
/// `X-Forwarded-Proto`，而它没给的时候「对外是 https」的可能性更大。
fn enroll_url_scheme(headers: &HeaderMap, tls_terminated_locally: bool) -> &'static str {
    if let Some(v) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        // 可能是 "https, http" 这种列表，取第一个
        let first = v.split(',').next().unwrap_or("").trim();
        if !first.is_empty() {
            return match first {
                "http" => "http",
                "https" => "https",
                _ => "https",
            };
        }
    }
    if tls_terminated_locally {
        return "https";
    }
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if host_is_local(host) {
        "http"
    } else {
        "https"
    }
}

/// Host 头（可带端口）是不是回环 / 内网 / mDNS 地址。
fn host_is_local(host: &str) -> bool {
    let bare = match host.rsplit_once(':') {
        // IPv6 字面量形如 [::1]:8443，去掉端口后还要剥掉方括号
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    let bare = bare.trim_start_matches('[').trim_end_matches(']');
    if bare.eq_ignore_ascii_case("localhost") || bare.ends_with(".local") {
        return true;
    }
    match bare.parse::<std::net::IpAddr>() {
        Ok(ip) => match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
        },
        Err(_) => false,
    }
}

/// `POST /v1/enroll-tokens` —— 创建一个临时入网令牌，返回完整 enroll 命令。
async fn create_enroll_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: CreateEnrollTokenBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let ttl_secs = b.ttl_secs.unwrap_or(86_400);
    if ttl_secs == 0 || ttl_secs > 7 * 24 * 3600 {
        return err(StatusCode::BAD_REQUEST, "ttl_secs 必须在 1..=604800");
    }
    let label = b.label.unwrap_or_default().trim().to_string();
    if label.chars().count() > 64 {
        return err(StatusCode::BAD_REQUEST, "label 过长（>64 字符）");
    }

    let token = BootstrapTokens::mint();
    state
        .bootstrap_tokens
        .add_with_label(token.clone(), ttl_secs, label.clone())
        .await;
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expires_at_unix = now_unix + ttl_secs;

    // 从请求头推断 monitor 公开 URL：
    //   1) 优先 `X-Forwarded-Proto` + `Host`（托管平台 / nginx 会注入）
    //   2) 回退见 enroll_url_scheme
    let monitor_url = public_base_url(&headers, state.tls_terminated_locally);

    // 设了 ZHIWEI_NODE_BASE_URL（国内 / 隔离网络的自建分发源）时，命令里自动
    // 多带一行，执行者不必自己记得加。没设就保持原样（走 GitHub Releases）。
    let base_url_line = match state.node_base_url.as_deref() {
        Some(u) => format!("    ZHIWEI_BASE_URL={u} \\\n"),
        None => String::new(),
    };
    let enroll_command = format!(
        "curl -sSL {monitor_url}/install-node.sh \\\n  | ZHIWEI_MONITOR_URL={monitor_url} \\\n    ZHIWEI_BOOTSTRAP_TOKEN={token} \\\n{base_url_line}    bash -s"
    );

    // 从 list_active 找到刚加的那条 id（保证 id 与服务端一致）。
    let just_added = state
        .bootstrap_tokens
        .list_active()
        .into_iter()
        .find(|m| m.label == label && m.expires_at_unix == expires_at_unix)
        .map(|m| m.id);

    Json(serde_json::json!({
        "id": just_added,
        "token": token,
        "label": label,
        "created_at_unix": now_unix,
        "expires_at_unix": expires_at_unix,
        // UI 的 EnrollTokenCreated 类型 extends EnrollTokenMeta，字段要对齐。
        // 运行时生成的令牌永远是 ephemeral（长期那种来自环境变量，不在这条路径）。
        "permanent": false,
        "monitor_url": monitor_url,
        "enroll_command": enroll_command,
    }))
    .into_response()
}

/// `DELETE /v1/enroll-tokens/:id` —— 撤销一个入网令牌。立即生效。
async fn delete_enroll_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    if state.bootstrap_tokens.revoke_by_id(&id) {
        Json(serde_json::json!({ "ok": true, "id": id })).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "id 不存在或已过期")
    }
}

/// `GET /install-node.sh` —— 原样吐出节点入网脚本（`text/plain`）。
///
/// 不鉴权：目标机器执行 `curl ... | bash` 时还没有凭据；秘密在 enroll 命令的
/// 环境变量里。脚本在**编译期**从仓库根的 `scripts/install-node.sh` 内嵌
/// （见 `main.rs` 的 `INSTALL_NODE_SH`），启动时挂到 state 上。
async fn install_node_script_handler(State(state): State<AppState>) -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        state.install_script.clone(),
    )
        .into_response()
}

#[cfg(test)]
mod ai_token_auth_tests {
    use super::*;

    #[test]
    fn sha256_hex_is_lowercase_and_stable() {
        // 已知向量：空串的 SHA-256 是 e3b0c44298fc1c149afbf4c8996fb924...
        let h = sha256_hex(b"");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // 同样输入两次结果一致
        assert_eq!(h, sha256_hex(b""));
    }

    #[test]
    fn sha256_hex_distinguishes_inputs() {
        assert_ne!(sha256_hex(b"ait_a"), sha256_hex(b"ait_b"));
    }

    #[test]
    fn sha256_hex_accepts_binary_bytes() {
        // 0x00 与 UTF-8 NUL 字符串同字节序列，哈希应一致
        let bytes = [0u8, 159, 146, 150];
        assert_eq!(sha256_hex(&bytes), sha256_hex(&bytes[..]));
    }
}

#[cfg(test)]
mod node_meta_tests {
    use super::*;

    #[test]
    fn alias_limited_by_chars_not_bytes() {
        // 10 个汉字（30 字节）合法，11 个不合法
        assert!(normalize_alias("一二三四五六七八九十").is_ok());
        assert!(normalize_alias("一二三四五六七八九十一").is_err());
        // 首尾空白先裁掉再算长度
        assert_eq!(normalize_alias("  bj-1  ").unwrap(), "bj-1");
    }

    #[test]
    fn tags_trim_dedupe_and_limit() {
        let got = normalize_tags(&[
            " prod ".to_string(),
            "prod".to_string(),
            "".to_string(),
            "bj".to_string(),
        ])
        .unwrap();
        assert_eq!(got, vec!["prod", "bj"]);
        // 单个标签按字符计
        assert!(normalize_tags(&["字".repeat(24)]).is_ok());
        assert!(normalize_tags(&["字".repeat(25)]).is_err());
        // 个数上限
        let many: Vec<String> = (0..11).map(|i| format!("t{i}")).collect();
        assert!(normalize_tags(&many).is_err());
    }
}

#[cfg(test)]
mod enroll_token_tests {
    use super::*;

    #[tokio::test]
    async fn add_with_label_records_metadata() {
        let tokens = BootstrapTokens::default();
        tokens
            .add_with_label("zhi-bt-labeled".into(), 3600, "prod-web".into())
            .await;
        let metas = tokens.list_active();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].label, "prod-web");
        assert!(!metas[0].permanent);
        // id 形如 boot-<6 hex>
        assert!(metas[0].id.starts_with("boot-"), "got id={}", metas[0].id);
    }

    #[tokio::test]
    async fn expired_tokens_drop_out_of_list_active() {
        let tokens = BootstrapTokens::default();
        tokens
            .add_with_label("zhi-bt-expired".into(), 0, String::new())
            .await;
        // TTL=0 → expires_at == now，过滤条件是 `> now`，所以立刻不可见
        assert!(tokens.list_active().is_empty());
        assert!(!tokens.check("zhi-bt-expired"));
    }

    #[tokio::test]
    async fn revoke_by_id_removes_the_token() {
        let tokens = BootstrapTokens::default();
        tokens
            .add_with_label("zhi-bt-a".into(), 3600, "a".into())
            .await;
        tokens
            .add_with_label("zhi-bt-b".into(), 3600, "b".into())
            .await;
        let target_id = tokens
            .list_active()
            .into_iter()
            .find(|m| m.label == "a")
            .expect("label a present")
            .id;

        assert!(tokens.revoke_by_id(&target_id));
        // a 已被撤销，b 还在
        assert!(!tokens.check("zhi-bt-a"));
        assert!(tokens.check("zhi-bt-b"));

        // 再撤一次同一个 id：找不到，返回 false
        assert!(!tokens.revoke_by_id(&target_id));
    }

    #[tokio::test]
    async fn revoke_unknown_id_is_a_noop() {
        let tokens = BootstrapTokens::default();
        tokens.add("zhi-bt-x".into(), 3600).await;
        assert!(!tokens.revoke_by_id("boot-does-not-exist"));
        assert!(tokens.check("zhi-bt-x"));
    }

    #[tokio::test]
    async fn static_token_is_marked_permanent() {
        let tokens = BootstrapTokens::default();
        tokens.add_static("zhi-bt-static".into());
        let metas = tokens.list_active();
        assert_eq!(metas.len(), 1);
        assert!(metas[0].permanent);
        assert_eq!(metas[0].expires_at_unix, NEVER_EXPIRES);
    }

    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn enroll_scheme_prefers_forwarded_proto() {
        // 边缘说的算：内网 Host + 明文监听，对外仍是 https
        let h = headers_with(&[("host", "127.0.0.1:18445"), ("x-forwarded-proto", "https")]);
        assert_eq!(enroll_url_scheme(&h, false), "https");
        // 代理链可能给列表，取第一段
        let h = headers_with(&[
            ("host", "example.com"),
            ("x-forwarded-proto", "http, https"),
        ]);
        assert_eq!(enroll_url_scheme(&h, true), "http");
        // 空值 / 乱值回退到下面的推断
        let h = headers_with(&[("host", "10.0.0.5:8443"), ("x-forwarded-proto", "")]);
        assert_eq!(enroll_url_scheme(&h, false), "http");
    }

    #[test]
    fn enroll_scheme_uses_http_for_lan_plain_http() {
        // 自建「内网 IP + 明文」：命令必须是 http，否则节点照抄命令连不上
        for host in [
            "127.0.0.1:18445",
            "10.0.0.5:8443",
            "192.168.1.9",
            "172.16.3.4:8443",
            "[::1]:8443",
            "localhost:8443",
            "zhiwei.local:8443",
        ] {
            let h = headers_with(&[("host", host)]);
            assert_eq!(enroll_url_scheme(&h, false), "http", "host={host}");
        }
    }

    #[test]
    fn enroll_scheme_stays_https_for_public_and_local_tls() {
        for host in [
            "example.com",
            "monitor.hancic.site",
            "49.232.168.161:8443",
            "170.106.103.36",
        ] {
            let h = headers_with(&[("host", host)]);
            assert_eq!(enroll_url_scheme(&h, false), "https", "host={host}");
            // 本进程自己终结 TLS 时更明确：一律 https
            assert_eq!(enroll_url_scheme(&h, true), "https", "host={host}");
        }
        // 没有 Host 头也不该崩
        assert_eq!(enroll_url_scheme(&HeaderMap::new(), false), "https");
    }

    #[test]
    fn connection_refused_detected_even_without_the_right_kind() {
        // 正常路径：errno 111 → kind 也是 ConnectionRefused
        let e = std::io::Error::from_raw_os_error(111);
        assert_eq!(e.kind(), std::io::ErrorKind::ConnectionRefused);
        assert!(is_connection_refused(&e));

        // 线上真实形态：kind 掉成 Other，只剩文案能认
        let e = std::io::Error::other("Connection refused (os error 111)");
        assert!(is_connection_refused(&e));
        let e = std::io::Error::new(std::io::ErrorKind::NotConnected, "Connection refused");
        assert!(is_connection_refused(&e));

        // 其它网络错误不要误判成「没起 ops-server」
        for msg in ["connection reset by peer", "timed out", "dns error"] {
            assert!(!is_connection_refused(&std::io::Error::other(msg)));
        }
    }

    #[test]
    fn ops_connect_hint_always_says_what_to_do() {
        let refused = std::io::Error::other("Connection refused (os error 111)");
        let hint = ops_connect_error_hint(&refused, "http://127.0.0.1:8444");
        assert!(hint.contains("该地址上没有进程在监听"), "{hint}");
        assert!(hint.contains("entrypoint"), "{hint}");

        let other = std::io::Error::other("connection reset by peer");
        let hint = ops_connect_error_hint(&other, "http://127.0.0.1:8444");
        assert!(hint.contains("zhiwei-ops"), "{hint}");
        assert!(hint.contains("entrypoint"), "{hint}");
    }
}
