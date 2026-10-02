//! ZhiWei node-agent.
//!
//! First-run flow (enroll):
//!   1. Generate the Ed25519 signing key under `<state>/signing.key` (0600)
//!   2. POST /v1/enroll with bootstrap-token + that public key + hostname/labels
//!   3. Persist `<state>/node.id` (+ `<state>/ops.pub` for command verification)
//!   4. From now on, sign every request with the key — no client certificate
//!
//! Steady-state:
//!   - Every `interval` seconds, build a TelemetryBatch (CPU/mem/disk/net)
//!     and POST it to /v1/telemetry.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;
use zhiwei_common::KeyPair as EdKeyPair;
use zhiwei_proto::common::{EnrollRequest, EnrollResponse};
use zhiwei_proto::telemetry::{
    DiskMount, HostInfo, InterfaceInfo, InventoryReport, IpAddress, Metric, NetworkInterface,
    ProcessInfo, ProcessSnapshot, TelemetryBatch,
};

/// node-agent 进程启动的 Unix 秒——在 `main()` 第一行写一次，之后 `build_host_info` 读。
/// 不把时间从 main 一路透到 inventory 调用点：`build_inventory` 是定时任务里调的，
/// 单独再加一个参数会让签名/容器/进程的调用链都跟着改。
/// 控制台拿这个字段展示「本次部署时间」；老 agent 不写这个字段，反序列化时是 0，
/// UI 看到 0 当成 unknown 不显示。
static AGENT_STARTED_AT_UNIX_SECONDS: OnceLock<u64> = OnceLock::new();

mod certs;
mod control;
mod docker;
mod http;
mod probes;

#[derive(Parser, Debug)]
#[command(name = "zhiwei-node", about = "ZhiWei node-agent", version)]
struct Args {
    /// Monitor URL (e.g. https://127.0.0.1:8443)
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
    /// 默认 5；env `ZHIWEI_INTERVAL` 可覆盖（与 install 脚本写到
    /// /etc/zhiwei/node.env 的 KEY 一致——想改 telemetry 频率只动 env 文件，
    /// `systemctl restart zhiwei-node` 即可，不用碰 unit）。
    #[arg(long, default_value_t = 5, env = "ZHIWEI_INTERVAL")]
    interval: u64,

    /// Inventory（主机信息 / 容器 / 进程 / 证书快照）上报间隔（秒）
    #[arg(long, default_value_t = 300, env = "ZHIWEI_INVENTORY_INTERVAL")]
    inventory_interval: u64,

    /// 证书扫描 glob，逗号分隔；留空用内置默认位置
    #[arg(long, env = "ZHIWEI_CERT_GLOBS", default_value = "")]
    cert_globs: String,

    /// 上报给 monitor 的节点名。留空则用系统主机名，
    /// 但遇到 bogon / localhost 这类占位值会自动换用更好的来源。
    #[arg(long, env = "ZHIWEI_NODE_NAME", default_value = "")]
    node_name: String,

    /// 入网时一并设置的别名（≤10 字符）。
    ///
    /// 只在**首次 enroll** 生效：节点一旦有 node.id 就不再 enroll，重复设置无效。
    /// 已经入网的机器请在控制台改（或删掉 state 目录重新入网）。
    #[arg(long, env = "ZHIWEI_NODE_ALIAS", default_value = "")]
    alias: String,

    /// 入网时一并设置的标签，空格 / 逗号 / 顿号分隔（最多 10 个，每个 ≤24 字符）。
    /// 同样只在首次 enroll 生效。
    #[arg(long, env = "ZHIWEI_NODE_TAGS", default_value = "")]
    tags: String,

    /// 覆盖 pin 用的 monitor CA，传文件路径。
    ///
    /// 默认（留空）用 enroll 下发的那个 CA——自建 TLS 部署该这样用。
    /// 部署在 Render / Railway 这类边缘终结 TLS 的平台后面时，enroll 不会下发 CA
    /// （本地 CA 与边缘的正经证书无关），节点自动走系统根。
    ///
    /// 传 `-` 表示**强制不 pin、走系统根**：给「曾经对着自建 monitor 入网过、
    /// 机器上留着旧 CA，现在改指向托管平台」的节点用，省得删文件重 enroll。
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

    // 进程启动的第一时间记一下，之后 inventory 反复读这个值。后续 main 里
    // 任一步失败重启都重新写，所以即便「看似同一进程」重启了也对得上。
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

    // --monitor-ca：显式覆盖 pin 用的 CA；`-` 表示强制走系统根
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
                .with_context(|| format!("读取 --monitor-ca 指定的 CA：{path}"))?;
            state.ca_cert_pem = Some(pem);
            state.no_ca_pin = false;
            tracing::info!(path, "Using --monitor-ca specified CA to pin monitor");
        }
    }

    if !state.enrolled() {
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
    } else {
        tracing::info!(node_id = %state.node_id.clone().unwrap_or_default(), "already enrolled");
    }

    let node_id = state
        .node_id
        .clone()
        .context("node id missing after enrollment")?;
    let state = std::sync::Arc::new(state);
    let monitor = args.monitor.clone();

    // 控制通道与控制台轮询并行：拉命令 → 验签 → 执行 → 签名回执
    {
        let s = state.clone();
        let m = monitor.clone();
        let id = node_id.clone();
        tokio::spawn(async move { control::run_poll_loop(m, s, id).await });
    }

    // 服务探活循环：拉探针配置 → 到点执行 → 上报结果
    {
        let s = state.clone();
        let m = monitor.clone();
        let id = node_id.clone();
        tokio::spawn(async move { probes::run_loop(m, s, id).await });
    }
    let interval = args.interval.max(1);
    let inventory_interval = args.inventory_interval.max(interval);
    // 证书扫描位置：默认 globs 与用户传入的 ZHIWEI_CERT_GLOBS / --cert-globs **并集**。
    // 旧行为是「用户传了就完全替换默认」——但默认 globs 已经覆盖 Let's Encrypt、
    // nginx 自管目录等最常见位置，用户传自定义路径 99% 是「额外再加几处」，不是
    // 「我不要默认的」。替换语义踩过坑：硅谷节点证书放在 /root/nginx-certs/ 下，
    // 之前某次给 zhiwei-node 配了 --cert-globs 之后默认位置反而扫不到。
    // 同路径去重交给 `certs::scan_entries` 里的 `by_path`（先到者胜），这里不去重。
    let cert_globs: Vec<String> = certs::merge_globs(&args.cert_globs);

    // 启动后立即上报一次快照（让控制台马上有数据），之后按间隔周期上报
    let mut next_inventory = std::time::Instant::now();

    loop {
        match collect_and_send(&monitor, &state, &node_id).await {
            Ok(()) => tracing::debug!("batch ok"),
            Err(e) => tracing::warn!(error = %e, "batch failed"),
        }

        // 周期到点，或控制台点了「立刻重采快照」（容器启停之后要马上看到新状态）
        let forced = state
            .inventory_due
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        if forced || std::time::Instant::now() >= next_inventory {
            match send_inventory(&monitor, &state, &node_id, &cert_globs).await {
                Ok(()) => {
                    next_inventory =
                        std::time::Instant::now() + Duration::from_secs(inventory_interval);
                    tracing::debug!("inventory ok");
                }
                Err(e) => tracing::warn!(error = %e, "inventory failed"),
            }
        }

        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

struct NodeState {
    state_dir: PathBuf,
    #[allow(dead_code)]
    signing_key: EdKeyPair, // 32-byte ed25519, used to sign telemetry batches
    node_id: Option<String>,
    /// monitor 的 CA 证书（自建 TLS 部署下用于 pinning；无则走系统根）
    ca_cert_pem: Option<String>,
    /// 强制不 pin monitor CA（`--monitor-ca -`），一律走系统根
    no_ca_pin: bool,
    /// ops 控制平面公钥（base64）。enroll 时拿到并长期保存，用于验命令签名。
    ops_public_key: Option<String>,
    /// 上报给 monitor 的节点名（解析后的）
    node_name: String,
    /// 控制台要求「立刻重采一次快照」时置位，主循环下一拍就发一次 inventory
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

        // mTLS 已废弃：身份改由 signing.key 的签名承担，不再需要客户端证书
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

    fn enrolled(&self) -> bool {
        self.node_id.is_some()
    }

    /// Persist the enrollment result to disk and update in-memory state, so the
    /// agent can start reporting without a restart.
    async fn persist_enrollment(&mut self, resp: &EnrollResponse) -> anyhow::Result<()> {
        let id_path = self.state_dir.join("node.id");
        let ca_path = self.state_dir.join("ca.crt.pem");
        tokio::fs::write(&id_path, resp.node_id.as_bytes()).await?;
        if !resp.ca_cert_pem.is_empty() {
            tokio::fs::write(&ca_path, resp.ca_cert_pem.as_bytes()).await?;
        } else {
            // 没下发 = 对端不终结 TLS（托管平台）。把上一次 enroll 留下的 CA 删掉，
            // 否则它会继续被当成信任根，表现成「enroll 成功、之后每个请求都 TLS 失败」。
            let _ = tokio::fs::remove_file(&ca_path).await;
        }

        // ops 公钥随 enrollment 一起存下（TOFU）：此后即便 monitor 被攻陷，
        // 也无法伪造出一条能被本节点验签通过的命令。
        if !resp.ops_public_key.is_empty() {
            let ops_pub_path = self.state_dir.join("ops.pub");
            tokio::fs::write(&ops_pub_path, resp.ops_public_key.as_bytes()).await?;
            self.ops_public_key = Some(resp.ops_public_key.clone());
        }

        self.node_id = Some(resp.node_id.clone());
        if !resp.ca_cert_pem.is_empty() {
            self.ca_cert_pem = Some(resp.ca_cert_pem.clone());
        } else {
            self.ca_cert_pem = None;
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

    // 身份就是这个 Ed25519 公钥：enroll 时登记，此后每次请求用它对应的私钥签名
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
        anyhow::bail!("enroll 响应缺少 node_id");
    }
    state.persist_enrollment(&resp).await?;
    Ok(resp)
}

/// 首次 enroll：此时还不知道该信任谁（等价于原来的跳过校验 + TOFU）。
/// enroll 成功后节点会存下 monitor 的 CA，后续请求改用它做 pinning。
async fn enroll_post(monitor: &str, token: &str, body: &[u8]) -> anyhow::Result<Vec<u8>> {
    // enroll 阶段还没有签名身份，所以走 bootstrap token 走 Authorization: Bearer
    // 此时没有 CA 可 pin，因此跳过服务端证书校验（TOFU：enroll 成功后存下 CA）
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

async fn collect_and_send(monitor: &str, state: &NodeState, node_id: &str) -> anyhow::Result<()> {
    let batch = build_batch(node_id, &state.signing_key)?;
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

/// 构造带签名的传输层：自建部署 pin monitor CA，托管平台走系统根
fn transport(monitor: &str, state: &NodeState) -> anyhow::Result<http::HttpTransport> {
    let ca = if state.no_ca_pin {
        None
    } else {
        state.ca_cert_pem.as_deref()
    };
    http::HttpTransport::new(monitor, ca, false)
}

/// 取 CPU 占用最高的前 N 个进程。
fn snapshot_processes(sys: &sysinfo::System) -> ProcessSnapshot {
    const TOP_N: usize = 20;

    // 控制台要在「按 CPU」与「按内存」两种排序下都取前 10，所以两榜各取前 20 再求并集：
    // 只按 CPU 截断会让「按内存」榜缺人（内存大户常常 CPU 很低）。
    let mut all: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32() as i32,
            name: p.name().to_string_lossy().to_string(),
            cmdline: p
                .cmd()
                .iter()
                .map(|c| c.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            user: p.user_id().map(|u| u.to_string()).unwrap_or_default(),
            cpu_percent: p.cpu_usage() as f64,
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

/// 解析 `--tags` / `ZHIWEI_NODE_TAGS`：空白、英文逗号、中文逗号、顿号都算分隔符；
/// 去空串、去重（保持顺序）。分隔符与控制台（node-meta-dialog）保持一致，
/// 免得「这里能分开、那里分不开」。
fn split_tags(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、'))
        .map(str::trim)
        .filter(|s| !s.is_empty() && seen.insert(s.to_string()))
        .map(str::to_string)
        .collect()
}

/// 系统主机名可能是占位值（macOS 在反查 PTR 拿到 bogon 时会把主机名设成
/// 「bogon」，即 bogus），对监控毫无辨识度。这里按优先级挑一个像样的名字。
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

    // macOS：LocalHostName（Bonjour 名）通常是人设过的，比 bogon 有意义
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
                    "系统主机名是占位值，已改用 macOS LocalHostName；可用 --node-name 覆盖"
                );
                return name;
            }
        }
    }

    if !system.is_empty() {
        tracing::warn!(
            system_hostname = %system,
            "系统主机名是占位值，建议用 --node-name 指定一个可辨识的名字"
        );
        return system;
    }
    "unknown".into()
}

/// 采集主机基本信息：操作系统 / 内核 / 架构 / CPU / 内存 / 运行时长 / 各网卡 IP。
fn build_host_info(node_name: &str) -> HostInfo {
    use sysinfo::{Networks, System};

    let hostname = node_name.to_string();
    let os_name = System::name().unwrap_or_default();
    let os_version = System::os_version().unwrap_or_default();
    let long_os_version = System::long_os_version().unwrap_or_default();
    let kernel_version = System::kernel_version().unwrap_or_default();
    let arch = System::cpu_arch();

    let sys = System::new_all();
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    let cpu_cores = sys.cpus().len() as u32;

    let networks = Networks::new_with_refreshed_list();
    let interfaces = networks
        .iter()
        .map(|(name, data)| {
            let addresses = data
                .ip_networks()
                .iter()
                .map(|n| IpAddress {
                    addr: n.addr.to_string(),
                    prefix: n.prefix as u32,
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
        // main() 没跑过（极少见，比如某条测试路径直接调 build_host_info）→ 0，
        // 控制台看到 0 就当 unknown 不显示，逻辑跟老 agent 行为兼容。
        agent_started_at_unix_seconds: AGENT_STARTED_AT_UNIX_SECONDS.get().copied().unwrap_or(0),
    }
}

fn build_batch(node_id: &str, key: &EdKeyPair) -> anyhow::Result<TelemetryBatch> {
    use sysinfo::{Disks, Networks, System};

    let mut sys = System::new_all();
    sys.refresh_all();

    let mut metrics = vec![
        Metric {
            name: "host.cpu.usage".into(),
            value: sys.global_cpu_usage() as f64,
            labels: Default::default(),
        },
        Metric {
            name: "host.mem.used_bytes".into(),
            value: sys.used_memory() as f64,
            labels: Default::default(),
        },
        Metric {
            name: "host.mem.total_bytes".into(),
            value: sys.total_memory() as f64,
            labels: Default::default(),
        },
    ];
    // 内存使用率（百分比）在节点侧就算好：控制台的内存列、告警规则
    // 「内存使用率过高」、以及小时聚合都直接用这个指标名。
    // 只在节点侧补一次，别让每个消费方各算一遍、算法还会漂。
    if let Some(total) = metrics
        .iter()
        .find(|m| m.name == "host.mem.total_bytes")
        .map(|m| m.value)
    {
        if total > 0.0 {
            let used = metrics
                .iter()
                .find(|m| m.name == "host.mem.used_bytes")
                .map(|m| m.value)
                .unwrap_or(0.0);
            metrics.push(Metric {
                name: "host.mem.usage".into(),
                value: used * 100.0 / total,
                labels: Default::default(),
            });
        }
    }

    let networks = Networks::new_with_refreshed_list();
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

    let disks = Disks::new_with_refreshed_list();
    // 「使用率最高的挂载点」（跳过总容量为 0 的伪挂载）。百分比与字节量取同一块盘，
    // 大字卡片与趋势图的「绝对值 / 占比」两个视图才自洽。
    let fullest = disks.iter().filter(|d| d.total_space() > 0).max_by(|a, b| {
        let ra = (a.total_space() - a.available_space()) as f64 / a.total_space() as f64;
        let rb = (b.total_space() - b.available_space()) as f64 / b.total_space() as f64;
        ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let max_disk_usage = fullest
        .map(|d| (d.total_space() - d.available_space()) as f64 * 100.0 / d.total_space() as f64)
        .unwrap_or(0.0);
    let disk_used_bytes = fullest
        .map(|d| d.total_space().saturating_sub(d.available_space()))
        .unwrap_or(0);
    let disk_total_bytes = fullest.map(|d| d.total_space()).unwrap_or(0);
    metrics.push(Metric {
        name: "host.disk.usage".into(),
        value: max_disk_usage,
        labels: Default::default(),
    });
    metrics.push(Metric {
        name: "host.disk.used_bytes".into(),
        value: disk_used_bytes as f64,
        labels: Default::default(),
    });
    metrics.push(Metric {
        name: "host.disk.total_bytes".into(),
        value: disk_total_bytes as f64,
        labels: Default::default(),
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

// ---------- inventory（快照） ----------

/// 构造快照报告：主机信息 + 容器 + 进程。低频上报，服务端只留最新一份。
async fn build_inventory(
    node_id: &str,
    key: &EdKeyPair,
    cert_globs: &[String],
    node_name: &str,
    cert_sources: &[certs::CertSourceSpec],
) -> anyhow::Result<InventoryReport> {
    use sysinfo::System;

    let containers = match docker::list_containers().await {
        Ok(Some(list)) => list,
        // 没有 Docker（或 socket 不可用）时上报空列表，表示本机无容器运行时
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to collect containers");
            Vec::new()
        }
    };

    // sysinfo 需要「两次采样 + 中间有间隔」才能算出 CPU 使用率，
    // 否则所有进程的 cpu_usage() 都是 0。
    let mut sys = System::new_all();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_all();
    let processes = Some(snapshot_processes(&sys));

    // 控制台配置的路径优先（同一路径以来源为准），本机基线兜底
    let certificates = certs::scan_with_sources(cert_sources, cert_globs);

    let mut report = InventoryReport {
        node_id: node_id.into(),
        ts_unix_nano: zhiwei_common::Timestamp::now().unix_nano(),
        host_info: Some(build_host_info(node_name)),
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
) -> anyhow::Result<()> {
    // 每次快照前拉一次证书路径配置：拉不到就退回本机基线，
    // 不让「monitor 抖了一下」变成「这台机器的证书全不见了」
    let cert_sources = fetch_cert_sources(monitor, state, node_id).await;
    let report = build_inventory(
        node_id,
        &state.signing_key,
        cert_globs,
        &state.node_name,
        &cert_sources,
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

/// 拉取控制台为本节点配置的证书路径（`GET /v1/cert-config`）。
/// 失败只记 debug 日志并返回空列表——扫描照旧走本机基线。
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
            "拉取证书配置返回 {status}: {}",
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

    /// 全局 env 在同一进程内是共享的；cargo test 默认并行跑测试，
    /// 设 env 的测试会互相污染。下面这把锁强制相关测试串行。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 锁定 `Args.interval` 的 env 集成行为：env 覆盖默认；命令行覆盖 env。
    /// 之前装好的节点改不了 interval，是因为漏了 `env = "ZHIWEI_INTERVAL"`——
    /// systemd unit 写了 `EnvironmentFile=/etc/zhiwei/node.env`，但 clap 不读
    /// 这个 key 就形同虚设；这条测试把这条约束钉死。
    #[test]
    fn interval_reads_from_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ZHIWEI_INTERVAL", "120");
        let a = Args::try_parse_from(["zhiwei-node", "--monitor", "http://x"]).unwrap();
        assert_eq!(a.interval, 120, "env ZHIWEI_INTERVAL=120 应被采纳");
        std::env::remove_var("ZHIWEI_INTERVAL");
    }

    #[test]
    fn interval_cli_overrides_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ZHIWEI_INTERVAL", "120");
        let a = Args::try_parse_from(["zhiwei-node", "--monitor", "http://x", "--interval", "5"])
            .unwrap();
        assert_eq!(a.interval, 5, "命令行 --interval 5 应覆盖 env");
        std::env::remove_var("ZHIWEI_INTERVAL");
    }

    /// 入网时带的标签：空格也要能分开（以前只认逗号，粘贴「prod bj」会变成一个标签）。
    #[test]
    fn splits_tags_on_space_and_comma() {
        assert_eq!(split_tags("prod,bj"), vec!["prod", "bj"]);
        assert_eq!(split_tags("prod bj"), vec!["prod", "bj"]);
        assert_eq!(split_tags("prod，bj、入口"), vec!["prod", "bj", "入口"]);
        // 去空串与重复，保持顺序
        assert_eq!(split_tags("  a ,, a,b  "), vec!["a", "b"]);
        assert!(split_tags("   ").is_empty());
    }

    /// 别名 / 标签走 env（install-node.sh 写进 0600 的 env 文件，systemd / launchd 再注入）。
    #[test]
    fn alias_and_tags_read_from_env() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ZHIWEI_NODE_ALIAS", "Beijing Entry");
        std::env::set_var("ZHIWEI_NODE_TAGS", "prod bj");
        let a = Args::try_parse_from(["zhiwei-node", "--monitor", "http://x"]).unwrap();
        assert_eq!(a.alias, "北京入口");
        assert_eq!(split_tags(&a.tags), vec!["prod", "bj"]);
        std::env::remove_var("ZHIWEI_NODE_ALIAS");
        std::env::remove_var("ZHIWEI_NODE_TAGS");
    }
}
