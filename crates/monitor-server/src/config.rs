use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub data_dir: PathBuf,
    pub listen: String,
    pub server_cert_cn: String,
    /// ops-server 的本地地址（仅回环）
    pub ops_endpoint: String,
}

impl MonitorConfig {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let raw: RawConfig = if path.exists() {
            toml::from_str(&std::fs::read_to_string(path)?)?
        } else {
            tracing::warn!(?path, "config file not found, using defaults");
            RawConfig::default()
        };
        Ok(Self::from_raw(raw))
    }

    fn from_raw(raw: RawConfig) -> Self {
        Self {
            data_dir: PathBuf::from(raw.data_dir.unwrap_or_else(|| "data".into())),
            listen: raw.listen.unwrap_or_else(default_listen),
            server_cert_cn: raw
                .server_cert_cn
                .unwrap_or_else(|| "zhiwei-monitor".into()),
            ops_endpoint: raw
                .ops_endpoint
                .unwrap_or_else(|| "http://127.0.0.1:8444/exec".into()),
        }
    }
}

/// PaaS platforms (Render / Railway / Northflank / Heroku-style) inject `PORT`
/// and expect the process to bind `0.0.0.0:$PORT`. Honour it when the config
/// file does not pin a listen address.
fn default_listen() -> String {
    match std::env::var("PORT") {
        Ok(port) if !port.trim().is_empty() => format!("0.0.0.0:{}", port.trim()),
        _ => "127.0.0.1:8443".into(),
    }
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            listen: default_listen(),
            server_cert_cn: "zhiwei-monitor".into(),
            ops_endpoint: "http://127.0.0.1:8444/exec".into(),
        }
    }
}

#[derive(Default, serde::Deserialize)]
struct RawConfig {
    data_dir: Option<String>,
    listen: Option<String>,
    server_cert_cn: Option<String>,
    ops_endpoint: Option<String>,
}
