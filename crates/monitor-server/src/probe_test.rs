//! One-shot test when "create / edit probe": run the probe immediately on
//! the console side, don't persist.
//!
//! Judgment layers match the node side (`crates/node-agent/src/probes.rs`):
//!   - Can't connect / timeout / wrong status / TLS handshake fails → `down`
//!   - Connects but body, banner, latency, or cert validity fails → `degraded`
//!   - Otherwise → `ok`
//!
//! Why we implement our own instead of reusing node-side code: the node-side
//! probe depends on its runtime (`IoStream` abstraction + monitor CA pinning),
//! while the console needs to test before "the probe is bound to a node", so
//! only the monitor itself can initiate the connection. When the judgment
//! rules change, both sides must be updated together.
//!
//! The `reason` / `args` in the result are machine-readable; the frontend
//! picks display text — the monitor does not produce human-readable strings.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Max bytes read from response body (avoid memory blowup from huge responses),
/// matches node side
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Outcome {
    pub state: &'static str,
    pub latency_ms: Option<f64>,
    pub status_code: Option<i64>,
    /// Machine-readable reason; the frontend picks display text based on this
    pub reason: &'static str,
    pub args: Value,
}

impl Outcome {
    fn ok(latency_ms: f64, status_code: Option<i64>) -> Self {
        Self {
            state: "ok",
            latency_ms: Some(latency_ms),
            status_code,
            reason: "ok",
            args: json!({}),
        }
    }

    const fn degraded(
        latency_ms: Option<f64>,
        status_code: Option<i64>,
        reason: &'static str,
        args: Value,
    ) -> Self {
        Self {
            state: "degraded",
            latency_ms,
            status_code,
            reason,
            args,
        }
    }

    const fn down(reason: &'static str, args: Value) -> Self {
        Self {
            state: "down",
            latency_ms: None,
            status_code: None,
            reason,
            args,
        }
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Soft failure: latency exceeds `expect.max_latency_ms`
fn latency_verdict(expect: &Value, latency_ms: f64, status_code: Option<i64>) -> Option<Outcome> {
    let max = expect
        .get("max_latency_ms")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(f64::MAX);
    if latency_ms > max {
        return Some(Outcome::degraded(
            Some(latency_ms),
            status_code,
            "latency",
            json!({ "ms": latency_ms.round(), "threshold": max.round() }),
        ));
    }
    None
}

pub async fn run(kind: &str, target: &Value, expect: &Value, timeout_ms: i64) -> Outcome {
    let timeout = Duration::from_millis(u64::try_from(timeout_ms.clamp(100, 60_000)).unwrap_or(100));
    match kind {
        "http" => tokio::time::timeout(timeout, probe_http(target, expect))
            .await
            .unwrap_or_else(|_| Outcome::down("timeout", json!({ "ms": timeout.as_millis() }))),
        "tcp" => tokio::time::timeout(timeout, probe_tcp(target, expect))
            .await
            .unwrap_or_else(|_| Outcome::down("timeout", json!({ "ms": timeout.as_millis() }))),
        "tls" => tokio::time::timeout(timeout, probe_tls(target, expect))
            .await
            .unwrap_or_else(|_| Outcome::down("timeout", json!({ "ms": timeout.as_millis() }))),
        other => Outcome::down("unsupported", json!({ "kind": other })),
    }
}

// ---------- HTTP ----------

async fn probe_http(target: &Value, expect: &Value) -> Outcome {
    let url = str_field(target, "url");
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Outcome::down("bad_url", json!({}));
    }
    let method = {
        let m = str_field(target, "method");
        if m.is_empty() {
            "GET".to_string()
        } else {
            m.to_ascii_uppercase()
        }
    };
    let tls_verify = expect
        .get("tls_verify")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    let builder = reqwest::Client::builder()
        // Matches node side: don't follow redirects, 3xx itself counts as "ok"
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(!tls_verify)
        .user_agent("zhiwei-probe/0.1")
        // Outer run() already caps with probe timeout; leave a second cap here
        .timeout(Duration::from_millis(60_000));

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            return Outcome::down("connect", json!({ "target": url, "detail": e.to_string() }))
        }
    };

    let mut req = match method.as_str() {
        "HEAD" => client.head(&url),
        "POST" => client.post(&url),
        _ => client.get(&url),
    };
    if let Some(headers) = target.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in headers {
            if let Some(val) = v.as_str() {
                req = req.header(k, val);
            }
        }
    }
    if method == "POST" {
        let body = str_field(target, "body");
        req = req.body(body);
    }

    let started = Instant::now();
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let reason = if e.is_timeout() { "timeout" } else { "connect" };
            let detail = e.to_string();
            return Outcome::down(reason, json!({ "target": url, "detail": detail }));
        }
    };
    let status = resp.status().as_u16();
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let status_code = Some(i64::from(status));

    let body = match resp.bytes().await {
        Ok(b) => String::from_utf8_lossy(&b[..b.len().min(MAX_BODY_BYTES)]).to_string(),
        Err(e) => {
            return Outcome::down(
                "connect",
                json!({ "target": url, "detail": format!("Failed to read response body: {e}") }),
            )
        }
    };

    let expected: Vec<i64> = expect
        .get("status")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default();
    let status_ok = if expected.is_empty() {
        (200..400).contains(&status)
    } else {
        expected.contains(&i64::from(status))
    };
    if !status_ok {
        return Outcome {
            state: "down",
            latency_ms: Some(latency_ms),
            status_code,
            reason: "status",
            args: json!({ "got": status, "expected": expected }),
        };
    }

    if let Some(needles) = expect.get("body_contains").and_then(|v| v.as_array()) {
        for needle in needles.iter().filter_map(|x| x.as_str()) {
            if !body.contains(needle) {
                return Outcome::degraded(
                    Some(latency_ms),
                    status_code,
                    "body",
                    json!({ "needle": needle }),
                );
            }
        }
    }

    latency_verdict(expect, latency_ms, status_code)
        .unwrap_or_else(|| Outcome::ok(latency_ms, status_code))
}

// ---------- TCP ----------

async fn probe_tcp(target: &Value, expect: &Value) -> Outcome {
    let host = str_field(target, "host");
    let port = u16::try_from(target.get("port").and_then(serde_json::Value::as_u64).unwrap_or(0)).unwrap_or(0);
    if host.is_empty() || port == 0 {
        return Outcome::down("bad_url", json!({}));
    }
    let started = Instant::now();
    let mut stream = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            return Outcome::down(
                "connect",
                json!({ "target": format!("{host}:{port}"), "detail": e.to_string() }),
            )
        }
    };

    if let Some(needle) = expect.get("banner_contains").and_then(|v| v.as_str()) {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 512];
        let read = tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf))
            .await
            .ok()
            .and_then(std::result::Result::ok)
            .unwrap_or(0);
        let banner = String::from_utf8_lossy(&buf[..read]).to_string();
        // No banner is not a failure (many services don't speak first)
        if read > 0 && !banner.contains(needle) {
            let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
            return Outcome::degraded(
                Some(latency_ms),
                None,
                "banner",
                json!({ "needle": needle }),
            );
        }
    }

    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    latency_verdict(expect, latency_ms, None).unwrap_or_else(|| Outcome::ok(latency_ms, None))
}

// ---------- TLS ----------

async fn probe_tls(target: &Value, expect: &Value) -> Outcome {
    let host = str_field(target, "host");
    let port = u16::try_from(target.get("port").and_then(serde_json::Value::as_u64).unwrap_or(443)).unwrap_or(0);
    if host.is_empty() || port == 0 {
        return Outcome::down("bad_url", json!({}));
    }
    let sni = {
        let s = str_field(target, "sni");
        if s.is_empty() {
            host.clone()
        } else {
            s
        }
    };
    let verify = expect
        .get("verify")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    let started = Instant::now();
    let stream = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            return Outcome::down(
                "connect",
                json!({ "target": format!("{host}:{port}"), "detail": e.to_string() }),
            )
        }
    };
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config(!verify)));
    let name = match rustls::pki_types::ServerName::try_from(sni.clone()) {
        Ok(n) => n,
        Err(e) => return Outcome::down("tls", json!({ "detail": format!("Invalid SNI {sni}: {e}") })),
    };
    let tls_stream = match connector.connect(name, stream).await {
        Ok(s) => s,
        Err(e) => return Outcome::down("tls", json!({ "detail": e.to_string() })),
    };
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

    // Leaf cert remaining validity: same algorithm as node side
    let days_left = tls_stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .and_then(|der| {
            use x509_parser::prelude::FromDer;
            x509_parser::certificate::X509Certificate::from_der(der.as_ref())
                .ok()
                .map(|(_, cert)| cert.validity().not_after.timestamp())
        })
        .map(|exp| (exp - chrono::Utc::now().timestamp()) / 86_400);

    if let Some(days) = days_left {
        if days < 0 {
            return Outcome {
                state: "down",
                latency_ms: Some(latency_ms),
                status_code: None,
                reason: "cert_expired",
                args: json!({ "days": -days }),
            };
        }
        let min_days = expect
            .get("min_days_valid")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(30);
        if days < min_days {
            return Outcome::degraded(
                Some(latency_ms),
                None,
                "cert_days",
                json!({ "days": days, "min": min_days }),
            );
        }
    }

    latency_verdict(expect, latency_ms, None).unwrap_or_else(|| Outcome::ok(latency_ms, None))
}

fn client_config(skip_verify: bool) -> rustls::ClientConfig {
    if skip_verify {
        return rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
    }
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// "No verification" when `tls_verify = false`: testing only, affects this
/// one connection.
#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_reports_down_for_closed_port() {
        // Port 1 on the local host will definitely refuse connection
        let out = run(
            "tcp",
            &json!({ "host": "127.0.0.1", "port": 1 }),
            &json!({}),
            1000,
        )
        .await;
        assert_eq!(out.state, "down");
        assert_eq!(out.reason, "connect");
    }

    #[tokio::test]
    async fn tcp_ok_against_listening_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let out = run(
            "tcp",
            &json!({ "host": "127.0.0.1", "port": port }),
            &json!({}),
            1000,
        )
        .await;
        assert_eq!(out.state, "ok");
    }

    #[tokio::test]
    async fn http_checks_status_and_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}",
                        )
                        .await;
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/healthz");

        let out = run(
            "http",
            &json!({ "url": url }),
            &json!({ "status": [200], "body_contains": ["\"ok\""] }),
            2000,
        )
        .await;
        assert_eq!(out.state, "ok", "{out:?}");

        let mismatch = run(
            "http",
            &json!({ "url": url }),
            &json!({ "status": [500] }),
            2000,
        )
        .await;
        assert_eq!(mismatch.state, "down");
        assert_eq!(mismatch.reason, "status");

        let body_mismatch = run(
            "http",
            &json!({ "url": url }),
            &json!({ "body_contains": ["not-here"] }),
            2000,
        )
        .await;
        assert_eq!(body_mismatch.state, "degraded");
        assert_eq!(body_mismatch.reason, "body");
    }

    #[tokio::test]
    async fn http_rejects_bad_url_without_dialing() {
        let out = run("http", &json!({ "url": "ftp://x" }), &json!({}), 500).await;
        assert_eq!(out.state, "down");
        assert_eq!(out.reason, "bad_url");
    }

    #[tokio::test]
    async fn unknown_kind_is_reported() {
        let out = run("icmp", &json!({}), &json!({}), 500).await;
        assert_eq!(out.state, "down");
        assert_eq!(out.reason, "unsupported");
    }
}
