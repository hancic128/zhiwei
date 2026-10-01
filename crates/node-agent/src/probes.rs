//! 服务探活：拉配置 → 到点执行 → 上报结果。
//!
//! 探针在节点侧执行（能探 localhost / 容器端口 / 内网），
//! 配置由 monitor 通过签名接口下发，结果同样签名上报。
//!
//! 判定分层：
//!   - 连不上 / 超时 / 状态码不符 / 握手失败 → `down`（硬失败）
//!   - 能连上但内容或延迟不符 / 证书临近到期 → `degraded`（软失败）
//!   - 其余 → `ok`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::http::build_client_config;
use crate::NodeState;

/// 配置刷新间隔
const CONFIG_REFRESH: Duration = Duration::from_secs(30);
/// 调度轮询间隔
const TICK: Duration = Duration::from_secs(5);
/// 响应体最多读取的字节数（避免大响应把内存吃满）
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct ProbeSpec {
    id: String,
    service: String,
    name: String,
    kind: String,
    target: Value,
    expect: Value,
    interval_seconds: u64,
    timeout_ms: u64,
}

struct Outcome {
    state: &'static str,
    latency_ms: Option<f64>,
    status_code: Option<i64>,
    error: String,
}

impl Outcome {
    fn ok(latency_ms: f64, status_code: Option<i64>) -> Self {
        Self {
            state: "ok",
            latency_ms: Some(latency_ms),
            status_code,
            error: String::new(),
        }
    }

    fn degraded(
        latency_ms: Option<f64>,
        status_code: Option<i64>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            state: "degraded",
            latency_ms,
            status_code,
            error: error.into(),
        }
    }

    fn down(error: impl Into<String>) -> Self {
        Self {
            state: "down",
            latency_ms: None,
            status_code: None,
            error: error.into(),
        }
    }
}

/// 探针主循环：常驻在节点进程里
pub async fn run_loop(monitor: String, state: Arc<NodeState>, node_id: String) {
    let mut specs: Vec<ProbeSpec> = Vec::new();
    let mut next_due: HashMap<String, Instant> = HashMap::new();
    let mut last_fetch: Option<Instant> = None;
    let mut logged_empty = false;

    loop {
        if last_fetch
            .map(|t| t.elapsed() >= CONFIG_REFRESH)
            .unwrap_or(true)
        {
            match fetch_specs(&monitor, &state, &node_id).await {
                Ok(list) => {
                    if list.is_empty() && !logged_empty {
                        tracing::debug!("当前没有分配给本节点的探针");
                        logged_empty = true;
                    } else if !list.is_empty() {
                        logged_empty = false;
                    }
                    specs = list;
                    last_fetch = Some(Instant::now());
                }
                Err(e) => tracing::warn!(error = %e, "拉取探针配置失败"),
            }
        }

        let now = Instant::now();
        let mut results: Vec<Value> = Vec::new();
        for spec in &specs {
            if next_due.get(&spec.id).map(|t| *t > now).unwrap_or(false) {
                continue;
            }
            next_due.insert(
                spec.id.clone(),
                now + Duration::from_secs(spec.interval_seconds.clamp(10, 86_400)),
            );

            let outcome = execute(spec).await;
            tracing::debug!(
                service = %spec.service,
                probe = %spec.name,
                state = outcome.state,
                latency_ms = outcome.latency_ms,
                error = %outcome.error,
                "探针执行完成"
            );
            results.push(serde_json::json!({
                "probe_id": spec.id,
                "ts_unix_nano": zhiwei_common::Timestamp::now().unix_nano(),
                "state": outcome.state,
                "latency_ms": outcome.latency_ms,
                "status_code": outcome.status_code,
                "error": outcome.error,
            }));
        }

        if !results.is_empty() {
            if let Err(e) = post_results(&monitor, &state, &node_id, &results).await {
                tracing::warn!(error = %e, "上报探针结果失败，下一轮重试");
                // 上报失败不丢结果语义：立即重试（下一 tick 就会再跑一次）
                for r in &results {
                    if let Some(id) = r.get("probe_id").and_then(|v| v.as_str()) {
                        next_due.insert(id.to_string(), Instant::now());
                    }
                }
            }
        }

        // 已被删除的探针不再保留调度记录
        next_due.retain(|id, _| specs.iter().any(|s| &s.id == id));

        tokio::time::sleep(TICK).await;
    }
}

async fn fetch_specs(
    monitor: &str,
    state: &Arc<NodeState>,
    node_id: &str,
) -> anyhow::Result<Vec<ProbeSpec>> {
    let t = crate::transport(monitor, state)?;
    let path = format!("/v1/probe-config?node_id={node_id}");
    let (status, raw) = t
        .request_json("GET", &path, &[], node_id, &state.signing_key)
        .await?;
    if status != 200 {
        anyhow::bail!(
            "拉取探针配置返回 {status}: {}",
            String::from_utf8_lossy(&raw)
        );
    }

    let v: Value = serde_json::from_slice(&raw).context("解析探针配置失败")?;
    let list = v
        .get("probes")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(list
        .into_iter()
        .filter_map(|p| {
            Some(ProbeSpec {
                id: p.get("id")?.as_str()?.to_string(),
                service: p
                    .get("service")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                name: p
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                kind: p.get("kind")?.as_str()?.to_string(),
                target: p.get("target").cloned().unwrap_or(Value::Null),
                expect: p.get("expect").cloned().unwrap_or(Value::Null),
                interval_seconds: p
                    .get("interval_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(60),
                timeout_ms: p.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(5000),
            })
        })
        .collect())
}

async fn post_results(
    monitor: &str,
    state: &Arc<NodeState>,
    node_id: &str,
    results: &[Value],
) -> anyhow::Result<()> {
    let body = serde_json::json!({ "results": results }).to_string();
    let t = crate::transport(monitor, state)?;
    let (status, raw) = t
        .request_json(
            "POST",
            "/v1/probe-results",
            body.as_bytes(),
            node_id,
            &state.signing_key,
        )
        .await?;
    if status != 204 {
        anyhow::bail!(
            "上报探针结果返回 {status}: {}",
            String::from_utf8_lossy(&raw)
        );
    }
    Ok(())
}

async fn execute(spec: &ProbeSpec) -> Outcome {
    let timeout = Duration::from_millis(spec.timeout_ms.clamp(100, 60_000));
    match spec.kind.as_str() {
        "http" => tokio::time::timeout(timeout, probe_http(spec))
            .await
            .unwrap_or_else(|_| Outcome::down("检查超时")),
        "tcp" => tokio::time::timeout(timeout, probe_tcp(spec))
            .await
            .unwrap_or_else(|_| Outcome::down("检查超时")),
        "tls" => tokio::time::timeout(timeout, probe_tls(spec))
            .await
            .unwrap_or_else(|_| Outcome::down("检查超时")),
        other => Outcome::down(format!("不支持的探针类型 {other}")),
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// 软失败判定：延迟超过 `expect.max_latency_ms`
fn latency_verdict(spec: &ProbeSpec, latency_ms: f64, status_code: Option<i64>) -> Option<Outcome> {
    let max = spec
        .expect
        .get("max_latency_ms")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::MAX);
    if latency_ms > max {
        return Some(Outcome::degraded(
            Some(latency_ms),
            status_code,
            format!("延迟 {latency_ms:.0}ms 超过阈值 {max:.0}ms"),
        ));
    }
    None
}

// ---------- HTTP ----------

async fn probe_http(spec: &ProbeSpec) -> Outcome {
    let url = str_field(&spec.target, "url");
    let method = {
        let m = str_field(&spec.target, "method");
        if m.is_empty() {
            "GET".to_string()
        } else {
            m.to_ascii_uppercase()
        }
    };
    let body = str_field(&spec.target, "body");
    let tls_verify = spec
        .expect
        .get("tls_verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let started = Instant::now();
    let (status, text) = match fetch_http(&url, &method, &spec.target, &body, tls_verify).await {
        Ok(v) => v,
        Err(e) => return Outcome::down(format!("{e:#}")),
    };
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let status_code = Some(status as i64);

    // 期望状态码：未配置时按 2xx/3xx 视为正常
    let expected: Vec<i64> = spec
        .expect
        .get("status")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default();
    let status_ok = if expected.is_empty() {
        (200..400).contains(&status)
    } else {
        expected.contains(&(status as i64))
    };
    if !status_ok {
        return Outcome {
            state: "down",
            latency_ms: Some(latency_ms),
            status_code,
            error: format!("状态码 {status} 不符合期望"),
        };
    }

    if let Some(needles) = spec.expect.get("body_contains").and_then(|v| v.as_array()) {
        for needle in needles.iter().filter_map(|x| x.as_str()) {
            if !text.contains(needle) {
                return Outcome::degraded(
                    Some(latency_ms),
                    status_code,
                    format!("响应体不含 {needle:?}"),
                );
            }
        }
    }

    latency_verdict(spec, latency_ms, status_code)
        .unwrap_or_else(|| Outcome::ok(latency_ms, status_code))
}

/// 发一次 HTTP(S) 请求，返回 (状态码, 响应体文本)
async fn fetch_http(
    url: &str,
    method: &str,
    target: &Value,
    body: &str,
    tls_verify: bool,
) -> anyhow::Result<(u16, String)> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        anyhow::bail!("URL 必须以 http:// 或 https:// 开头");
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') && p.chars().all(|c| c.is_ascii_digit()) => (
            h.to_string(),
            p.parse::<u16>().unwrap_or(if tls { 443 } else { 80 }),
        ),
        _ => (authority.to_string(), if tls { 443 } else { 80 }),
    };

    let stream = tokio::net::TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| format!("连接 {host}:{port}"))?;

    let io: Box<dyn crate::http::IoStream> = if tls {
        let cfg = build_client_config(None, !tls_verify)?;
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg));
        let name =
            rustls::pki_types::ServerName::try_from(host.clone()).context("非法 server name")?;
        Box::new(connector.connect(name, stream).await?)
    } else {
        Box::new(stream)
    };

    let io = hyper_util::rt::TokioIo::new(io);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut req = hyper::Request::builder()
        .method(method)
        .uri(path)
        .header("Host", authority)
        .header("User-Agent", "zhiwei-probe/0.1");
    if let Some(headers) = target.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in headers {
            if let Some(val) = v.as_str() {
                req = req.header(k, val);
            }
        }
    }
    let req = req.body(http_body_util::Full::new(bytes::Bytes::from(
        body.to_string(),
    )))?;

    let res = sender.send_request(req).await?;
    let status = res.status().as_u16();
    use http_body_util::BodyExt;
    let bytes = res.into_body().collect().await?.to_bytes();
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BODY_BYTES)]).to_string();
    Ok((status, text))
}

// ---------- TCP ----------

async fn probe_tcp(spec: &ProbeSpec) -> Outcome {
    let host = str_field(&spec.target, "host");
    let port = spec
        .target
        .get("port")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u16;

    let started = Instant::now();
    let mut stream = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => return Outcome::down(format!("连接 {host}:{port} 失败: {e}")),
    };

    // 可选 banner 校验：读一小段（拿不到也不算失败）
    if let Some(needle) = spec.expect.get("banner_contains").and_then(|v| v.as_str()) {
        let mut buf = vec![0u8; 512];
        let read = tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf))
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(0);
        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
        let banner = String::from_utf8_lossy(&buf[..read]).to_string();
        if read > 0 && !banner.contains(needle) {
            return Outcome::degraded(Some(latency_ms), None, format!("banner 不含 {needle:?}"));
        }
    }

    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    latency_verdict(spec, latency_ms, None).unwrap_or_else(|| Outcome::ok(latency_ms, None))
}

// ---------- TLS ----------

async fn probe_tls(spec: &ProbeSpec) -> Outcome {
    let host = str_field(&spec.target, "host");
    let port = spec
        .target
        .get("port")
        .and_then(|v| v.as_u64())
        .unwrap_or(443) as u16;
    let sni = {
        let s = str_field(&spec.target, "sni");
        if s.is_empty() {
            host.clone()
        } else {
            s
        }
    };
    let verify = spec
        .expect
        .get("verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let started = Instant::now();
    let stream = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => return Outcome::down(format!("连接 {host}:{port} 失败: {e}")),
    };
    let cfg = match build_client_config(None, !verify) {
        Ok(c) => c,
        Err(e) => return Outcome::down(format!("TLS 配置失败: {e}")),
    };
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg));
    let name = match rustls::pki_types::ServerName::try_from(sni.clone()) {
        Ok(n) => n,
        Err(e) => return Outcome::down(format!("非法 SNI {sni}: {e}")),
    };
    let tls_stream = match connector.connect(name, stream).await {
        Ok(s) => s,
        Err(e) => return Outcome::down(format!("TLS 握手失败: {e}")),
    };
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

    // 从握手结果里取叶子证书，算剩余有效期
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
                error: format!("证书已过期 {days} 天"),
            };
        }
        let min_days = spec
            .expect
            .get("min_days_valid")
            .and_then(|v| v.as_i64())
            .unwrap_or(30);
        if days < min_days {
            return Outcome::degraded(
                Some(latency_ms),
                None,
                format!("证书剩余 {days} 天，少于阈值 {min_days} 天"),
            );
        }
    }

    latency_verdict(spec, latency_ms, None).unwrap_or_else(|| Outcome::ok(latency_ms, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn spec(kind: &str, target: Value, expect: Value) -> ProbeSpec {
        ProbeSpec {
            id: "p1".into(),
            service: "svc".into(),
            name: "probe".into(),
            kind: kind.into(),
            target,
            expect,
            interval_seconds: 60,
            timeout_ms: 1000,
        }
    }

    #[tokio::test]
    async fn tcp_probe_reports_down_for_closed_port() {
        // 端口 1 在本机必然拒绝连接
        let s = spec(
            "tcp",
            serde_json::json!({"host": "127.0.0.1", "port": 1}),
            serde_json::json!({}),
        );
        let out = execute(&s).await;
        assert_eq!(out.state, "down");
        assert!(!out.error.is_empty());
    }

    #[tokio::test]
    async fn tcp_probe_ok_against_listening_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let s = spec(
            "tcp",
            serde_json::json!({"host": "127.0.0.1", "port": port}),
            serde_json::json!({}),
        );
        let out = execute(&s).await;
        assert_eq!(out.state, "ok");
    }

    #[tokio::test]
    async fn http_probe_checks_status_and_body() {
        // 起一个只回固定内容的极简 HTTP 服务
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
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}")
                        .await;
                });
            }
        });

        let ok = spec(
            "http",
            serde_json::json!({"url": format!("http://127.0.0.1:{port}/healthz")}),
            serde_json::json!({"status": [200], "body_contains": ["\"ok\""]}),
        );
        assert_eq!(execute(&ok).await.state, "ok");

        let mismatch = spec(
            "http",
            serde_json::json!({"url": format!("http://127.0.0.1:{port}/healthz")}),
            serde_json::json!({"status": [500]}),
        );
        assert_eq!(execute(&mismatch).await.state, "down");

        let body_mismatch = spec(
            "http",
            serde_json::json!({"url": format!("http://127.0.0.1:{port}/healthz")}),
            serde_json::json!({"body_contains": ["not-here"]}),
        );
        assert_eq!(execute(&body_mismatch).await.state, "degraded");
    }
}
