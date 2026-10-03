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
}

struct OpsState {
    storage: zhiwei_storage::Storage,
    key: KeyPair,
    ttl: i64,
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

    let state = Arc::new(OpsState {
        storage,
        key,
        ttl: args.ttl,
    });

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/exec", post(exec))
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
