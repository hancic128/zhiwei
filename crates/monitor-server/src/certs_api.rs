//! HTTP API for certificate path (source) management.
//!
//! Two types of callers:
//!   - Nodes: `GET /v1/cert-config?node_id=` (pull paths for this machine to
//!     scan), authenticated via Ed25519 request signature, can only pull their
//!     own;
//!   - Console: `/v1/cert-sources` (CRUD + test), authenticated via admin token.
//!
//! On config change the node is sent a one-shot `refresh_inventory` so the new
//! paths take effect immediately rather than waiting for the 5-minute snapshot
//! cycle.

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

use crate::routes::{err, read_auth_ok, sign_command, verify_node, OpsSignError};
use crate::state::AppState;
use zhiwei_storage::cert_sources_repo::{CertSource, CertSourcePatch};

const DEFAULT_NOTIFY_DAYS: i64 = 30;
const MAX_NOTIFY_DAYS: i64 = 365;

// ---------- node-side ----------

/// `GET /v1/cert-config?node_id=<id>` — node fetches the certificate paths it should scan
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
        return err(StatusCode::FORBIDDEN, "Can only fetch this node's certificate config");
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

// ---------- console-side ----------

#[derive(Serialize)]
struct CertSourceView {
    id: String,
    node_id: String,
    /// node_id is empty string when true: this source applies to all nodes
    all_nodes: bool,
    node_hostname: Option<String>,
    path: String,
    enabled: bool,
    notify_enabled: bool,
    notify_days_before: i64,
    created_at_unix_nano: i64,
    updated_at_unix_nano: i64,
    /// Number of matched certificates (summed across nodes this source covers;
    /// nodes offline or without a snapshot don't count)
    matched: i64,
    /// Most recent snapshot time among relevant nodes
    snapshot_at_unix_nano: Option<i64>,
    /// Most urgent remaining days among matched certificates
    nearest_days_left: Option<f64>,
}

/// Whether a source "owns" a given certificate.
///
/// Newer node versions include `source_id` on each certificate entry; older
/// versions don't have this field, so it falls back to reverse-matching by
/// path rule — meaning neither side loses matches during mixed-version runs.
fn cert_belongs_to(source: &CertSource, cert: &serde_json::Value) -> bool {
    let sid = cert.get("source_id").and_then(|v| v.as_str()).unwrap_or("");
    if !sid.is_empty() {
        return sid == source.id;
    }
    cert.get("path")
        .and_then(|v| v.as_str())
        .is_some_and(|p| zhiwei_common::certpath::matches(&source.path, p))
}

/// Compute a source's match count and nearest expiry from snapshots.
/// "All nodes" sources aggregate snapshots from multiple machines — match
/// counts are summed, nearest expiry is the most urgent.
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

/// `GET /v1/cert-sources`: source list (with hit count / nearest expiry / snapshot time)
pub async fn list_cert_sources_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
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

    // At most one snapshot query per node ("all nodes" sources pull
    // snapshots from N machines)
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

/// `POST /v1/cert-sources`: add a source
pub async fn create_cert_source_handler(
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
    let b: CreateCertSourceBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    // Empty node_id = applies to all nodes (common case: every machine has
    // the same nginx cert directory)
    let node_id = b.node_id.trim().to_string();
    if !node_id.is_empty() {
        let node = zhiwei_common::NodeId::from_string(node_id.clone());
        match state.storage.nodes().find_by_id(&node).await {
            Ok(Some(_)) => {}
            Ok(None) => return err(StatusCode::NOT_FOUND, "Node not enrolled"),
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
            // Same node + same path is unique (give a human message on
            // duplicate add).
            let msg = e.to_string();
            return if msg.contains("UNIQUE") {
                err(StatusCode::CONFLICT, "A certificate source with this path already exists on this node")
            } else {
                err(StatusCode::INTERNAL_SERVER_ERROR, format!("create: {msg}"))
            };
        }
    };

    // Make relevant nodes rescan immediately: single node sends to one
    // device, "all nodes" sends per device (acceptable within scale limits)
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
    /// Empty string = change to "all nodes"
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

/// `PATCH /v1/cert-sources/:id`: change path / enable / notify toggle / expiry-days-ahead
pub async fn patch_cert_source_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
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
        Ok(None) => return err(StatusCode::NOT_FOUND, "Certificate source not found"),
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
                Ok(None) => return err(StatusCode::NOT_FOUND, "Node not enrolled"),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
            }
        }
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    if let Err(e) = state.storage.cert_sources().update(&id, &patch, now).await {
        let msg = e.to_string();
        return if msg.contains("UNIQUE") {
            err(StatusCode::CONFLICT, "A certificate source with this path already exists on this node")
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, format!("update: {msg}"))
        };
    }

    // After disabling notifications / disabling / changing path, old alerts
    // must be cleared immediately, not wait for the next snapshot.
    if b.notify_enabled == Some(false) || b.enabled == Some(false) || b.path.is_some() {
        let _ = state
            .storage
            .alerts()
            .resolve_open_cert_alerts(&id, None, now)
            .await;
    }
    // When the scope changes, both old and new nodes need to rescan.
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
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let existing = match state.storage.cert_sources().find(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "Certificate source not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    };
    match state.storage.cert_sources().delete(&id).await {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::NOT_FOUND, "Certificate source not found"),
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

/// `POST /v1/cert-sources/test`: have the node actually scan this path,
/// returns command_id.
///
/// Why not scan on the monitor side: the path lives on the **node's**
/// filesystem, which the monitor can't see. Use the existing command channel
/// (ops signs → node verifies → receipt), and the console polls the receipt.
pub async fn test_cert_source_handler(
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
    let b: TestCertSourceBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let node_id = b.node_id.trim().to_string();
    if node_id.is_empty() {
return err(StatusCode::BAD_REQUEST, "Must select a node");
    }
    let node = zhiwei_common::NodeId::from_string(node_id.clone());
    match state.storage.nodes().find_by_id(&node).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "Node not enrolled"),
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
    match sign_command(&state, &payload).await {
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
            format!("ops-server unavailable: {detail}"),
        ),
    }
}

fn clamp_days(v: Option<i64>) -> i64 {
    v.unwrap_or(DEFAULT_NOTIFY_DAYS).clamp(1, MAX_NOTIFY_DAYS)
}

/// Make a node immediately re-collect a snapshot (best effort: failure only
/// logs, doesn't block the config that was already persisted).
async fn refresh_node_inventory(state: &AppState, node_id: &str) {
    let payload = serde_json::json!({
        "node_id": node_id,
        "action": "refresh_inventory",
        "params": {},
        "actor": "console",
    });
    if let Err(e) = sign_command(state, &payload).await {
        warn!(%node_id, error = %e.message(), "Triggering snapshot refresh failed (config saved, next cycle will retry)");
    }
}

/// "All nodes" sources must notify per device (empty node_id). Node count
/// is well within design range (about 10 in the central cluster), so
/// per-device commands are fine; at hundreds of devices, switch to the node
/// pulling config by timestamp comparison.
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

    /// "All nodes" source: matches aggregate across machines, nearest expiry
    /// is the most urgent one
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

    /// Old node versions without source_id: reverse-match by path rule
    #[test]
    fn falls_back_to_path_matching_without_source_id() {
        let now = 1_800_000_000_000_000_000i64;
        let s = source("node-a", "/root/nginx-certs");
        let snapshots = vec![
            cert_json("/root/nginx-certs/old.crt", "", 20, now),
            cert_json("/etc/ssl/other.crt", "", 5, now),
        ];
        let (matched, nearest) = source_stats(&s, &snapshots, now);
        assert_eq!(matched, 1, "only the one inside the directory counts as a match");
        assert!((nearest.unwrap() - 20.0).abs() < 0.01);
    }

    /// Directory expansion is non-recursive; certs in subdirectories don't
    /// belong to this source.
    #[test]
    fn directory_source_is_not_recursive() {
        let now = 1_800_000_000_000_000_000i64;
        let s = source("node-a", "/root/nginx-certs");
        let snapshots = vec![cert_json("/root/nginx-certs/sub/deep.crt", "", 20, now)];
        assert_eq!(source_stats(&s, &snapshots, now).0, 0);
    }
}
