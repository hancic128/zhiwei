//! 证书路径（来源）管理的 HTTP 接口。
//!
//! 两类调用方：
//!   - 节点：`GET /v1/cert-config?node_id=`（拉本机要扫的路径），走 Ed25519
//!     请求签名，只能拉自己；
//!   - 控制台：`/v1/cert-sources`（增删改查 + 测试），走 admin token。
//!
//! 配置改动后会给节点补发一次 `refresh_inventory`，让新路径立刻生效，
//! 不必等 5 分钟的快照周期。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use tracing::warn;

use crate::routes::{err, ops_sign, read_auth_ok, verify_node, OpsSignError};
use crate::state::AppState;
use zhiwei_storage::cert_sources_repo::{CertSource, CertSourcePatch};

const DEFAULT_NOTIFY_DAYS: i64 = 30;
const MAX_NOTIFY_DAYS: i64 = 365;

// ---------- 节点侧 ----------

/// `GET /v1/cert-config?node_id=<id>` —— 节点拉取本机要扫描的证书路径
pub async fn cert_config_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, _pub) = match verify_node(&state, &headers, method.as_str(), pq, &[]).await {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };
    if q.get("node_id").map(String::as_str) != Some(node_id.as_str()) {
        return err(StatusCode::FORBIDDEN, "只能拉取本节点的证书配置");
    }

    let sources = match state
        .storage
        .cert_sources()
        .list_for_node(node_id.as_str(), true)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list cert sources: {e}"),
            )
        }
    };

    let list: Vec<serde_json::Value> = sources
        .iter()
        .map(|s| serde_json::json!({ "id": s.id, "path": s.path }))
        .collect();
    Json(serde_json::json!({ "node_id": node_id.as_str(), "sources": list })).into_response()
}

// ---------- 控制台侧 ----------

#[derive(Serialize)]
struct CertSourceView {
    id: String,
    node_id: String,
    /// node_id 为空串时为 true：这条来源作用于所有节点
    all_nodes: bool,
    node_hostname: Option<String>,
    path: String,
    enabled: bool,
    notify_enabled: bool,
    notify_days_before: i64,
    created_at_unix_nano: i64,
    updated_at_unix_nano: i64,
    /// 命中证书数（该来源作用的节点上加总；节点离线或未采集时不计入）
    matched: i64,
    /// 相关节点里最近一次快照时间
    snapshot_at_unix_nano: Option<i64>,
    /// 命中证书里最紧急的剩余天数
    nearest_days_left: Option<f64>,
}

/// 某条来源是否"拥有"这份证书。
///
/// 新版本节点会在证书条目里带上 `source_id`；老版本节点没有这个字段，
/// 就退化成用路径规则反查——所以两种节点混跑时命中数都不会丢。
fn cert_belongs_to(source: &CertSource, cert: &serde_json::Value) -> bool {
    let sid = cert.get("source_id").and_then(|v| v.as_str()).unwrap_or("");
    if !sid.is_empty() {
        return sid == source.id;
    }
    cert.get("path")
        .and_then(|v| v.as_str())
        .is_some_and(|p| zhiwei_common::certpath::matches(&source.path, p))
}

/// 用快照算某条来源的命中数与最近到期。
/// 「所有节点」来源会把多台机器的快照一起算——命中数是加总，最近到期取最紧急的。
fn source_stats(source: &CertSource, snapshots: &[String], now_ns: i64) -> (i64, Option<f64>) {
    let mut matched = 0i64;
    let mut nearest: Option<f64> = None;
    for certs_json in snapshots {
        let certs: Vec<serde_json::Value> = serde_json::from_str(certs_json).unwrap_or_default();
        for c in &certs {
            if !cert_belongs_to(source, c) {
                continue;
            }
            matched += 1;
            if c.get("parse_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            if let Some(exp) = c.get("not_after_unix_nano").and_then(|v| v.as_i64()) {
                let days = (exp - now_ns) as f64 / 86_400_000_000_000.0;
                nearest = Some(nearest.map_or(days, |cur: f64| cur.min(days)));
            }
        }
    }
    (matched, nearest)
}

/// `GET /v1/cert-sources`：来源列表（含命中数 / 最近到期 / 快照时间）
pub async fn list_cert_sources_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers) {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let sources = match state.storage.cert_sources().list_all().await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("list: {e}")),
    };
    let nodes = state.storage.nodes().list_all().await.unwrap_or_default();
    let now_ns = zhiwei_common::Timestamp::now().unix_nano();

    // 每个节点最多查一次快照（「所有节点」来源会把 N 台机器的快照都拉进来）
    let mut snapshots: HashMap<String, (Option<i64>, String)> = HashMap::new();
    let mut out = Vec::with_capacity(sources.len());
    for s in sources {
        let scope: Vec<(String, String)> = nodes
            .iter()
            .filter(|n| s.is_all_nodes() || n.id == s.node_id)
            .map(|n| (n.id.clone(), n.hostname.clone()))
            .collect();

        let mut certs_json: Vec<String> = Vec::with_capacity(scope.len());
        let mut snapshot_at: Option<i64> = None;
        for (id, _) in &scope {
            if !snapshots.contains_key(id) {
                let node = zhiwei_common::NodeId::from_string(id.clone());
                let row = state.storage.inventory().find(&node).await.ok().flatten();
                snapshots.insert(
                    id.clone(),
                    match row {
                        Some(r) => (Some(r.ts_unix_nano), r.certificates_json),
                        None => (None, "[]".to_string()),
                    },
                );
            }
            if let Some((ts, json)) = snapshots.get(id) {
                snapshot_at = match (snapshot_at, *ts) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                certs_json.push(json.clone());
            }
        }

        let (matched, nearest_days_left) = source_stats(&s, &certs_json, now_ns);
        let node_hostname = if s.is_all_nodes() {
            None
        } else {
            scope.first().map(|(_, hostname)| hostname.clone())
        };
        out.push(CertSourceView {
            id: s.id.clone(),
            node_id: s.node_id.clone(),
            all_nodes: s.is_all_nodes(),
            node_hostname,
            path: s.path,
            enabled: s.enabled,
            notify_enabled: s.notify_enabled,
            notify_days_before: s.notify_days_before,
            created_at_unix_nano: s.created_at_unix_nano,
            updated_at_unix_nano: s.updated_at_unix_nano,
            matched,
            snapshot_at_unix_nano: snapshot_at,
            nearest_days_left,
        });
    }
    Json(out).into_response()
}

#[derive(serde::Deserialize)]
pub struct CreateCertSourceBody {
    node_id: String,
    path: String,
    #[serde(default)]
    notify_enabled: Option<bool>,
    #[serde(default)]
    notify_days_before: Option<i64>,
}

/// `POST /v1/cert-sources`：新增来源
pub async fn create_cert_source_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers) {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: CreateCertSourceBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    // 空 node_id = 作用于所有节点（常见场景：每台机器都有同一个 nginx 证书目录）
    let node_id = b.node_id.trim().to_string();
    if !node_id.is_empty() {
        let node = zhiwei_common::NodeId::from_string(node_id.clone());
        match state.storage.nodes().find_by_id(&node).await {
            Ok(Some(_)) => {}
            Ok(None) => return err(StatusCode::NOT_FOUND, "节点未入网"),
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
        }
    }
    let path = match zhiwei_common::certpath::normalize(&b.path) {
        Ok(p) => p,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    let notify_enabled = b.notify_enabled.unwrap_or(true);
    let notify_days_before = clamp_days(b.notify_days_before);
    let now = zhiwei_common::Timestamp::now().unix_nano();

    let created = match state
        .storage
        .cert_sources()
        .create(&node_id, &path, notify_enabled, notify_days_before, now)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            // 同一节点同一路径唯一（用户重复添加时给一句人话）
            let msg = e.to_string();
            return if msg.contains("UNIQUE") {
                err(StatusCode::CONFLICT, "该节点下已经有相同的证书路径")
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, format!("create: {msg}"))
            };
        }
    };

    // 让相关节点立刻重扫：单节点只发一台，「所有节点」逐台发（规模上限内可接受）
    refresh_scope_inventory(&state, &node_id).await;
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": created.id,
            "node_id": created.node_id,
            "path": created.path,
        })),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct PatchCertSourceBody {
    /// 空串 = 改成「所有节点」
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    notify_enabled: Option<bool>,
    #[serde(default)]
    notify_days_before: Option<i64>,
}

/// `PATCH /v1/cert-sources/:id`：改路径 / 启用 / 通知开关 / 到期前天数
pub async fn patch_cert_source_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers) {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: PatchCertSourceBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let existing = match state.storage.cert_sources().find(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "证书来源不存在"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    };

    let path = match b.path.as_deref() {
        Some(p) => match zhiwei_common::certpath::normalize(p) {
            Ok(p) => Some(p),
            Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
        },
        None => None,
    };
    let patch = CertSourcePatch {
        node_id: b.node_id.as_ref().map(|n| n.trim().to_string()),
        path,
        enabled: b.enabled,
        notify_enabled: b.notify_enabled,
        notify_days_before: b.notify_days_before.map(|d| clamp_days(Some(d))),
    };
    if let Some(node_id) = &patch.node_id {
        if !node_id.is_empty() {
            let node = zhiwei_common::NodeId::from_string(node_id.clone());
            match state.storage.nodes().find_by_id(&node).await {
                Ok(Some(_)) => {}
                Ok(None) => return err(StatusCode::NOT_FOUND, "节点未入网"),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
            }
        }
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    if let Err(e) = state.storage.cert_sources().update(&id, &patch, now).await {
        let msg = e.to_string();
        return if msg.contains("UNIQUE") {
            err(StatusCode::CONFLICT, "该节点下已经有相同的证书路径")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, format!("update: {msg}"))
        };
    }

    // 关闭通知 / 停用 / 换路径后，旧告警要立刻收掉，不能等下一次快照
    if b.notify_enabled == Some(false) || b.enabled == Some(false) || b.path.is_some() {
        let _ = state
            .storage
            .alerts()
            .resolve_open_cert_alerts(&id, None, now)
            .await;
    }
    // 换过作用范围时，新旧两边的节点都要重扫
    let moved = patch
        .node_id
        .as_ref()
        .is_some_and(|n| *n != existing.node_id);
    let target = patch.node_id.clone().unwrap_or(existing.node_id.clone());
    refresh_scope_inventory(&state, &target).await;
    if moved {
        refresh_scope_inventory(&state, &existing.node_id).await;
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

/// `DELETE /v1/cert-sources/:id`
pub async fn delete_cert_source_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers) {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let existing = match state.storage.cert_sources().find(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "证书来源不存在"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    };
    match state.storage.cert_sources().delete(&id).await {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::NOT_FOUND, "证书来源不存在"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("delete: {e}")),
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    let _ = state
        .storage
        .alerts()
        .resolve_open_cert_alerts(&id, None, now)
        .await;
    refresh_scope_inventory(&state, &existing.node_id).await;
    (StatusCode::NO_CONTENT).into_response()
}

#[derive(serde::Deserialize)]
pub struct TestCertSourceBody {
    node_id: String,
    path: String,
}

/// `POST /v1/cert-sources/test`：让节点真扫一遍这条路径，返回 command_id。
///
/// 为什么不在 monitor 侧直接扫：路径存在于**节点**的文件系统上，monitor 看不到。
/// 走既有命令通道（ops 签名 → 节点验签 → 回执），控制台轮询回执即可。
pub async fn test_cert_source_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers) {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: TestCertSourceBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let node_id = b.node_id.trim().to_string();
    if node_id.is_empty() {
        return err(StatusCode::BAD_REQUEST, "需要选择节点");
    }
    let node = zhiwei_common::NodeId::from_string(node_id.clone());
    match state.storage.nodes().find_by_id(&node).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "节点未入网"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    }
    let path = match zhiwei_common::certpath::normalize(&b.path) {
        Ok(p) => p,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };

    let payload = serde_json::json!({
        "node_id": node_id,
        "action": "scan_certs",
        "params": { "path": path },
        "actor": "console",
    });
    match ops_sign(&state.ops_endpoint, &payload).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "command_id": id })),
        )
            .into_response(),
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

fn clamp_days(v: Option<i64>) -> i64 {
    v.unwrap_or(DEFAULT_NOTIFY_DAYS).clamp(1, MAX_NOTIFY_DAYS)
}

/// 让节点立刻重采一次快照（best effort：失败只记日志，不影响配置已落库）
async fn refresh_node_inventory(state: &AppState, node_id: &str) {
    let payload = serde_json::json!({
        "node_id": node_id,
        "action": "refresh_inventory",
        "params": {},
        "actor": "console",
    });
    if let Err(e) = ops_sign(&state.ops_endpoint, &payload).await {
        warn!(%node_id, error = %e.message(), "触发快照重采失败（配置已保存，等下一轮周期采集）");
    }
}

/// 「所有节点」来源要逐台通知（空 node_id）。节点数量级在定位之内（设计中心 10 台），
/// 逐台发命令完全可以接受；真到几百台时应该改成节点侧拉配置的时间戳比对。
async fn refresh_scope_inventory(state: &AppState, node_id: &str) {
    if !node_id.is_empty() {
        refresh_node_inventory(state, node_id).await;
        return;
    }
    let nodes = state.storage.nodes().list_all().await.unwrap_or_default();
    for n in nodes {
        refresh_node_inventory(state, &n.id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(node_id: &str, path: &str) -> CertSource {
        CertSource {
            id: "src-1".to_string(),
            node_id: node_id.to_string(),
            path: path.to_string(),
            enabled: true,
            notify_enabled: true,
            notify_days_before: 30,
            created_at_unix_nano: 0,
            updated_at_unix_nano: 0,
        }
    }

    fn cert_json(path: &str, source_id: &str, days_left: i64, now_ns: i64) -> String {
        serde_json::json!([{
            "path": path,
            "subject": "CN=test",
            "domains": ["test.local"],
            "not_after_unix_nano": now_ns + days_left * 86_400_000_000_000,
            "parse_error": false,
            "source_id": source_id,
        }])
        .to_string()
    }

    /// 「所有节点」来源：命中数跨机器加总，最近到期取最紧急的那台
    #[test]
    fn all_nodes_source_aggregates_snapshots() {
        let now = 1_800_000_000_000_000_000i64;
        let s = source("", "/root/nginx-certs");
        let snapshots = vec![
            cert_json("/root/nginx-certs/a.crt", "src-1", 10, now),
            cert_json("/root/nginx-certs/b.crt", "src-1", 3, now),
        ];
        let (matched, nearest) = source_stats(&s, &snapshots, now);
        assert_eq!(matched, 2);
        assert!((nearest.unwrap() - 3.0).abs() < 0.01);
    }

    /// 老版本节点不带 source_id：按路径规则反查归属
    #[test]
    fn falls_back_to_path_matching_without_source_id() {
        let now = 1_800_000_000_000_000_000i64;
        let s = source("node-a", "/root/nginx-certs");
        let snapshots = vec![
            cert_json("/root/nginx-certs/old.crt", "", 20, now),
            cert_json("/etc/ssl/other.crt", "", 5, now),
        ];
        let (matched, nearest) = source_stats(&s, &snapshots, now);
        assert_eq!(matched, 1, "只有目录内的那张算命中");
        assert!((nearest.unwrap() - 20.0).abs() < 0.01);
    }

    /// 目录展开是非递归的，子目录里的证书不属于这条来源
    #[test]
    fn directory_source_is_not_recursive() {
        let now = 1_800_000_000_000_000_000i64;
        let s = source("node-a", "/root/nginx-certs");
        let snapshots = vec![cert_json("/root/nginx-certs/sub/deep.crt", "", 20, now)];
        assert_eq!(source_stats(&s, &snapshots, now).0, 0);
    }
}
