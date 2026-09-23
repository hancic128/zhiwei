//! 最小 Docker Engine API 客户端。
//!
//! 目前只需要「列出容器」，所以直接通过 `/var/run/docker.sock` 发一个
//! HTTP GET，不引入 bollard。等 P2-3 要做容器启停 / 看日志 / 取 stats 时，
//! 再按 DESIGN.md 的 D3 决策换成 bollard（届时可保持本模块的对外签名不变）。

use anyhow::Context;
use bytes::Bytes;
use futures::StreamExt;
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
    /// 内存限额（`HostConfig.Memory` 字节）；0 = 不限
    pub mem_limit_bytes: u64,
    /// CPU 限额（`HostConfig.NanoCpus` 纳秒）；0 = 不限
    pub cpu_limit_nano: u64,
}

#[derive(Debug, Deserialize)]
struct InspectResponse {
    #[serde(rename = "State", default)]
    state: Option<InspectState>,
    #[serde(rename = "Config", default)]
    config: Option<InspectConfig>,
    #[serde(rename = "HostConfig", default)]
    host_config: Option<InspectHostConfig>,
}

/// 只取我们需要的 HostConfig 部分：限额。
#[derive(Debug, Default, Deserialize)]
struct InspectHostConfig {
    #[serde(rename = "Memory", default)]
    memory: u64,
    #[serde(rename = "NanoCpus", default)]
    nano_cpus: u64,
    /// `--cpu-quota` 老写法：限额靠 quota/period 两个数一起表达
    #[serde(rename = "CpuQuota", default)]
    cpu_quota: i64,
    #[serde(rename = "CpuPeriod", default)]
    cpu_period: i64,
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
    let host = parsed.host_config.unwrap_or_default();
    // 老写法 `--cpu-quota`：NanoCpus 为空时用 quota/period 换算（1e9 = 1 核）
    let quota_period = match (host.cpu_quota, host.cpu_period) {
        (q, p) if q > 0 && p > 0 => (q as u64).saturating_mul(1_000_000_000) / p as u64,
        _ => 0,
    };
    ContainerRuntime {
        state: state.status,
        started_at_unix_nano: parse_docker_time(&state.started_at),
        finished_at_unix_nano: parse_docker_time(&state.finished_at),
        mem_limit_bytes: host.memory,
        cpu_limit_nano: if host.nano_cpus > 0 {
            host.nano_cpus
        } else {
            quota_period
        },
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
    // 304 = 已经在目标状态。docker 拿它表示「这次点击没有改变任何东西」，
    // 而控制台那份状态来自最多 5 分钟前的快照——把 304 当失败，用户就会看到
    // 「容器关不掉 / 启不了」这种假故障。
    if status == 304 {
        return Ok(already_message(container, action));
    }
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

/// 「本来就是这个状态」时给用户看的话——说清没做改动，而不是含糊地说成功
fn already_message(container: &str, action: &str) -> String {
    let zh = match action {
        "start" => "已经在运行",
        "stop" => "已经是停止状态",
        _ => "刚刚重启过",
    };
    format!("容器 {container} {zh}（本次未做改动）")
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
    // 已经不在了：控制台那行是快照留下的幽灵。删除意图已经满足，别报错——
    // 刷新之后这行就消失了。
    if status == 404 {
        return Ok(format!("容器 {container} 已经不存在（可能已被删除）"));
    }
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
            mem_limit_bytes: rt.mem_limit_bytes,
            cpu_limit_nano: rt.cpu_limit_nano,
            // 用量随后并发补齐
            mem_usage_bytes: 0,
            cpu_percent: 0.0,
        });
    }

    // 用量：`stats` 每次采样要等一个间隔（one-shot 可立即返回），所以只对运行中的
    // 容器取，并且并发跑、上限 12 条连接——几十个容器也要在一秒内采完。
    // 单个容器失败只意味着这一格显示「—」，不该影响整份快照。
    let usage: Vec<(String, ContainerUsage)> = futures::stream::iter(
        containers
            .iter()
            .filter(|c| c.state == "running")
            .map(|c| c.id.clone())
            .collect::<Vec<_>>(),
    )
    .map(|id| async move {
        let got = tokio::time::timeout(CONTAINER_STATS_TIMEOUT, container_usage(&id))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        (id, got)
    })
    .buffer_unordered(12)
    .collect()
    .await;
    for (id, u) in usage {
        if let Some(c) = containers.iter_mut().find(|c| c.id == id) {
            c.mem_usage_bytes = u.mem_usage_bytes;
            c.cpu_percent = u.cpu_percent;
        }
    }

    Ok(Some(containers))
}

/// 单次采样的用量。
#[derive(Debug, Default, Clone, Copy)]
pub struct ContainerUsage {
    /// 已扣 page cache 的常驻内存（和 `docker stats` 同一算法）
    pub mem_usage_bytes: u64,
    /// 相对本机全部核心的占用百分比（和进程列表同一口径）
    pub cpu_percent: f64,
}

/// stats 是即时查询、正常毫秒级返回；卡住就别拖累整份快照
const CONTAINER_STATS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Default, Deserialize)]
struct StatsResponse {
    #[serde(default)]
    memory_stats: Option<StatsMemory>,
    #[serde(default)]
    cpu_stats: Option<StatsCpu>,
    #[serde(default)]
    precpu_stats: Option<StatsCpu>,
}

#[derive(Debug, Default, Deserialize)]
struct StatsMemory {
    #[serde(default)]
    usage: u64,
    /// cgroup 明细：v1 叫 cache / total_inactive_file，v2 叫 inactive_file
    #[serde(default)]
    stats: Option<std::collections::BTreeMap<String, u64>>,
}

#[derive(Debug, Default, Deserialize)]
struct StatsCpu {
    #[serde(default)]
    cpu_usage: Option<StatsCpuUsage>,
    #[serde(default)]
    system_cpu_usage: u64,
    #[serde(default)]
    online_cpus: u64,
}

#[derive(Debug, Default, Deserialize)]
struct StatsCpuUsage {
    #[serde(default)]
    total_usage: u64,
    #[serde(default)]
    percpu_usage: Option<Vec<u64>>,
}

/// stats 响应 → 用量（纯函数，便于单测）。
///
/// CPU 百分比沿用 docker CLI 的算法：两次采样的差值之比 × 核心数。
/// 单次采样（stream=false）时 `cpu_stats` 与 `precpu_stats` 恰好是相邻两拍。
fn usage_from_stats(s: &StatsResponse) -> ContainerUsage {
    let mem = s.memory_stats.as_ref();
    let cache = mem
        .and_then(|m| m.stats.as_ref())
        .map(|st| {
            ["inactive_file", "cache", "total_inactive_file"]
                .iter()
                .find_map(|k| st.get(*k).copied())
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let mem_usage_bytes = mem.map(|m| m.usage.saturating_sub(cache)).unwrap_or(0);

    let cpu = s.cpu_stats.as_ref().and_then(|c| c.cpu_usage.as_ref());
    let pre = s.precpu_stats.as_ref().and_then(|c| c.cpu_usage.as_ref());
    let system_delta = s
        .cpu_stats
        .as_ref()
        .map(|c| c.system_cpu_usage)
        .unwrap_or(0)
        .saturating_sub(
            s.precpu_stats
                .as_ref()
                .map(|c| c.system_cpu_usage)
                .unwrap_or(0),
        );
    let cpu_delta = cpu
        .map(|c| c.total_usage)
        .unwrap_or(0)
        .saturating_sub(pre.map(|c| c.total_usage).unwrap_or(0));
    let cores = s
        .cpu_stats
        .as_ref()
        .map(|c| {
            if c.online_cpus > 0 {
                c.online_cpus
            } else {
                c.cpu_usage
                    .as_ref()
                    .map(|u| u.percpu_usage.as_ref().map(|v| v.len()).unwrap_or(0) as u64)
                    .unwrap_or(0)
            }
        })
        .unwrap_or(0);
    let cpu_percent = if system_delta == 0 || cores == 0 {
        0.0
    } else {
        cpu_delta as f64 / system_delta as f64 * cores as f64 * 100.0
    };

    ContainerUsage {
        mem_usage_bytes,
        cpu_percent,
    }
}

/// 采一次容器的用量。老 daemon（API < 1.41）不认 `one-shot`，退回等一拍的老写法。
async fn container_usage(id: &str) -> anyhow::Result<ContainerUsage> {
    let body = match docker_get(&format!("/containers/{id}/stats?stream=false&one-shot=1")).await {
        Ok(b) => b,
        Err(_) => docker_get(&format!("/containers/{id}/stats?stream=false")).await?,
    };
    let parsed: StatsResponse = serde_json::from_slice(&body).context("解析 docker stats 响应")?;
    Ok(usage_from_stats(&parsed))
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

    /// 真实 `docker stats?stream=false` 的骨架（cgroup v2：inactive_file）
    const STATS_CGROUP_V2: &str = r#"{
        "read": "2026-09-23T10:00:00.000000000Z",
        "memory_stats": {
            "usage": 104857600,
            "limit": 536870912,
            "stats": { "inactive_file": 20971520, "anon": 83886080 }
        },
        "cpu_stats": {
            "cpu_usage": { "total_usage": 300000000, "percpu_usage": [1, 2, 3, 4] },
            "system_cpu_usage": 2000000000,
            "online_cpus": 4
        },
        "precpu_stats": {
            "cpu_usage": { "total_usage": 200000000, "percpu_usage": [1, 2, 3, 4] },
            "system_cpu_usage": 1000000000
        }
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

    #[test]
    fn limits_come_from_host_config() {
        // compose 里写了 cpus: "2" 与 mem_limit: 512m 时 inspect 长这样
        let rt = runtime_of(
            r#"{
                "State": { "Status": "running" },
                "HostConfig": { "Memory": 536870912, "NanoCpus": 2000000000 }
            }"#,
        );
        assert_eq!(rt.mem_limit_bytes, 536870912);
        assert_eq!(rt.cpu_limit_nano, 2_000_000_000);
    }

    #[test]
    fn cpu_limit_falls_back_to_quota_over_period() {
        // `--cpu-quota 50000 --cpu-period 100000` = 半核，NanoCpus 是空的
        let rt = runtime_of(
            r#"{
                "State": { "Status": "running" },
                "HostConfig": { "CpuQuota": 50000, "CpuPeriod": 100000 }
            }"#,
        );
        assert_eq!(rt.cpu_limit_nano, 500_000_000);
    }

    #[test]
    fn no_host_config_means_no_limit() {
        // 老 daemon / 异常响应：限额未知就留 0（界面显示「不限」，不是编一个数）
        let rt = runtime_of(NO_CONFIG_INSPECT);
        assert_eq!(rt.mem_limit_bytes, 0);
        assert_eq!(rt.cpu_limit_nano, 0);
    }

    #[test]
    fn repeat_click_on_a_settled_container_is_not_an_error() {
        // docker 对「已经在跑还让你 start」回 304；把它当失败，用户就会以为
        // 「容器关不掉 / 启不了」——这正是控制台那份 5 分钟前的快照最常见的撞车
        assert_eq!(
            already_message("nginx", "start"),
            "容器 nginx 已经在运行（本次未做改动）"
        );
        assert_eq!(
            already_message("nginx", "stop"),
            "容器 nginx 已经是停止状态（本次未做改动）"
        );
        assert_eq!(
            already_message("nginx", "restart"),
            "容器 nginx 刚刚重启过（本次未做改动）"
        );
    }

    #[test]
    fn usage_subtracts_page_cache_and_computes_cpu_percent() {
        let parsed: StatsResponse =
            serde_json::from_str(STATS_CGROUP_V2).expect("stats JSON 应能解析");
        let u = usage_from_stats(&parsed);
        // 100 MiB - 20 MiB 的 inactive_file
        assert_eq!(u.mem_usage_bytes, 83886080);
        // (300e6-200e6)/(2000e6-1000e6) * 4 核 * 100 = 40%
        assert!((u.cpu_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn usage_falls_back_to_cgroup_v1_cache_key() {
        let parsed: StatsResponse = serde_json::from_str(
            r#"{
                "memory_stats": { "usage": 104857600, "stats": { "cache": 41943040, "total_inactive_file": 10485760 } },
                "cpu_stats": { "cpu_usage": { "total_usage": 100 }, "system_cpu_usage": 1000 },
                "precpu_stats": { "cpu_usage": { "total_usage": 50 }, "system_cpu_usage": 500 }
            }"#,
        )
        .expect("stats JSON 应能解析");
        let u = usage_from_stats(&parsed);
        // v1 用 cache（和 docker CLI 一致），取不到才退回 total_inactive_file
        assert_eq!(u.mem_usage_bytes, 104857600 - 41943040);
        // online_cpus 缺失时用 percpu_usage 的长度；这里两者都没有 → 不算百分比，免得撒谎
        assert_eq!(u.cpu_percent, 0.0);
    }

    #[test]
    fn usage_uses_percpu_len_when_online_cpus_missing() {
        let parsed: StatsResponse = serde_json::from_str(
            r#"{
                "cpu_stats": {
                    "cpu_usage": { "total_usage": 300000000, "percpu_usage": [1, 2] },
                    "system_cpu_usage": 2000000000
                },
                "precpu_stats": {
                    "cpu_usage": { "total_usage": 200000000 },
                    "system_cpu_usage": 1000000000
                }
            }"#,
        )
        .expect("stats JSON 应能解析");
        let u = usage_from_stats(&parsed);
        // (100e6/1000e6) * 2 核 * 100 = 20%
        assert!((u.cpu_percent - 20.0).abs() < 1e-9);
        assert_eq!(u.mem_usage_bytes, 0);
    }

    #[test]
    fn usage_of_a_stopped_container_is_all_zero() {
        // 停掉的容器 docker 返回的 stats 里没有 memory/cpu 明细，不解析失败也不算错
        let parsed: StatsResponse = serde_json::from_str(r#"{"read":"2026-09-23T10:00:00Z"}"#)
            .expect("stats JSON 应能解析");
        let u = usage_from_stats(&parsed);
        assert_eq!(u.mem_usage_bytes, 0);
        assert_eq!(u.cpu_percent, 0.0);
    }
}
