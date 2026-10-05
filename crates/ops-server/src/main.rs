//! `ZhiWei` ops-server -- control plane.
//!
//! Design D7 requires monitor (data plane) and ops (control plane) to be separate processes.
//! The purpose is specific: **signing private key only exists within ops process**. Commands are
//! signed by ops before being stored. Nodes receive ops public key during enroll (TOFU) and
//! verify each one -- so even if monitor is compromised, it cannot forge a command that nodes will execute.
//!
//! Ops only listens on loopback address, for monitor to forward commands initiated by the console.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]
// `multiple_crate_versions` flags transitive deps (e.g. ed25519-dalek pulls
// `rand_core` 0.10 while `rand` 0.8 pulls 0.6). Not actionable from project
// code — pinned by upstream crates.
#![allow(clippy::multiple_crate_versions)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing_subscriber::EnvFilter;
use zhiwei_common::{KeyPair, Timestamp};
use zhiwei_proto::control::{Action, Command};

#[derive(Parser, Debug)]
#[command(
    name = "zhiwei-ops",
    about = "ZhiWei ops-server (control plane)",
    version
)]
struct Args {
    /// Data directory (shared with monitor, commands written to same database)
    #[arg(long, default_value = "data", env = "ZHIWEI_DATA_DIR")]
    data_dir: PathBuf,

    /// Listen address, default loopback only
    #[arg(long, default_value = "127.0.0.1:8444", env = "ZHIWEI_OPS_LISTEN")]
    listen: String,

    /// Default command TTL (seconds)
    #[arg(long, default_value_t = 60, env = "ZHIWEI_OPS_TTL")]
    ttl: i64,

    /// Directory to store agent upgrade packages
    #[arg(
        long,
        default_value = "/opt/zhiwei/agent-upgrades",
        env = "ZHIWEI_UPGRADE_DIR"
    )]
    upgrade_dir: PathBuf,
}

struct OpsState {
    storage: zhiwei_storage::Storage,
    key: KeyPair,
    ttl: i64,
    upgrade_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    tokio::fs::create_dir_all(&args.data_dir).await?;

    // Signing key: only exists within ops process, persisted as 0600
    let key_path = args.data_dir.join("ops.key");
    let pub_path = args.data_dir.join("ops.pub");
    let key = if key_path.exists() {
        let raw = tokio::fs::read(&key_path).await?;
        KeyPair::from_bytes(&raw)
            .map_err(|e| anyhow::anyhow!("Failed to read ops private key: {e}"))?
    } else {
        let kp = KeyPair::generate();
        tokio::fs::write(&key_path, kp.to_bytes()).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = tokio::fs::metadata(&key_path).await?.permissions();
            perm.set_mode(0o600);
            tokio::fs::set_permissions(&key_path, perm).await?;
        }
        tracing::warn!("Generated new ops signing key at {:?}", key_path);
        kp
    };

    // Public key given to monitor to distribute to nodes during enroll
    let pub_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(key.public_key().as_bytes())
    };
    tokio::fs::write(&pub_path, pub_b64.as_bytes()).await?;
    tracing::info!(fingerprint = %&pub_b64[..16.min(pub_b64.len())], "ops public key written to {:?}", pub_path);

    let db_path = args.data_dir.join("monitor.db");
    let storage = zhiwei_storage::Storage::open(&db_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open storage: {e}"))?;

    // Ensure upgrade directory exists
    tokio::fs::create_dir_all(&args.upgrade_dir).await?;
    tracing::info!(dir = %args.upgrade_dir.display(), "upgrade directory ready");

    let state = Arc::new(OpsState {
        storage,
        key,
        ttl: args.ttl,
        upgrade_dir: args.upgrade_dir,
    });

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/exec", post(exec))
        .route("/upgrade-packages", post(upload_upgrade_package))
        .route("/upgrade-packages/latest", get(get_latest_package))
        .route("/upgrade-packages", get(list_upgrade_packages))
        .with_state(state);

    let addr: SocketAddr = args.listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "zhiwei-ops ready (loopback only, for monitor to forward signing requests)");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct ExecRequest {
    node_id: String,
    action: String,
    #[serde(default)]
    params: serde_json::Value,
    #[serde(default)]
    actor: String,
}

#[derive(Serialize)]
struct ExecResponse {
    command_id: String,
}

fn parse_action(s: &str) -> Option<Action> {
    match s {
        "noop" => Some(Action::Noop),
        "fetch_logs" => Some(Action::FetchLogs),
        "kill_process" => Some(Action::KillProcess),
        "restart_host" => Some(Action::RestartHost),
        "shutdown_host" => Some(Action::ShutdownHost),
        "container_start" => Some(Action::ContainerStart),
        "container_stop" => Some(Action::ContainerStop),
        "container_restart" => Some(Action::ContainerRestart),
        "container_remove" => Some(Action::ContainerRemove),
        "refresh_inventory" => Some(Action::RefreshInventory),
        "scan_certs" => Some(Action::ScanCerts),
        "upgrade_agent" => Some(Action::UpgradeAgent),
        "rollback_agent" => Some(Action::RollbackAgent),
        _ => None,
    }
}

/// Validate command parameters. Control plane is the last checkpoint that can block malformed parameters --
/// node side validates again (defense in depth), but errors returned here let console see the cause immediately.
fn validate_params(action: Action, params: &serde_json::Value) -> Result<(), String> {
    match action {
        Action::KillProcess => {
            let pid = params
                .get("pid")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if pid <= 1 {
                return Err("kill_process requires pid (positive integer, 1 not allowed)".into());
            }
            let signal = params
                .get("signal")
                .and_then(|v| v.as_str())
                .unwrap_or("term");
            if !matches!(signal, "term" | "kill") {
                return Err("signal must be term (graceful) or kill (forced)".into());
            }
            Ok(())
        }
        Action::RestartHost | Action::ShutdownHost => {
            // No-parameter actions: if params are provided, caller got the action wrong, reject directly
            if params.as_object().is_some_and(|o| !o.is_empty()) {
                return Err("This action does not accept parameters".into());
            }
            Ok(())
        }
        Action::ContainerStart
        | Action::ContainerStop
        | Action::ContainerRestart
        | Action::ContainerRemove => {
            let name = params
                .get("container")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim();
            if name.is_empty() {
                return Err("This action requires container (name or ID)".into());
            }
            // Container name/ID gets concatenated into docker API path, block path traversal and control chars
            if name.len() > 128
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err("container allows only alphanumeric and - _ .".into());
            }
            Ok(())
        }
        Action::RefreshInventory => {
            if params.as_object().is_some_and(|o| !o.is_empty()) {
                return Err("This action does not accept parameters".into());
            }
            Ok(())
        }
        Action::ScanCerts => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            // Path rules shared with cert source config (absolute path / no .. / no control chars)
            zhiwei_common::certpath::normalize(path)
                .map(|_| ())
                .map_err(|msg| format!("scan_certs requires a valid path: {msg}"))
        }
        Action::UpgradeAgent => {
            // Validate required fields: version, download_url, sha256
            let version = params
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if version.is_empty() {
                return Err("upgrade_agent requires version".into());
            }
            let download_url = params
                .get("download_url")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if download_url.is_empty() {
                return Err("upgrade_agent requires download_url".into());
            }
            let sha256 = params
                .get("sha256")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if sha256.is_empty() {
                return Err("upgrade_agent requires sha256".into());
            }
            // SHA256 should be 64 hex characters
            if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("upgrade_agent sha256 must be 64 hex characters".into());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn exec(State(state): State<Arc<OpsState>>, Json(req): Json<ExecRequest>) -> Response {
    let Some(action) = parse_action(&req.action) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Unknown action {}", req.action) })),
        )
            .into_response();
    };

    if let Err(msg) = validate_params(action, &req.params) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response();
    }

    let now = Timestamp::now().unix_nano();
    let id = uuid::Uuid::new_v4().to_string();
    let mut nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    let params_json = req.params.to_string();

    let mut cmd = Command {
        id: id.clone(),
        node_id: req.node_id.clone(),
        action: action as i32,
        params_json: params_json.clone(),
        nonce: nonce.to_vec(),
        issued_at_unix_nano: now,
        ttl_seconds: state.ttl,
        signature: Vec::new(),
    };

    // Signature covers the entire message with signature field set to empty
    let mut preimage = Vec::new();
    if let Err(e) = prost::Message::encode(&cmd, &mut preimage) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("encode: {e}") })),
        )
            .into_response();
    }
    cmd.signature = state.key.sign(&preimage).0;

    let mut payload = Vec::new();
    if let Err(e) = prost::Message::encode(&cmd, &mut payload) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("encode: {e}") })),
        )
            .into_response();
    }

    if let Err(e) = state
        .storage
        .commands()
        .insert(
            &id,
            &req.node_id,
            &req.action,
            &params_json,
            &payload,
            now,
            state.ttl,
        )
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("store: {e}") })),
        )
            .into_response();
    }

    let actor = if req.actor.is_empty() {
        "ops-cli".to_string()
    } else {
        req.actor
    };
    if let Err(e) = state
        .storage
        .commands()
        .audit(now, &actor, &req.node_id, &id, &req.action, &params_json)
        .await
    {
        tracing::warn!(error = %e, "Failed to write audit log");
    }

    tracing::info!(%id, node = %req.node_id, action = %req.action, "Command issued");
    (StatusCode::CREATED, Json(ExecResponse { command_id: id })).into_response()
}

// ============================================================================
// Upgrade package management
// ============================================================================

/// Index file name in upgrade directory
const INDEX_FILE: &str = "index.json";

/// Package metadata stored in index.json
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PackageInfo {
    sha256: String,
    size_bytes: u64,
    uploaded_at: String,
}

/// Index file format
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UpgradeIndex {
    latest: Option<String>,
    packages: std::collections::HashMap<String, PackageInfo>,
}

/// Upload a new agent upgrade package.
async fn upload_upgrade_package(
    State(state): State<Arc<OpsState>>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let mut version: Option<String> = None;
    let mut binary_data: Option<Vec<u8>> = None;

    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "version" => {
                let bytes = match field.bytes().await {
                    Ok(b) => b.to_vec(),
                    Err(e) => {
                        return bad_request(&format!("failed to read version field: {e}"));
                    }
                };
                version = Some(String::from_utf8_lossy(&bytes).trim().to_string());
            }
            "binary" => {
                binary_data = match field.bytes().await {
                    Ok(b) => Some(b.to_vec()),
                    Err(e) => {
                        return bad_request(&format!("failed to read binary field: {e}"));
                    }
                };
            }
            _ => {}
        }
    }

    let Some(version) = version else {
        return bad_request("missing version field");
    };
    let Some(binary_data) = binary_data else {
        return bad_request("missing binary field");
    };

    // Validate version format (simple check: alphanumeric, dots, dashes)
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == 'v')
    {
        return bad_request("version must be alphanumeric with dots, dashes, or leading 'v'");
    }

    let version_dir = state.upgrade_dir.join(&version);
    let binary_path = version_dir.join("node-agent");

    // Create version directory
    if let Err(e) = tokio::fs::create_dir_all(&version_dir).await {
        return server_error(&format!("failed to create version directory: {e}"));
    }

    // Write binary
    if let Err(e) = tokio::fs::write(&binary_path, &binary_data).await {
        return server_error(&format!("failed to write binary: {e}"));
    }

    // Calculate SHA256
    let mut hasher = Sha256::new();
    hasher.update(&binary_data);
    let sha256 = format!("{:x}", hasher.finalize());

    // Update index
    let index_path = state.upgrade_dir.join(INDEX_FILE);
    let content = tokio::fs::read_to_string(&index_path).await.ok();
    let mut index: UpgradeIndex = content
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| UpgradeIndex {
            latest: None,
            packages: std::collections::HashMap::new(),
        });

    // Add or update package
    index.packages.insert(
        version.clone(),
        PackageInfo {
            sha256: sha256.clone(),
            size_bytes: binary_data.len() as u64,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
        },
    );

    // Update latest
    index.latest = Some(version.clone());

    // Write index
    if let Err(e) =
        tokio::fs::write(&index_path, serde_json::to_string_pretty(&index).unwrap()).await
    {
        return server_error(&format!("failed to write index: {e}"));
    }

    tracing::info!(version = %version, sha256 = %sha256, size = %binary_data.len(), "upgrade package uploaded");

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "version": version,
            "sha256": sha256,
            "size_bytes": binary_data.len()
        })),
    )
        .into_response()
}

/// Get information about the latest available upgrade package.
async fn get_latest_package(State(state): State<Arc<OpsState>>) -> Response {
    let index_path = state.upgrade_dir.join(INDEX_FILE);

    let Ok(content) = tokio::fs::read_to_string(&index_path).await else {
        return not_found("no upgrade packages available");
    };

    let index: UpgradeIndex = match serde_json::from_str(&content) {
        Ok(i) => i,
        Err(_) => {
            return server_error("corrupted index file");
        }
    };

    let Some(latest_version) = index.latest else {
        return not_found("no upgrade packages available");
    };

    let Some(info) = index.packages.get(&latest_version) else {
        return not_found("latest version info not found");
    };

    // Construct download URL (monitor server will serve this)
    let download_url = format!("/v1/upgrade/{latest_version}");

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "version": latest_version,
            "download_url": download_url,
            "sha256": info.sha256,
            "size_bytes": info.size_bytes
        })),
    )
        .into_response()
}

/// List all available upgrade packages.
async fn list_upgrade_packages(State(state): State<Arc<OpsState>>) -> Response {
    let index_path = state.upgrade_dir.join(INDEX_FILE);

    let Ok(content) = tokio::fs::read_to_string(&index_path).await else {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "packages": [], "latest": null })),
        )
            .into_response();
    };

    let index: UpgradeIndex = match serde_json::from_str(&content) {
        Ok(i) => i,
        Err(_) => {
            return server_error("corrupted index file");
        }
    };

    let packages: Vec<serde_json::Value> = index
        .packages
        .iter()
        .map(|(version, info)| {
            serde_json::json!({
                "version": version,
                "sha256": info.sha256,
                "size_bytes": info.size_bytes,
                "uploaded_at": info.uploaded_at
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "packages": packages,
            "latest": index.latest
        })),
    )
        .into_response()
}

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}

fn server_error(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}
