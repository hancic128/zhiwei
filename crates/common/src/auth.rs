//! Node identity: Ed25519 request signing (replaces mTLS mutual certificates).
//!
//! Why not mTLS: managed platforms terminate TLS at the edge and do not
//! forward client certificates into containers, so the "authenticate nodes via
//! client certificates" approach won't work on PaaS. After switching to
//! "authenticate nodes via signatures", it can be deployed anywhere that can
//! run HTTP — which is also the literal meaning of
//! DESIGN.md principle #3 "signatures instead of credentials".
//!
//! Signature covers: method + path (with query) + timestamp + nonce + raw request body.
//! Server verifies signature, time window, and nonce non-replay -- all three required.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::{KeyPair, PublicKey, Result, Signature};

/// Protocol version, written into string to sign, facilitates future protocol evolution
const SCHEME: &[u8] = b"zhiwei-v1\n";

pub const HEADER_NODE: &str = "x-zhiwei-node";
pub const HEADER_TIMESTAMP: &str = "x-zhiwei-timestamp";
pub const HEADER_NONCE: &str = "x-zhiwei-nonce";
pub const HEADER_SIGNATURE: &str = "x-zhiwei-signature";

/// Allowed clock skew (seconds). Cross-region links + unsynced clocks must still work.
pub const MAX_SKEW_SECONDS: i64 = 300;

/// Canonicalize content to sign.
///
/// Format: `zhiwei-v1\n<method>\n<path_and_query>\n<ts_nanos>\n<nonce_b64>\n<body>`
/// First three fields contain no newlines (newlines in path are URL-encoded), and body comes last,
/// so this concatenation is unambiguous, no extra hashing needed.
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

/// Headers signed by the node side
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedHeaders {
    pub node_id: String,
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}

impl SignedHeaders {
    /// Sign one request using the node's private key
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

    /// Server verification: time window -> signature. Nonce deduplication is caller's responsibility via NonceCache
    /// (requires cross-request state, not suitable here).
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
                "Request timestamp outside ±{MAX_SKEW_SECONDS}s time window"
            )));
        }

        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(self.signature.as_bytes())
            .map_err(|e| crate::Error::Invalid(format!("Signature is not valid base64: {e}")))?;

        let preimage = canonical(method, path_and_query, self.timestamp, &self.nonce, body);
        KeyPair::verify(public_key, &preimage, &Signature(sig_bytes))
            .map_err(|_| crate::Error::Invalid("Signature verification failed".into()))
    }
}

/// Short-term cache of used nonces, for replay protection.
///
/// Nonce within time window can only be used once; expired entries are cleaned up together.
/// Note: currently in-process storage. Multi-instance deployments need shared storage
/// (otherwise same nonce hitting different instances will each accept it).
#[derive(Default)]
pub struct NonceCache {
    inner: parking_lot::Mutex<std::collections::HashMap<String, i64>>,
}

impl NonceCache {
    /// Record a nonce; returns false if it was already seen within validity window (replay detected)
    pub fn accept(&self, nonce: &str, now_unix_nano: i64) -> bool {
        let ttl_ns = MAX_SKEW_SECONDS.saturating_mul(1_000_000_000);
        let mut map = self.inner.lock();

        // Clean up expired entries while we're here, avoid unbounded growth
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
            .expect("same request should pass verification");
    }

    #[test]
    fn tampering_is_rejected() {
        let k = key();
        let signed = SignedHeaders::sign(&k, "node-1", "POST", "/v1/telemetry", b"original");
        let pub_key = k.public_key();

        // Change body
        assert!(signed
            .verify(&pub_key, "POST", "/v1/telemetry", b"forged")
            .is_err());
        // Change path
        assert!(signed
            .verify(&pub_key, "POST", "/v1/commands", b"original")
            .is_err());
        // Change method
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

        assert!(cache.accept("nonce-a", now), "first occurrence should be accepted");
        assert!(
            !cache.accept("nonce-a", now + 1),
            "same window repeat should be rejected"
        );
        assert!(cache.accept("nonce-b", now), "different nonces don't affect each other");

        let after_window = now + (MAX_SKEW_SECONDS + 1) * 1_000_000_000;
        assert!(cache.accept("nonce-a", after_window), "reuse allowed after expiry");
    }
}
