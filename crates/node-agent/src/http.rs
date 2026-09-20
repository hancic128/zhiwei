//! 节点 → monitor 的 HTTP 客户端。
//!
//! 两种传输都支持：
//!   - `https://`：自建部署，用 rustls；可 pin monitor 的 CA，也可跳过校验（首次 enroll）
//!   - `http://`：置于托管平台边缘之后（边缘已终止 TLS）
//!
//! 身份不再靠客户端证书，而是每次请求带一组 Ed25519 签名头
//! （见 zhiwei_common::auth）。这让节点能部署到任何 HTTP 环境。

use std::sync::Arc;

use anyhow::{bail, Context};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsConnector;
use zhiwei_common::auth::{HEADER_NODE, HEADER_NONCE, HEADER_SIGNATURE, HEADER_TIMESTAMP};
use zhiwei_common::{KeyPair, SignedHeaders};

pub struct HttpTransport {
    host: String,
    port: u16,
    #[allow(dead_code)]
    tls: bool,
    connector: Option<TlsConnector>,
    /// 请求行与 Host 头里用的 authority（含端口时带端口）
    authority: String,
}

impl HttpTransport {
    /// `base` 形如 `https://127.0.0.1:8443` 或 `http://monitor.example.com`
    ///
    /// `ca_pem`：自建部署时用于 pin monitor 的 CA；托管平台（正经证书）传 None 走系统根。
    /// `skip_verify`：仅首次 enroll 需要（此时还没有 CA 可 pin）。
    pub fn new(base: &str, ca_pem: Option<&str>, skip_verify: bool) -> anyhow::Result<Self> {
        let (tls, rest) = if let Some(r) = base.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = base.strip_prefix("http://") {
            (false, r)
        } else {
            bail!("monitor 地址必须以 http:// 或 https:// 开头");
        };
        let rest = rest.trim_end_matches('/');
        let authority = rest.to_string();
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) if !h.contains(']') && p.chars().all(|c| c.is_ascii_digit()) => (
                h.to_string(),
                p.parse::<u16>().unwrap_or(if tls { 443 } else { 80 }),
            ),
            _ => (rest.to_string(), if tls { 443 } else { 80 }),
        };

        let connector = if tls {
            let cfg = build_client_config(ca_pem, skip_verify)?;
            Some(TlsConnector::from(Arc::new(cfg)))
        } else {
            None
        };

        Ok(Self {
            host,
            port,
            tls,
            connector,
            authority,
        })
    }

    /// 发一次带签名的请求，返回 (状态码, 响应体)
    pub async fn request(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        node_id: &str,
        key: &KeyPair,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        self.request_with_type(
            method,
            path_and_query,
            body,
            node_id,
            key,
            "application/protobuf",
        )
        .await
    }

    /// 同上，但用 `application/json`（探针配置 / 结果这类 JSON 载荷）
    pub async fn request_json(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        node_id: &str,
        key: &KeyPair,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        self.request_with_type(
            method,
            path_and_query,
            body,
            node_id,
            key,
            "application/json",
        )
        .await
    }

    async fn request_with_type(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        node_id: &str,
        key: &KeyPair,
        content_type: &str,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        let signed = SignedHeaders::sign(key, node_id, method, path_and_query, body);
        let headers = [
            (HEADER_NODE, signed.node_id),
            (HEADER_TIMESTAMP, signed.timestamp.to_string()),
            (HEADER_NONCE, signed.nonce),
            (HEADER_SIGNATURE, signed.signature),
        ];
        self.send(method, path_and_query, body, &headers, content_type)
            .await
    }

    /// 首次 enroll：还没有签名身份，凭一次性 bootstrap token 自证。
    /// 此时也没有 CA 可 pin（`new(.., skip_verify = true)`）。
    pub async fn enroll(&self, token: &str, body: &[u8]) -> anyhow::Result<(u16, Vec<u8>)> {
        self.send(
            "POST",
            "/v1/enroll",
            body,
            &[("authorization", format!("Bearer {token}"))],
            "application/protobuf",
        )
        .await
    }

    /// 底层发送：`headers` 是除 Host / Content-Length / Content-Type 之外的附加头
    async fn send(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        headers: &[(&str, String)],
        content_type: &str,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        let stream = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("连接 {}:{}", self.host, self.port))?;

        let mut io: Box<dyn IoStream> = if let Some(connector) = &self.connector {
            let server_name =
                ServerName::try_from(self.host.clone()).context("非法 server name")?;
            Box::new(connector.connect(server_name, stream).await?)
        } else {
            Box::new(stream)
        };

        let mut req = format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: {}\r\nContent-Type: {content_type}\r\n",
            self.authority,
        );
        for (name, value) in headers {
            req.push_str(name);
            req.push_str(": ");
            req.push_str(value);
            req.push_str("\r\n");
        }
        req.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ));

        AsyncWriteExt::write_all(&mut io, req.as_bytes()).await?;
        AsyncWriteExt::write_all(&mut io, body).await?;
        let mut resp = Vec::new();
        AsyncReadExt::read_to_end(&mut io, &mut resp).await?;

        let split = resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .context("响应缺少头部终止符")?;
        let head = String::from_utf8_lossy(&resp[..split]).to_string();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .context("无法解析响应状态码")?;

        Ok((status, resp[split + 4..].to_vec()))
    }
}

/// 让 TlsStream 与 TcpStream 能共用一个 trait object
pub(crate) trait IoStream:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> IoStream for T {}

pub(crate) fn build_client_config(
    ca_pem: Option<&str>,
    skip_verify: bool,
) -> anyhow::Result<ClientConfig> {
    if skip_verify {
        return Ok(ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth());
    }

    let mut roots = RootCertStore::empty();
    if let Some(pem) = ca_pem {
        for c in rustls_pemfile::certs(&mut pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .context("解析 monitor CA")?
        {
            roots.add(c).context("加入 root store")?;
        }
    } else {
        // 托管平台：正经证书，用系统根
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    Ok(ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

#[derive(Debug)]
struct NoVerify;
impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _e: &CertificateDer<'_>,
        _i: &[CertificateDer<'_>],
        _n: &ServerName<'_>,
        _o: &[u8],
        _t: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
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
