//! 最小 Docker Engine API 客户端。
//!
//! 目前只需要「列出容器」，所以直接通过 `/var/run/docker.sock` 发一个
//! HTTP GET，不引入 bollard。等 P2-3 要做容器启停 / 看日志 / 取 stats 时，
//! 再按 DESIGN.md 的 D3 决策换成 bollard（届时可保持本模块的对外签名不变）。

use anyhow::Context;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use zhiwei_proto::telemetry::Container;

const SOCKET: &str = "/var/run/docker.sock";

#[derive(Debug, Deserialize)]
struct DockerContainer {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Names", default)]
    names: Vec<String>,
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(rename = "State", default)]
    state: String,
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "Created", default)]
    created: i64,
}

/// 单个容器的运行时状态（来自 `/containers/{id}/json`）。
/// 列表接口不返回 StartedAt / FinishedAt，只有 inspect 才有——
/// 「启动时长」与「上次更新」两列都靠它。
#[derive(Debug, Clone, Default)]
pub struct ContainerRuntime {
    pub state: String,
    pub started_at_unix_nano: i64,
    pub finished_at_unix_nano: i64,
    /// compose 项目名（容器标签 `com.docker.compose.project`）——不是 compose 起的就是空串。
    /// 用作「应用」维度：同一 compose 项目的容器天然成组，无需用户手工建层级。
    pub compose_project: String,
    /// compose 服务名（`com.docker.compose.service`）
    pub compose_service: String,
}

#[derive(Debug, Deserialize)]
struct InspectResponse {
    #[serde(rename = "State", default)]
    state: Option<InspectState>,
    #[serde(rename = "Config", default)]
    config: Option<InspectConfig>,
}

/// 只取我们需要的 Config 部分。
/// 注意 Docker 对「没有标签的容器」返回的是 `"Labels": null`，所以这里必须是 Option。
#[derive(Debug, Default, Deserialize)]
struct InspectConfig {
    #[serde(rename = "Labels", default)]
    labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
struct InspectState {
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "StartedAt", default)]
    started_at: String,
    #[serde(rename = "FinishedAt", default)]
    finished_at: String,
}

/// RFC3339 → Unix 纳秒；Docker 的零值时间（0001-01-01T00:00:00Z）当作 0
fn parse_docker_time(s: &str) -> i64 {
    if s.is_empty() || s.starts_with("0001-01-01") {
        return 0;
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_nanos_opt().unwrap_or(0))
        .unwrap_or(0)
}

const LABEL_COMPOSE_PROJECT: &str = "com.docker.compose.project";
const LABEL_COMPOSE_SERVICE: &str = "com.docker.compose.service";

/// inspect 响应 → 运行时状态（纯函数，便于单测）
fn runtime_from_inspect(parsed: InspectResponse) -> ContainerRuntime {
    let state = parsed.state.unwrap_or_default();
    let labels = parsed.config.and_then(|c| c.labels).unwrap_or_default();
    ContainerRuntime {
        state: state.status,
        started_at_unix_nano: parse_docker_time(&state.started_at),
        finished_at_unix_nano: parse_docker_time(&state.finished_at),
        compose_project: labels
            .get(LABEL_COMPOSE_PROJECT)
            .cloned()
            .unwrap_or_default(),
        compose_service: labels
            .get(LABEL_COMPOSE_SERVICE)
            .cloned()
            .unwrap_or_default(),
    }
}

/// inspect 单个容器（失败只记 debug，不影响整份快照）
pub async fn inspect_container(container: &str) -> anyhow::Result<ContainerRuntime> {
    let body = docker_get(&format!("/containers/{container}/json")).await?;
    let parsed: InspectResponse =
        serde_json::from_slice(&body).context("解析 docker inspect 响应")?;
    Ok(runtime_from_inspect(parsed))
}

/// docker 的失败响应体是 `{"message":"..."}`。把这句话取出来给用户看，
/// 比「docker API 返回 409」有用得多——例如删除运行中的容器时 docker 会说
/// `container is running`，用户立刻知道该先停掉它。
pub fn docker_error_message(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .filter(|s| !s.trim().is_empty());
    match msg {
        Some(m) => format!("docker 拒绝了这个操作：{m}"),
        None => format!("docker API 返回 {status}"),
    }
}

/// 容器生命周期动作：start / stop / restart
pub async fn container_action(container: &str, action: &str) -> anyhow::Result<String> {
    let path = match action {
        "start" | "stop" | "restart" => format!("/containers/{container}/{action}"),
        other => anyhow::bail!("未知容器动作 {other}"),
    };
    let (status, body) = docker_write("POST", &path).await?;
    if !(200..300).contains(&status) {
        anyhow::bail!("{}", docker_error_message(status, &body));
    }
    let zh = match action {
        "start" => "已启动",
        "stop" => "已停止",
        _ => "已重启",
    };
    Ok(format!("容器 {container} {zh}"))
}

/// 删除容器。
///
/// `force = false` 时刻意不传 force：运行中的容器 docker 会直接拒绝，
/// 用户看到「先停掉它」的提示。**不做「顺手帮你强删」这件事**——
/// 一个人的环境里，悄悄删掉一个正在跑的服务代价太大。
pub async fn container_remove(container: &str, force: bool) -> anyhow::Result<String> {
    let path = format!(
        "/containers/{container}?force={}&v=1",
        if force { "1" } else { "0" }
    );
    let (status, body) = docker_write("DELETE", &path).await?;
    if !(200..300).contains(&status) {
        anyhow::bail!("{}", docker_error_message(status, &body));
    }
    Ok(format!("容器 {container} 已删除"))
}

/// 列出本机容器（含已停止的）。
///
/// Docker 未安装 / socket 不可用时返回 `Ok(None)`，由调用方决定是否上报空列表——
/// 这不是错误，只是这台机器上没有 Docker。
pub async fn list_containers() -> anyhow::Result<Option<Vec<Container>>> {
    if !std::path::Path::new(SOCKET).exists() {
        return Ok(None);
    }

    let stream = match tokio::net::UnixStream::connect(SOCKET).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!("docker socket 权限不足，跳过容器采集");
            return Ok(None);
        }
        Err(e) => return Err(e).context("连接 docker socket"),
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("docker socket HTTP 握手")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("http://localhost/containers/json?all=1")
        .header("Host", "localhost")
        .body(Empty::<Bytes>::new())
        .context("构造 docker 请求")?;

    let res = sender.send_request(req).await.context("docker 请求失败")?;

    if !res.status().is_success() {
        anyhow::bail!("docker API 返回 {}", res.status());
    }

    let body = res
        .into_body()
        .collect()
        .await
        .context("读取 docker 响应")?
        .to_bytes();

    let raw: Vec<DockerContainer> =
        serde_json::from_slice(&body).context("解析 docker 容器列表")?;

    let mut containers = Vec::with_capacity(raw.len());
    for c in raw {
        // Docker 返回的名称带前导斜杠，如 "/nginx"
        let name = c
            .names
            .first()
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_default();
        // 逐个 inspect 拿 StartedAt / FinishedAt（本地 socket，几十个容器 ~几十毫秒）
        let rt = match inspect_container(&c.id).await {
            Ok(rt) => rt,
            Err(e) => {
                tracing::debug!(container = %c.id, error = %e, "inspect 容器失败，启动时长留空");
                ContainerRuntime::default()
            }
        };
        containers.push(Container {
            id: c.id,
            name,
            image: c.image,
            state: if rt.state.is_empty() {
                c.state
            } else {
                rt.state
            },
            created_at_unix_nano: c.created.saturating_mul(1_000_000_000),
            status: c.status,
            runtime: "docker".into(),
            started_at_unix_nano: rt.started_at_unix_nano,
            finished_at_unix_nano: rt.finished_at_unix_nano,
            compose_project: rt.compose_project,
            compose_service: rt.compose_service,
        });
    }

    Ok(Some(containers))
}

/// 取容器日志末尾 N 行。
///
/// Docker 的非 TTY 容器日志是「8 字节帧头 + 载荷」的复用流，需要解复用；
/// TTY 容器则是裸流。这里按帧头是否合法自动判断。
pub async fn container_logs(
    container: &str,
    tail: u32,
    timestamps: bool,
) -> anyhow::Result<String> {
    let path = format!(
        "/containers/{container}/logs?stdout=1&stderr=1&tail={}&timestamps={}",
        tail.clamp(1, 5000),
        if timestamps { 1 } else { 0 }
    );
    let body = docker_get(&path).await?;
    Ok(demux_log_stream(&body))
}

fn demux_log_stream(raw: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut framed = false;
    // 先探测第一帧：byte0 ∈ {0,1,2}，byte1..4 = 0，且长度合理
    if raw.len() >= 8 && raw[0] <= 2 && raw[1] == 0 && raw[2] == 0 && raw[3] == 0 {
        let len = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize;
        if len <= raw.len().saturating_sub(8) {
            framed = true;
        }
    }

    if framed {
        while i + 8 <= raw.len() {
            let len = u32::from_be_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
            let start = i + 8;
            let end = (start + len).min(raw.len());
            out.extend_from_slice(&raw[start..end]);
            i = end;
        }
    } else {
        out.extend_from_slice(raw);
    }

    String::from_utf8_lossy(&out).to_string()
}

/// 读文件末尾 N 行（用于非容器场景，如 nginx access.log）
pub fn read_file_tail(path: &str, tail: u32) -> anyhow::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    // 粗略按每行 200 字节倒推需要读多少，最多读回 1 MiB
    let want = (tail.clamp(1, 5000) as u64)
        .saturating_mul(200)
        .min(1 << 20);
    let start = size.saturating_sub(want);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;

    let lines: Vec<&str> = buf.lines().collect();
    let from = lines.len().saturating_sub(tail as usize);
    Ok(lines[from..].join("\n"))
}

async fn docker_get(path: &str) -> anyhow::Result<Vec<u8>> {
    if !std::path::Path::new(SOCKET).exists() {
        anyhow::bail!("docker socket 不存在");
    }
    let stream = tokio::net::UnixStream::connect(SOCKET).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("docker socket HTTP 握手")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri(format!("http://localhost{path}"))
        .header("Host", "localhost")
        .body(Empty::<Bytes>::new())?;
    let res = sender.send_request(req).await?;
    if !res.status().is_success() {
        anyhow::bail!("docker API 返回 {}", res.status());
    }
    Ok(res.into_body().collect().await?.to_bytes().to_vec())
}

/// 需要写动作的 docker 调用（start / stop / restart / remove）。
/// 返回 `(状态码, 响应体)`——失败时响应体里有 docker 自己那句话，要透给用户。
async fn docker_write(method: &str, path: &str) -> anyhow::Result<(u16, String)> {
    if !std::path::Path::new(SOCKET).exists() {
        anyhow::bail!("docker socket 不存在");
    }
    let stream = tokio::net::UnixStream::connect(SOCKET).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("docker socket HTTP 握手")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("Host", "localhost")
        .body(Empty::<Bytes>::new())?;
    let res = sender.send_request(req).await?;
    let status = res.status().as_u16();
    let body = res
        .into_body()
        .collect()
        .await
        .map(|b| String::from_utf8_lossy(&b.to_bytes()).to_string())
        .unwrap_or_default();
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 `docker inspect` 的骨架：compose 起出来的容器带 com.docker.compose.* 标签
    const COMPOSE_INSPECT: &str = r#"{
        "Id": "0f2b1c",
        "State": {
            "Status": "running",
            "StartedAt": "2026-09-19T10:00:00.123456789Z",
            "FinishedAt": "0001-01-01T00:00:00Z"
        },
        "Config": {
            "Labels": {
                "com.docker.compose.project": "zhiwei",
                "com.docker.compose.service": "monitor",
                "com.docker.compose.project.working_dir": "/root/apps/zhiwei",
                "com.docker.compose.project.config_files": "/root/apps/zhiwei/docker-compose.yml",
                "com.docker.compose.container-number": "1"
            }
        }
    }"#;

    /// 非 compose 起的容器：有 Labels，但没有 compose 那几个键
    const PLAIN_INSPECT: &str = r#"{
        "Id": "9a8b7c",
        "State": { "Status": "exited", "StartedAt": "0001-01-01T00:00:00Z", "FinishedAt": "2026-09-18T08:00:00Z" },
        "Config": { "Labels": { "org.opencontainers.image.title": "nginx" } }
    }"#;

    /// 老版本 Docker / 异常响应：整个 Config 缺失
    const NO_CONFIG_INSPECT: &str = r#"{
        "Id": "111111",
        "State": { "Status": "running", "StartedAt": "0001-01-01T00:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z" }
    }"#;

    /// Docker 对没有标签的容器返回的是 `"Labels": null`，不是空对象
    const NULL_LABELS_INSPECT: &str = r#"{
        "Id": "222222",
        "State": { "Status": "running", "StartedAt": "0001-01-01T00:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z" },
        "Config": { "Labels": null }
    }"#;

    fn runtime_of(raw: &str) -> ContainerRuntime {
        let parsed: InspectResponse = serde_json::from_str(raw).expect("inspect JSON 应能解析");
        runtime_from_inspect(parsed)
    }

    #[test]
    fn extracts_compose_project_and_service() {
        let rt = runtime_of(COMPOSE_INSPECT);
        assert_eq!(rt.compose_project, "zhiwei");
        assert_eq!(rt.compose_service, "monitor");
        assert_eq!(rt.state, "running");
    }

    #[test]
    fn non_compose_container_has_no_project() {
        let rt = runtime_of(PLAIN_INSPECT);
        assert!(rt.compose_project.is_empty());
        assert!(rt.compose_service.is_empty());
    }

    #[test]
    fn missing_config_is_not_an_error() {
        let rt = runtime_of(NO_CONFIG_INSPECT);
        assert!(rt.compose_project.is_empty());
        assert_eq!(rt.state, "running");
    }

    #[test]
    fn null_labels_is_not_an_error() {
        let rt = runtime_of(NULL_LABELS_INSPECT);
        assert!(rt.compose_project.is_empty());
        assert!(rt.compose_service.is_empty());
    }

    #[test]
    fn docker_error_message_surfaces_dockers_own_wording() {
        // 删除运行中的容器时 docker 会回这个——用户该看到的是它，不是状态码
        assert_eq!(
            docker_error_message(409, r#"{"message":"You cannot remove a running container abc. Stop the container before attempting removal or force remove"}"#),
            "docker 拒绝了这个操作：You cannot remove a running container abc. Stop the container before attempting removal or force remove"
        );
    }

    #[test]
    fn docker_error_message_falls_back_to_the_status_code() {
        // 没有 message / 不是 JSON / 空对象，都不该让回执变成空白
        assert_eq!(docker_error_message(500, ""), "docker API 返回 500");
        assert_eq!(
            docker_error_message(404, "<html>Not Found</html>"),
            "docker API 返回 404"
        );
        assert_eq!(docker_error_message(400, "{}"), "docker API 返回 400");
        assert_eq!(
            docker_error_message(400, r#"{"message":"   "}"#),
            "docker API 返回 400"
        );
    }
}
