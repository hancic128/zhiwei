//! `ZhiWei` node-agent.
//!
//! First-run flow (enroll):
//!   1. Generate the Ed25519 signing key under `<state>/signing.key` (0600)
//!   2. POST /v1/enroll with bootstrap-token + that public key + hostname/labels
//!   3. Persist `<state>/node.id` (+ `<state>/ops.pub` for command verification)
//!   4. From now on, sign every request with the key — no client certificate
//!
//! Steady-state:
//!   - Every `interval` seconds, build a `TelemetryBatch` (CPU/mem/disk/net)
//!     and POST it to /v1/telemetry.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]
// `multiple_crate_versions` flags transitive deps (e.g. ed25519-dalek pulls
// `rand_core` 0.10 while `rand` 0.8 pulls 0.6). Not actionable from project
// code — pinned by upstream crates.
#![allow(clippy::multiple_crate_versions)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use sysinfo::{Networks, System};
use tracing_subscriber::EnvFilter;
use zhiwei_common::KeyPair as EdKeyPair;
use zhiwei_proto::common::{EnrollRequest, EnrollResponse};
use zhiwei_proto::telemetry::{
    DiskMount, HostInfo, InterfaceInfo, InventoryReport, IpAddress, Metric, NetworkInterface,
    ProcessInfo, ProcessSnapshot, TelemetryBatch,
};

/// Unix seconds when the node-agent process started — written once at the top of
/// `main()`, then read by `build_host_info`. We don't pass the time through from
/// main down to the inventory call site — `build_inventory` is called from the
/// scheduled task — threading it through would touch the signature/container/process
/// call chains.
/// The console uses this field to display "deployment time"; old agents didn't
/// write this field, deserialization yields 0, and the UI shows 0 as unknown.
static AGENT_STARTED_AT_UNIX_SECONDS: OnceLock<u64> = OnceLock::new();

mod certs;
mod control;
mod docker;
mod http;
mod probes;

#[derive(Parser, Debug)]
#[command(name = "zhiwei-node", about = "ZhiWei node-agent", version)]
struct Args {
    /// Monitor URL (e.g. <https://127.0.0.1:8443>)
    #[arg(long, env = "ZHIWEI_MONITOR_URL")]
    monitor: String,

    /// Bootstrap token (first-run only; ignored if already enrolled)
    #[arg(long, env = "ZHIWEI_BOOTSTRAP_TOKEN")]
    bootstrap_token: Option<String>,

    /// State directory for key/cert/node-id persistence
    #[arg(long, default_value = "data/node", env = "ZHIWEI_STATE_DIR")]
    state_dir: PathBuf,

    /// Telemetry interval (seconds).
    ///
    /// Default 5; env `ZHIWEI_INTERVAL` overrides (the KEY matches what install-node.sh
    /// writes to /etc/zhiwei/node.env — to change telemetry frequency only edit the
    /// env file, then `systemctl restart zhiwei-node`; no need to touch the unit).
    #[arg(long, default_value_t = 5, env = "ZHIWEI_INTERVAL")]
    interval: u64,

    /// Inventory (host info / containers / processes / cert snapshots) reporting interval (seconds)
    #[arg(long, default_value_t = 300, env = "ZHIWEI_INVENTORY_INTERVAL")]
    inventory_interval: u64,

    /// Cert scan globs, comma-separated; empty uses built-in default locations
    #[arg(long, env = "ZHIWEI_CERT_GLOBS", default_value = "")]
    cert_globs: String,

    /// Node name reported to monitor. Empty uses system hostname,
    /// but placeholder values like bogon / localhost automatically switch to a better source.
    #[arg(long, env = "ZHIWEI_NODE_NAME", default_value = "")]
    node_name: String,

    /// Alias set during enrollment (≤10 characters).
    ///
    /// Only effective on **first enroll**: once a node has a node.id it doesn't enroll
    /// again, so repeating the setting has no effect. For already-enrolled machines,
    /// change it in the console (or delete the state directory and re-enroll).
    #[arg(long, env = "ZHIWEI_NODE_ALIAS", default_value = "")]
    alias: String,

    /// Tags set during enrollment; whitespace / comma separated (up to 10, each ≤24 characters).
    /// Also only effective on first enroll.
    #[arg(long, env = "ZHIWEI_NODE_TAGS", default_value = "")]
    tags: String,

    /// Override the monitor CA used for pinning; pass a file path.
    ///
    /// Default (empty) uses the CA delivered by enroll — this is the right thing for
    /// self-hosted TLS deployments. When deployed behind platforms like Render / Railway
    /// that terminate TLS at the edge, enroll won't deliver a CA (the local CA is
    /// unrelated to the edge's real certificate), and the node automatically falls
    /// back to system roots.
    ///
    /// Pass `-` to **force no pin, use system roots**: intended for machines that once
    /// enrolled against a self-hosted monitor and still have the old CA on disk but
    /// now point at a managed platform, so you don't need to delete files and re-enroll.
    #[arg(long, env = "ZHIWEI_MONITOR_CA", default_value = "")]
    monitor_ca: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,zhiwei=debug")),
        )
        .init();

    // Record the startup timestamp immediately; inventory reads it repeatedly. Any
    // failure later in main that causes a restart rewrites it, so even a "looks
    // like the same process" restart stays consistent.
    let _ = AGENT_STARTED_AT_UNIX_SECONDS.set(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );

    let args = Args::parse();
    tokio::fs::create_dir_all(&args.state_dir).await?;
    let mut state = NodeState::load_or_init(&args.state_dir).await?;
    if !args.node_name.trim().is_empty() {
        state.node_name = args.node_name.trim().to_string();
    }
    tracing::info!(node_name = %state.node_name, "Node name");

    // --monitor-ca: explicitly override the CA used for pinning; `-` means force system roots
    match args.monitor_ca.trim() {
        "" => {}
        "-" => {
            state.no_ca_pin = true;
            state.ca_cert_pem = None;
            tracing::info!("--monitor-ca=-: no monitor CA pin, using system root");
        }
        path => {
            let pem = tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("reading --monitor-ca specified CA: {path}"))?;
            state.ca_cert_pem = Some(pem);
            state.no_ca_pin = false;
            tracing::info!(path, "Using --monitor-ca specified CA to pin monitor");
        }
    }

    if state.enrolled() {
        tracing::info!(node_id = %state.node_id.clone().unwrap_or_default(), "already enrolled");
    } else {
        let token = args
            .bootstrap_token
            .clone()
            .context("--bootstrap-token (or ZHIWEI_BOOTSTRAP_TOKEN) required for first run")?;
        tracing::info!("first run: enrolling with monitor");
        let resp = enroll(
            &args.monitor,
            &token,
            &mut state,
            args.alias.as_str(),
            args.tags.as_str(),
        )
        .await?;
        tracing::info!(node_id = %resp.node_id, "enrollment complete");
    }

    let node_id = state
        .node_id
        .clone()
        .context("node id missing after enrollment")?;
    let state = std::sync::Arc::new(state);
    let monitor = args.monitor.clone();

    // Control channel and console polling run in parallel: pull command → verify → run → sign receipt
    {
        let s = state.clone();
        let m = monitor.clone();
        let id = node_id.clone();
        tokio::spawn(async move { control::run_poll_loop(m, s, id).await });
    }

    // Service liveness loop: pull probe config → execute when due → report results
    {
        let s = state.clone();
        let m = monitor.clone();
        let id = node_id.clone();
        tokio::spawn(async move { probes::run_loop(m, s, id).await });
    }

    // Cert scan locations: the **union** of the default globs and the user-provided
    // ZHIWEI_CERT_GLOBS / --cert-globs. The old behavior was "user input fully
    // replaces defaults" — but the default globs already cover Let's Encrypt,
    // nginx self-managed dirs and other common locations, and 99% of the time users
    // pass custom paths to "add a few more places", not "I don't want the defaults".
    // The replace semantics burned us once: a Silicon Valley node had certs in
    // /root/nginx-certs/, and after configuring --cert-globs the default locations
    // stopped being scanned. Same-path dedup is handled by `by_path` inside
    // `certs::scan_entries` (first writer wins); we don't dedup here.
    let cert_globs: Vec<String> = certs::merge_globs(&args.cert_globs);

    telemetry_loop(
        &monitor,
        state,
        &node_id,
        args.interval,
        args.inventory_interval,
        &cert_globs,
    )
    .await
}

/// Run the telemetry + inventory loop. sysinfo `System` and `Networks` are created once
/// and reused (`refresh_all` instead of `new_all` each round) to avoid O(n) per-tick cost.
async fn telemetry_loop(
    monitor: &str,
    state: std::sync::Arc<NodeState>,
    node_id: &str,
    interval: u64,
    inventory_interval: u64,
    cert_globs: &[String],
) -> anyhow::Result<()> {
    // sysinfo needs two samples with a gap between them to compute CPU usage.
    // Create once and reuse — refresh_all is much cheaper than new_all.
    let mut sys = System::new_all();
    sys.refresh_all();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_all();

    let mut networks = Networks::new_with_refreshed_list();

    let interval = interval.max(1);
    let inventory_interval = inventory_interval.max(interval);
    let mut next_inventory = std::time::Instant::now();

    loop {
        run_telemetry_tick(
            &mut sys,
            &mut networks,
            monitor,
            &state,
            node_id,
            cert_globs,
            interval,
            inventory_interval,
            &mut next_inventory,
        )
        .await;
    }
}

#[allow(clippy::cognitive_complexity)]
async fn run_telemetry_tick(
    sys: &mut System,
    networks: &mut Networks,
    monitor: &str,
    state: &std::sync::Arc<NodeState>,
    node_id: &str,
    cert_globs: &[String],
    interval: u64,
    inventory_interval: u64,
    next_inventory: &mut std::time::Instant,
) {
    sys.refresh_all();
    networks.refresh(true);

    match collect_and_send(monitor, state, node_id, sys, networks).await {
        Ok(()) => tracing::debug!("batch ok"),
        Err(e) => tracing::warn!(error = %e, "batch failed"),
    }

    let forced = state
        .inventory_due
        .swap(false, std::sync::atomic::Ordering::Relaxed);
    if forced || std::time::Instant::now() >= *next_inventory {
        match send_inventory(monitor, state, node_id, cert_globs, sys).await {
            Ok(()) => {
                *next_inventory =
                    std::time::Instant::now() + Duration::from_secs(inventory_interval);
                tracing::debug!("inventory ok");
            }
            Err(e) => tracing::warn!(error = %e, "inventory failed"),
        }
    }

    tokio::time::sleep(Duration::from_secs(interval)).await;
}

struct NodeState {
    state_dir: PathBuf,
    #[allow(dead_code)]
    signing_key: EdKeyPair, // 32-byte ed25519, used to sign telemetry batches
    node_id: Option<String>,
    /// monitor CA certificate (used for pinning on self-hosted TLS deployments; falls back to system roots when absent)
    ca_cert_pem: Option<String>,
    /// Force no monitor-CA pinning (`--monitor-ca -`); always use system roots
    no_ca_pin: bool,
    /// ops control-plane public key (base64). Received at enroll and persisted long-term; used to verify command signatures.
    ops_public_key: Option<String>,
    /// Node name reported to monitor (after resolution)
    node_name: String,
    /// Set when the console requests "re-snapshot now"; the next main-loop tick sends an inventory
    inventory_due: std::sync::atomic::AtomicBool,
}

impl NodeState {
    async fn load_or_init(state_dir: &Path) -> anyhow::Result<Self> {
        let signing_key_path = state_dir.join("signing.key");
        let id_path = state_dir.join("node.id");
        let ca_path = state_dir.join("ca.crt.pem");

        let signing_bytes = if signing_key_path.exists() {
            tokio::fs::read(&signing_key_path).await?
        } else {
            let kp = EdKeyPair::generate();
            let bytes = kp.to_bytes();
            tokio::fs::write(&signing_key_path, bytes).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = tokio::fs::metadata(&signing_key_path).await?.permissions();
                perm.set_mode(0o600);
                tokio::fs::set_permissions(&signing_key_path, perm).await?;
            }
            bytes.to_vec()
        };
        let signing_key = EdKeyPair::from_bytes(&signing_bytes)
            .map_err(|e| anyhow::anyhow!("load signing key: {e}"))?;

        let ops_pub_path = state_dir.join("ops.pub");
        let ops_public_key = if ops_pub_path.exists() {
            Some(
                tokio::fs::read_to_string(&ops_pub_path)
                    .await?
                    .trim()
                    .to_string(),
            )
        } else {
            None
        };

        // mTLS is deprecated: identity is now carried by signing.key signatures; client certs are no longer required
        let (node_id, ca_cert_pem) = if id_path.exists() {
            (
                Some(
                    tokio::fs::read_to_string(&id_path)
                        .await?
                        .trim()
                        .to_string(),
                ),
                tokio::fs::read_to_string(&ca_path).await.ok(),
            )
        } else {
            (None, None)
        };

        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            signing_key,
            node_id,
            ca_cert_pem,
            no_ca_pin: false,
            ops_public_key,
            node_name: resolve_node_name(""),
            inventory_due: std::sync::atomic::AtomicBool::new(false),
        })
    }

    const fn enrolled(&self) -> bool {
        self.node_id.is_some()
    }

    /// Persist the enrollment result to disk and update in-memory state, so the
    /// agent can start reporting without a restart.
    async fn persist_enrollment(&mut self, resp: &EnrollResponse) -> anyhow::Result<()> {
        let id_path = self.state_dir.join("node.id");
        let ca_path = self.state_dir.join("ca.crt.pem");
        tokio::fs::write(&id_path, resp.node_id.as_bytes()).await?;
        if resp.ca_cert_pem.is_empty() {
            // Not delivered = peer does not terminate TLS (managed platform). Remove any
            // CA left over from a previous enroll, otherwise it would keep being
            // trusted and every request would fail TLS even though enroll succeeded.
            let _ = tokio::fs::remove_file(&ca_path).await;
        } else {
            tokio::fs::write(&ca_path, resp.ca_cert_pem.as_bytes()).await?;
        }

        // Persist the ops public key alongside enrollment (TOFU): after this,
        // even a compromised monitor cannot forge a command that this node would
        // verify.
        if !resp.ops_public_key.is_empty() {
            let ops_pub_path = self.state_dir.join("ops.pub");
            tokio::fs::write(&ops_pub_path, resp.ops_public_key.as_bytes()).await?;
            self.ops_public_key = Some(resp.ops_public_key.clone());
        }

        self.node_id = Some(resp.node_id.clone());
        if resp.ca_cert_pem.is_empty() {
            self.ca_cert_pem = None;
        } else {
            self.ca_cert_pem = Some(resp.ca_cert_pem.clone());
        }
        Ok(())
    }
}

async fn enroll(
    monitor: &str,
    token: &str,
    state: &mut NodeState,
    alias: &str,
    tags: &str,
) -> anyhow::Result<EnrollResponse> {
    let hostname = state.node_name.clone();

    // Identity is exactly this Ed25519 public key: registered at enroll, and the
    // matching private key signs every subsequent request.
    let public_key = state.signing_key.public_key().as_bytes().to_vec();

    let alias = alias.trim().to_string();
    let tags = split_tags(tags);
    if !alias.is_empty() || !tags.is_empty() {
        tracing::info!(alias = %alias, tags = ?tags, "Setting alias/tags on enrollment");
    }

    let req = EnrollRequest {
        hostname: hostname.clone(),
        public_key,
        labels: vec![zhiwei_proto::common::Label {
            key: "role".into(),
            value: "node".into(),
        }],
        alias,
        tags,
    };
    let mut buf = Vec::new();
    prost::Message::encode(&req, &mut buf)?;

    let resp_bytes = enroll_post(monitor, token, &buf).await?;
    let resp: EnrollResponse = prost::Message::decode(&resp_bytes[..])?;
    if resp.node_id.is_empty() {
        anyhow::bail!("enroll response missing node_id");
    }
    state.persist_enrollment(&resp).await?;
    Ok(resp)
}

/// First-time enroll: we don't yet know who to trust (equivalent to the old
/// skip-validation + TOFU). After successful enroll the node persists the
/// monitor's CA and pins it for subsequent requests.
async fn enroll_post(monitor: &str, token: &str, body: &[u8]) -> anyhow::Result<Vec<u8>> {
    // During enroll we don't yet have a signing identity, so we use the bootstrap
    // token via Authorization: Bearer. There is no CA to pin yet, so we skip
    // server-cert verification (TOFU: store the CA after a successful enroll).
    let t = http::HttpTransport::new(monitor, None, true)?;
    let (status, resp) = t.enroll(token, body).await?;
    if status != 200 {
        anyhow::bail!(
            "enroll failed HTTP {status}: {}",
            String::from_utf8_lossy(&resp)
                .chars()
                .take(200)
                .collect::<String>()
        );
    }
    Ok(resp)
}

async fn collect_and_send(
    monitor: &str,
    state: &NodeState,
    node_id: &str,
    sys: &sysinfo::System,
    networks: &sysinfo::Networks,
) -> anyhow::Result<()> {
    let batch = build_batch(node_id, &state.signing_key, sys, networks)?;
    let mut buf = Vec::new();
    prost::Message::encode(&batch, &mut buf)?;
    let t = transport(monitor, state)?;
    let (status, body) = t
        .request("POST", "/v1/telemetry", &buf, node_id, &state.signing_key)
        .await?;
    if status != 204 {
        anyhow::bail!(
            "telemetry returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(())
}

/// Build the signed transport: self-hosted deployments pin the monitor CA, managed platforms use system roots
fn transport(monitor: &str, state: &NodeState) -> anyhow::Result<http::HttpTransport> {
    let ca = if state.no_ca_pin {
        None
    } else {
        state.ca_cert_pem.as_deref()
    };
    http::HttpTransport::new(monitor, ca, false)
}

/// Convert byte counts to f64 for telemetry display.
///
/// Byte counts must survive the full `u64` range: RAM and disk are routinely
/// above 4 GiB, and capping at `u32::MAX` makes used/total collapse to 100%.
/// Values are rounded for display, so precision loss beyond 2^52 is acceptable
/// (a single byte at that scale is meaningless).
#[allow(clippy::cast_precision_loss)] // display-only; 2^52 exceeds any real byte counter
const fn bytes_to_f64(bytes: u64) -> f64 {
    bytes as f64
}

/// The fullest mount point's `(usage_percent, used_bytes, total_bytes)`.
///
/// Percent and byte-count come from the same disk, so the "absolute value /
/// ratio" views in the big-card and trend graph stay consistent. Mounts with
/// zero total space are skipped.
fn fullest_disk_stats(disks: &sysinfo::Disks) -> (f64, u64, u64) {
    let fullest = disks.iter().filter(|d| d.total_space() > 0).max_by(|a, b| {
        let ra =
            bytes_to_f64(a.total_space() - a.available_space()) / bytes_to_f64(a.total_space());
        let rb =
            bytes_to_f64(b.total_space() - b.available_space()) / bytes_to_f64(b.total_space());
        ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let usage = fullest.map_or(0.0, |d| {
        bytes_to_f64(d.total_space() - d.available_space()) * 100.0 / bytes_to_f64(d.total_space())
    });
    let used = fullest.map_or(0, |d| d.total_space().saturating_sub(d.available_space()));
    let total = fullest.map_or(0, sysinfo::Disk::total_space);
    (usage, used, total)
}

/// Take the top N processes by CPU usage.
fn snapshot_processes(sys: &sysinfo::System) -> ProcessSnapshot {
    const TOP_N: usize = 20;

    // The console needs both "by CPU" and "by memory" rankings with the top 10,
    // so take top 20 for each and union them. Truncating by CPU alone makes the
    // "by memory" list incomplete (memory hogs often have very low CPU usage).
    let mut all: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, p)| ProcessInfo {
            // Linux PIDs are i32 in practice; sysinfo returns u32. Convert with try_from
            // so a hypothetical huge pid surfaces as -1 instead of silently wrapping.
            pid: i32::try_from(pid.as_u32()).unwrap_or(-1),
            name: p.name().to_string_lossy().to_string(),
            cmdline: p
                .cmd()
                .iter()
                .map(|c| c.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            user: p.user_id().map(|u| u.to_string()).unwrap_or_default(),
            cpu_percent: f64::from(p.cpu_usage()),
            memory_bytes: p.memory(),
        })
        .collect();

    let by_cpu = |a: &ProcessInfo, b: &ProcessInfo| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
    };
    let by_mem = |a: &ProcessInfo, b: &ProcessInfo| {
        b.memory_bytes.cmp(&a.memory_bytes).then_with(|| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };

    all.sort_by(by_cpu);
    let mut procs: Vec<ProcessInfo> = all.iter().take(TOP_N).cloned().collect();
    let mut seen: std::collections::HashSet<i32> = procs.iter().map(|p| p.pid).collect();

    all.sort_by(by_mem);
    for p in all.into_iter().take(TOP_N) {
        if seen.insert(p.pid) {
            procs.push(p);
        }
    }

    procs.sort_by(by_cpu);

    ProcessSnapshot { processes: procs }
}

/// Parse `--tags` / `ZHIWEI_NODE_TAGS`: whitespace, English comma, Chinese comma,
/// and the ideographic enumeration comma are all valid separators; drop empty
/// strings, deduplicate (preserving order).
/// Separators must match the console (node-meta-dialog), so users don't end up in
/// the "split here, don't split there" situation.
fn split_tags(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、'))
        .map(str::trim)
        .filter(|s| !s.is_empty() && seen.insert((*s).to_string()))
        .map(str::to_string)
        .collect()
}

/// The system hostname can be a placeholder value (on macOS, when a reverse
/// PTR returns bogus, the system sets hostname to "bogon", i.e. bogus), which is
/// useless for monitoring. Pick a sensible name by priority.
fn resolve_node_name(explicit: &str) -> String {
    const PLACEHOLDERS: &[&str] = &[
        "bogon",
        "localhost",
        "localhost.localdomain",
        "(none)",
        "none",
    ];

    if !explicit.trim().is_empty() {
        return explicit.trim().to_string();
    }

    let system = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default();

    if !system.is_empty() && !PLACEHOLDERS.contains(&system.to_lowercase().as_str()) {
        return system;
    }

    // macOS: LocalHostName (Bonjour name) is usually set by humans and is more meaningful than bogon
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
    {
        if out.status.success() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                tracing::warn!(
                    system_hostname = %system,
                    resolved = %name,
                    "System hostname was a placeholder; using macOS LocalHostName instead (override with --node-name)"
                );
                return name;
            }
        }
    }

    if !system.is_empty() {
        tracing::warn!(
            system_hostname = %system,
            "System hostname is a placeholder value; recommend setting --node-name to a distinguishable name"
        );
        return system;
    }
    "unknown".into()
}

/// Collect basic host info: OS / kernel / arch / CPU / memory / uptime / per-interface IPs.
/// Takes a pre-created System (which already has two samples for CPU usage).
fn build_host_info(node_name: &str, sys: &sysinfo::System) -> HostInfo {
    use sysinfo::Networks;

    let hostname = node_name.to_string();
    let os_name = System::name().unwrap_or_default();
    let os_version = System::os_version().unwrap_or_default();
    let long_os_version = System::long_os_version().unwrap_or_default();
    let kernel_version = System::kernel_version().unwrap_or_default();
    let arch = System::cpu_arch();

    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    let cpu_cores = u32::try_from(sys.cpus().len()).unwrap_or(u32::MAX);

    let networks = Networks::new_with_refreshed_list();
    let interfaces = networks
        .iter()
        .map(|(name, data)| {
            let addresses = data
                .ip_networks()
                .iter()
                .map(|n| IpAddress {
                    addr: n.addr.to_string(),
                    prefix: u32::from(n.prefix),
                })
                .collect();
            InterfaceInfo {
                name: name.clone(),
                addresses,
                loopback: name == "lo" || name == "lo0",
            }
        })
        .collect();

    HostInfo {
        hostname,
        os_name,
        os_version,
        long_os_version,
        kernel_version,
        arch,
        cpu_brand,
        cpu_cores,
        total_memory_bytes: sys.total_memory(),
        uptime_seconds: System::uptime(),
        boot_time_unix_seconds: System::boot_time(),
        interfaces,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        // main() didn't run (rare, e.g. a test path calls build_host_info directly) → 0;
        // the console shows 0 as unknown, matching legacy agent behavior.
        agent_started_at_unix_seconds: AGENT_STARTED_AT_UNIX_SECONDS.get().copied().unwrap_or(0),
    }
}

fn build_batch(
    node_id: &str,
    key: &EdKeyPair,
    sys: &sysinfo::System,
    networks: &sysinfo::Networks,
) -> anyhow::Result<TelemetryBatch> {
    let mut metrics = vec![
        Metric {
            name: "host.cpu.usage".into(),
            value: f64::from(sys.global_cpu_usage()),
            labels: HashMap::default(),
        },
        Metric {
            name: "host.mem.used_bytes".into(),
            value: bytes_to_f64(sys.used_memory()),
            labels: HashMap::default(),
        },
        Metric {
            name: "host.mem.total_bytes".into(),
            value: bytes_to_f64(sys.total_memory()),
            labels: HashMap::default(),
        },
    ];
    // Memory-usage percent is computed once on the node side: the console's memory
    // column, the "memory usage too high" alert rule, and the hourly aggregates all
    // use this metric name directly. Compute once here so each consumer doesn't
    // re-implement it (and risk drift).
    if let Some(total) = metrics
        .iter()
        .find(|m| m.name == "host.mem.total_bytes")
        .map(|m| m.value)
    {
        if total > 0.0 {
            let used = metrics
                .iter()
                .find(|m| m.name == "host.mem.used_bytes")
                .map_or(0.0, |m| m.value);
            metrics.push(Metric {
                name: "host.mem.usage".into(),
                value: used * 100.0 / total,
                labels: HashMap::default(),
            });
        }
    }

    let network = networks
        .iter()
        .map(|(name, data)| NetworkInterface {
            name: name.clone(),
            rx_bytes: data.total_received(),
            tx_bytes: data.total_transmitted(),
            rx_packets: data.total_packets_received(),
            tx_packets: data.total_packets_transmitted(),
        })
        .collect();

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let (max_disk_usage, disk_used_bytes, disk_total_bytes) = fullest_disk_stats(&disks);
    metrics.push(Metric {
        name: "host.disk.usage".into(),
        value: max_disk_usage,
        labels: HashMap::default(),
    });
    metrics.push(Metric {
        name: "host.disk.used_bytes".into(),
        value: bytes_to_f64(disk_used_bytes),
        labels: HashMap::default(),
    });
    metrics.push(Metric {
        name: "host.disk.total_bytes".into(),
        value: bytes_to_f64(disk_total_bytes),
        labels: HashMap::default(),
    });

    let disks: Vec<DiskMount> = disks
        .iter()
        .map(|d| DiskMount {
            mountpoint: d.mount_point().to_string_lossy().into_owned(),
            filesystem: d.file_system().to_string_lossy().into_owned(),
            total_bytes: d.total_space(),
            used_bytes: d.total_space().saturating_sub(d.available_space()),
        })
        .collect();

    let ts = zhiwei_common::Timestamp::now().unix_nano();
    let mut batch = TelemetryBatch {
        node_id: node_id.into(),
        ts_unix_nano: ts,
        interval_seconds: 30,
        metrics,
        network,
        disks,
        signature: vec![],
    };

    let mut preimage = Vec::new();
    prost::Message::encode(&batch, &mut preimage)?;
    let sig = key.sign(&preimage);
    batch.signature = sig.0;
    Ok(batch)
}

// ---------- inventory (snapshot) ----------

/// Build the snapshot report: host info + containers + processes. Low-frequency uploads; server keeps only the latest.
/// Takes a pre-created System (already has two samples for CPU usage).
async fn build_inventory(
    node_id: &str,
    key: &EdKeyPair,
    cert_globs: &[String],
    node_name: &str,
    cert_sources: &[certs::CertSourceSpec],
    sys: &sysinfo::System,
) -> anyhow::Result<InventoryReport> {
    let containers = match docker::list_containers().await {
        Ok(Some(list)) => list,
        // No Docker (or socket unavailable) → report empty list, indicating no container runtime on this host
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to collect containers");
            Vec::new()
        }
    };

    // Processes from the already-sampled sysinfo (CPU usage valid from first sample)
    let processes = Some(snapshot_processes(sys));

    // Console-configured paths take priority (same path: first source wins); fall back to local baseline
    let certificates = certs::scan_with_sources(cert_sources, cert_globs);

    let mut report = InventoryReport {
        node_id: node_id.into(),
        ts_unix_nano: zhiwei_common::Timestamp::now().unix_nano(),
        host_info: Some(build_host_info(node_name, sys)),
        containers,
        processes,
        certificates,
        signature: vec![],
    };

    let mut preimage = Vec::new();
    prost::Message::encode(&report, &mut preimage)?;
    report.signature = key.sign(&preimage).0;
    Ok(report)
}

async fn send_inventory(
    monitor: &str,
    state: &NodeState,
    node_id: &str,
    cert_globs: &[String],
    sys: &sysinfo::System,
) -> anyhow::Result<()> {
    // Pull cert path config before each snapshot: if unreachable, fall back to local
    // baseline — don't let "monitor glitched for a moment" become "this machine's certs vanished"
    let cert_sources = fetch_cert_sources(monitor, state, node_id).await;
    let report = build_inventory(
        node_id,
        &state.signing_key,
        cert_globs,
        &state.node_name,
        &cert_sources,
        sys,
    )
    .await?;
    let mut buf = Vec::new();
    prost::Message::encode(&report, &mut buf)?;
    let t = transport(monitor, state)?;
    let (status, body) = t
        .request("POST", "/v1/inventory", &buf, node_id, &state.signing_key)
        .await?;
    if status != 204 {
        anyhow::bail!(
            "inventory returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(())
}

/// Fetch cert paths configured by the console for this node (`GET /v1/cert-config`).
/// On failure only log a debug message and return empty — scanning continues with local baseline.
async fn fetch_cert_sources(
    monitor: &str,
    state: &NodeState,
    node_id: &str,
) -> Vec<certs::CertSourceSpec> {
    let path = format!("/v1/cert-config?node_id={node_id}");
    let result = async {
        let t = crate::transport(monitor, state)?;
        let (status, raw) = t
            .request_json("GET", &path, &[], node_id, &state.signing_key)
            .await?;
        anyhow::ensure!(
            status == 200,
            "fetching cert config returned {status}: {}",
            String::from_utf8_lossy(&raw)
        );
        let v: serde_json::Value = serde_json::from_slice(&raw)?;
        Ok::<Vec<certs::CertSourceSpec>, anyhow::Error>(
            v.get("sources")
                .and_then(|s| s.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|s| {
                            Some(certs::CertSourceSpec {
                                id: s.get("id")?.as_str()?.to_string(),
                                path: s.get("path")?.as_str()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    }
    .await;

    match result {
        Ok(sources) => sources,
        Err(e) => {
            tracing::debug!(error = %e, "Failed to fetch cert config, using local baseline paths only");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::sync::Mutex;

    /// Global env is shared within a single process; cargo test runs tests in parallel
    /// by default, and tests that set env will pollute each other. The lock below
    /// forces the relevant tests to run serially.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Pin the env-integration behavior of `Args.interval`: env overrides default; CLI overrides env.
    /// Previously installed nodes couldn't change interval because `env = "ZHIWEI_INTERVAL"`
    /// was missing — the systemd unit wrote `EnvironmentFile=/etc/zhiwei/node.env`, but
    /// clap not reading that key made it useless; this test nails down that contract.
    #[test]
    fn interval_reads_from_env() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("ZHIWEI_INTERVAL", "120");
        let a = Args::try_parse_from(["zhiwei-node", "--monitor", "http://x"]).unwrap();
        assert_eq!(a.interval, 120, "env ZHIWEI_INTERVAL=120 should be applied");
        std::env::remove_var("ZHIWEI_INTERVAL");
    }

    #[test]
    fn interval_cli_overrides_env() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("ZHIWEI_INTERVAL", "120");
        let a = Args::try_parse_from(["zhiwei-node", "--monitor", "http://x", "--interval", "5"])
            .unwrap();
        assert_eq!(a.interval, 5, "CLI --interval 5 should override env");
        std::env::remove_var("ZHIWEI_INTERVAL");
    }

    /// Tags passed at enrollment: whitespace must also split (previously only comma was recognized;
    /// pasting "prod bj" would become a single tag).
    #[test]
    fn splits_tags_on_space_and_comma() {
        assert_eq!(split_tags("prod,bj"), vec!["prod", "bj"]);
        assert_eq!(split_tags("prod bj"), vec!["prod", "bj"]);
        // Drop empty strings and duplicates, preserving order
        assert_eq!(split_tags("  a ,, a,b  "), vec!["a", "b"]);
        assert!(split_tags("   ").is_empty());
    }

    /// Alias / tags read from env (install-node.sh writes to a 0600 env file; systemd / launchd then inject).
    #[test]
    fn alias_and_tags_read_from_env() {
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("ZHIWEI_NODE_ALIAS", "Beijing Entry");
        std::env::set_var("ZHIWEI_NODE_TAGS", "prod bj");
        let a = Args::try_parse_from(["zhiwei-node", "--monitor", "http://x"]).unwrap();
        assert_eq!(a.alias, "Beijing Entry");
        assert_eq!(split_tags(&a.tags), vec!["prod", "bj"]);
        std::env::remove_var("ZHIWEI_NODE_ALIAS");
        std::env::remove_var("ZHIWEI_NODE_TAGS");
    }
}
