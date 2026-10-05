//! HTTP routes for the monitor server.
//!
//! Two endpoint families:
//!   POST /v1/enroll       — bootstrap token + node Ed25519 public key → `node_id`
//!   POST /v1/telemetry    — node Ed25519 request signature, protobuf payload
//!
//! Bootstrap tokens are short-lived (default 10 minutes) and stored in memory.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
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
        .merge(monitor_routes())
        .merge(certs_routes())
        .merge(alerts_routes())
        .merge(probes_routes())
        .merge(admin_routes())
        .merge(commands_routes())
        // Node enrollment script. **Intentionally unauthenticated**: the target machine has no
        // credentials yet; the real secret is the ZHIWEI_BOOTSTRAP_TOKEN passed in the enroll command.
        // The script itself contains no secrets; exposing it is equivalent to exposing the install
        // method (same approach as Tailscale et al.).
        .route("/install-node.sh", get(install_node_script_handler))
        // Fallback: console static assets + SPA deep links (returns 404 when console isn't built)
        .fallback(ui_handler)
        .with_state(state)
}

/// Core node/telemetry endpoints: health, enroll, telemetry, inventory, node views.
fn monitor_routes() -> Router<AppState> {
    Router::new()
        .route("/v1", get(index_handler))
        .route("/v1/enroll", post(enroll_handler))
        .route("/v1/telemetry", post(telemetry_handler))
        .route("/v1/nodes", get(list_nodes_handler))
        .route(
            "/v1/nodes/:id",
            axum::routing::patch(patch_node_handler).delete(delete_node_handler),
        )
        .route("/v1/nodes/:id/telemetry", get(node_telemetry_handler))
        .route("/v1/nodes/:id/series", get(node_series_handler))
        .route("/v1/nodes/:id/containers", get(node_containers_handler))
        .route("/v1/nodes/:id/processes", get(node_processes_handler))
        .route("/v1/inventory", post(inventory_handler))
        .route("/v1/containers", get(all_containers_handler))
        .route("/v1/certificates", get(all_certificates_handler))
        .route("/v1/series/nodes", get(all_nodes_series_handler))
}

/// Certificate-source configuration endpoints.
fn certs_routes() -> Router<AppState> {
    Router::new()
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
}

/// Alerting: alerts, rules, builtin toggles, notification channels, CA.
fn alerts_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/alerts", get(alerts_handler))
        .route("/v1/alerts/:id/silence", post(silence_alert_handler))
        .route("/v1/alerts/:id/resolve", post(resolve_alert_handler))
        .route(
            "/v1/rules",
            get(list_rules_handler).post(create_rule_handler),
        )
        .route(
            "/v1/rules/:id",
            axum::routing::patch(patch_rule_handler).delete(delete_rule_handler),
        )
        .route("/v1/builtin-alerts", get(list_builtin_alerts_handler))
        .route(
            "/v1/builtin-alerts/:id",
            axum::routing::patch(patch_builtin_alert_handler),
        )
        .route(
            "/v1/channels",
            get(list_channels_handler).post(create_channel_handler),
        )
        .route("/v1/channels/test", post(test_channel_handler))
        .route(
            "/v1/channels/:id",
            axum::routing::patch(patch_channel_handler).delete(delete_channel_handler),
        )
        .route("/v1/ca", get(ca_handler))
}

/// Service / probe definitions, timelines and result ingestion.
fn probes_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/services/timeline",
            get(crate::probes_api::services_timeline_handler),
        )
        .route(
            "/v1/probes",
            get(crate::probes_api::list_probes_handler)
                .post(crate::probes_api::create_probe_handler),
        )
        // Static segments must be registered before `:id`: /v1/probes/test must not be treated as id=test
        .route(
            "/v1/probes/test",
            axum::routing::post(crate::probes_api::test_probe_handler),
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
}

/// Console dashboards, token management, MCP transport and health.
fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/todo", get(crate::todo_api::todo_handler))
        .route(
            "/v1/retention",
            get(crate::retention::retention_handler)
                .patch(crate::retention::patch_retention_handler),
        )
        .route("/v1/help", get(help_handler))
        .route("/v1/admin/token", post(change_admin_token_handler))
        .route(
            "/mcp/sse",
            get(crate::mcp::health_handler).post(crate::mcp::sse_handler),
        )
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
        .route("/healthz", get(healthz))
}

/// Command channel: execute, poll results, history.
fn commands_routes() -> Router<AppState> {
    Router::new()
        .route("/v1/commands", get(node_commands_handler))
        .route("/v1/commands/:id/result", post(command_result_handler))
        .route("/v1/exec", post(exec_handler))
        .route("/v1/commands/history", get(command_history_handler))
        .route("/v1/commands/:id", get(command_detail_handler))
}

/// Validate the node request signature (replacing the old mTLS client certificates).
///
/// Checks in order: signature headers present → node enrolled with public key → time window
/// → nonce not replayed → signature. Any step failure returns 401 without revealing which
/// step failed (to avoid giving feedback to attackers).
///
/// Returns `(node id, node public key)` — callers that need to verify a further layer of
/// payload signatures (e.g. command receipts) can use this public key directly without
/// hitting the database again.
pub async fn verify_node(
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
            .map(std::string::ToString::to_string)
    };

    let unauthorized = || {
        (
            StatusCode::UNAUTHORIZED,
            "Node signature verification failed".to_string(),
        )
    };

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
        // Old nodes (enrolled in the mTLS era, without a public key) need to re-enroll
        return Err((
            StatusCode::UNAUTHORIZED,
            "this node has no registered public key, please re-enroll".to_string(),
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

    // Consume the nonce only after the signature is valid: invalid requests shouldn't pollute the cache
    if !state
        .nonce_cache
        .accept(&nonce, zhiwei_common::Timestamp::now().unix_nano())
    {
        return Err((StatusCode::UNAUTHORIZED, "request replayed".to_string()));
    }

    Ok((id, pub_key))
}

/// Read endpoints accept only one credential: `Authorization: Bearer <admin token>`
/// (used by the browser console). Nodes use signature auth and do not call these endpoints.
/// Auth tier v2: returns not just bool but also distinguishes credential type.
///
/// All currently protected endpoints are read endpoints, so an AI token effectively
/// has the same permissions as an admin token (minus the ability to change admin.token
/// itself). When write-class "manage" endpoints are added later, handlers can just add
/// `matches!(kind, Admin | AiToken)` to open them up to AI tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadAuthKind {
    /// Console / browser / anyone with the admin token.
    Admin,
    /// AI token, with the token id (used to audit `last_used_at`).
    AiToken(String),
    /// No valid credentials.
    None,
}

/// Parse the Authorization header and return the credential type.
///
/// Order: admin token (in-memory `ct_eq` compare, fastest) → AI token (SHA-256 then
/// query `SQLite`). An AI token that doesn't match or has been revoked returns None.
pub async fn read_auth_ok_v2(state: &AppState, headers: &HeaderMap) -> ReadAuthKind {
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
    // Take the String out and drop the guard; otherwise RwLockReadGuard is not Send,
    // and the entire handler future stops being Send, which axum rejects.
    let admin_ok = {
        let current = state
            .admin_token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::admin::ct_eq(token.as_bytes(), current.as_bytes())
    };
    if admin_ok {
        return ReadAuthKind::Admin;
    }

    // 2. AI token
    let hash = sha256_hex(token.as_bytes());
    match state.storage.ai_tokens().find_active_by_hash(&hash).await {
        Ok(Some(t)) => {
            // best-effort: update last_used_at. Failures don't affect the main request.
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

/// Legacy interface: kept for compatibility. New code should use `read_auth_ok_v2` directly.
///
/// Equivalent to `matches!(v2(...), Admin | AiToken(_))`, but without unwrapping or
/// dispatching — callers are already using bool, no need to add mental overhead for new code.
pub async fn read_auth_ok(state: &AppState, headers: &HeaderMap) -> bool {
    !matches!(read_auth_ok_v2(state, headers).await, ReadAuthKind::None)
}

/// SHA-256 → lowercase hex (using ring, which is already in dependencies). AI token hashing only.
fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Metadata for an enrollment token. `id` is for UI display + revocation; the `token`
/// string itself is used only on the validation path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BootstrapTokenMeta {
    pub id: String,
    pub label: String,
    pub created_at_unix: u64,
    pub expires_at_unix: u64,
    /// Long-lived (from `ZHIWEI_BOOTSTRAP_TOKEN`); UI can flag it specially.
    pub permanent: bool,
}

#[derive(Default)]
pub struct BootstrapTokens {
    /// token string -> metadata
    inner: Mutex<HashMap<String, BootstrapTokenMeta>>,
}

/// Never-expiring timestamp. Used for `ZHIWEI_BOOTSTRAP_TOKEN` — managed platforms
/// (especially free tiers) have no shell and can't mount persistent volumes, so
/// racing to grab a "valid for 10 minutes from startup logs" one-shot token isn't realistic.
const NEVER_EXPIRES: u64 = u64::MAX;

impl BootstrapTokens {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
    pub fn add(&self, token: String, ttl_secs: u64) {
        self.add_with_label(token, ttl_secs, String::new());
    }
    /// Add a labeled temporary token. An empty `label` is also valid.
    /// `id` is auto-generated (`boot-<6 hex>`), used only for UI display and revocation.
    pub fn add_with_label(&self, token: String, ttl_secs: u64, label: String) {
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
    /// Register a long-lived enrollment token (`ZHIWEI_BOOTSTRAP_TOKEN`).
    /// Removing the environment variable and restarting is equivalent to revocation.
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
    /// List currently non-expired token metadata. **Does not include the plaintext token**.
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
    /// Revoke by id (find the first matching token string and remove it).
    /// Returns whether anything was actually revoked.
    // The lock guard must stay alive until `remove`, so it can't be scoped tighter.
    #[allow(clippy::significant_drop_tightening)]
    pub fn revoke_by_id(&self, id: &str) -> bool {
        let mut guard = self.inner.lock();
        let target = guard
            .iter()
            .find(|(_, m)| m.id == id)
            .map(|(t, _)| t.clone());
        target.is_some_and(|token| {
            guard.remove(&token);
            true
        })
    }
}

fn rand_bytes_3() -> [u8; 3] {
    let mut buf = [0u8; 3];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

pub fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ErrorBody { error: msg.into() })).into_response()
}

/// Console static assets + SPA fallback.
///
/// Deep links (`/nodes`, `/services`, ...) must return **200 + index.html**: earlier
/// when using `ServeDir::not_found_service` the page rendered but the status was 404,
/// causing online probes, crawlers, and proxy caches to misinterpret it as "page does
/// not exist". Here we read the file ourselves and set the correct MIME type.
pub async fn ui_handler(State(state): State<AppState>, uri: axum::http::Uri) -> Response {
    use axum::http::header::CONTENT_TYPE;

    let Some(dir) = state.ui_dir.clone() else {
        return err(StatusCode::NOT_FOUND, "console build not found");
    };
    let rel = uri.path().trim_start_matches('/');

    // Unknown API paths still return JSON 404, to avoid mixing HTML into client responses
    if rel.starts_with("v1/") || rel == "v1" {
        return err(StatusCode::NOT_FOUND, "unknown endpoint");
    }

    // Existing static files take priority; reject directory traversal
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
            format!("failed to read console index.html: {e}"),
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

/// Whether to send the node a local CA in the enroll response.
///
/// Only issued when "this process terminates TLS" — in that case the monitor certificate
/// is signed by this local CA, so pinning it makes sense for the node.
///
/// When deployed behind an edge-terminating TLS platform like Render / Railway (`--plain-http`),
/// the edge uses a proper certificate, unrelated to the local CA; if we still issue it, the node
/// will treat it as its only trust root, resulting in **enroll succeeds, every subsequent request
/// fails TLS validation** — a very time-consuming thing to debug.
/// So we simply don't issue it here, letting the node use the system roots.
pub fn enroll_ca_pem(ca_cert_pem: &str, tls_terminated_locally: bool) -> String {
    if tls_terminated_locally {
        ca_cert_pem.to_string()
    } else {
        String::new()
    }
}

/// Ops signing public key (base64), sent to nodes during enroll for TOFU.
///
/// The monitor caches a copy at startup, but **reads from disk again when the cache is empty**:
/// in same-container multi-process setups, ops-server may start later than monitor
/// (see `scripts/docker-entrypoint.sh`). An early-starting monitor shouldn't permanently
/// withhold the public key from new nodes — those nodes would remain "without ops public
/// key", and the command channel would silently fail.
pub async fn ops_public_key(state: &AppState) -> String {
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
    let Some(token) = auth.and_then(|s| s.strip_prefix("Bearer ")) else {
        return err(StatusCode::UNAUTHORIZED, "missing bearer token");
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
    // Identity = node's own Ed25519 public key (no longer a CA-signed client certificate)
    if req.public_key.len() != 32 {
        return err(
            StatusCode::BAD_REQUEST,
            "public_key must be a 32-byte Ed25519 public key",
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

    // Optional alias / tags from the node side (`zhiwei-node --alias/--tags`, also supported by install-node.sh).
    // Rules are identical to console PATCH: out-of-range values return 400 directly, not silently truncated.
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
        client_cert_pem: String::new(), // Historical column, mTLS is deprecated
        enrolled_at_unix_nano: now,
        last_seen_unix_nano: None,
        // Host info is populated on the first telemetry report
        host_info_json: "{}".to_string(),
        public_key: public_key_b64.clone(),
        // Alias and tags: the node can provide them (optional), then maintained by the admin in the console
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
    let pq = uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
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
    // Identity is already confirmed by the request signature; here we additionally check that the body
    // node_id matches the signature subject, preventing A's signature being used to submit B's data
    let node_id_str = batch.node_id.clone();
    if node_id_str.is_empty() || node_id_str != node_id.as_str() {
        return err(
            StatusCode::BAD_REQUEST,
            "node_id does not match signature subject",
        );
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

    // Run alert rules after writing to the database (no rules = no DB query, overhead is negligible).
    // Alert text uses the **display name** (alias if set), otherwise a hostname
    // like an auto-generated cloud name is unrecognizable.
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
    /// When the monitor process started (= most recent deploy / restart). The console shows
    /// "Deployed at ..." next to the version number in the top-right — same semantic as
    /// "current deploy time" in node details.
    started_at_unix_nano: i64,
}

async fn index_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let nodes = state
        .storage
        .nodes()
        .list_all()
        .await
        .map(|v| i64::try_from(v.len()).unwrap_or(i64::MAX))
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
        started_at_unix_nano: state.started_at_ms.saturating_mul(1_000_000),
    })
    .into_response()
}

#[derive(Serialize)]
struct NodeView {
    id: String,
    hostname: String,
    labels: serde_json::Value,
    /// Short alias assigned by the admin (≤10 characters); empty string means not set
    alias: String,
    /// Tags assigned by the admin (≤10)
    tags: Vec<String>,
    enrolled_at_unix_nano: i64,
    last_seen_unix_nano: Option<i64>,
    /// Host basic info (OS / kernel / CPU / IP etc.); empty object when no report has been received
    host_info: serde_json::Value,
    /// Node Ed25519 public key (base64); for debugging/auditing, not shown directly in UI
    public_key: String,
    /// Key metrics from the latest frame (list page's CPU / memory / disk columns read from here,
    /// avoiding one series request per node)
    latest: Option<NodeLatestView>,
    /// Command channel status — when a node is online but doesn't pull commands, the console's
    /// start/stop / log buttons are all dead; the UI needs to surface this fact.
    /// Rules: [`crate::control_channel`].
    command_channel: NodeChannelView,
}

#[derive(Serialize)]
struct NodeChannelView {
    /// `ok` | `down` | `unknown` (unknown = just restarted, insufficient observation / node not online)
    state: &'static str,
    /// Milliseconds since the most recent command poll; None if this process has never seen it poll
    last_poll_age_ms: Option<i64>,
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
    /// Sum of all NICs' cumulative rx / tx bytes (list page only needs the timestamp; rates are computed on detail page)
    net_rx_bytes: Option<f64>,
    net_tx_bytes: Option<f64>,
}

/// Pull the metrics the list page needs from one telemetry frame
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
        net_rx_bytes: sum_u64_to_f64(batch.network.iter().map(|n| n.rx_bytes)),
        net_tx_bytes: sum_u64_to_f64(batch.network.iter().map(|n| n.tx_bytes)),
    }
}

/// Sum u64 counters into f64 for JSON serialization. f64 cannot represent integers
/// above 2^53 exactly (≈9 PB); callers should treat values above that as approximate.
#[allow(clippy::cast_precision_loss)] // intentional: display-only, precision above 2^53 irrelevant
fn sum_u64_to_f64(it: impl Iterator<Item = u64>) -> Option<f64> {
    let mut total: u64 = 0;
    let mut any = false;
    for v in it {
        total = total.saturating_add(v);
        any = true;
    }
    if any {
        // Intentional cast: result is for display, byte precision above 2^53 is unnecessary.
        Some(total as f64)
    } else {
        None
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

    // Fetch the latest frame for each node in one go; list page no longer issues per-node series requests
    let mut latest_by_node: HashMap<String, NodeLatestView> = HashMap::new();
    if let Ok(rows) = state.storage.telemetry().latest_per_node().await {
        for (node_id, ts, payload) in rows {
            if let Ok(batch) = TelemetryBatch::decode(&payload[..]) {
                latest_by_node.insert(node_id, latest_view(ts, &batch));
            }
        }
    }

    let now_ms = zhiwei_common::Timestamp::now().unix_nano() / 1_000_000;
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        out.push(NodeView {
            labels: serde_json::from_str(&n.labels_json).unwrap_or_else(|_| serde_json::json!({})),
            alias: n.alias.clone(),
            tags: parse_tags(&n.tags_json),
            latest: latest_by_node.remove(&n.id),
            command_channel: channel_view(
                &state,
                &n.id,
                n.last_seen_unix_nano.map(|ns| ns / 1_000_000),
                now_ms,
            ),
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

/// Build a "command channel" view for a given node. The decision logic is in
/// [`crate::control_channel::channel_state`]; this function just supplies the inputs.
fn channel_view(
    state: &AppState,
    node_id: &str,
    last_seen_ms: Option<i64>,
    now_ms: i64,
) -> NodeChannelView {
    let last_poll_ms = state.control_polls.last(node_id);
    let st = crate::control_channel::channel_state(
        last_seen_ms,
        last_poll_ms,
        now_ms - state.started_at_ms,
        now_ms,
    );
    NodeChannelView {
        state: st.as_str(),
        last_poll_age_ms: last_poll_ms.map(|t| now_ms - t),
    }
}

/// `tags_json` stores a string array; historical rows may be `{}` or corrupt;
/// degrade uniformly to empty array, don't let one dirty row turn the whole list into 500.
fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Alias upper limit (by character count, not bytes): a Chinese alias should also count as "a few characters".
const MAX_ALIAS_CHARS: usize = 10;
/// Upper limit on number of tags
const MAX_TAGS: usize = 10;
/// Upper limit on a single tag length
const MAX_TAG_CHARS: usize = 24;

/// Normalize alias: trim whitespace and validate by character count.
fn normalize_alias(raw: &str) -> Result<String, String> {
    let alias = raw.trim().to_string();
    if alias.chars().count() > MAX_ALIAS_CHARS {
        return Err(format!("alias can be at most {MAX_ALIAS_CHARS} characters"));
    }
    Ok(alias)
}

/// Normalize tags: trim whitespace / drop empty strings / deduplicate (preserving order),
/// then validate count and length.
///
/// Shared between console metadata edits (`PATCH /v1/nodes/:id`) and node self-reported
/// tags on enrollment (`EnrollRequest`) — the rules must be identical, otherwise the
/// node side could sneak in values the console rejects.
fn normalize_tags(raw: &[String]) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut cleaned = Vec::new();
    for tag in raw {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > MAX_TAG_CHARS {
            return Err(format!(
                "a single tag can be at most {MAX_TAG_CHARS} characters"
            ));
        }
        if seen.insert(tag.to_string()) {
            cleaned.push(tag.to_string());
        }
    }
    if cleaned.len() > MAX_TAGS {
        return Err(format!("at most {MAX_TAGS} tags are allowed"));
    }
    Ok(cleaned)
}

#[derive(Deserialize)]
struct NodeMetaPatch {
    /// Absent = no change (distinguished from passing an empty string = clear alias)
    #[serde(default)]
    alias: Option<String>,
    /// Absent = no change (distinguished from passing an empty array = clear tags)
    #[serde(default)]
    tags: Option<Vec<String>>,
}

/// `PATCH /v1/nodes/:id`: change the admin-maintained alias and tags.
///
/// Only accepts alias / tags; node identity (id, public key, hostname, reported data)
/// cannot be changed from the console. Validation failures return 400 with the reason
/// passed through to the console.
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
        return err(
            StatusCode::BAD_REQUEST,
            "alias and tags: at least one must be provided",
        );
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

/// `DELETE /v1/nodes/:id`: permanently delete a node.
///
/// Scope: the node row + all data referencing it (telemetry / inventory / probe results /
/// certificate sources / command history / alerts / alert state machine) are wiped.
/// `audit_log` is retained — it's for post-hoc accountability and shouldn't be
/// laundered just because the node is gone.
///
/// Precondition: no "still-valid" pending commands — these are in transit and would
/// leave dangling records if the node is deleted (no one will ever receive the receipt).
/// Expired ones (TTL passed) don't count: the node would refuse them upon receipt,
/// and using them to block deletion would make the node permanently undeletable
/// (a re-installed node gets a new `node_id` and never comes back to poll).
///
/// `?force=1`: voids all unsent commands for this node (writes `audit_log`, outcome=cancelled),
/// then deletes. "Cancel first" needs a button somewhere — when the command channel is broken
/// or the node is reinstalled, "wait for the node to pull" will simply never happen.
///
/// Probe bindings: remove the node id from every `probes.node_ids_json` array. A probe with
/// no remaining node bindings falls back to "any node" (empty array = all nodes) — this
/// is the semantics of the write path, not a newly introduced side effect.
async fn delete_node_handler(
    State(state): State<AppState>,
    axum::extract::Path(node_id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let trimmed = node_id.trim().to_string();
    if trimmed.is_empty() {
        return err(StatusCode::BAD_REQUEST, "node id cannot be empty");
    }
    let id = zhiwei_common::NodeId::from_string(trimmed.clone());
    let exists = match state.storage.nodes().find_by_id(&id).await {
        Ok(Some(_)) => true,
        Ok(None) => return err(StatusCode::NOT_FOUND, "node not enrolled"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    };

    let force = matches!(
        q.get("force").map(String::as_str),
        Some("1" | "true" | "yes")
    );

    let now_ns = zhiwei_common::Timestamp::now().unix_nano();
    let pending = match state
        .storage
        .commands()
        .count_pending_for_node(&trimmed, now_ns)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("count commands: {e}"),
            )
        }
    };
    if pending > 0 {
        if !force {
            return err(
                StatusCode::CONFLICT,
                format!(
                    "this node still has {pending} unsent commands (pending state): wait for the node to pull before deleting,\
                     or void them and delete (DELETE …?force=1)"
                ),
            );
        }
        if let Err(resp) = void_pending_commands(&state, &trimmed, now_ns).await {
            return resp;
        }
    }

    delete_node_row(&state, &trimmed, exists).await
}

/// `?force=1` path: void every unsent command for the node and write one `audit_log`
/// row per command (command history goes with the node on delete, so the trail must
/// be recorded separately).
async fn void_pending_commands(
    state: &AppState,
    node_id: &str,
    now_ns: i64,
) -> Result<(), Response> {
    let cancelled = match state
        .storage
        .commands()
        .cancel_pending_for_node(node_id)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cancel commands: {e}"),
            ))
        }
    };
    for c in &cancelled {
        if let Err(e) = state
            .storage
            .commands()
            .audit_with_outcome(
                now_ns,
                "admin",
                node_id,
                &c.id,
                &c.action,
                &c.params_json,
                "cancelled",
            )
            .await
        {
            warn!(node_id = %node_id, command_id = %c.id, error = %e, "failed to write audit log when voiding command");
        }
    }
    warn!(
        node_id = %node_id,
        cancelled = cancelled.len(),
        "force-deleted node: unsent commands have been voided"
    );
    Ok(())
}

async fn delete_node_row(state: &AppState, node_id: &str, exists: bool) -> Response {
    match state.storage.nodes().delete(node_id).await {
        Ok(true) => {
            info!(node_id = %node_id, "Node deleted");
            (StatusCode::NO_CONTENT).into_response()
        }
        Ok(false) => {
            // exists=true reaching here shouldn't happen, but guard for the case where the node was concurrently deleted
            if exists {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "delete reported 0 rows affected, but the node was just there",
                )
            } else {
                err(StatusCode::NOT_FOUND, "node not enrolled")
            }
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("delete: {e}")),
    }
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
    /// `raw` = original 10-second data; `hourly` = hourly aggregation (when the window start
    /// is earlier than the raw retention). The front end can hint "this segment is
    /// hourly-level" based on this — see design doc §8.
    resolution: &'static str,
}

/// `GET /v1/nodes/:id/series?metric=<name>&from=<ms>&to=<ms>&limit=N&rate=1`
///
/// Returns one metric's recent points in chronological order. Decoding happens
/// server-side so the browser receives a compact array instead of N protobuf
/// blobs.
///
/// - `from` / `to` (milliseconds, inclusive): the time-range control on the detail page;
///   defaults to the most recent hour.
/// - `rate=1`: for **cumulative** metrics (NIC rx/tx bytes), compute per-second deltas to
///   get bytes/s; negative increments from counter wrap or node restart are clamped to 0
///   to avoid downward spikes in the chart.
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
        return err(StatusCode::BAD_REQUEST, "from must be earlier than to");
    }
    let rate = matches!(q.get("rate").map(String::as_str), Some("1" | "true"));

    // Window start earlier than raw retention → hourly aggregation; otherwise raw data.
    // Raw retention days: see retention::RAW_RETENTION_DAYS.
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
/// Same metric across all nodes — **one line per node**, the front end renders the legend.
/// Uses the same data fetching and downsampling as the per-node `/v1/nodes/:id/series`
/// (long windows also go through hourly aggregation).
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
    let rate = matches!(q.get("rate").map(String::as_str), Some("1" | "true"));
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
        return err(StatusCode::BAD_REQUEST, "from must be earlier than to");
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
        let raw_points = node_series_points(
            &telemetry,
            &n.id,
            &n.hostname,
            &metric,
            from_ms,
            to_ms,
            limit,
            rate,
            hourly,
        )
        .await;
        let points: Vec<SeriesPoint> = if rate {
            to_rate(&raw_points)
        } else {
            raw_points
                .iter()
                .map(|(t, v)| SeriesPoint { t: *t, v: *v })
                .collect()
        };
        // Nodes without data don't get a line drawn, so no meaningless empty entries appear in the legend
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

/// Fetch one node's `(ts_ms, value)` points for the cross-node series view, from either the
/// hourly aggregate table (long windows) or raw telemetry.
///
/// Network metrics are counters; computing rates needs at least two samples. In the
/// "downsample by window" raw path, the interval between adjacent points is determined by
/// sampling density, so when converting to bytes/s we subtract the previous frame's cumulative
/// value, then divide by dt.
#[allow(clippy::too_many_arguments)] // the query window + flags are all needed; grouping would not aid readability
async fn node_series_points(
    telemetry: &zhiwei_storage::telemetry_repo::TelemetryRepo,
    node_id: &str,
    hostname: &str,
    metric: &str,
    from_ms: i64,
    to_ms: i64,
    limit: i64,
    rate: bool,
    hourly: bool,
) -> Vec<(i64, f64)> {
    if hourly {
        return match telemetry
            .hourly_range(
                node_id,
                metric,
                from_ms * 1_000_000,
                to_ms * 1_000_000,
                limit,
            )
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(ts, avg, _min, _max, first, last, _samples)| {
                    // Within an hour it's already an average; in rate mode fall back to "(last-first) spread over 1h"
                    let v = if rate {
                        ((last - first) / 3600.0).max(0.0)
                    } else {
                        avg
                    };
                    (ts / 1_000_000, v)
                })
                .collect(),
            Err(e) => {
                warn!(error = %e, node = %hostname, "failed to fetch hourly aggregation");
                Vec::new()
            }
        };
    }
    match telemetry
        .range(node_id, from_ms * 1_000_000, to_ms * 1_000_000, limit)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|(ts, _interval, payload)| {
                let batch = TelemetryBatch::decode(&payload[..]).ok()?;
                extract_metric(&batch, metric).map(|v| (ts / 1_000_000, v))
            })
            .collect(),
        Err(e) => {
            warn!(error = %e, node = %hostname, "failed to fetch telemetry");
            Vec::new()
        }
    }
}

/// Long windows use the hourly aggregation table. One entry per bucket: non-rate uses
/// the average; rate is reconstructed from the bucket's first/last (`(last - first) / 3600`),
/// so counter-type metrics (network bytes) can still be plotted here.
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

/// Metric extraction: `metrics[]` is the primary source; **derived metrics** are filled
/// in here, so the console can plot / alert on them just like regular metrics:
///
/// - `host.net.rx_bytes` / `host.net.tx_bytes`: not in `metrics[]` (one per NIC),
///   sum across NICs;
/// - `host.mem.usage`: the node reports used/total in bytes, percentage is computed here.
///   This used to be missing — the seeded rule "memory usage too high" uses this metric
///   name, but nodes never report it and evaluation only looks at `metrics[]`,
///   so **that rule would never fire**.
pub fn extract_metric(batch: &TelemetryBatch, name: &str) -> Option<f64> {
    if let Some(m) = batch.metrics.iter().find(|m| m.name == name) {
        return Some(m.value);
    }
    match name {
        "host.net.rx_bytes" => sum_u64_to_f64(batch.network.iter().map(|n| n.rx_bytes)),
        "host.net.tx_bytes" => sum_u64_to_f64(batch.network.iter().map(|n| n.tx_bytes)),
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

/// Cumulative counter → per-second rate (bytes/s). Take the delta between adjacent
/// points; negative increments are clamped to 0 (counter wrap or node restart).
fn to_rate(raw: &[(i64, f64)]) -> Vec<SeriesPoint> {
    raw.windows(2)
        .map(|w| {
            let elapsed_ms = w[1].0 - w[0].0;
            let dt_s = f64::from(i32::try_from(elapsed_ms).unwrap_or(i32::MAX)) / 1000.0_f64;
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
    /// Unix seconds at which the current node-agent process started. 0 = old agent /
    /// unknown, console shows it as unknown.
    agent_started_at_unix_seconds: u64,
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
        agent_started_at_unix_seconds: info.agent_started_at_unix_seconds,
    }
}

// ---------- inventory (snapshot: host info + containers + processes) ----------

/// `POST /v1/inventory`
///
/// Low-frequency (default 5 minutes) "current state" report: host info is written back
/// to nodes; containers and processes go into `node_inventory` (one latest snapshot per node).
/// This kind of data doesn't go into `telemetry_batches`, avoiding repeated storage of static
/// data every 30 seconds.
async fn inventory_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
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
        return err(
            StatusCode::BAD_REQUEST,
            "node_id does not match signature subject",
        );
    }

    // Certificate alerts need the hostname; save it first (host_info is Option, may be missing)
    let hostname = report
        .host_info
        .as_ref()
        .map_or_else(|| node_id.as_str().to_string(), |i| i.hostname.clone());

    apply_host_info(&state, &node_id, &report).await;

    let containers = view_containers(&report);
    let processes = view_processes(&report);
    let certificates = view_certificates(&report);

    let containers_json = serde_json::to_string(&containers).unwrap_or_else(|_| "[]".into());
    let processes_json = serde_json::to_string(&processes).unwrap_or_else(|_| "[]".into());
    let certificates_json = serde_json::to_string(&certificates).unwrap_or_else(|_| "[]".into());

    // Container start/stop event detection needs the "previous snapshot" as a baseline —
    // must be read before upsert overwrites it. The baseline lives in the database, surviving
    // node / monitor restarts; on the first report (no history), on_container_events skips
    // events and only sets the baseline.
    let previous_containers = state
        .storage
        .inventory()
        .find(&node_id)
        .await
        .ok()
        .flatten()
        .map(|r| r.containers_json);

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

    evaluate_snapshot_alerts(
        &state,
        node_id.as_str(),
        &hostname,
        previous_containers.as_deref(),
        &containers_json,
        &certificates_json,
    )
    .await;
    (StatusCode::NO_CONTENT).into_response()
}

/// Write the node's self-reported host info back to `nodes` (serialized `host_info_json`
/// plus the possibly-updated hostname).
async fn apply_host_info(
    state: &AppState,
    node_id: &zhiwei_common::NodeId,
    report: &InventoryReport,
) {
    let Some(info) = report.host_info.as_ref() else {
        return;
    };
    persist_host_info(state, node_id, info).await;
    // Nodes can self-report a new name: what we got at enrollment may be just a placeholder
    // like "bogon"; after switching to --node-name or LocalHostName no re-enrollment is needed
    if let Err(e) = state
        .storage
        .nodes()
        .update_hostname(node_id, &info.hostname)
        .await
    {
        warn!(error = %e, "update_hostname failed");
    }
}

async fn persist_host_info(
    state: &AppState,
    node_id: &zhiwei_common::NodeId,
    info: &zhiwei_proto::telemetry::HostInfo,
) {
    match serde_json::to_string(&host_info_view(info)) {
        Ok(json) => {
            if let Err(e) = state.storage.nodes().update_host_info(node_id, &json).await {
                warn!(error = %e, "update_host_info failed");
            }
        }
        Err(e) => warn!(error = %e, "serialize host_info failed"),
    }
}

fn view_containers(report: &InventoryReport) -> Vec<ContainerView> {
    report
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
            mem_usage_bytes: c.mem_usage_bytes,
            mem_limit_bytes: c.mem_limit_bytes,
            cpu_percent: c.cpu_percent,
            cpu_limit_nano: c.cpu_limit_nano,
        })
        .collect()
}

fn view_processes(report: &InventoryReport) -> Vec<ProcessView> {
    report
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
        .unwrap_or_default()
}

fn view_certificates(report: &InventoryReport) -> Vec<CertView> {
    report
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
        .collect()
}

/// Certificate expiry + container start/stop evaluation, sharing one display name for
/// consistent text. Same snapshot timing, so config changes / renewals immediately reflect.
async fn evaluate_snapshot_alerts(
    state: &AppState,
    node_id: &str,
    hostname: &str,
    previous_containers: Option<&str>,
    containers_json: &str,
    certificates_json: &str,
) {
    // Text uses display name (alias if set), same as metric alerts.
    let display = state
        .storage
        .nodes()
        .find_by_id(&zhiwei_common::NodeId::from_string(node_id.to_string()))
        .await
        .ok()
        .flatten()
        .map_or_else(
            || hostname.to_string(),
            |n| node_display_name(&n.alias, hostname),
        );
    crate::alerts::evaluate_cert_expiry(state, node_id, &display, certificates_json).await;
    crate::alerts::on_container_events(
        state,
        node_id,
        &display,
        previous_containers,
        containers_json,
    )
    .await;
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
    /// Matched server-certificate source id; empty = built-in / command-line glob scan
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
    /// From per-container inspect; 0 = unknown (older node agents don't report it)
    started_at_unix_nano: i64,
    finished_at_unix_nano: i64,
    /// Compose project ("application" dimension); empty string when reported by older node agents
    #[serde(default)]
    compose_project: String,
    #[serde(default)]
    compose_service: String,
    /// Usage and limit — all 0 when reported by older node agents (UI shows "—" / "unlimited")
    #[serde(default)]
    mem_usage_bytes: u64,
    #[serde(default)]
    mem_limit_bytes: u64,
    #[serde(default)]
    cpu_percent: f64,
    #[serde(default)]
    cpu_limit_nano: u64,
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
    /// Admin alias (empty string = not set); containers page node dropdown prefers it
    alias: String,
    ts_unix_nano: Option<i64>,
    containers: serde_json::Value,
}

/// `GET /v1/containers`: containers from all nodes, grouped by node (one request gets the whole containers page)
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

// ---------- Certificates ----------

#[derive(Serialize)]
struct NodeCertsView {
    node_id: String,
    hostname: String,
    ts_unix_nano: Option<i64>,
    certificates: serde_json::Value,
}

/// `GET /v1/certificates`: certificates from all nodes (certificates page gets everything in one request; front end sorts by expiry)
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

// ---------- Alerts and Rules ----------

#[derive(Deserialize)]
struct AlertsQuery {
    since: Option<i64>,
    until: Option<i64>,
    status: Option<String>,
    sources: Option<String>,
    #[serde(default = "default_alerts_limit")]
    limit: i64,
    #[serde(default)]
    offset: Option<i64>,
}

const fn default_alerts_limit() -> i64 {
    50
}

#[derive(Serialize)]
struct AlertsView {
    open: Vec<zhiwei_storage::alerts_repo::Alert>,
    resolved: Vec<zhiwei_storage::alerts_repo::Alert>,
    total: Option<i64>,
}

async fn alerts_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AlertsQuery>,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let repo = state.storage.alerts();
    let alert_query = zhiwei_storage::alerts_repo::AlertQuery {
        since_ms: query.since,
        until_ms: query.until,
        status: query.status.clone(),
        sources: query.sources.clone(),
        limit: Some(query.limit),
        offset: query.offset,
    };

    let (open_alerts, total) = if query.status.as_deref() == Some("resolved") {
        (vec![], None)
    } else {
        repo.open_alerts_with_total()
            .await
            .map_or_else(|_| (vec![], None), |(alerts, total)| (alerts, Some(total)))
    };

    let resolved_alerts = if query.status.as_deref() == Some("open") {
        vec![]
    } else {
        repo.query_alerts_filtered(&alert_query)
            .await
            .unwrap_or_default()
            .0
    };

    Json(AlertsView {
        open: open_alerts,
        resolved: resolved_alerts,
        total,
    })
    .into_response()
}

async fn silence_alert_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(serde::Deserialize)]
    struct Body {
        minutes: i64,
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
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
    info!(alert_id = id, minutes, "alert silenced");
    (StatusCode::NO_CONTENT).into_response()
}

async fn resolve_alert_handler(
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
    if let Err(e) = state.storage.alerts().resolve_alert(id, now).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("resolve alert: {e}"),
        );
    }
    info!(alert_id = id, "alert manually closed");
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
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if !["gt", "gte", "lt", "lte", "eq"].contains(&b.op.as_str()) {
        return err(StatusCode::BAD_REQUEST, "op must be gt/gte/lt/lte/eq");
    }
    if !["warning", "critical"].contains(&b.severity.as_str()) {
        return err(StatusCode::BAD_REQUEST, "severity must be warning/critical");
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
    #[derive(serde::Deserialize)]
    struct Body {
        name: Option<String>,
        metric: Option<String>,
        op: Option<String>,
        threshold: Option<f64>,
        duration_seconds: Option<i64>,
        severity: Option<String>,
        enabled: Option<bool>,
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    // Validate op and severity
    if let Some(ref op) = b.op {
        if !["gt", "gte", "lt", "lte", "eq"].contains(&op.as_str()) {
            return err(StatusCode::BAD_REQUEST, "op must be gt/gte/lt/lte/eq");
        }
    }
    if let Some(ref sev) = b.severity {
        if !["warning", "critical"].contains(&sev.as_str()) {
            return err(StatusCode::BAD_REQUEST, "severity must be warning/critical");
        }
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    // Read existing rule
    let rule = match state.storage.alerts().get_rule(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "rule not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("get rule: {e}")),
    };
    let updated = zhiwei_storage::alerts_repo::AlertRule {
        id: rule.id,
        name: b.name.unwrap_or(rule.name),
        metric: b.metric.unwrap_or(rule.metric),
        op: b.op.unwrap_or(rule.op),
        threshold: b.threshold.unwrap_or(rule.threshold),
        duration_seconds: b.duration_seconds.unwrap_or(rule.duration_seconds),
        severity: b.severity.unwrap_or(rule.severity),
        enabled: b.enabled.unwrap_or(rule.enabled),
        created_at_unix_nano: rule.created_at_unix_nano,
        updated_at_unix_nano: now,
    };
    match state.storage.alerts().update_rule(id, &updated).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("update rule: {e}"),
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

/// `GET /v1/builtin-alerts`: list built-in alert rules (node offline / node online etc.)
///
/// These rules aren't user-configured, but users need to see them on the alerts page
/// and be able to enable / disable them — otherwise they silently send notifications in
/// the background with no corresponding toggle on the console.
async fn list_builtin_alerts_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.alerts().list_builtin_rules().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list builtin alerts: {e}"),
        ),
    }
}

/// `PATCH /v1/builtin-alerts/:id`: update settings of a built-in alert rule
///
/// `id` is a stable string (e.g. '`node_offline`' / '`node_online`'). Unknown id returns 404.
async fn patch_builtin_alert_handler(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(serde::Deserialize)]
    struct Body {
        enabled: Option<bool>,
        threshold: Option<f64>,
        duration_seconds: Option<i64>,
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };

    // If no updates provided, just return success
    if b.enabled.is_none() && b.threshold.is_none() && b.duration_seconds.is_none() {
        return StatusCode::NO_CONTENT.into_response();
    }

    let now = zhiwei_common::Timestamp::now().unix_nano();

    // Get current rule to merge with updates
    match state.storage.alerts().list_builtin_rules().await {
        Ok(rules) => {
            let rule = rules.iter().find(|r| r.id == id);
            let Some(current) = rule else {
                return err(
                    StatusCode::NOT_FOUND,
                    format!("unknown builtin alert: {id}"),
                );
            };

            let threshold = b.threshold.unwrap_or(current.threshold);
            let duration_seconds = b.duration_seconds.unwrap_or(current.duration_seconds);
            let enabled = b.enabled.unwrap_or(current.enabled);

            match state
                .storage
                .alerts()
                .update_builtin_rule(&id, threshold, duration_seconds, enabled, now)
                .await
            {
                Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
                Ok(None) => err(
                    StatusCode::NOT_FOUND,
                    format!("unknown builtin alert: {id}"),
                ),
                Err(e) => err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("update builtin alert: {e}"),
                ),
            }
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list builtin alerts: {e}"),
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

// ---------- CA info ----------

#[derive(Serialize)]
struct CaView {
    subject: String,
    not_before_unix_nano: i64,
    not_after_unix_nano: i64,
    serial: String,
    /// SHA-256 fingerprint (colon-separated uppercase hex, easier for manual comparison)
    fingerprint_sha256: String,
    nodes_enrolled: i64,
    /// Whether this process terminates TLS itself.
    ///
    /// When false (managed platform, edge-terminated TLS), this CA is **not involved**:
    /// it's neither the node's trust root (nodes use system roots), nor is the monitor's
    /// certificate signed by it (the edge signs it). The front end accordingly renders
    /// this section as "not used by current deployment" rather than implying it's the
    /// cluster's identity root.
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
        .and_then(std::result::Result::ok);
    let Some((subject, nb, na, serial, fp)) = info else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to parse CA certificate",
        );
    };

    let nodes_enrolled = state
        .storage
        .nodes()
        .list_all()
        .await
        .map(|v| i64::try_from(v.len()).unwrap_or(i64::MAX))
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

/// Parse the local CA certificate (PEM) to extract subject / validity period / serial / fingerprint
#[allow(clippy::type_complexity)]
fn parse_ca(pem: &str) -> anyhow::Result<(String, i64, i64, String, String)> {
    use base64::Engine;
    use x509_parser::prelude::FromDer;
    let der = pem
        .lines()
        .filter(|l| !l.trim_start().starts_with("-----"))
        .collect::<String>();
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

// ---------- Notification channels ----------

async fn create_channel_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(serde::Deserialize)]
    struct Body {
        name: String,
        #[serde(default = "default_kind")]
        kind: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        secret: String,
        #[serde(default)]
        app_id: String,
        #[serde(default)]
        receive_id: String,
        #[serde(default)]
        receive_id_type: String,
        #[serde(default = "default_sev")]
        min_severity: String,
    }
    fn default_kind() -> String {
        "webhook".into()
    }
    fn default_sev() -> String {
        "warning".into()
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if b.name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "name cannot be empty");
    }
    // Old clients that only fill in `url`, hand-crafted curl, all hit this path: which field
    // is missing depends on the type; keep the check rule in one place (alerts::validate_channel),
    // don't write it twice
    let rid_type = default_receive_id_type(&b.receive_id_type);
    if let Err(msg) = crate::alerts::validate_channel(
        b.kind.trim(),
        &b.url,
        &b.secret,
        &b.app_id,
        &b.receive_id,
        &rid_type,
    ) {
        return err(StatusCode::BAD_REQUEST, msg);
    }
    if !matches!(b.min_severity.as_str(), "warning" | "critical") {
        return err(
            StatusCode::BAD_REQUEST,
            "min_severity must be warning or critical",
        );
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    let channel = zhiwei_storage::alerts_repo::NewChannel {
        name: b.name.trim(),
        kind: b.kind.trim(),
        url: b.url.trim(),
        secret: b.secret.trim(),
        app_id: b.app_id.trim(),
        receive_id: b.receive_id.trim(),
        receive_id_type: &rid_type,
        min_severity: &b.min_severity,
    };
    match state.storage.alerts().create_channel(&channel, now).await {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create channel: {e}"),
        ),
    }
}

/// Feishu's `receive_id_type` defaults to `chat_id` (matches Feishu's own default)
fn default_receive_id_type(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        "chat_id".into()
    } else {
        raw.to_string()
    }
}

/// `POST /v1/channels/test` — actually **sends one** with the current params; returns success or the reason for failure.
///
/// Use params rather than a channel id: in the "create channel" dialog, "Test" must be clickable before saving.
async fn test_channel_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(serde::Deserialize)]
    struct Body {
        kind: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        secret: String,
        #[serde(default)]
        app_id: String,
        #[serde(default)]
        receive_id: String,
        #[serde(default)]
        receive_id_type: String,
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let rid_type = default_receive_id_type(&b.receive_id_type);
    if let Err(msg) = crate::alerts::validate_channel(
        b.kind.trim(),
        &b.url,
        &b.secret,
        &b.app_id,
        &b.receive_id,
        &rid_type,
    ) {
        return err(StatusCode::BAD_REQUEST, msg);
    }

    let now = zhiwei_common::Timestamp::now().unix_nano();
    let ch = crate::alerts::test_channel(
        &b.kind,
        &b.url,
        &b.secret,
        &b.app_id,
        &b.receive_id,
        &b.receive_id_type,
    );
    // Share the same delivery path with real alerts: if the test passes, real events will also go out
    match crate::alerts::deliver(
        &ch,
        &crate::alerts::test_rule(),
        &crate::alerts::test_facts(),
        now,
    )
    .await
    {
        Ok(()) => Json(serde_json::json!({ "ok": true, "detail": "" })).into_response(),
        // Delivery failure is not a server error — pass the reason back to the console for the user to judge
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "detail": e.to_string(),
        }))
        .into_response(),
    }
}

/// `POST /v1/admin/token` — change console credentials.
///
/// Requires the current credential (equivalent to confirming the operation); the new
/// credential is written to `<data-dir>/admin.token` and takes effect immediately.
/// Nodes aren't affected — they use signatures and don't trust this credential.
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
        let current = state
            .admin_token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !crate::admin::ct_eq(b.current.trim().as_bytes(), current.as_bytes()) {
            return err(StatusCode::UNAUTHORIZED, "current credential is incorrect");
        }
    }
    if let Err(msg) = crate::admin::validate_new_token(b.new.trim()) {
        return err(StatusCode::BAD_REQUEST, msg);
    }

    if let Err(e) = crate::admin::save(&state.data_dir, b.new.trim()).await {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write new credential: {e}"),
        );
    }
    // Write to disk before changing memory: reversing the order would let a disk-write failure leave memory and file inconsistent
    {
        let mut current = state
            .admin_token
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *current = b.new.trim().to_string();
    }
    // The headers parameter is here only to match the shape of other handlers; we don't pre-check auth here —
    // changing credentials requires the current credential, which is itself a proof of identity
    let _ = headers;
    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Channel PATCH request body: all fields optional (absent = keep existing value).
#[derive(serde::Deserialize)]
struct ChannelPatchBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    receive_id: Option<String>,
    #[serde(default)]
    receive_id_type: Option<String>,
    #[serde(default)]
    min_severity: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// Merge "only the fields being changed" onto an existing channel.
///
/// Credentials may be overwritten but never cleared: an absent or empty `secret` keeps the
/// existing value (delete and recreate if you really need to reset). Validation failures
/// return error text ready to pass through to the console.
fn merge_channel_patch(
    mut ch: zhiwei_storage::alerts_repo::NotifyChannel,
    b: &ChannelPatchBody,
) -> Result<zhiwei_storage::alerts_repo::NotifyChannel, String> {
    if let Some(v) = &b.name {
        if v.trim().is_empty() {
            return Err("name cannot be empty".into());
        }
        ch.name = v.trim().to_string();
    }
    if let Some(v) = &b.url {
        ch.url = v.trim().to_string();
    }
    if let Some(v) = &b.secret {
        if !v.trim().is_empty() {
            ch.secret = v.trim().to_string();
        }
    }
    if let Some(v) = &b.app_id {
        ch.app_id = v.trim().to_string();
    }
    if let Some(v) = &b.receive_id {
        ch.receive_id = v.trim().to_string();
    }
    if let Some(v) = &b.receive_id_type {
        ch.receive_id_type = v.trim().to_string();
    }
    if let Some(v) = &b.min_severity {
        if !matches!(v.as_str(), "warning" | "critical") {
            return Err("min_severity must be warning or critical".into());
        }
        ch.min_severity.clone_from(v);
    }
    if let Some(v) = b.enabled {
        ch.enabled = v;
    }
    Ok(ch)
}

/// `PATCH /v1/channels/:id`: update a notification channel.
///
/// All fields optional; "absent = keep existing"; passing an empty `secret` also keeps
/// the existing value (credentials may be overwritten, never cleared — delete and
/// recreate if you really need to reset). `kind` is not modifiable: changing the type
/// changes the required fields; deleting and recreating is clearer. `enabled` alone works too.
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
    let b: ChannelPatchBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let ch = match state.storage.alerts().find_channel(id).await {
        Ok(Some(ch)) => ch,
        Ok(None) => return err(StatusCode::NOT_FOUND, "channel not found"),
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("find channel: {e}"),
            )
        }
    };
    let ch = match merge_channel_patch(ch, &b) {
        Ok(ch) => ch,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    // Validate the merged row as a whole: which required field is missing (per type) must be reported here
    if let Err(msg) = crate::alerts::validate_channel(
        &ch.kind,
        &ch.url,
        &ch.secret,
        &ch.app_id,
        &ch.receive_id,
        &ch.receive_id_type,
    ) {
        return err(StatusCode::BAD_REQUEST, msg);
    }
    let new = zhiwei_storage::alerts_repo::NewChannel {
        name: &ch.name,
        kind: &ch.kind,
        url: &ch.url,
        secret: &ch.secret,
        app_id: &ch.app_id,
        receive_id: &ch.receive_id,
        receive_id_type: &ch.receive_id_type,
        min_severity: &ch.min_severity,
    };
    match state
        .storage
        .alerts()
        .update_channel(id, &new, ch.enabled)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "channel not found"),
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

// ---------- Control channel ----------

#[derive(Serialize)]
struct NodeCommandsView {
    /// base64-encoded Command protobuf list; nodes verify the signature themselves
    commands: Vec<String>,
}

/// Maximum node long-poll hang time (seconds). When there are no commands, hang here;
/// once a new command is issued (signed by local ops), return immediately — orders of
/// magnitude faster than a 10s poll.
const MAX_COMMAND_WAIT_SECS: u64 = 25;
/// Even with no wake-up, re-check the DB this often: covers the edge case where
/// the command wasn't issued by this process (multi-instance, manual DB inserts),
/// guaranteeing commands are picked up within 2s at most.
const COMMAND_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);

/// Fetch pending commands for a node. With `wait_secs > 0`, long-poll: wait until
/// there's a command or the timeout (returns empty list).
///
/// The critical order is "subscribe first, then query the DB": this way commands that
/// land between the query and the await don't miss the signal, otherwise you'd get a
/// gap where the command is already in the DB but the node sleeps through a full timeout.
async fn collect_pending(
    state: &AppState,
    node_id: &str,
    wait_secs: u64,
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
    let mut rx = state.command_signal.subscribe();
    loop {
        // Each DB query uses the current time: a long-poll may hang for several seconds,
        // TTL judgment must move along
        let now_ns = zhiwei_common::Timestamp::now().unix_nano();
        let rows = state
            .storage
            .commands()
            .pending_for(node_id, 10, now_ns)
            .await?;
        if !rows.is_empty() || wait_secs == 0 {
            return Ok(rows);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(Vec::new());
        }
        tokio::select! {
            _ = rx.changed() => {}
            () = tokio::time::sleep(remaining.min(COMMAND_RECHECK)) => {}
        }
    }
}

/// Node pulls pending commands (request signature). Returns base64-encoded Command
/// protobuf; the node validates the command signature.
///
/// `wait=<seconds>` (optional, upper bound [`MAX_COMMAND_WAIT_SECS`]) enables long-polling;
/// without it, returns immediately (legacy node behavior).
async fn node_commands_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let pq = uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
    let (node_id, _node_pub) = match verify_node(&state, &headers, method.as_str(), pq, &[]).await {
        Ok(v) => v,
        Err((code, msg)) => return err(code, msg),
    };
    let Some(query_node_id) = q.get("node_id").cloned() else {
        return err(StatusCode::BAD_REQUEST, "node_id required");
    };
    if query_node_id != node_id.as_str() {
        return err(
            StatusCode::BAD_REQUEST,
            "node_id does not match signature subject",
        );
    }
    // A legitimate pull = this node's control loop is still alive. Only recorded after
    // signature verification passes — otherwise forged requests could mark someone
    // else's node as "channel OK".
    state.control_polls.note(
        node_id.as_str(),
        zhiwei_common::Timestamp::now().unix_nano() / 1_000_000,
    );
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

/// Node receipt (request signature). Only the signature subject itself can submit its own receipt.
async fn command_result_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
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
        return err(StatusCode::BAD_REQUEST, "command_id does not match path");
    }
    if result.node_id != node_id.as_str() {
        return err(
            StatusCode::BAD_REQUEST,
            "receipt node_id does not match signature subject",
        );
    }

    // Receipt is signed by the node's private key: even if the transport layer is compromised,
    // no one can forge someone else's execution results
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
            return err(
                StatusCode::UNAUTHORIZED,
                "Receipt signature verification failed",
            );
        }
    }

    // Receipts can only be written under your own commands, preventing A's signature from
    // overwriting B's command results
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
        Some(_) => {
            return err(
                StatusCode::FORBIDDEN,
                "receipt subject does not match command ownership",
            )
        }
        None => return err(StatusCode::NOT_FOUND, "command does not exist"),
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
            debug!(command_id = %id, ok = result.ok, "command receipt recorded");
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

/// Console initiates a command: forward to ops-server for signing, then write to DB
/// (see crates/ops-server).
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

    // Node must exist; avoid issuing commands to unknown nodes
    let node = zhiwei_common::NodeId::from_string(b.node_id.clone());
    match state.storage.nodes().find_by_id(&node).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "node not enrolled"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("lookup: {e}")),
    }

    // Forward to ops-server for signing (localhost, plain HTTP; ops is a separate process holding the signing private key)
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
        // ops's 4xx means "action/params rejected"; the reason must be passed through to the console verbatim —
        // otherwise the UI just shows "service unavailable", and the user can't fix the params, only retry
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
    /// Payload returned as UTF-8 text (e.g. logs)
    result_text: Option<String>,
    result_received_at_unix_nano: Option<i64>,
}

/// pending but past TTL → display as `expired`.
///
/// "Command expiry" was previously only checked on the node side
/// (`node-agent/src/control.rs::verify`); monitor never wrote `expired`, so history
/// always showed "still in transit", and operators couldn't see it had long since been
/// discarded — yet the default TTL is only 60s (ops-server `--ttl`). Filled in here
/// on the display side, same criterion as `zhiwei_storage::commands_repo`'s
/// `PENDING_LIVE_SQL`: `ttl_seconds <= 0` means never expires.
fn effective_state(state: &str, issued_at_unix_nano: i64, ttl_seconds: i64, now_ns: i64) -> String {
    let overdue = ttl_seconds > 0
        && issued_at_unix_nano.saturating_add(ttl_seconds.saturating_mul(1_000_000_000)) <= now_ns;
    if state == "pending" && overdue {
        "expired".to_string()
    } else {
        state.to_string()
    }
}

/// Command row → console view. Shared between history and single-record query; this is
/// the only place field semantics are defined.
fn command_view(
    c: zhiwei_storage::commands_repo::CommandRow,
    node_hostname: String,
    now_ns: i64,
) -> CommandHistoryView {
    CommandHistoryView {
        node_hostname,
        result_text: c
            .result_payload
            .as_ref()
            .and_then(|p| String::from_utf8(p.clone()).ok()),
        state: effective_state(&c.state, c.issued_at_unix_nano, c.ttl_seconds, now_ns),
        id: c.id,
        node_id: c.node_id,
        action: c.action,
        params_json: c.params_json,
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
    // Expired pending rows should display as expired: TTL is only 60s, and these rows only
    // change state when the node next polls; if the node never polls again, they stay
    // "still in transit" forever.
    let now_ns = zhiwei_common::Timestamp::now().unix_nano();
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
            command_view(c, host, now_ns)
        })
        .collect();
    Json(out).into_response()
}

/// `GET /v1/commands/:id` — current state of a single command.
///
/// When the console is waiting for a command receipt, it only cares about this one:
/// pulling the whole history (100 rows) is heavy and easily disturbed by other actions;
/// single-record queries let the polling interval drop to a few hundred milliseconds,
/// so results appear within seconds of clicking the action.
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
        Ok(None) => return err(StatusCode::NOT_FOUND, "Command not found"),
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
    Json(command_view(
        row,
        hostname,
        zhiwei_common::Timestamp::now().unix_nano(),
    ))
    .into_response()
}

/// Forward the signing request to ops-server (localhost)
/// Two failure modes when forwarding the signing request: ops explicitly rejects (4xx,
/// showable to user) and link problems (can't connect / timeout / 5xx, classified as
/// "service unavailable").
pub enum OpsSignError {
    Rejected { status: u16, message: String },
    Unavailable(String),
}

impl OpsSignError {
    /// Short description for logs only (user-facing text handled by the two branches above)
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Rejected { message, .. } => message.clone(),
            Self::Unavailable(detail) => detail.clone(),
        }
    }
}

/// Issue a command: forward to ops-server for signing, then wake any node long-polls.
///
/// The mandatory entry for all write actions — bypassing it forces nodes to wait for the
/// next fallback DB query before getting the command.
pub async fn sign_command(
    state: &AppState,
    payload: &serde_json::Value,
) -> Result<String, OpsSignError> {
    let id = ops_sign(&state.ops_endpoint, payload).await?;
    state.notify_command();
    Ok(id)
}

pub async fn ops_sign(endpoint: &str, payload: &serde_json::Value) -> Result<String, OpsSignError> {
    use http_body_util::BodyExt;
    let authority = endpoint.strip_prefix("http://").ok_or_else(|| {
        OpsSignError::Unavailable("ops endpoint only supports http:// (loopback)".into())
    })?;
    let (host_port, path) = authority
        .find('/')
        .map_or((authority, "/exec"), |i| (&authority[..i], &authority[i..]));
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
            "can't reach {endpoint}: no response within 3 seconds. For self-hosted deployments, ensure zhiwei-ops is running;\
             for container / managed-platform deployments, use the bundled image entrypoint."
        ))
    })?
    .map_err(|e| OpsSignError::Unavailable(ops_connect_error_hint(&e, endpoint)))?;

    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| OpsSignError::Unavailable(format!("handshake failed: {e}")))?;
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
        .map_err(|e| OpsSignError::Unavailable(format!("failed to build request: {e}")))?;

    let resp = tokio::time::timeout(std::time::Duration::from_secs(3), sender.send_request(req))
        .await
        .map_err(|_| OpsSignError::Unavailable("request timed out".into()))?
        .map_err(|e| OpsSignError::Unavailable(format!("request failed: {e}")))?;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| OpsSignError::Unavailable(format!("failed to read response: {e}")))?
        .to_bytes();
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        if status.is_client_error() {
            // ops's error body looks like {"error":"..."}, extract it and display verbatim
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
        .map_err(|e| OpsSignError::Unavailable(format!("response is not JSON: {e}")))?;
    v.get("command_id")
        .and_then(|x| x.as_str())
        .map(std::string::ToString::to_string)
        .ok_or_else(|| OpsSignError::Unavailable("ops response missing command_id".into()))
}

#[cfg(test)]
mod series_tests {
    // Test asserts exact rate deltas (e.g. `(300 - 100) / (1 - 0) == 200.0`).
    // Floating-point equality is intentional — inputs are integer-valued by
    // construction, so the rate is exactly representable.
    #![allow(clippy::float_cmp)]

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
                    labels: std::collections::HashMap::default(),
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
        // Network cumulative values aren't in metrics[], sum across NICs
        assert_eq!(extract_metric(&b, "host.net.rx_bytes"), Some(400.0));
        assert_eq!(extract_metric(&b, "host.net.tx_bytes"), Some(600.0));
        assert_eq!(extract_metric(&b, "host.disk.total_bytes"), None);

        // Nodes without NICs: should not return 0, but None (leave the chart blank, not a zero line)
        let empty = batch_with(vec![], vec![]);
        assert_eq!(extract_metric(&empty, "host.net.rx_bytes"), None);
    }

    #[test]
    fn extract_metric_derives_memory_usage_percent() {
        // The seeded rule "memory usage too high" uses host.mem.usage, but nodes only report
        // used/total in bytes — without derivation here, that rule would never fire.
        let b = batch_with(
            vec![
                ("host.mem.used_bytes", 8_000_000_000.0),
                ("host.mem.total_bytes", 16_000_000_000.0),
            ],
            vec![],
        );
        assert_eq!(extract_metric(&b, "host.mem.usage"), Some(50.0));

        // When the node has already reported a real value, the real value wins — no override
        let reported = batch_with(
            vec![
                ("host.mem.usage", 42.0),
                ("host.mem.used_bytes", 8_000_000_000.0),
                ("host.mem.total_bytes", 16_000_000_000.0),
            ],
            vec![],
        );
        assert_eq!(extract_metric(&reported, "host.mem.usage"), Some(42.0));

        // total missing or 0: return None, don't fabricate a number
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

        // Counter wrap / node restart: negative increments clamped to 0, no downward spike
        let reset = to_rate(&[(0, 9_000.0), (1_000, 100.0)]);
        assert_eq!(reset[0].v, 0.0);

        // Single point can't compute a delta → empty sequence (empty state on the chart, not 0)
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
        // This process terminates TLS: the monitor certificate is signed by this CA, node should pin it
        assert_eq!(enroll_ca_pem(LOCAL_CA, true), LOCAL_CA);
    }

    #[test]
    fn edge_terminated_tls_hands_nothing_so_the_node_uses_system_roots() {
        // Render / Railway: edge uses proper certificates, local CA is irrelevant.
        // Issuing it would become "enroll succeeds, every subsequent request fails TLS validation".
        assert_eq!(enroll_ca_pem(LOCAL_CA, false), "");
    }
}

#[cfg(test)]
mod bootstrap_token_tests {
    use super::*;

    #[test]
    fn static_token_never_expires_and_survives_repeated_checks() {
        // ZHIWEI_BOOTSTRAP_TOKEN goes this route: managed-platform free tiers can't race to grab
        // a "10-minute-from-startup" one-shot token, so we need a long-lived one.
        let tokens = BootstrapTokens::default();
        tokens.add_static("zhi-bt-fixed-token-0123456789".into());

        // Repeated check: the expiry branch removes; static tokens must not disappear.
        for _ in 0..3 {
            assert!(tokens.check("zhi-bt-fixed-token-0123456789"));
        }
        assert!(!tokens.check("zhi-bt-something-else"));
    }

    #[tokio::test]
    async fn static_token_keeps_the_ephemeral_one_from_being_minted() {
        // With a fixed token set, no ephemeral one-shot token should be generated/printed at startup.
        let tokens = BootstrapTokens::default();
        assert!(tokens.is_empty());
        tokens.add_static("zhi-bt-fixed-token-0123456789".into());
        assert!(!tokens.is_empty());
    }

    #[tokio::test]
    async fn one_shot_token_still_expires() {
        // Regression: the 10-minute TTL on one-shot tokens must not be lost by the changes above.
        let tokens = BootstrapTokens::default();
        tokens.add("zhi-bt-short-lived".into(), 0); // expire immediately
        assert!(!tokens.check("zhi-bt-short-lived"));
    }
}

/// `GET /v1/help` — help page markdown content.
///
/// Auth: admin token or AI token both allowed (AI clients doing onboarding may also need it).
/// When the help file is not found, return empty body; the UI side shows placeholder text.
///
/// Language selection (by priority): `?locale=` URL param → `Accept-Language`
/// header → default locale (en-US). Unknown locales fall back silently;
/// missing translations return 200 with the default locale's markdown.
///
/// The `{{BASE_URL}}` in the body is replaced with **the console's current access address**
/// (scheme + host, same origin as the enroll command); users can copy commands directly
/// from the help page without manually editing example domains.
async fn help_handler(
    State(state): State<AppState>,
    Query(q): Query<HelpQuery>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin or AI token)",
        );
    }
    let locale = q
        .locale
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_accept_language)
        });
    let mut snap = state.help.snapshot_for(locale.as_deref());
    let base = public_base_url(&headers, state.tls_terminated_locally);
    snap.body = snap.body.replace(BASE_URL_PLACEHOLDER, &base);
    Json(snap).into_response()
}

#[derive(serde::Deserialize)]
struct HelpQuery {
    #[serde(default)]
    locale: Option<String>,
}

/// Pick the first recognizable locale from an `Accept-Language` header (e.g.
/// `"zh-CN,zh;q=0.9,en;q=0.8"` → `"zh-CN"`). Empty / unparseable values
/// return None and let the caller fall back.
///
/// Tags arrive in arbitrary casing (`zh-CN`, `zh-cn`, `en`, `en-GB`) but the
/// help-content lookups are exact-match against canonical keys, so case-fold
/// before matching instead of returning the raw tag. Unknown locales pass
/// through lower-cased; the caller's lookup misses and falls back.
fn parse_accept_language(value: &str) -> Option<String> {
    for part in value.split(',') {
        let tag = part.split(';').next().unwrap_or("").trim();
        if tag.is_empty() {
            continue;
        }
        let lowered = tag.to_ascii_lowercase();
        return Some(match lowered.as_str() {
            "en" | "en-us" | "en-uk" | "en-gb" => "en-US".to_string(),
            "zh" | "zh-cn" | "zh-hans" | "zh-sg" => "zh-CN".to_string(),
            _ => lowered,
        });
    }
    None
}

/// Placeholder representing the "console address" in the help page (see `assets/help.md`).
const BASE_URL_PLACEHOLDER: &str = "{{BASE_URL}}";

#[cfg(test)]
mod help_locale_tests {
    use super::{parse_accept_language, HelpQuery};
    use crate::state::HelpContent;

    #[test]
    fn parse_accept_language_picks_first_known() {
        // First tag is zh-CN, should be picked and kept canonical.
        assert_eq!(
            parse_accept_language("zh-CN,zh;q=0.9,en;q=0.8"),
            Some("zh-CN".to_string()),
        );
        assert_eq!(
            parse_accept_language("en-US,en;q=0.9"),
            Some("en-US".to_string()),
        );
    }

    #[test]
    fn parse_accept_language_canonicalizes_case_and_short_tags() {
        // Browsers send `zh-cn` / `zh` / `en`; all must map to the canonical
        // keys the help lookup uses, or the lookup misses and falls back.
        assert_eq!(
            parse_accept_language("zh-cn,zh;q=0.9"),
            Some("zh-CN".to_string())
        );
        assert_eq!(parse_accept_language("zh"), Some("zh-CN".to_string()));
        assert_eq!(parse_accept_language("en"), Some("en-US".to_string()));
        assert_eq!(parse_accept_language("EN-GB"), Some("en-US".to_string()));
        // Unknown locales pass through lower-cased; the caller falls back.
        assert_eq!(
            parse_accept_language("fr-FR,en;q=0.5"),
            Some("fr-fr".to_string()),
        );
        assert_eq!(parse_accept_language(""), None);
    }

    #[test]
    fn help_query_default_is_none() {
        // When `?` has no locale, direct deserialization should still work.
        let q: HelpQuery = serde_json::from_str("{}").unwrap();
        assert!(q.locale.is_none());
    }

    #[test]
    fn help_content_picks_locale_and_falls_back() {
        let h = HelpContent::new([("en-US".to_string(), "english content".to_string())]);
        // Unknown locale falls back to en-US.
        assert_eq!(h.snapshot_for(Some("zh-CN")).body, "english content");
        assert_eq!(h.snapshot_for(Some("fr-FR")).body, "english content");
        // No locale = default locale (en-US).
        assert_eq!(h.snapshot_for(None).body, "english content");
    }
}

// ---------- AI Tokens ----------

/// `GET /v1/ai-tokens` — list all AI token metadata (excluding plaintext).
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

/// `POST /v1/ai-tokens` — create an AI token. Returns the plaintext token (**only this once**).
async fn create_ai_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(Deserialize)]
    struct Body {
        name: String,
    }
    use base64::Engine;
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let name = b.name.trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name cannot be empty");
    }
    if name.chars().count() > 64 {
        return err(StatusCode::BAD_REQUEST, "name too long (>64 characters)");
    }

    // Generate a 32-byte entropy plaintext token → base64url encoded.
    // Prefix `ait_` distinguishes from the bootstrap token's `zhi-bt-`, easier to grep.
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
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
            "warning": "plaintext token is shown only once, please copy and save it now",
        }))
        .into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create ai token: {e}"),
        ),
    }
}

/// `DELETE /v1/ai-tokens/:id` — revoke. Takes effect right away (next request 401).
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
        Ok(false) => err(
            StatusCode::NOT_FOUND,
            "id does not exist or has been revoked",
        ),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("revoke ai token: {e}"),
        ),
    }
}

// ---------- Enroll Tokens (runtime enrollment commands) ----------

/// `GET /v1/enroll-tokens` — list metadata of all currently non-expired enrollment tokens.
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
    /// TTL in seconds; default 86400 (24h). Maximum 7 days.
    #[serde(default)]
    ttl_secs: Option<u64>,
    /// Optional label; empty is also valid.
    #[serde(default)]
    label: Option<String>,
}

/// Console-facing message when ops-server is unreachable.
///
/// "ops-server not started" is a deployment shape issue; the raw OS error
/// (`Connection refused (os error 111)`) is no help to the user, so the
/// "what to do" goes directly into the text.
/// The caller already prepends "ops-server unavailable: "; don't repeat it here.
fn ops_connect_error_hint(e: &std::io::Error, endpoint: &str) -> String {
    if is_connection_refused(e) {
        format!(
            "can't reach {endpoint} ({e}): no process is listening on that address.\
             For self-hosted deployments, start zhiwei-ops; for container / managed-platform\
             deployments, use the bundled image entrypoint (it will start zhiwei-ops\
             in the same container)."
        )
    } else {
        format!(
            "can't reach {endpoint} ({e}): for self-hosted deployments, ensure zhiwei-ops\
             is running; for container / managed-platform deployments, use the bundled\
             image entrypoint."
        )
    }
}

/// Determine "the other end isn't listening".
///
/// Looking at `ErrorKind` alone isn't enough: when resolving hostnames / going through
/// intermediate layers, the kind degrades to `Uncategorized`, leaving only the
/// "Connection refused" in the text recognizable — and what production logs report
/// is `connection failed: Connection refused (os error 111)`. Relying only on kind
/// misses these and leaves the user with raw, headless errors. errno 111 is
/// Linux's ECONNREFUSED.
fn is_connection_refused(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::ConnectionRefused
        || e.raw_os_error() == Some(111)
        || e.to_string().contains("Connection refused")
}

/// The name used for a node in "human-facing" contexts: alias if set, otherwise the hostname.
///
/// Used by alert text, notifications, todos — hostnames are often auto-generated
/// names like `ip-10-0-0-5.ec2.internal` that don't tell the user which machine
/// it is; aliases are user-chosen like "Beijing Edge".
pub fn node_display_name(alias: &str, hostname: &str) -> String {
    let alias = alias.trim();
    if alias.is_empty() {
        hostname.to_string()
    } else {
        alias.to_string()
    }
}

/// Console's external access address (scheme + host), inferred from request headers.
///
/// Shared between the enroll command and the help page: the addresses they emit must
/// match, otherwise commands copied from the help page connect to the wrong place.
/// When behind nginx / a managed platform, prefer `X-Forwarded-Proto`; other fallbacks
/// see [`enroll_url_scheme`].
fn public_base_url(headers: &HeaderMap, tls_terminated_locally: bool) -> String {
    let scheme = enroll_url_scheme(headers, tls_terminated_locally);
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:8443");
    format!("{scheme}://{host}")
}

/// Whether the URL scheme in the generated enroll command should be http or https.
///
/// 1) `X-Forwarded-Proto` is the most authoritative — only the edge knows whether
///    the outward leg is plaintext or TLS;
/// 2) Without it: this process terminates TLS itself → https;
/// 3) This process is plain HTTP, and Host is loopback / private IP → http.
///
/// Rule 3 specifically targets "private IP + plaintext" self-hosted deployments:
/// previously always guessed https, so the console gave commands like
/// `https://10.0.0.5:8443/install-node.sh` which obviously can't connect.
/// Public domains aren't covered by rule 3 (still guess https): managed-platform
/// edges almost always send `X-Forwarded-Proto`, and when they don't, "external is https"
/// is the more likely scenario.
fn enroll_url_scheme(headers: &HeaderMap, tls_terminated_locally: bool) -> &'static str {
    if let Some(v) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        // May be a list like "https, http"; take the first segment
        let first = v.split(',').next().unwrap_or("").trim();
        if !first.is_empty() {
            return match first {
                "http" => "http",
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

/// Whether the Host header (may include port) is a loopback / private / mDNS address.
fn host_is_local(host: &str) -> bool {
    let bare = match host.rsplit_once(':') {
        // IPv6 literal looks like [::1]:8443; after stripping the port, also strip the brackets
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    let bare = bare.trim_start_matches('[').trim_end_matches(']');
    let is_mdns = bare
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("local"));
    if bare.eq_ignore_ascii_case("localhost") || is_mdns {
        return true;
    }
    bare.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            // `is_unique_local` is stable since 1.84; project MSRV is 1.75, so inline the fc00::/7 check.
            let octets = v6.octets();
            v6.is_loopback() || (octets[0] & 0xfe) == 0xfc
        }
    })
}

/// `POST /v1/enroll-tokens` — create a temporary enrollment token and return the full enroll command.
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
        return err(StatusCode::BAD_REQUEST, "ttl_secs must be in 1..=604800");
    }
    let label = b.label.unwrap_or_default().trim().to_string();
    if label.chars().count() > 64 {
        return err(StatusCode::BAD_REQUEST, "label too long (>64 characters)");
    }

    let token = BootstrapTokens::mint();
    state
        .bootstrap_tokens
        .add_with_label(token.clone(), ttl_secs, label.clone());
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expires_at_unix = now_unix + ttl_secs;

    // Infer the monitor's public URL from request headers:
    // Infer the monitor's public URL from request headers:
    //   1) Prefer `X-Forwarded-Proto` + `Host` (managed platforms / nginx inject these)
    //   2) Fallback: see enroll_url_scheme
    let monitor_url = public_base_url(&headers, state.tls_terminated_locally);

    // When ZHIWEI_NODE_BASE_URL is set (domestic / isolated-network self-hosted distribution source),
    // the command automatically includes an extra line so the runner doesn't have to remember
    // to add it. If unset, leave as-is (go through GitHub Releases).
    let base_url_line = state
        .node_base_url
        .as_deref()
        .map_or_else(String::new, |u| format!("    ZHIWEI_BASE_URL={u} \\\n"));
    let enroll_command = format!(
        "curl -sSL {monitor_url}/install-node.sh \\\n  | ZHIWEI_MONITOR_URL={monitor_url} \\\n    ZHIWEI_BOOTSTRAP_TOKEN={token} \\\n{base_url_line}    bash -s"
    );

    // Find the just-added id from list_active (guarantees id matches server-side).
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
        // UI's EnrollTokenCreated type extends EnrollTokenMeta; fields must align.
        // Runtime-generated tokens are always ephemeral (the long-lived kind comes from env vars, not this path).
        "permanent": false,
        "monitor_url": monitor_url,
        "enroll_command": enroll_command,
    }))
    .into_response()
}

/// `DELETE /v1/enroll-tokens/:id` — revoke an enrollment token. Takes effect immediately.
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
        err(StatusCode::NOT_FOUND, "id does not exist or has expired")
    }
}

/// `GET /install-node.sh` — emit the node enrollment script verbatim (`text/plain`).
///
/// No authentication: when the target machine runs `curl ... | bash` it has no credentials yet;
/// the secret is in the enroll command's environment variables. The script is embedded at
/// **compile time** from `scripts/install-node.sh` at the repo root (see `INSTALL_NODE_SH`
/// in `main.rs`), loaded onto state at startup.
async fn install_node_script_handler(State(state): State<AppState>) -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        state.install_script,
    )
        .into_response()
}

#[cfg(test)]
mod ai_token_auth_tests {
    use super::*;

    #[test]
    fn sha256_hex_is_lowercase_and_stable() {
        // Known vector: SHA-256 of empty string is e3b0c44298fc1c149afbf4c8996fb924...
        let h = sha256_hex(b"");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // Same input twice produces identical result
        assert_eq!(h, sha256_hex(b""));
    }

    #[test]
    fn sha256_hex_distinguishes_inputs() {
        assert_ne!(sha256_hex(b"ait_a"), sha256_hex(b"ait_b"));
    }

    #[test]
    fn sha256_hex_accepts_binary_bytes() {
        // 0x00 and the UTF-8 NUL string are the same byte sequence, hashes must agree
        let bytes = [0u8, 159, 146, 150];
        assert_eq!(sha256_hex(&bytes), sha256_hex(&bytes[..]));
    }
}

#[cfg(test)]
mod node_meta_tests {
    use super::*;

    #[test]
    fn alias_limited_by_chars_not_bytes() {
        // 10 multi-byte chars (20 bytes) are valid, 11 are not. We use
        // Greek letters here so the test stays ASCII-free in a global
        // project while still exercising multi-byte width counting.
        assert!(normalize_alias("αβγδεζηθικ").is_ok());
        assert!(normalize_alias("αβγδεζηθικλ").is_err());
        // Trim leading/trailing whitespace before counting length
        assert_eq!(normalize_alias("  bj-1  ").unwrap(), "bj-1");
    }

    #[test]
    fn tags_trim_dedupe_and_limit() {
        let got = normalize_tags(&[
            " prod ".to_string(),
            "prod".to_string(),
            String::new(),
            "bj".to_string(),
        ])
        .unwrap();
        assert_eq!(got, vec!["prod", "bj"]);
        // Single tag counted by characters
        assert!(normalize_tags(&["α".repeat(24)]).is_ok());
        assert!(normalize_tags(&["α".repeat(25)]).is_err());
        // Count upper limit
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
        tokens.add_with_label("zhi-bt-labeled".into(), 3600, "prod-web".into());
        let metas = tokens.list_active();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].label, "prod-web");
        assert!(!metas[0].permanent);
        // id looks like boot-<6 hex>
        assert!(metas[0].id.starts_with("boot-"), "got id={}", metas[0].id);
    }

    #[tokio::test]
    async fn expired_tokens_drop_out_of_list_active() {
        let tokens = BootstrapTokens::default();
        tokens.add_with_label("zhi-bt-expired".into(), 0, String::new());
        // TTL=0 → expires_at == now, filter condition is `> now`, so immediately invisible
        assert!(tokens.list_active().is_empty());
        assert!(!tokens.check("zhi-bt-expired"));
    }

    #[tokio::test]
    async fn revoke_by_id_removes_the_token() {
        let tokens = BootstrapTokens::default();
        tokens.add_with_label("zhi-bt-a".into(), 3600, "a".into());
        tokens.add_with_label("zhi-bt-b".into(), 3600, "b".into());
        let target_id = tokens
            .list_active()
            .into_iter()
            .find(|m| m.label == "a")
            .expect("label a present")
            .id;

        assert!(tokens.revoke_by_id(&target_id));
        // a has been revoked, b is still present
        assert!(!tokens.check("zhi-bt-a"));
        assert!(tokens.check("zhi-bt-b"));

        // Revoke the same id again: not found, returns false
        assert!(!tokens.revoke_by_id(&target_id));
    }

    #[tokio::test]
    async fn revoke_unknown_id_is_a_noop() {
        let tokens = BootstrapTokens::default();
        tokens.add("zhi-bt-x".into(), 3600);
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
        // Edge decides: private Host + plaintext listener, external is still https
        let h = headers_with(&[("host", "127.0.0.1:18445"), ("x-forwarded-proto", "https")]);
        assert_eq!(enroll_url_scheme(&h, false), "https");
        // Proxy chain may give a list; take the first segment
        let h = headers_with(&[
            ("host", "example.com"),
            ("x-forwarded-proto", "http, https"),
        ]);
        assert_eq!(enroll_url_scheme(&h, true), "http");
        // Empty / junk values fall back to the inference below
        let h = headers_with(&[("host", "10.0.0.5:8443"), ("x-forwarded-proto", "")]);
        assert_eq!(enroll_url_scheme(&h, false), "http");
    }

    #[test]
    fn enroll_scheme_uses_http_for_lan_plain_http() {
        // Self-hosted "private IP + plaintext": command must be http, otherwise the node can't connect
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
            // TODO(open-source): replace personal domain with a neutral test fixture
            // (e.g. "monitor.example.test") before tagging the public release.
            "monitor.example.test",
            // RFC 2606 / 5737 reserved ranges — neutral test fixtures.
            "192.0.2.1:8443",
            "203.0.113.1",
        ] {
            let h = headers_with(&[("host", host)]);
            assert_eq!(enroll_url_scheme(&h, false), "https", "host={host}");
            // When this process terminates TLS itself, it's clearer — always https
            assert_eq!(enroll_url_scheme(&h, true), "https", "host={host}");
        }
        // Missing Host header shouldn't crash either
        assert_eq!(enroll_url_scheme(&HeaderMap::new(), false), "https");
    }

    #[test]
    fn connection_refused_detected_even_without_the_right_kind() {
        // Real ECONNREFUSED maps to the right kind. The raw errno differs by
        // platform (Linux 111, macOS/BSD 61), so derive it from the OS.
        let refused_errno = if cfg!(target_os = "linux") { 111 } else { 61 };
        let e = std::io::Error::from_raw_os_error(refused_errno);
        assert_eq!(e.kind(), std::io::ErrorKind::ConnectionRefused);
        assert!(is_connection_refused(&e));

        // Real production form: kind degrades to Other, only the text is recognizable
        let e = std::io::Error::other("Connection refused (os error 111)");
        assert!(is_connection_refused(&e));
        let e = std::io::Error::new(std::io::ErrorKind::NotConnected, "Connection refused");
        assert!(is_connection_refused(&e));

        // Other network errors shouldn't be misclassified as "ops-server not started"
        for msg in ["connection reset by peer", "timed out", "dns error"] {
            assert!(!is_connection_refused(&std::io::Error::other(msg)));
        }
    }

    #[test]
    fn ops_connect_hint_always_says_what_to_do() {
        let refused = std::io::Error::other("Connection refused (os error 111)");
        let hint = ops_connect_error_hint(&refused, "http://127.0.0.1:8444");
        assert!(
            hint.contains("no process is listening on that address"),
            "{hint}"
        );
        assert!(hint.contains("entrypoint"), "{hint}");

        let other = std::io::Error::other("connection reset by peer");
        let hint = ops_connect_error_hint(&other, "http://127.0.0.1:8444");
        assert!(hint.contains("zhiwei-ops"), "{hint}");
        assert!(hint.contains("entrypoint"), "{hint}");
    }

    /// Overdue pending rows should be shown as expired: default TTL is only 60s; if the
    /// node stops polling, these rows stay "pending" forever and operators can't see in
    /// command history that they've long since been voided.
    #[test]
    fn overdue_pending_is_shown_as_expired() {
        const NOW: i64 = 1_700_000_000_000_000_000;
        const SEC: i64 = 1_000_000_000;

        // 60s TTL, issued 90s ago → expired
        assert_eq!(
            effective_state("pending", NOW - 90 * SEC, 60, NOW),
            "expired"
        );
        // Still within TTL → unchanged
        assert_eq!(
            effective_state("pending", NOW - 30 * SEC, 60, NOW),
            "pending"
        );
        // Boundary: issued + TTL == now is treated as expired (same criterion as commands_repo SQL)
        assert_eq!(
            effective_state("pending", NOW - 60 * SEC, 60, NOW),
            "expired"
        );
        // ttl<=0 = never expires (semantics of node-side control.rs::verify)
        assert_eq!(
            effective_state("pending", NOW - 86_400 * SEC, 0, NOW),
            "pending"
        );
        // Terminal states other than pending are left untouched
        for s in ["delivered", "done", "failed", "expired"] {
            assert_eq!(effective_state(s, NOW - 86_400 * SEC, 60, NOW), s);
        }
    }
}

#[cfg(test)]
mod channel_patch_tests {
    //! Channel edit (`PATCH /v1/channels/:id`) merge semantics.
    //!
    //! Key invariants:
    //!   1. Absent fields keep their existing values;
    //!   2. `secret` can only be overwritten; empty / absent cannot clear the stored credential;
    //!   3. The merged row must still pass the per-type required-field validation.

    use super::*;
    use zhiwei_storage::alerts_repo::NotifyChannel;

    fn channel() -> NotifyChannel {
        NotifyChannel {
            id: 7,
            name: "ops".into(),
            kind: "webhook".into(),
            url: "http://old/hook".into(),
            secret: "tok-old".into(),
            app_id: String::new(),
            receive_id: String::new(),
            receive_id_type: "chat_id".into(),
            enabled: true,
            min_severity: "warning".into(),
        }
    }

    fn body(json: &str) -> ChannelPatchBody {
        serde_json::from_str(json).expect("patch body")
    }

    fn validate(ch: &NotifyChannel) -> Result<(), String> {
        crate::alerts::validate_channel(
            &ch.kind,
            &ch.url,
            &ch.secret,
            &ch.app_id,
            &ch.receive_id,
            &ch.receive_id_type,
        )
    }

    #[test]
    fn absent_fields_keep_their_values() {
        let merged = merge_channel_patch(channel(), &body(r#"{"name":"ops2"}"#)).unwrap();
        assert_eq!(merged.name, "ops2");
        assert_eq!(merged.url, "http://old/hook");
        assert_eq!(merged.secret, "tok-old");
        assert_eq!(merged.min_severity, "warning");
        assert!(merged.enabled);
    }

    #[test]
    fn secret_can_be_overwritten_but_not_blanked() {
        // Empty string = keep existing value (credentials cannot be cleared by an empty string)
        let merged = merge_channel_patch(channel(), &body(r#"{"secret":"  "}"#)).unwrap();
        assert_eq!(merged.secret, "tok-old");

        // With value = overwrite
        let merged = merge_channel_patch(channel(), &body(r#"{"secret":"tok-new"}"#)).unwrap();
        assert_eq!(merged.secret, "tok-new");
    }

    #[test]
    fn other_fields_and_enabled_are_updated() {
        let merged = merge_channel_patch(
            channel(),
            &body(r#"{"enabled":false,"min_severity":"critical","url":" http://new/hook "}"#),
        )
        .unwrap();
        assert!(!merged.enabled);
        assert_eq!(merged.min_severity, "critical");
        assert_eq!(merged.url, "http://new/hook");
    }

    #[test]
    fn blank_name_and_unknown_severity_are_rejected() {
        assert!(merge_channel_patch(channel(), &body(r#"{"name":"   "}"#)).is_err());
        assert!(merge_channel_patch(channel(), &body(r#"{"min_severity":"info"}"#)).is_err());
    }

    #[test]
    fn merged_row_still_validates_per_kind() {
        let mut feishu = channel();
        feishu.kind = "feishu".into();
        feishu.app_id = "cli_x".into();
        feishu.secret = "sec".into();
        feishu.receive_id = "oc_1".into();

        // Empty string won't clear the App Secret, so the merge stays complete
        let merged = merge_channel_patch(feishu.clone(), &body(r#"{"secret":""}"#)).unwrap();
        assert!(validate(&merged).is_ok(), "{:?}", validate(&merged));

        // Clear receive ID -> Feishu validation should report it
        let merged = merge_channel_patch(feishu, &body(r#"{"receive_id":""}"#)).unwrap();
        let msg = validate(&merged).unwrap_err();
        assert!(msg.contains("receive ID"), "{msg}");
    }
}
