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
}
