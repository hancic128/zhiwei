//! ZhiWei ops-server —— 控制平面。
//!
//! 设计稿 D7 要求 monitor（数据平面）与 ops（控制平面）拆成两个进程，
//! 目的很具体：**签名私钥只在 ops 进程内**。命令由 ops 签名后落库，
//! 节点在 enroll 时拿到 ops 公钥（TOFU）并逐个验签——因此即便 monitor
//! 被攻陷，它也无法伪造一条节点会执行的命令。
//!
//! ops 只监听回环地址，供 monitor 转发控制台发起的命令。

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
    /// 数据目录（与 monitor 共用，命令写进同一个库）
    #[arg(long, default_value = "data", env = "ZHIWEI_DATA_DIR")]
    data_dir: PathBuf,

    /// 监听地址，默认只绑回环
    #[arg(long, default_value = "127.0.0.1:8444", env = "ZHIWEI_OPS_LISTEN")]
    listen: String,

    /// 命令默认有效期（秒）
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

    // 签名密钥：只在 ops 进程内，落盘 0600
    let key_path = args.data_dir.join("ops.key");
    let pub_path = args.data_dir.join("ops.pub");
    let key = if key_path.exists() {
        let raw = tokio::fs::read(&key_path).await?;
        KeyPair::from_bytes(&raw).map_err(|e| anyhow::anyhow!("读取 ops 私钥: {e}"))?
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
        tracing::warn!("已生成新的 ops 签名密钥 {:?}", key_path);
        kp
    };

    // 公钥给 monitor 在 enroll 时下发给节点
    let pub_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(key.public_key().as_bytes())
    };
    tokio::fs::write(&pub_path, pub_b64.as_bytes()).await?;
    tracing::info!(fingerprint = %&pub_b64[..16.min(pub_b64.len())], "ops 公钥已写入 {:?}", pub_path);

    let db_path = args.data_dir.join("monitor.db");
    let storage = zhiwei_storage::Storage::open(&db_path)
        .await
        .map_err(|e| anyhow::anyhow!("打开存储: {e}"))?;

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
    tracing::info!(%addr, "zhiwei-ops ready（仅回环，供 monitor 转发签发请求）");
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

/// 命令参数校验。控制平面是最后一个能拦住畸形参数的关口——
/// 节点侧还会再校验一次（纵深防御），但错误在这里返回，控制台能立刻看到原因。
fn validate_params(action: Action, params: &serde_json::Value) -> Result<(), String> {
    match action {
        Action::KillProcess => {
            let pid = params.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
            if pid <= 1 {
                return Err("kill_process 需要 pid（正整数，且不允许 1）".into());
            }
            let signal = params
                .get("signal")
                .and_then(|v| v.as_str())
                .unwrap_or("term");
            if !matches!(signal, "term" | "kill") {
                return Err("signal 只能是 term（优雅）或 kill（强杀）".into());
            }
            Ok(())
        }
        Action::RestartHost | Action::ShutdownHost => {
            // 无参数动作：带上参数说明调用方搞错了动作，直接拒绝
            if params.as_object().is_some_and(|o| !o.is_empty()) {
                return Err("该动作不接受参数".into());
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
                return Err("该动作需要 container（容器名或 ID）".into());
            }
            // 容器名/ID 会拼进 docker API 路径，挡掉路径穿越与控制字符
            if name.len() > 128
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err("container 只允许字母数字与 - _ .".into());
            }
            Ok(())
        }
        Action::RefreshInventory => {
            if params.as_object().is_some_and(|o| !o.is_empty()) {
                return Err("该动作不接受参数".into());
            }
            Ok(())
        }
        Action::ScanCerts => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            // 路径规则与证书来源配置共用同一份实现（绝对路径 / 无 .. / 无控制字符）
            zhiwei_common::certpath::normalize(path)
                .map(|_| ())
                .map_err(|msg| format!("scan_certs 需要合法路径：{msg}"))
        }
        _ => Ok(()),
    }
}

async fn exec(State(state): State<Arc<OpsState>>, Json(req): Json<ExecRequest>) -> Response {
    let Some(action) = parse_action(&req.action) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("未知动作 {}", req.action) })),
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

    // 签名覆盖 signature 置空后的整个消息
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
        tracing::warn!(error = %e, "写审计失败");
    }

    tracing::info!(%id, node = %req.node_id, action = %req.action, "已签发命令");
    (StatusCode::CREATED, Json(ExecResponse { command_id: id })).into_response()
}
