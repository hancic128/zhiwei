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
use tokio::io::AsyncWriteExt;
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

        read_response(&mut io).await
    }
}

/// 读 HTTP/1.1 响应,按 framing 拆出 body。
///
/// 支持三种 framing(按 RFC 7230 优先级):
///   1. `Transfer-Encoding: chunked` —— chunked 解码
///   2. `Content-Length: N`           —— 读定长
///   3. 都没有(`Connection: close`)   —— 读到 EOF
///
/// 历史上这里只用 `read_to_end` + 按 `\r\n\r\n` 切 header,把整段 body 喂给
/// `serde_json` —— 这条路径在 Render / Cloudflare 这类「HTTP/1.1 + close
/// 时强制 chunked」的边缘后面会爆炸,body 被 `<hex>\r\n...\r\n0\r\n\r\n`
/// 污染,JSON 解析报 `trailing characters at line 1 column 2`(2026-09-21)。
pub(crate) async fn read_response<R>(io: &mut R) -> anyhow::Result<(u16, Vec<u8>)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let mut br = BufReader::new(io);

    // ---- 状态行 ----
    let mut status_line = Vec::new();
    br.read_until(b'\n', &mut status_line).await?;
    if status_line.is_empty() {
        anyhow::bail!("响应:状态行缺失(对端立即关闭?)");
    }
    let status_line_str = std::str::from_utf8(&status_line)
        .map_err(|e| anyhow::anyhow!("响应:状态行非 UTF-8: {e}"))?
        .trim_end_matches(|c| c == '\r' || c == '\n');
    let mut parts = status_line_str.split_whitespace();
    let _version = parts.next().context("响应:状态行为空")?;
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .with_context(|| format!("响应:无法解析状态码: {status_line_str:?}"))?;

    // ---- header 列表(直到空行)----
    let mut transfer_encoding: Option<String> = None;
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        let n = br.read_until(b'\n', &mut line).await?;
        if n == 0 {
            anyhow::bail!("响应:header 中途 EOF");
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let s = std::str::from_utf8(&line)
            .map_err(|e| anyhow::anyhow!("响应:header 非 UTF-8: {e}"))?
            .trim_end_matches(|c| c == '\r' || c == '\n');
        let Some((k, v)) = s.split_once(':') else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "transfer-encoding" => transfer_encoding = Some(v.trim().to_string()),
            "content-length" => content_length = v.trim().parse().ok(),
            _ => {}
        }
    }

    let is_chunked = transfer_encoding
        .as_deref()
        .map(|s| {
            s.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("chunked"))
        })
        .unwrap_or(false);

    // ---- body ----
    let body = if is_chunked {
        decode_chunked_body(&mut br).await?
    } else if let Some(len) = content_length {
        let mut body = vec![0u8; len];
        br.read_exact(&mut body)
            .await
            .with_context(|| format!("响应:读 Content-Length={len} 字节失败"))?;
        body
    } else {
        // close-delimited:RFC 允许无 framing 信息,读到 EOF(对端按 Connection: close 关连接)
        let mut body = Vec::new();
        br.read_to_end(&mut body).await?;
        body
    };

    Ok((status, body))
}

/// 解码 chunked transfer-encoding(RFC 7230 §4.1)。
///
/// chunk = size-line CRLF data CRLF,size-line = 1*HEX [ ";" ext ]。
/// 终止 chunk = "0" CRLF *( trailer CRLF ) CRLF。
pub(crate) async fn decode_chunked_body<R>(
    br: &mut tokio::io::BufReader<R>,
) -> anyhow::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let mut out = Vec::new();
    loop {
        // size-line
        let mut size_line = Vec::new();
        let n = br.read_until(b'\n', &mut size_line).await?;
        if n == 0 {
            anyhow::bail!("chunked:期望 size 行,先收到 EOF");
        }
        let size_str = std::str::from_utf8(&size_line)
            .map_err(|e| anyhow::anyhow!("chunked:size 行非 UTF-8: {e}"))?
            .trim_end_matches(|c| c == '\r' || c == '\n');
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|e| anyhow::anyhow!("chunked:无法解析 size {size_hex:?}: {e}"))?;

        if size == 0 {
            // 终止 chunk:后面跟 0 个或多个 trailer(每行 CRLF),最后空行 CRLF 收尾
            loop {
                let mut trailer = Vec::new();
                let n = br.read_until(b'\n', &mut trailer).await?;
                if n == 0 || trailer == b"\r\n" || trailer == b"\n" {
                    break;
                }
                // 非终止 trailer:继续读
            }
            return Ok(out);
        }

        // data
        let mut chunk = vec![0u8; size];
        br.read_exact(&mut chunk)
            .await
            .map_err(|e| anyhow::anyhow!("chunked:读 {size} 字节 data 失败: {e}"))?;
        out.extend_from_slice(&chunk);

        // data 后的 CRLF
        let mut crlf = [0u8; 2];
        br.read_exact(&mut crlf).await?;
        if &crlf != b"\r\n" {
            anyhow::bail!(
                "chunked:data 后期望 CRLF,实际 {:?}",
                std::str::from_utf8(&crlf).unwrap_or("<bin>")
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 把字节流喂给被测代码:实现 `AsyncRead`,把 `inner` 用完就返回 EOF。
    /// 绕开 TCP listener / duplex 的 race,专心测 framing 解码。
    struct PreloadedStream {
        inner: Vec<u8>,
        pos: usize,
    }

    impl PreloadedStream {
        fn from_static(bytes: &'static [u8]) -> Self {
            Self {
                inner: bytes.to_vec(),
                pos: 0,
            }
        }
        fn from_vec(bytes: Vec<u8>) -> Self {
            Self {
                inner: bytes,
                pos: 0,
            }
        }
    }

    impl tokio::io::AsyncRead for PreloadedStream {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let remaining = self.inner.len() - self.pos;
            if remaining == 0 {
                return std::task::Poll::Ready(Ok(()));
            }
            let n = remaining.min(buf.remaining());
            buf.put_slice(&self.inner[self.pos..self.pos + n]);
            self.pos += n;
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// 把字节流喂给 `read_response`,拿到 (status, body)。
    async fn read_from_payload(payload: &'static [u8]) -> (u16, Vec<u8>) {
        let mut s = PreloadedStream::from_static(payload);
        read_response(&mut s).await.unwrap()
    }

    /// 历史上 send() 在 chunked 响应上炸 —— body 是 `2\r\nok\r\n0\r\n\r\n`,
    /// 原实现直接喂给调用方,JSON 解析报 `trailing characters at line 1 column 2`。
    /// 修后应得到干净 body `ok`。
    #[tokio::test]
    async fn decodes_chunked_single_chunk() {
        let (status, body) = read_from_payload(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\nok\r\n0\r\n\r\n",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"ok");
    }

    #[tokio::test]
    async fn decodes_chunked_multiple_chunks() {
        let (status, body) = read_from_payload(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"hello world");
    }

    #[tokio::test]
    async fn decodes_chunked_with_extension() {
        // RFC 7230 §4.1.1:chunk size 后可带 `;ext=val`
        let (status, body) = read_from_payload(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;foo=bar\r\nhello\r\n0\r\n\r\n",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"hello");
    }

    #[tokio::test]
    async fn decodes_chunked_with_trailers() {
        // 终止 chunk 后允许 trailer(常见于云厂商给签名 / trace id)
        let (status, body) = read_from_payload(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\nX-Trace-Id: abc\r\n\r\n",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"ok");
    }

    #[tokio::test]
    async fn decodes_chunked_zero_body() {
        let (status, body) =
            read_from_payload(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n")
                .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"");
    }

    #[tokio::test]
    async fn decodes_content_length() {
        let (status, body) = read_from_payload(
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"hello world");
    }

    #[tokio::test]
    async fn decodes_close_delimited_no_body() {
        // 204 No Content:无 framing,Connection: close
        let (status, body) =
            read_from_payload(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await;
        assert_eq!(status, 204);
        assert_eq!(body, b"");
    }

    #[tokio::test]
    async fn decodes_close_delimited_with_body() {
        // 自定义文本响应,无 Content-Length 也无 chunked —— 历史上由 EOF 终止
        let (status, body) = read_from_payload(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nhello",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, b"hello");
    }

    /// 关键回归:`/v1/probe-config` 与 `/v1/cert-config` 在 Render 边缘返回的真实
    /// 形态 —— HTTP/1.1 + close + JSON + chunked。修后 body 必须是合法 JSON,
    /// `serde_json::from_slice::<Value>` 不能失败。
    #[tokio::test]
    async fn probe_config_json_response_parses_as_json() {
        let body = r#"{"node_id":"abc","probes":[{"id":"p1","service":"s1","name":"n","kind":"tcp","target":{"host":"127.0.0.1","port":80},"expect":{},"interval_seconds":60,"timeout_ms":1000}]}"#;
        let payload = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        );
        let mut s = PreloadedStream::from_vec(payload.into_bytes());
        let (status, resp_body) = read_response(&mut s).await.unwrap();
        assert_eq!(status, 200);
        let v: serde_json::Value =
            serde_json::from_slice(&resp_body).expect("chunked JSON 必须能解析");
        assert_eq!(v["node_id"], "abc");
        assert_eq!(v["probes"].as_array().unwrap().len(), 1);
    }
}
