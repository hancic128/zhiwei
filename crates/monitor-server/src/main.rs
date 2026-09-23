//! ZhiWei monitor-server (data plane).
//!
//! Endpoints:
//!   POST /v1/enroll      — bootstrap token + node Ed25519 public key → node_id
//!   POST /v1/telemetry   — 签名请求（Ed25519），protobuf TelemetryBatch
//!   GET  /healthz        — liveness
//!
//! 节点身份由请求签名承担（见 `zhiwei_common::auth`），不依赖客户端证书：
//! 托管平台在边缘终止 TLS、不向容器转发客户端证书，所以必须能在「明文
//! HTTP + 边缘 TLS」下工作（`--plain-http`）。自建部署仍可用内置 rustls
//! 直接终结 TLS（只配服务端证书，不要求客户端证书）。
//!
//! hyper-util 的 auto builder 按连接协商 HTTP/1.1 或 HTTP/2。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::Parser;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;

mod admin;
mod alerts;
mod ca;
mod certs_api;
mod config;
mod mcp;
mod probe_test;
mod probes_api;
mod retention;
mod routes;
mod state;
mod tls;
mod todo_api;

use state::AppState;

#[derive(Parser, Debug)]
#[command(
    name = "zhiwei-monitor",
    about = "ZhiWei monitor-server (data plane)",
    version
)]
struct Args {
    /// Path to config TOML
    #[arg(long, default_value = "config/monitor.toml")]
    config: PathBuf,

    /// Override data directory (where DB + certs live)
    #[arg(long, env = "ZHIWEI_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Override listen address (e.g. 0.0.0.0:8443)
    #[arg(long, env = "ZHIWEI_LISTEN")]
    listen: Option<String>,

    /// Directory holding the built console (SPA). Served at / when present.
    #[arg(long, env = "ZHIWEI_UI_DIR", default_value = "ui/dist")]
    ui_dir: PathBuf,

    /// 以明文 HTTP 提供服务，TLS 由前置边缘（PaaS / 反代）终止。
    /// 节点身份靠请求签名，不依赖传输层，因此明文传输不影响鉴权强度。
    /// 环境变量接受 1/0/true/false/yes/no/on/off（PaaS 面板里常填 1）。
    ///
    /// 不传或留空时，会按常见 PaaS 环境变量自动判断：
    /// `RENDER` / `RAILWAY_*` / `NORTHFLANK_*` / `DYNO`（Heroku）下默认 true。
    /// 显式传 0/false 总是覆盖自动判断。
    #[arg(
        long,
        env = "ZHIWEI_PLAIN_HTTP",
        value_parser = clap::builder::BoolishValueParser::new(),
        default_missing_value = "auto",
        num_args = 0..=1,
    )]
    plain_http: Option<bool>,
}

/// 是否处于「边缘终结 TLS 的 PaaS」环境：Render / Railway / Northflank / Heroku。
/// 仅作为 `--plain-http` 未显式配置时的默认判断，自建主机不受影响。
///
/// 注意：这只是一层便利。Northflank 这类平台既不注入 `PORT`、也没有稳定的
/// 环境变量前缀（实测 `listen` 会落到默认的回环地址），自动判断覆盖不到，
/// 必须显式设 `ZHIWEI_PLAIN_HTTP=1`。`--plain-http` 的显式配置永远优先。
fn detect_paas_edge_terminates_tls() -> bool {
    // Render 永远注入 `RENDER=true`。
    if std::env::var("RENDER").ok().as_deref() == Some("true") {
        return true;
    }
    // Railway 注入 RAILWAY_*（任一存在即视为 Railway）。
    if std::env::vars().any(|(k, _)| k.starts_with("RAILWAY_")) {
        return true;
    }
    // Northflank 没有单一稳定标记，组合几个常见变量。
    if std::env::vars().any(|(k, _)| k.starts_with("NORTHFLANK_")) {
        return true;
    }
    // Heroku / 旧式 PaaS。
    if std::env::var("DYNO").is_ok() {
        return true;
    }
    false
}

/// 监听地址是否是回环。
///
/// 只做字符串判断——传入的是 `host:port`，不需要真正解析 IP。
/// IPv6 形如 `[::1]:8443`，所以要把方括号剥掉再比。
fn listen_is_loopback(listen: &str) -> bool {
    let host = match listen.rsplit_once(':') {
        Some((h, _)) => h,
        None => listen,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "localhost" | "::1") || host.starts_with("127.")
}

/// TLS 握手失败的告警间隔（秒）。
///
/// 托管平台的健康检查会按固定节奏戳容器，配置错了就是每来一次打一条。
/// 不设窗口的话启动信息会被刷掉，真正的原因反而看不见。
const TLS_WARN_INTERVAL_SECS: u64 = 60;

/// 打一条 TLS 握手失败告警，同一窗口内只打一次。
///
/// 特意把 `InvalidContentType` 认出来：那条错误的含义是「对方发的是明文 HTTP，
/// 本进程却按 TLS 解析」，在托管平台上是典型的「忘了开 `--plain-http`」。
/// 光看 rustls 的原文根本猜不到，所以这里直接给出下一步动作。
fn warn_tls_handshake_once(
    err: &std::io::Error,
    peer: std::net::SocketAddr,
    last_warn: &AtomicU64,
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let prev = last_warn.load(Ordering::Relaxed);

    // 窗口内已经报过，或者抢输了 CAS（别的任务刚报过）→ 降级为 debug
    if now.saturating_sub(prev) < TLS_WARN_INTERVAL_SECS
        || last_warn
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        tracing::debug!(error = %err, %peer, "TLS handshake failed（同类告警已限流）");
        return;
    }

    if err.to_string().contains("InvalidContentType") {
        tracing::warn!(
            error = %err,
            %peer,
            "TLS 握手失败：对方发的是明文 HTTP，而本进程按 TLS 处理。\
             部署在托管平台（边缘已终结 TLS）后面时，请设 ZHIWEI_PLAIN_HTTP=1 并重新部署；\
             自建部署请检查客户端是不是用 http:// 连了 https 端口。"
        );
    } else {
        tracing::warn!(error = %err, %peer, "TLS handshake failed");
    }
}

/// 从 ops endpoint（`http://127.0.0.1:8444/exec`）里取出 `(host, port)`。
fn endpoint_host_port(endpoint: &str) -> Option<(String, u16)> {
    let authority = endpoint.strip_prefix("http://")?;
    let host_port = authority.split('/').next()?;
    match host_port.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((host_port.to_string(), 8444)),
    }
}

/// 端口上有人监听就算「在跑」，重试 `attempts` 次（每次间隔 150ms）。
async fn wait_for_port(host: &str, port: u16, attempts: u32) -> bool {
    for _ in 0..attempts {
        if tokio::net::TcpStream::connect((host, port)).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

/// ops-server 二进制的候选路径：`ZHIWEI_OPS_BIN` 优先，其次与 monitor 同目录。
fn ops_binary_candidates(exe: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("ZHIWEI_OPS_BIN") {
        let p = p.trim();
        if !p.is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    if let Some(dir) = exe.and_then(Path::parent) {
        out.push(dir.join("zhiwei-ops"));
    }
    out
}

fn locate_ops_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    ops_binary_candidates(exe.as_deref())
        .into_iter()
        .find(|p| p.is_file())
}

/// 命令通道的兜底启动：等价于 `scripts/docker-entrypoint.sh` 里那段 `start_ops`，
/// 但放进 monitor 进程内，好让「只装了一个二进制」的部署也能用写操作。
///
/// 边界没变：ops 仍是独立进程，签名私钥只在它手里，monitor 只读 `ops.pub`。
///
/// - `ZHIWEI_OPS_DISABLE=1` → 明确声明「只跑数据平面」，不动
/// - 端口上已经在监听 → 外部已经起了（entrypoint / dev.sh / 自建 unit）
/// - 找不到 `zhiwei-ops` → 给一句可操作的提示，不假装成功
async fn ensure_ops_control_plane(data_dir: &Path, endpoint: &str) {
    if std::env::var("ZHIWEI_OPS_DISABLE").ok().as_deref() == Some("1") {
        tracing::info!("ZHIWEI_OPS_DISABLE=1：不拉起 ops-server，命令通道不可用（数据平面照常）");
        return;
    }
    let Some((host, port)) = endpoint_host_port(endpoint) else {
        tracing::warn!(%endpoint, "ops endpoint 不是 http://host:port 形态，跳过兜底启动");
        return;
    };
    // 外部可能已经在跑（entrypoint / dev.sh / 自建 unit）。给它一点重试窗口，
    // 别因为几毫秒的启动差把第二个 ops 也拉起来。
    if wait_for_port(&host, port, 8).await {
        tracing::debug!(%endpoint, "ops-server 已在运行（命令通道可用）");
        return;
    }

    let Some(bin) = locate_ops_binary() else {
        tracing::warn!(
            %endpoint,
            "命令通道不可用：该地址上没有进程在监听，也没找到 zhiwei-ops 二进制。\
             把 zhiwei-ops 与 zhiwei-monitor 放在同一目录（或用 ZHIWEI_OPS_BIN 指定路径）后重启；\
             只想跑数据平面的话显式设 ZHIWEI_OPS_DISABLE=1。"
        );
        return;
    };

    match tokio::process::Command::new(&bin)
        .env("ZHIWEI_DATA_DIR", data_dir)
        // 让被拉起的 ops 监听 monitor 真正要连的那个地址（ops 默认 8444，
        // 配置里改过 ops_endpoint 时不能靠默认值撞运气）
        .env("ZHIWEI_OPS_LISTEN", format!("{host}:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => {
            tracing::info!(
                bin = %bin.display(),
                pid = child.id().unwrap_or(0),
                "已拉起 ops-server（控制平面）；签名私钥只在它进程内"
            );
            // 等它「就绪」再往下走，判据是端口能被连上，而不是 ops.pub 存在：
            // ops 是先开库跑迁移、后 bind，所以端口通了同时意味着
            //   ① ops.pub 已落盘——monitor 启动时读一次并缓存，读早了新入网
            //      节点会拿到空公钥、命令通道静默失效；
            //   ② 迁移已跑完——两个进程同时对同一个 SQLite 跑 `CREATE TABLE`
            //      会撞车（实测 monitor 直接起不来：`table nodes already exists`）。
            if wait_for_port(&host, port, 60).await {
                tracing::info!(%endpoint, "ops-server 已就绪（命令通道可用）");
            } else {
                tracing::warn!(%endpoint, "ops-server 9 秒内没起来，命令通道可能不可用");
            }
            // 交给 tokio 的 SIGCHLD 收尸，这里不 wait（monitor 会一直跑）
            drop(child);
        }
        Err(e) => tracing::warn!(bin = %bin.display(), error = %e, "拉起 ops-server 失败"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,zhiwei=debug")),
        )
        .init();

    let args = Args::parse();
    let cfg = config::MonitorConfig::load(&args.config).context("loading config")?;
    let data_dir = args.data_dir.unwrap_or(cfg.data_dir.clone());
    let listen = args.listen.unwrap_or(cfg.listen.clone());

    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;

    // 命令通道（控制台里的删容器 / 拉日志 / 重启主机…）由 ops-server 签发。
    // 镜像的 entrypoint 与 dev.sh 都会先起它；只有「直接跑二进制」的部署没有
    // 第二个进程的位置，控制台里所有写操作都会报「ops-server 不可用」。
    // 这里做兜底：连不上且旁边就有 zhiwei-ops 时自己把它拉起来。
    ensure_ops_control_plane(&data_dir, &cfg.ops_endpoint).await;

    // 没有显式传 --plain-http / ZHIWEI_PLAIN_HTTP 时，按 PaaS 环境自动判断。
    // 显式 false 永远覆盖自动判断——自建部署保持 TLS 本地终结。
    let plain_http = args
        .plain_http
        .unwrap_or_else(detect_paas_edge_terminates_tls);
    tracing::info!(
        ?data_dir,
        %listen,
        plain_http,
        paas_auto_detected = args.plain_http.is_none(),
        "starting zhiwei-monitor"
    );

    // 绑回环在「自建 + 本机反代」下是对的，在容器里几乎一定是错的：
    // 平台边缘从容器外部转发进来，回环地址它够不着，表现为健康检查失败 / 502。
    // 这个坑光看日志很难反应过来（端口明明对），所以主动喊一声。
    if listen_is_loopback(&listen) {
        tracing::warn!(
            %listen,
            "监听在回环地址：本机之外的任何东西（托管平台边缘、浏览器、其它主机）都连不上。\
             托管平台请设 ZHIWEI_LISTEN=0.0.0.0:<端口>，或直接设 PORT=<端口>（本程序检测到 PORT 会自动绑 0.0.0.0）。"
        );
    }

    let db_path = data_dir.join("monitor.db");
    let storage = zhiwei_storage::Storage::open(&db_path)
        .await
        .context("opening sqlite storage")?;

    let ca = ca::Ca::load_or_init(&data_dir)
        .await
        .context("initializing CA")?;
    // 明文模式下 TLS 由前置边缘终结，不需要本地签发服务端证书
    let server_cert = if plain_http {
        None
    } else {
        Some(
            tls::ensure_server_cert(&ca, &data_dir, &cfg.server_cert_cn)
                .context("ensuring server cert")?,
        )
    };

    // ops 公钥由 ops-server 首次启动时写入；没有它时 enroll 会下发空串，
    // 节点将拒绝任何命令（安全侧默认拒绝）。
    let ops_pub_path = data_dir.join("ops.pub");
    let ops_public_key = tokio::fs::read_to_string(&ops_pub_path)
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if ops_public_key.is_empty() {
        tracing::warn!(
            "未找到 {}；节点将无法验证命令签名。启动 zhiwei-ops 后会生成。",
            ops_pub_path.display()
        );
    }

    let (admin_token, admin_token_source) = admin::load_or_init(&data_dir)
        .await
        .context("initializing admin token")?;
    match admin_token_source {
        // 本次生成的：只在这里打印一次。托管平台免费层拿不到 Shell 时，
        // 这行日志是唯一的出口（日志面板免费可看）。
        admin::TokenSource::Generated => {
            eprintln!(
                "\
\n[ADMIN TOKEN] {admin_token}
   Console:    https://<host>:<port>/    (paste this token when prompted)
   Read API:   Authorization: Bearer <token>
   Stored at:  {}/admin.token\n",
                data_dir.display()
            );
            tracing::info!(
                ?data_dir,
                "admin token ready (use `cat <data-dir>/admin.token`)"
            );
        }
        // 来自环境变量：token 已经在部署面板里，不重复打印（日志里不该留明文凭据）。
        admin::TokenSource::Env => {
            tracing::info!(
                "admin token ready (来自 ZHIWEI_ADMIN_TOKEN，不会写盘、也不会随重启变化)"
            );
        }
        // 来自文件：自建 / 有持久卷的常规路径。
        admin::TokenSource::File => {
            tracing::info!(
                ?data_dir,
                "admin token ready (use `cat <data-dir>/admin.token`)"
            );
        }
    }

    let bootstrap_tokens = Arc::new(routes::BootstrapTokens::default());
    // 固定入网令牌优先：托管平台（免费层没 Shell、没持久卷）用它替掉
    // 「抢启动日志里 10 分钟有效期的一次性 token」这套流程。
    match std::env::var("ZHIWEI_BOOTSTRAP_TOKEN") {
        Ok(raw) => {
            // 太短一律拒绝，理由同 admin token：它长期有效、又暴露在公网边缘，
            // 短了就能被暴力猜解并注册假节点。拒绝后照旧生成一次性 token 兜底，
            // 所以服务不会起不来，但绝不会接受一个弱凭据。
            match admin::validate_env_token("ZHIWEI_BOOTSTRAP_TOKEN", &raw) {
                Ok(token) => {
                    // 刻意不打印值：它已经在部署面板里，日志不该留明文凭据。
                    tracing::info!("入网令牌来自 ZHIWEI_BOOTSTRAP_TOKEN（长期有效，不随重启变化）");
                    bootstrap_tokens.add_static(token);
                }
                Err(reason) => {
                    tracing::error!(
                        %reason,
                        "ZHIWEI_BOOTSTRAP_TOKEN 不可用，已忽略；本次改回一次性 token"
                    );
                }
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            tracing::warn!(error = %e, "读取 ZHIWEI_BOOTSTRAP_TOKEN 失败，改走一次性 token");
        }
    }
    // 没有固定令牌时才随机生成一个一次性 token 并打印（自建 / 本地开发）。
    if bootstrap_tokens.is_empty().await {
        let token = routes::BootstrapTokens::mint();
        bootstrap_tokens.add(token.clone(), 600).await;
        eprintln!("\n[BOOTSTRAP TOKEN] {token}   (valid 10 minutes)\n");
    }

    let ca_cert_pem = ca.cert_pem.clone();
    let ui_dir = if args.ui_dir.join("index.html").exists() {
        Some(args.ui_dir.clone())
    } else {
        None
    };

    // 帮助页与入网脚本都是 `include_str!` 内嵌的常量，这里只是挂到 state 上。
    let help = load_help_markdown();
    let install_script = INSTALL_NODE_SH.to_string();

    // 自建分发源（可选）：设了它，控制台生成的入网命令会自动带 `ZHIWEI_BASE_URL`，
    // 节点不再从 GitHub Releases 拉二进制。国内 / 隔离网络部署用。
    let node_base_url = std::env::var("ZHIWEI_NODE_BASE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if let Some(u) = &node_base_url {
        tracing::info!(%u, "入网命令将带 ZHIWEI_BASE_URL（节点走自建分发源）");
    }
    let state = AppState {
        storage,
        data_dir: data_dir.clone(),
        ca: Arc::new(ca),
        ca_cert_pem,
        bootstrap_tokens,
        admin_token: Arc::new(std::sync::RwLock::new(admin_token)),
        ops_public_key,
        ops_endpoint: cfg.ops_endpoint.clone(),
        nonce_cache: Arc::new(zhiwei_common::NonceCache::default()),
        server_cert_cn: cfg.server_cert_cn.clone(),
        tls_terminated_locally: !plain_http,
        ui_dir: ui_dir.clone(),
        help,
        // MCP server 调自己 REST 时用。listen 是 host:port，
        // 内部通信强制走 127.0.0.1（外面到不了）。
        mcp_base_url: format!(
            "http://127.0.0.1:{}",
            listen.rsplit(':').next().unwrap_or("8443")
        ),
        install_script,
        node_base_url,
        command_signal: tokio::sync::watch::channel(0u64).0,
    };

    if let Err(e) = alerts::seed_default_rules(&state).await {
        tracing::warn!(error = %e, "写入默认告警规则失败");
    }

    // 探针结果明细滚动保留 7 天（启动先清一次，之后每 6 小时一次）
    {
        let state = state.clone();
        tokio::spawn(async move {
            const RETENTION_NS: i64 = 7 * 24 * 60 * 60 * 1_000_000_000;
            loop {
                let cutoff = zhiwei_common::Timestamp::now().unix_nano() - RETENTION_NS;
                match state.storage.probes().cleanup_results(cutoff).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "清理过期探针结果"),
                    Err(e) => tracing::warn!(error = %e, "清理探针结果失败"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
            }
        });
    }

    // 遥测留存：原始 10 秒数据滚动保留 + 小时聚合长期保留。
    // 失败会开一条平台告警进待办，见 retention.rs 与设计文档 §8。
    retention::spawn(state.clone());

    let app = routes::router(state);

    // Serve the console when a build is present; everything not under /v1 or
    // /healthz falls through to index.html so client-side routing works.
    if ui_dir.is_some() {
        tracing::info!(ui_dir = ?args.ui_dir, "serving console");
    } else {
        tracing::warn!(ui_dir = ?args.ui_dir, "console build not found; UI disabled");
    }
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("binding {listen}"))?;

    // 明文模式：TLS 由前置边缘终结。节点身份来自请求签名而非传输层，
    // 所以这里不做任何额外鉴权——每条请求都要自证。
    if server_cert.is_none() {
        tracing::info!(%listen, "zhiwei-monitor ready (明文 HTTP，TLS 由前置边缘终结)");
        loop {
            let (stream, peer) = listener.accept().await?;
            let app = app.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = hyper::service::service_fn(
                    move |req: hyper::Request<hyper::body::Incoming>| {
                        let app = app.clone();
                        async move { app.oneshot(req).await }
                    },
                );
                if let Err(e) = AutoBuilder::new(TokioExecutor::new())
                    .serve_connection(io, svc)
                    .await
                {
                    tracing::debug!(error = %e, %peer, "connection closed");
                }
            });
        }
    }

    let server_cert = server_cert.expect("plain-http 关闭时服务端证书已生成");
    let rustls_config = tls::build_server_config(
        &server_cert.cert_pem,
        &server_cert.key_pem,
        &server_cert.ca_cert_pem,
    )?;
    let acceptor = TlsAcceptor::from(Arc::new(rustls_config));
    tracing::info!(%listen, "zhiwei-monitor ready (内置 TLS)");

    // 同类失败在窗口内只打一条：托管平台的健康检查会每分钟戳一次，
    // 每次都打完整日志会把启动信息刷没，反而看不清真正的问题。
    let last_tls_warn = Arc::new(AtomicU64::new(0));

    loop {
        let (stream, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();
        let last_tls_warn = last_tls_warn.clone();
        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    warn_tls_handshake_once(&e, peer, &last_tls_warn);
                    return;
                }
            };
            let io = TokioIo::new(tls_stream);
            // 不要求也不读取客户端证书：节点身份一律由请求签名证明
            let svc =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let app = app.clone();
                    async move { app.oneshot(req).await }
                });
            if let Err(e) = AutoBuilder::new(TokioExecutor::new())
                .serve_connection(io, svc)
                .await
            {
                tracing::debug!(error = %e, %peer, "connection closed");
            }
        });
    }
}

/// 内嵌的节点入网安装脚本。
///
/// 用 `include_str!` 编进二进制，而不是运行时读文件：Docker 运行镜像只拷
/// `zhiwei-monitor` + `ui/dist`，不带 `crates/`，运行时路径根本不存在。
/// 内嵌同时也让「单二进制 + 无外部资源」这个定位成立。
///
/// 路径直接指向仓库根的 `scripts/install-node.sh`，**不在这里留副本**：
/// 之前 `assets/` 下有一份拷贝，改了 `scripts/` 那份忘了同步，线上
/// `GET /install-node.sh` 吐的还是旧脚本（国内 `ZHIWEI_BASE_URL` 因此不生效）。
/// 单一来源后不存在漂移可能；代价是 Dockerfile 构建阶段要 `COPY scripts/`。
const INSTALL_NODE_SH: &str = include_str!("../../../scripts/install-node.sh");

/// 内嵌的帮助页 markdown（理由同上）。
const HELP_MD: &str = include_str!("../assets/help.md");

/// 帮助页内容。`locale` 固定 zh-CN——v0.1.0 阶段不翻译帮助（spec 里明确），
/// 等真的做翻译时再按 locale 返回不同 body。
fn load_help_markdown() -> crate::state::HelpContent {
    crate::state::HelpContent::new("zh-CN", HELP_MD)
}

#[cfg(test)]
mod listen_tests {
    use super::{endpoint_host_port, listen_is_loopback, ops_binary_candidates};
    use std::path::Path;

    #[test]
    fn parses_ops_endpoint() {
        assert_eq!(
            endpoint_host_port("http://127.0.0.1:8444/exec"),
            Some(("127.0.0.1".to_string(), 8444))
        );
        // 没有端口时按 ops 的默认 8444
        assert_eq!(
            endpoint_host_port("http://127.0.0.1/exec"),
            Some(("127.0.0.1".to_string(), 8444))
        );
        // 只支持本机明文 http（ops 的监听形态）
        assert_eq!(endpoint_host_port("https://127.0.0.1:8444/exec"), None);
        assert_eq!(endpoint_host_port("127.0.0.1:8444"), None);
    }

    #[test]
    fn ops_binary_candidates_include_sibling() {
        // 不设 ZHIWEI_OPS_BIN 时，候选里必须有「与自己同目录的 zhiwei-ops」，
        // 否则单独装二进制（release 包把两个二进制放一起）的部署永远拉不起命令通道
        let list = ops_binary_candidates(Some(Path::new("/usr/local/bin/zhiwei-monitor")));
        assert!(
            list.contains(&std::path::PathBuf::from("/usr/local/bin/zhiwei-ops")),
            "候选路径里应有与 monitor 同目录的 zhiwei-ops：{list:?}"
        );
    }

    #[test]
    fn flags_loopback_addresses() {
        // 这些在容器里都意味着「平台边缘够不着」，要触发告警
        for addr in [
            "127.0.0.1:8443",
            "127.0.0.1:10000",
            "127.1.2.3:8443",
            "localhost:8443",
            "[::1]:8443",
            "::1:8443",
        ] {
            assert!(listen_is_loopback(addr), "{addr} 应判为回环");
        }
    }

    #[test]
    fn does_not_flag_reachable_addresses() {
        // 这些是本机之外能连上的绑定，不该告警——否则自建用户会被误导
        for addr in [
            "0.0.0.0:8443",
            "0.0.0.0:10000",
            "[::]:8443",
            "192.168.1.10:8443",
            "10.0.0.5:8443",
            "monitor.internal:8443",
        ] {
            assert!(!listen_is_loopback(addr), "{addr} 不应判为回环");
        }
    }
}
