use std::path::PathBuf;
use std::sync::Arc;

use crate::ca::Ca;
use crate::routes::BootstrapTokens;
use zhiwei_storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub storage: Storage,
    /// 数据目录（改凭据时要写回 `<data-dir>/admin.token`）
    pub data_dir: PathBuf,
    pub ca: Arc<Ca>,
    pub ca_cert_pem: String,
    pub bootstrap_tokens: Arc<BootstrapTokens>,
    /// Browser-facing credential for the read API (节点走请求签名，不走这里)。
    /// 控制台凭据。可改（设置页的「修改密码」），所以放在 RwLock 里。
    pub admin_token: Arc<std::sync::RwLock<String>>,
    /// ops 控制平面公钥（base64）。随 enroll 下发给节点做 TOFU；
    /// ops 私钥只在 ops-server 进程内，monitor 不持有，因此伪造不了命令。
    pub ops_public_key: String,
    /// ops-server 的本地地址（控制台发起命令时由 monitor 转发给它签名）
    pub ops_endpoint: String,
    /// 节点请求签名用过的 nonce，用于防重放
    pub nonce_cache: Arc<zhiwei_common::NonceCache>,
    #[allow(dead_code)]
    pub server_cert_cn: String,
    /// 是否由本进程终结 TLS。
    ///
    /// 为 false（`--plain-http`，即部署在 Render 这类边缘终结 TLS 的平台后面）时，
    /// **enroll 不下发本地 CA**：那个 CA 与边缘用的正经证书没有任何关系，
    /// 节点一旦 pin 它，enroll 之后每个请求都会 TLS 校验失败。
    pub tls_terminated_locally: bool,
    /// 控制台构建目录（存在时由 monitor 托管，供 SPA 兜底渲染 index.html）
    pub ui_dir: Option<PathBuf>,
    /// 帮助页 markdown 内容（启动时从 `assets/help.md` 加载）
    pub help: crate::state::HelpContent,
    /// MCP server 内部调自己 REST 时用的 base URL，
    /// 形如 `http://127.0.0.1:8443`。见 `mcp.rs`。
    pub mcp_base_url: String,
    /// 节点入网安装脚本正文（编译期从 `scripts/install-node.sh` 内嵌，
    /// 由 `GET /install-node.sh` 原样吐出）。
    pub install_script: String,
    /// 节点二进制的自建分发源（可选，来自 `ZHIWEI_NODE_BASE_URL`）。
    ///
    /// 设了它，控制台生成的入网命令会自动多带一行 `ZHIWEI_BASE_URL=<此值>`，
    /// 节点就不必从 GitHub Releases 拉二进制。国内 / 隔离网络部署用这个：
    /// 在自己的 monitor 上设一次，之后所有入网命令天然走自建源，
    /// 不需要让每个执行者记住多带一个变量。
    pub node_base_url: Option<String>,
    /// 命令落库信号：watch 里存一个自增的「代数」。节点长轮询订阅它，
    /// 签发成功即唤醒——把「点删除 → 节点执行」从最长 10s 压到 1s 内。
    ///
    /// 命令一律由本进程转发给 ops 签发，所以落库时机在进程内可观测；
    /// 长轮询里仍留了周期性查库兜住多实例等边角（见 routes::collect_pending）。
    pub command_signal: tokio::sync::watch::Sender<u64>,
    /// 各节点最近一次拉命令的时刻——「命令通道还活着吗」的唯一证据。见
    /// [`crate::control_channel`]。
    pub control_polls: Arc<crate::control_channel::ControlPolls>,
    /// 本进程启动时刻（毫秒）。只看内存证据的判定都要先过观察窗口，
    /// 否则每次重启都会把全世界报成故障。
    pub started_at_ms: i64,
}

impl AppState {
    /// 签发出一条命令后唤醒所有等待中的节点长轮询。
    /// 值本身无意义，只要「变了」就代表有新命令。
    pub fn notify_command(&self) {
        self.command_signal.send_modify(|v| *v = v.wrapping_add(1));
    }
}

/// Help markdown content per locale (loaded from `assets/help.{locale}.md`
/// at startup). Missing locales fall back to zh-CN; an empty map means UI
/// shows a placeholder.
#[derive(Clone, Default)]
pub struct HelpContent(pub Arc<std::sync::RwLock<HelpInner>>);

#[derive(Default)]
pub struct HelpInner {
    /// Map of locale ("zh-CN" / "en-US") → markdown body. First entry wins
    /// when no locale is requested.
    pub by_locale: std::collections::BTreeMap<String, String>,
    /// Which locale is the implicit default when none is requested.
    pub default_locale: String,
}

impl HelpContent {
    /// Build from a `(locale, body)` list; first entry becomes the default
    /// when callers don't pass `Accept-Language` / `?locale=` (typically zh-CN
    /// is loaded first, so it stays the implicit fallback).
    pub fn new(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut by_locale = std::collections::BTreeMap::new();
        let mut default_locale = String::new();
        for (i, (locale, body)) in entries.into_iter().enumerate() {
            if i == 0 {
                default_locale = locale.clone();
            }
            by_locale.insert(locale, body);
        }
        Self(Arc::new(std::sync::RwLock::new(HelpInner {
            by_locale,
            default_locale,
        })))
    }
    /// Snapshot for a specific locale. Unknown / missing locale falls back
    /// to the default; an entirely empty map yields empty body so the UI
    /// can render its "not loaded" placeholder.
    pub fn snapshot_for(&self, locale: Option<&str>) -> HelpSnapshot {
        let g = self.0.read().unwrap_or_else(|e| e.into_inner());
        let resolved = locale
            .and_then(|l| g.by_locale.get_key_value(l))
            .map(|(l, _)| l.clone())
            .unwrap_or_else(|| {
                if g.by_locale.contains_key(&g.default_locale) {
                    g.default_locale.clone()
                } else {
                    // empty map — synthesize locale string so UI still knows
                    g.default_locale.clone()
                }
            });
        let body = g
            .by_locale
            .get(&resolved)
            .cloned()
            .unwrap_or_default();
        HelpSnapshot {
            locale: resolved,
            body,
        }
    }
    /// Snapshot using the default locale (kept for back-compat / tests).
    #[allow(dead_code)]
    pub fn snapshot(&self) -> HelpSnapshot {
        self.snapshot_for(None)
    }
}

#[derive(serde::Serialize)]
pub struct HelpSnapshot {
    pub locale: String,
    pub body: String,
}
