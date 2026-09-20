//! 节点身份：Ed25519 请求签名（取代 mTLS 双向证书）。
//!
//! 为什么不用 mTLS：托管平台在边缘终止 TLS，不会把客户端证书转发进
//! 容器，所以「用客户端证书认节点」这条路在 PaaS 上走不通。改成
//! 「用签名认节点」后，任何能跑 HTTP 的地方都能部署——这也正是
//! DESIGN.md 原则 #3「签名而非凭据」的字面含义。
//!
//! 签名覆盖：方法 + 路径(含 query) + 时间戳 + nonce + 原始请求体。
//! 服务端校验签名、时间窗与 nonce 未重放，三者缺一不可。

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::{KeyPair, PublicKey, Result, Signature};

/// 协议版本，写进待签名字符串，便于日后演进时区分
const SCHEME: &[u8] = b"zhiwei-v1\n";

pub const HEADER_NODE: &str = "x-zhiwei-node";
pub const HEADER_TIMESTAMP: &str = "x-zhiwei-timestamp";
pub const HEADER_NONCE: &str = "x-zhiwei-nonce";
pub const HEADER_SIGNATURE: &str = "x-zhiwei-signature";

/// 允许的时钟偏差（秒）。跨境链路 + 未同步时钟也要能过。
pub const MAX_SKEW_SECONDS: i64 = 300;

/// 规范化待签名内容。
///
/// 结构：`zhiwei-v1\n<method>\n<path_and_query>\n<ts_nanos>\n<nonce_b64>\n<body>`
/// 前三项都不含换行（路径中的换行会被 URL 编码），且 body 放在最后，
/// 因此这个拼接是无歧义的，不需要额外做哈希。
pub fn canonical(
    method: &str,
    path_and_query: &str,
    ts_unix_nano: i64,
    nonce_b64: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 96);
    out.extend_from_slice(SCHEME);
    out.extend_from_slice(method.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(path_and_query.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(ts_unix_nano.to_string().as_bytes());
    out.push(b'\n');
    out.extend_from_slice(nonce_b64.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(body);
    out
}

/// 节点侧生成的一组签名头
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedHeaders {
    pub node_id: String,
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}

impl SignedHeaders {
    /// 用节点私钥对一次请求签名
    pub fn sign(
        key: &KeyPair,
        node_id: &str,
        method: &str,
        path_and_query: &str,
        body: &[u8],
    ) -> Self {
        let timestamp = crate::Timestamp::now().unix_nano();
        let mut raw = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw);
        let nonce = base64::engine::general_purpose::STANDARD.encode(raw);

        let preimage = canonical(method, path_and_query, timestamp, &nonce, body);
        let sig = key.sign(&preimage);

        Self {
            node_id: node_id.to_string(),
            timestamp,
            nonce,
            signature: base64::engine::general_purpose::STANDARD.encode(sig.as_bytes()),
        }
    }

    /// 服务端校验：时间窗 → 签名。nonce 去重由调用方用 NonceCache 负责
    /// （需要跨请求状态，不适合放在这里）。
    pub fn verify(
        &self,
        public_key: &PublicKey,
        method: &str,
        path_and_query: &str,
        body: &[u8],
    ) -> Result<()> {
        let now = crate::Timestamp::now().unix_nano();
        let skew_ns = MAX_SKEW_SECONDS.saturating_mul(1_000_000_000);
        if (now - self.timestamp).abs() > skew_ns {
            return Err(crate::Error::Invalid(format!(
                "请求时间戳超出 ±{MAX_SKEW_SECONDS}s 时间窗"
            )));
        }

        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(self.signature.as_bytes())
            .map_err(|e| crate::Error::Invalid(format!("签名不是合法 base64: {e}")))?;

        let preimage = canonical(method, path_and_query, self.timestamp, &self.nonce, body);
        KeyPair::verify(public_key, &preimage, &Signature(sig_bytes))
            .map_err(|_| crate::Error::Invalid("签名校验失败".into()))
    }
}

/// 已用 nonce 的短期缓存，用于防重放。
///
/// 时间窗内 nonce 只能用一次；过期条目会被顺带清理。
/// 注意：当前是进程内存储。多实例部署时需要换成共享存储
/// （否则同一 nonce 打到不同实例会被各自接受）。
#[derive(Default)]
pub struct NonceCache {
    inner: parking_lot::Mutex<std::collections::HashMap<String, i64>>,
}

impl NonceCache {
    /// 记下 nonce；若在有效期内已出现过则返回 false（判定为重放）
    pub fn accept(&self, nonce: &str, now_unix_nano: i64) -> bool {
        let ttl_ns = MAX_SKEW_SECONDS.saturating_mul(1_000_000_000);
        let mut map = self.inner.lock();

        // 顺带清理过期项，避免无界增长
        if map.len() > 4096 {
            map.retain(|_, ts| now_unix_nano - *ts <= ttl_ns);
        }

        if let Some(prev) = map.get(nonce) {
            if now_unix_nano - *prev <= ttl_ns {
                return false;
            }
        }
        map.insert(nonce.to_string(), now_unix_nano);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timestamp;

    fn key() -> KeyPair {
        KeyPair::generate()
    }

    #[test]
    fn signature_roundtrip() {
        let k = key();
        let body = b"telemetry-batch-bytes";
        let signed = SignedHeaders::sign(&k, "node-1", "POST", "/v1/telemetry", body);
        signed
            .verify(&k.public_key(), "POST", "/v1/telemetry", body)
            .expect("同一条请求应当验签通过");
    }

    #[test]
    fn tampering_is_rejected() {
        let k = key();
        let signed = SignedHeaders::sign(&k, "node-1", "POST", "/v1/telemetry", b"original");
        let pub_key = k.public_key();

        // 换 body
        assert!(signed
            .verify(&pub_key, "POST", "/v1/telemetry", b"forged")
            .is_err());
        // 换路径
        assert!(signed
            .verify(&pub_key, "POST", "/v1/commands", b"original")
            .is_err());
        // 换方法
        assert!(signed
            .verify(&pub_key, "GET", "/v1/telemetry", b"original")
            .is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let k = key();
        let other = key();
        let signed = SignedHeaders::sign(&k, "node-1", "POST", "/v1/telemetry", b"x");
        assert!(signed
            .verify(&other.public_key(), "POST", "/v1/telemetry", b"x")
            .is_err());
    }

    #[test]
    fn stale_timestamp_is_rejected() {
        let k = key();
        let signed = SignedHeaders::sign(&k, "node-1", "POST", "/v1/telemetry", b"x");
        let mut stale = signed.clone();
        stale.timestamp -= (MAX_SKEW_SECONDS + 30) * 1_000_000_000;
        assert!(stale
            .verify(&k.public_key(), "POST", "/v1/telemetry", b"x")
            .is_err());
    }

    #[test]
    fn nonce_cache_detects_replay() {
        let cache = NonceCache::default();
        let now = Timestamp::now().unix_nano();

        assert!(cache.accept("nonce-a", now), "首次出现应当放行");
        assert!(
            !cache.accept("nonce-a", now + 1),
            "同一时间窗内重复应当拒绝"
        );
        assert!(cache.accept("nonce-b", now), "不同 nonce 互不影响");

        let after_window = now + (MAX_SKEW_SECONDS + 1) * 1_000_000_000;
        assert!(cache.accept("nonce-a", after_window), "过期后允许复用");
    }
}
