//! `ZhiWei` monitor-server (data plane).
//!
//! Endpoints:
//!   POST /v1/enroll      — bootstrap token + node Ed25519 public key → `node_id`
//!   POST /v1/telemetry   — signed request (Ed25519), protobuf `TelemetryBatch`
//!   GET  /healthz        — liveness
//!
//! Node identity is carried by request signatures (see `zhiwei_common::auth`),
//! not client certificates: managed platforms terminate TLS at the edge and do
//! not forward client certificates to containers, so the service must work
//! under "plain HTTP + edge TLS" (`--plain-http`). Self-hosted deployments can
//! still use the built-in rustls to terminate TLS directly (server certificate
//! only; no client certificate required).
//!
//! hyper-util's auto builder negotiates HTTP/1.1 or HTTP/2 per connection.


#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]
// `multiple_crate_versions` flags transitive deps (e.g. ed25519-dalek pulls
// `rand_core` 0.10 while `rand` 0.8 pulls 0.6; sqlx pulls `thiserror` 2 while
// we pin 1). Not actionable from project code — pinned by upstream crates.
#![allow(clippy::multiple_crate_versions)]

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
mod control_channel;
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

    /// Serve plain HTTP; TLS is terminated by the upstream edge (`PaaS` / reverse proxy).
    /// Node identity comes from request signatures, not the transport layer, so
    /// plain transport does not weaken authentication.
    /// The env var accepts 1/0/true/false/yes/no/on/off (`PaaS` panels commonly use 1).
    ///
    /// Bare `--plain-http` (no value) is equivalent to `--plain-http 1` — when docs
    /// say "add --plain-http" this is what they mean. Pass 0/false to explicitly
    /// turn it off.
    /// Only when not passed at all do we auto-detect from common `PaaS` env vars:
    /// `RENDER` / `RAILWAY_*` / `NORTHFLANK_*` / `DYNO` (Heroku) default to true.
    #[arg(
        long,
        env = "ZHIWEI_PLAIN_HTTP",
        value_parser = clap::builder::BoolishValueParser::new(),
        default_missing_value = "true",
        num_args = 0..=1,
    )]
    plain_http: Option<bool>,
}

/// Whether we are running in a "`PaaS` that terminates TLS at the edge" environment:
/// Render / Railway / Northflank / Heroku.
    /// Only used as a default when `--plain-http` is not explicitly configured;
    /// self-hosted hosts are not affected.
    ///
    /// Note: this is just a convenience layer. Platforms like Northflank neither
    /// inject `PORT` nor have a stable env var prefix (in practice `listen` falls
    /// back to the default loopback), so auto-detection does not cover them —
    /// you must explicitly set `ZHIWEI_PLAIN_HTTP=1`. An explicit `--plain-http`
    /// always wins over auto-detection.
fn detect_paas_edge_terminates_tls() -> bool {
    // Render always injects `RENDER=true`.
    if std::env::var("RENDER").ok().as_deref() == Some("true") {
        return true;
    }
    // Railway injects RAILWAY_* (any one is enough to consider it Railway).
    if std::env::vars().any(|(k, _)| k.starts_with("RAILWAY_")) {
        return true;
    }
    // Northflank has no single stable marker; combine a few common variables.
    if std::env::vars().any(|(k, _)| k.starts_with("NORTHFLANK_")) {
        return true;
    }
    // Heroku / older PaaS.
    if std::env::var("DYNO").is_ok() {
        return true;
    }
    false
}

/// Whether the listen address is a loopback address.
///
/// Plain string check — the input is `host:port`, no need to actually parse an IP.
/// IPv6 looks like `[::1]:8443`, so strip the square brackets before comparing.
fn listen_is_loopback(listen: &str) -> bool {
    let host = match listen.rsplit_once(':') {
        Some((h, _)) => h,
        None => listen,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "localhost" | "::1") || host.starts_with("127.")
}

/// Alert interval (seconds) for repeated TLS handshake failures.
///
/// Managed platforms' health checks poke containers at a fixed cadence; if the config
/// is wrong, every probe triggers a log line. Without a window the startup info gets
/// scrolled away and the real cause becomes invisible.
const TLS_WARN_INTERVAL_SECS: u64 = 60;

/// Emit one TLS handshake-failure warning per window.
///
/// `InvalidContentType` is recognized specifically: the meaning is "the peer sent
/// plain HTTP, but this process is parsing it as TLS" — on managed platforms the
/// typical cause is "didn't enable `--plain-http`". The raw rustls message gives no
/// hint of this, so the next action is called out here.
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

    // Already reported within the window, or lost the CAS race (another task just reported) → demote to debug
    if now.saturating_sub(prev) < TLS_WARN_INTERVAL_SECS
        || last_warn
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        tracing::debug!(error = %err, %peer, "TLS handshake failed (similar warning throttled)"); // already has context
        return;
    }

    if err.to_string().contains("InvalidContentType") {
        tracing::warn!(
            error = %err,
            %peer,
            "TLS handshake failed: the peer sent plain HTTP, but this process is handling it as TLS.\
             When deployed behind a managed platform (TLS already terminated at the edge),\
             set ZHIWEI_PLAIN_HTTP=1 and redeploy;\
             for self-hosted deployments, check whether the client used http:// to connect to an https port."
        );
    } else {
        tracing::warn!(error = %err, %peer, "TLS handshake failed");
    }
}

/// Extract `(host, port)` from the ops endpoint (`http://127.0.0.1:8444/exec`).
fn endpoint_host_port(endpoint: &str) -> Option<(String, u16)> {
    let authority = endpoint.strip_prefix("http://")?;
    let host_port = authority.split('/').next()?;
    match host_port.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((host_port.to_string(), 8444)),
    }
}

/// Someone listening on the port means "running"; retry up to `attempts` times (150 ms apart).
async fn wait_for_port(host: &str, port: u16, attempts: u32) -> bool {
    for _ in 0..attempts {
        if tokio::net::TcpStream::connect((host, port)).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

/// Candidate paths for the ops-server binary: `ZHIWEI_OPS_BIN` first, then a sibling of the monitor.
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

/// Fallback startup for the command channel — same effect as the `start_ops` block in
/// `scripts/docker-entrypoint.sh`, but inlined into the monitor process so that
/// "single-binary" deployments also get write operations.
///
/// Boundaries unchanged: ops is still an independent process, the signing private
/// key lives only inside it, and monitor reads `ops.pub`.
///
/// - `ZHIWEI_OPS_DISABLE=1` → explicitly declare "data plane only"; do nothing.
/// - Port is already listening → external instance is up (entrypoint / dev.sh / self-hosted unit).
/// - `zhiwei-ops` not found → print an actionable hint instead of pretending success.
async fn ensure_ops_control_plane(data_dir: &Path, endpoint: &str) {
    if ops_disabled_by_env() {
        tracing::info!("ZHIWEI_OPS_DISABLE=1: not starting ops-server, command channel unavailable (data plane still running)");
        return;
    }
    let Some((host, port)) = endpoint_host_port(endpoint) else {
        tracing::warn!(%endpoint, "ops endpoint is not in http://host:port form, skipping fallback startup");
        return;
    };
    // External instance may already be running (entrypoint / dev.sh / self-hosted unit).
    // Give it a short retry window so a few-millisecond startup gap doesn't cause a
    // second ops to be launched.
    if wait_for_port(&host, port, 8).await {
        tracing::debug!(%endpoint, "ops-server already running (command channel available)");
        return;
    }

    let Some(bin) = locate_ops_binary() else {
        warn_missing_ops_binary(endpoint);
        return;
    };

    spawn_ops_server(&bin, data_dir, &host, port, endpoint).await;
}

fn ops_disabled_by_env() -> bool {
    std::env::var("ZHIWEI_OPS_DISABLE").ok().as_deref() == Some("1")
}

fn warn_missing_ops_binary(endpoint: &str) {
    tracing::warn!(
        %endpoint,
        "Command channel unavailable: no process is listening at this address, and the zhiwei-ops binary was not found.\
         Place zhiwei-ops in the same directory as zhiwei-monitor (or set ZHIWEI_OPS_BIN to its path) and retry;\
         to run the data plane only, set ZHIWEI_OPS_DISABLE=1 explicitly."
    );
}

/// Spawn the ops process and wait (briefly) for its port to accept connections, so the
/// command channel is usable as soon as monitor is ready.
async fn spawn_ops_server(
    bin: &Path,
    data_dir: &Path,
    host: &str,
    port: u16,
    endpoint: &str,
) {
    let spawned = tokio::process::Command::new(bin)
        .env("ZHIWEI_DATA_DIR", data_dir)
        // Let the spawned ops listen on the address monitor actually connects to
        // (ops defaults to 8444; if ops_endpoint is overridden in config, don't gamble on the default).
        .env("ZHIWEI_OPS_LISTEN", format!("{host}:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    match spawned {
        Ok(child) => {
            tracing::info!(
                bin = %bin.display(),
                pid = child.id().unwrap_or(0),
                "Started ops-server (control plane); signing key only in its process"
            );
            await_ops_ready(host, port, endpoint).await;
            // Let tokio's SIGCHLD handler reap it; no wait() here (monitor runs forever).
            drop(child);
        }
        Err(e) => tracing::warn!(bin = %bin.display(), error = %e, "failed to spawn ops-server"),
    }
}

/// Wait for the freshly-spawned ops to become "ready".
///
/// The test is that the port is connectable, not that `ops.pub` exists: ops opens the DB
/// and runs migrations before binding, so the port being up simultaneously means:
///   ① `ops.pub` has been written to disk — monitor reads it once at startup and caches
///      it; reading too soon gives new nodes an empty public key and the command channel
///      silently fails.
///   ② Migrations have finished — two processes running `CREATE TABLE` on the same
///      `SQLite` will collide (verified in practice, monitor fails to start: `table
///      nodes already exists`).
async fn await_ops_ready(host: &str, port: u16, endpoint: &str) {
    if wait_for_port(host, port, 60).await {
        tracing::info!(%endpoint, "ops-server ready (command channel available)");
    } else {
        tracing::warn!(%endpoint, "ops-server did not start within 9 seconds, command channel may be unavailable");
    }
}

/// Values resolved from CLI args + config that are needed throughout startup.
struct StartupContext {
    args: Args,
    cfg: config::MonitorConfig,
    data_dir: PathBuf,
    listen: String,
    plain_http: bool,
}

/// External resources + credentials gathered before [`AppState`] is assembled.
struct RuntimeResources {
    storage: zhiwei_storage::Storage,
    ca: ca::Ca,
    server_cert: Option<tls::IssuedCert>,
    admin_token: String,
    bootstrap_tokens: Arc<routes::BootstrapTokens>,
    ops_public_key: String,
    ui_dir: Option<PathBuf>,
    help: crate::state::HelpContent,
    node_base_url: Option<String>,
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,zhiwei=debug")),
        )
        .init();
}

/// Parse args + config, ensure the data dir, and resolve the listen / plain-HTTP policy.
async fn load_startup_context() -> anyhow::Result<StartupContext> {
    let args = Args::parse();
    let cfg = config::MonitorConfig::load(&args.config).context("loading config")?;
    let data_dir = args.data_dir.clone().unwrap_or_else(|| cfg.data_dir.clone());
    let listen = args.listen.clone().unwrap_or_else(|| cfg.listen.clone());

    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;

    // The command channel (delete container / pull logs / restart host ... in the console)
    // is signed by ops-server. The image's entrypoint and dev.sh start it first; only
    // "running the binary directly" deployments have no place for a second process,
    // and every console write operation will report "ops-server unavailable".
    // Here we add a fallback: if it's unreachable and a zhiwei-ops is right next to us, start it ourselves.
    ensure_ops_control_plane(&data_dir, &cfg.ops_endpoint).await;

    // When neither --plain-http nor ZHIWEI_PLAIN_HTTP is set explicitly, auto-detect
    // based on the PaaS environment. An explicit false always overrides auto-detection —
    // self-hosted deployments keep TLS terminated locally.
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

    // Binding to loopback is correct in "self-hosted + local reverse proxy" setups,
    // but almost always wrong inside containers: the platform edge forwards traffic
    // from outside the container, can't reach the loopback, and you'll see health
    // checks fail / 502s. This pitfall is hard to spot from logs alone (the port
    // looks right), so we announce it explicitly.
    if listen_is_loopback(&listen) {
        tracing::warn!(
            %listen,
            "Listening on a loopback address: nothing outside this host (managed platform edge, browser, other hosts) can connect. \
             On managed platforms, set ZHIWEI_LISTEN=0.0.0.0:<port>, or just set PORT=<port> (this program auto-binds 0.0.0.0 when PORT is detected)."
        );
    }

    Ok(StartupContext {
        args,
        cfg,
        data_dir,
        listen,
        plain_http,
    })
}

async fn init_storage_and_ca(data_dir: &Path) -> anyhow::Result<(zhiwei_storage::Storage, ca::Ca)> {
    let db_path = data_dir.join("monitor.db");
    let storage = zhiwei_storage::Storage::open(&db_path)
        .await
        .context("opening sqlite storage")?;
    let ca = ca::Ca::load_or_init(data_dir)
        .await
        .context("initializing CA")?;
    Ok((storage, ca))
}

fn init_server_cert(
    context: &StartupContext,
    ca: &ca::Ca,
) -> anyhow::Result<Option<tls::IssuedCert>> {
    // In plain-HTTP mode TLS is terminated by the upstream edge; no local server cert needed
    if context.plain_http {
        return Ok(None);
    }
    Ok(Some(
        tls::ensure_server_cert(ca, &context.data_dir, &context.cfg.server_cert_cn)
            .context("ensuring server cert")?,
    ))
}

/// ops public key is written by ops-server on its first startup; without it enroll
/// will return an empty string, and nodes will reject every command (security default).
async fn load_ops_public_key(data_dir: &Path) -> String {
    let ops_pub_path = data_dir.join("ops.pub");
    let key = tokio::fs::read_to_string(&ops_pub_path)
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if key.is_empty() {
        tracing::warn!(
            "{} not found; nodes will not be able to verify command signatures. Will be generated after starting zhiwei-ops.",
            ops_pub_path.display()
        );
    }
    key
}

fn log_admin_token(data_dir: &Path, admin_token: &str, source: admin::TokenSource) {
    match source {
        // Generated this time: printed once right here. On managed-platform free tiers
        // with no console shell access, this log line is the only way out (the log
        // viewer is free to read).
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
        // From env var: token is already in the deploy panel; don't reprint (logs must not hold plaintext credentials).
        admin::TokenSource::Env => {
            tracing::info!(
                "admin token ready (sourced from ZHIWEI_ADMIN_TOKEN; not written to disk, does not change across restarts)"
            );
        }
        // From file: normal path for self-hosted / persistent-volume deployments.
        admin::TokenSource::File => {
            tracing::info!(
                ?data_dir,
                "admin token ready (use `cat <data-dir>/admin.token`)"
            );
        }
    }
}

/// Fixed enrollment token takes priority; otherwise mint a random one-time token and print it.
fn init_bootstrap_tokens() -> Arc<routes::BootstrapTokens> {
    let bootstrap_tokens = Arc::new(routes::BootstrapTokens::default());
    // Fixed enrollment token takes priority: managed platforms (free tiers have no
    // shell, no persistent volume) use it to replace the "grab a one-time 10-minute
    // token from startup logs" flow.
    match std::env::var("ZHIWEI_BOOTSTRAP_TOKEN") {
        Ok(raw) => {
            // Reject anything too short, same reasoning as admin token: it has long
            // validity and is exposed on the public edge, so a short one could be
            // brute-forced and used to register fake nodes. After rejection we still
            // generate a one-time token as fallback — service won't fail to start —
            // but we never accept a weak credential.
            match admin::validate_env_token("ZHIWEI_BOOTSTRAP_TOKEN", &raw) {
                Ok(token) => {
                    // Intentionally do not print the value: it's already in the deploy
                    // panel; logs must not hold plaintext credentials.
                    tracing::info!("Enrollment token from ZHIWEI_BOOTSTRAP_TOKEN (persistent, survives restarts)");
                    bootstrap_tokens.add_static(token);
                }
                Err(reason) => {
                    tracing::error!(
                        %reason,
                        "ZHIWEI_BOOTSTRAP_TOKEN is unusable; ignored, falling back to one-time token"
                    );
                }
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read ZHIWEI_BOOTSTRAP_TOKEN, falling back to one-time token");
        }
    }
    // Only when no fixed token is set do we mint a random one-time token and print it (self-hosted / local dev).
    if bootstrap_tokens.is_empty() {
        let token = routes::BootstrapTokens::mint();
        bootstrap_tokens.add(token.clone(), 600);
        eprintln!("\n[BOOTSTRAP TOKEN] {token}   (valid 10 minutes)\n");
    }
    bootstrap_tokens
}

async fn init_runtime_resources(context: &StartupContext) -> anyhow::Result<RuntimeResources> {
    let (storage, ca) = init_storage_and_ca(&context.data_dir).await?;
    let server_cert = init_server_cert(context, &ca)?;
    let ops_public_key = load_ops_public_key(&context.data_dir).await;

    let (admin_token, admin_token_source) = admin::load_or_init(&context.data_dir)
        .await
        .context("initializing admin token")?;
    log_admin_token(&context.data_dir, &admin_token, admin_token_source);
    let bootstrap_tokens = init_bootstrap_tokens();

    let ui_dir = if context.args.ui_dir.join("index.html").exists() {
        Some(context.args.ui_dir.clone())
    } else {
        None
    };

    // Self-hosted distribution base URL (optional): when set, console-generated
    // enrollment commands automatically include `ZHIWEI_BASE_URL`, so nodes no
    // longer pull the binary from GitHub Releases. For China / air-gapped deployments.
    let node_base_url = std::env::var("ZHIWEI_NODE_BASE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if let Some(u) = &node_base_url {
        tracing::info!(%u, "Enrollment command will include ZHIWEI_BASE_URL (nodes use self-hosted distribution)");
    }

    Ok(RuntimeResources {
        storage,
        ca,
        server_cert,
        admin_token,
        bootstrap_tokens,
        ops_public_key,
        ui_dir,
        // The help page and enrollment script are `include_str!`-embedded constants; here we just attach them to state.
        help: load_help_markdown(),
        node_base_url,
    })
}

/// Assemble [`AppState`], returning the TLS cert alongside it (still needed to serve).
fn build_app_state(
    context: &StartupContext,
    res: RuntimeResources,
) -> (AppState, Option<tls::IssuedCert>) {
    let ca_cert_pem = res.ca.cert_pem.clone();
    let state = AppState {
        storage: res.storage,
        data_dir: context.data_dir.clone(),
        ca: Arc::new(res.ca),
        ca_cert_pem,
        bootstrap_tokens: res.bootstrap_tokens,
        admin_token: Arc::new(std::sync::RwLock::new(res.admin_token)),
        ops_public_key: res.ops_public_key,
        ops_endpoint: context.cfg.ops_endpoint.clone(),
        nonce_cache: Arc::new(zhiwei_common::NonceCache::default()),
        server_cert_cn: context.cfg.server_cert_cn.clone(),
        tls_terminated_locally: !context.plain_http,
        ui_dir: res.ui_dir,
        help: res.help,
        // Used by the MCP server to call its own REST. `listen` is host:port,
        // and internal traffic is forced to 127.0.0.1 (not reachable from outside).
        mcp_base_url: format!(
            "http://127.0.0.1:{}",
            context.listen.rsplit(':').next().unwrap_or("8443")
        ),
        install_script: INSTALL_NODE_SH.to_string(),
        node_base_url: res.node_base_url,
        command_signal: tokio::sync::watch::channel(0u64).0,
        control_polls: Arc::new(control_channel::ControlPolls::new()),
        started_at_ms: zhiwei_common::Timestamp::now().unix_nano() / 1_000_000,
    };
    (state, res.server_cert)
}

/// Seed default rules, then spawn the recurring background tasks (probe-result cleanup,
/// telemetry retention, node-offline watcher).
async fn start_background_tasks(state: &AppState) {
    if let Err(e) = alerts::seed_default_rules(state).await {
        tracing::warn!(error = %e, "Failed to write default alert rules");
    }

    // Probe result detail rolling retention of 7 days (one sweep at startup, then every 6 hours)
    {
        let state = state.clone();
        tokio::spawn(async move {
            const RETENTION_NS: i64 = 7 * 24 * 60 * 60 * 1_000_000_000;
            loop {
                let cutoff = zhiwei_common::Timestamp::now().unix_nano() - RETENTION_NS;
                match state.storage.probes().cleanup_results(cutoff).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "Cleaned up expired probe results"),
                    Err(e) => tracing::warn!(error = %e, "Failed to clean up probe results"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
            }
        });
    }

    // Telemetry retention: raw 10-second data rolling + hourly aggregates kept long-term.
    // On failure a platform-level alert enters the todo queue; see retention.rs and design doc §8.
    retention::spawn(state.clone());

    // Node-offline alerting: every 30s, scan nodes' last_seen_unix_nano; if no
    // report within 60s, the node is considered offline — write a real alert with
    // source='node_offline' and notify. See alerts::spawn_node_liveness_watcher.
    alerts::spawn_node_liveness_watcher(state.clone());
}

/// Serve forever over plain HTTP; TLS is terminated by the upstream edge.
async fn serve_plain_http(listener: tokio::net::TcpListener, app: axum::Router) -> anyhow::Result<()> {
    // Node identity comes from request signatures, not the transport layer, so we do no
    // extra authentication here — every request has to prove itself.
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

/// Serve forever over the built-in rustls TLS acceptor.
async fn serve_tls(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    server_cert: tls::IssuedCert,
) -> anyhow::Result<()> {
    let rustls_config = tls::build_server_config(
        &server_cert.cert_pem,
        &server_cert.key_pem,
        &server_cert.ca_cert_pem,
    )?;
    let acceptor = TlsAcceptor::from(Arc::new(rustls_config));

    // One log line per similar failure within the window: managed-platform health
    // checks probe once per minute; logging the full message every time would
    // scroll the startup info away and obscure the real issue.
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
            // We neither require nor read client certificates: node identity is always proven by request signatures
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let context = load_startup_context().await?;
    let resources = init_runtime_resources(&context).await?;
    let (state, server_cert) = build_app_state(&context, resources);
    start_background_tasks(&state).await;

    let app = routes::router(state);

    // Serve the console when a build is present; everything not under /v1 or
    // /healthz falls through to index.html so client-side routing works.
    if context.args.ui_dir.join("index.html").exists() {
        tracing::info!(ui_dir = ?context.args.ui_dir, "serving console");
    } else {
        tracing::warn!(ui_dir = ?context.args.ui_dir, "console build not found; UI disabled");
    }
    let listener = tokio::net::TcpListener::bind(&context.listen)
        .await
        .with_context(|| format!("binding {}", context.listen))?;

    // Plain-HTTP mode: TLS is terminated by the upstream edge.
    if server_cert.is_none() {
        tracing::info!(listen = %context.listen, "zhiwei-monitor ready (plain HTTP, TLS terminated at edge)");
        return serve_plain_http(listener, app).await;
    }

    tracing::info!(listen = %context.listen, "zhiwei-monitor ready (built-in TLS)");
    let server_cert = server_cert.expect("server cert already generated when plain-http is off");
    serve_tls(listener, app, server_cert).await
}

/// Embedded node enrollment install script.
///
/// Embedded via `include_str!` rather than read at runtime: Docker runtime images
/// only copy `zhiwei-monitor` + `ui/dist` and do not include `crates/` — the
/// runtime path simply does not exist. Embedding also preserves the
/// "single-binary, no external resources" positioning.
///
/// The path points directly to the repo-root file, **no duplicate is kept here**:
/// previously there was a copy under `assets/`; changing the `scripts/` one and
/// forgetting to sync meant the deployed `GET /install-node.sh` still served the
/// old script (so `ZHIWEI_BASE_URL` did not take effect in China). With a single
/// source, drift is impossible; the trade-off is that the Dockerfile build stage
/// must `COPY scripts/`.
const INSTALL_NODE_SH: &str = include_str!("../../../scripts/install-node.sh");

/// Embedded help-page markdown (same reasoning).
const HELP_MD_ZH: &str = include_str!("../assets/help.md");
const HELP_MD_EN: &str = include_str!("../assets/help.en.md");

/// Help page content. Both zh-CN and en-US are loaded at startup.
fn load_help_markdown() -> crate::state::HelpContent {
    crate::state::HelpContent::new([
        ("zh-CN".to_string(), HELP_MD_ZH.to_string()),
        ("en-US".to_string(), HELP_MD_EN.to_string()),
    ])
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
        // No port → use ops default 8444
        assert_eq!(
            endpoint_host_port("http://127.0.0.1/exec"),
            Some(("127.0.0.1".to_string(), 8444))
        );
        // Only plain http on localhost is supported (ops' listen form)
        assert_eq!(endpoint_host_port("https://127.0.0.1:8444/exec"), None);
        assert_eq!(endpoint_host_port("127.0.0.1:8444"), None);
    }

    #[test]
    fn ops_binary_candidates_include_sibling() {
        // When ZHIWEI_OPS_BIN is unset, candidates must include a "zhiwei-ops next to self";
        // otherwise, deployments that install just the binary (release package puts both binaries together)
        // can never spin up the command channel
        let list = ops_binary_candidates(Some(Path::new("/usr/local/bin/zhiwei-monitor")));
        assert!(
            list.contains(&std::path::PathBuf::from("/usr/local/bin/zhiwei-ops")),
            "Expected zhiwei-ops in the same dir as monitor in candidate paths: {list:?}"
        );
    }

    #[test]
    fn flags_loopback_addresses() {
        // In containers, all of these mean "the platform edge can't reach it" — must trigger an alert
        for addr in [
            "127.0.0.1:8443",
            "127.0.0.1:10000",
            "127.1.2.3:8443",
            "localhost:8443",
            "[::1]:8443",
            "::1:8443",
        ] {
            assert!(listen_is_loopback(addr), "{addr} should be classified as loopback");
        }
    }

    #[test]
    fn does_not_flag_reachable_addresses() {
        // These binds are reachable from beyond localhost; should NOT alert — otherwise self-hosted users would be misled
        for addr in [
            "0.0.0.0:8443",
            "0.0.0.0:10000",
            "[::]:8443",
            "192.168.1.10:8443",
            "10.0.0.5:8443",
            "monitor.internal:8443",
        ] {
            assert!(!listen_is_loopback(addr), "{addr} should not be classified as loopback");
        }
    }
}
