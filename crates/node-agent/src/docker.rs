//! Minimal Docker Engine API client.
//!
//! Currently only needs "list containers", so sends one HTTP GET over `/var/run/docker.sock`
//! without introducing bollard. When P2-3 needs container start/stop / logs / stats,
//! switch to bollard per D3 decision in DESIGN.md (can keep this module's public API unchanged).

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

/// Runtime state of a single container (from `/containers/{id}/json`).
/// List endpoint doesn't return StartedAt / FinishedAt, only inspect has them --
/// both "uptime" and "last updated" columns depend on it.
#[derive(Debug, Clone, Default)]
pub struct ContainerRuntime {
    pub state: String,
    pub started_at_unix_nano: i64,
    pub finished_at_unix_nano: i64,
    /// Compose project name (container label `com.docker.compose.project`) -- not compose-managed is empty.
    /// Used as "app" dimension: containers in same compose project naturally group together, no manual hierarchy needed.
    pub compose_project: String,
    /// Compose service name (`com.docker.compose.service`)
    pub compose_service: String,
    /// Memory limit (`HostConfig.Memory` bytes); 0 = unlimited
    pub mem_limit_bytes: u64,
    /// CPU limit (`HostConfig.NanoCpus` nanoseconds); 0 = unlimited
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

/// Only the HostConfig parts we need: limits.
#[derive(Debug, Default, Deserialize)]
struct InspectHostConfig {
    #[serde(rename = "Memory", default)]
    memory: u64,
    #[serde(rename = "NanoCpus", default)]
    nano_cpus: u64,
    /// `--cpu-quota` legacy syntax: limit expressed as two numbers quota/period
    #[serde(rename = "CpuQuota", default)]
    cpu_quota: i64,
    #[serde(rename = "CpuPeriod", default)]
    cpu_period: i64,
}

/// Only the Config parts we need.
/// Note: Docker returns `"Labels": null` for containers without labels, so this must be Option.
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

/// RFC3339 -> Unix nanoseconds; Docker's zero time (0001-01-01T00:00:00Z) treated as 0
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

/// inspect response -> runtime state (pure function, easy to unit test)
fn runtime_from_inspect(parsed: InspectResponse) -> ContainerRuntime {
    let state = parsed.state.unwrap_or_default();
    let labels = parsed.config.and_then(|c| c.labels).unwrap_or_default();
    let host = parsed.host_config.unwrap_or_default();
    // Legacy `--cpu-quota` syntax: use quota/period conversion when NanoCpus is empty (1e9 = 1 core)
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

/// Inspect a single container (failure only logs debug, doesn't affect entire snapshot)
pub async fn inspect_container(container: &str) -> anyhow::Result<ContainerRuntime> {
    let body = docker_get(&format!("/containers/{container}/json")).await?;
    let parsed: InspectResponse =
        serde_json::from_slice(&body).context("failed to parse docker inspect response")?;
    Ok(runtime_from_inspect(parsed))
}

/// Docker's error response body is `{"message":"..."}`. Extract that message for the user,
/// much more useful than "docker API returned 409" -- e.g., when deleting a running container,
/// docker says "container is running", user immediately knows they need to stop it first.
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
        Some(m) => format!("docker rejected this operation: {m}"),
        None => format!("docker API returned {status}"),
    }
}

/// Container lifecycle actions: start / stop / restart
pub async fn container_action(container: &str, action: &str) -> anyhow::Result<String> {
    let path = match action {
        "start" | "stop" | "restart" => format!("/containers/{container}/{action}"),
        other => anyhow::bail!("unknown container action {other}"),
    };
    let (status, body) = docker_write("POST", &path).await?;
    // 304 = already in target state. Docker uses it to mean "this click didn't change anything",
    // but the console's state comes from a snapshot up to 5 minutes old -- treating 304 as failure
    // makes user see fake "container won't stop/start" errors.
    if status == 304 {
        return Ok(already_message(container, action));
    }
    if !(200..300).contains(&status) {
        anyhow::bail!("{}", docker_error_message(status, &body));
    }
    let zh = match action {
        "start" => "started",
        "stop" => "stopped",
        _ => "restarted",
    };
    Ok(format!("Container {container} {zh}"))
}

/// Message for "already in this state" -- clarify no change was made, not just "success"
fn already_message(container: &str, action: &str) -> String {
    let zh = match action {
        "start" => "already running",
        "stop" => "already stopped",
        _ => "just restarted",
    };
    format!("Container {container} {zh} (no change made this time)")
}

/// Remove a container.
///
/// When `force = false`, intentionally don't pass force: docker will directly reject running containers,
/// user sees "stop it first" prompt. **Not doing "force delete for you"** --
/// in someone's environment, silently deleting a running service is too costly.
pub async fn container_remove(container: &str, force: bool) -> anyhow::Result<String> {
    let path = format!(
        "/containers/{container}?force={}&v=1",
        if force { "1" } else { "0" }
    );
    let (status, body) = docker_write("DELETE", &path).await?;
    // Already gone: console row is a ghost from the snapshot. Delete intent is satisfied, don't error --
    // refresh and this row disappears.
    if status == 404 {
        return Ok(format!("Container {container} no longer exists (may have already been deleted)"));
    }
    if !(200..300).contains(&status) {
        anyhow::bail!("{}", docker_error_message(status, &body));
    }
    Ok(format!("Container {container} deleted"))
}

/// List local containers (includes stopped).
///
/// When Docker is not installed / socket unavailable, returns `Ok(None)`, caller decides whether to report empty list --
/// this is not an error, just means this machine has no Docker.
pub async fn list_containers() -> anyhow::Result<Option<Vec<Container>>> {
    if !std::path::Path::new(SOCKET).exists() {
        return Ok(None);
    }

    let stream = match tokio::net::UnixStream::connect(SOCKET).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!("docker socket permission denied, skipping container collection");
            return Ok(None);
        }
        Err(e) => return Err(e).context("failed to connect to docker socket"),
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("docker socket HTTP handshake")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("http://localhost/containers/json?all=1")
        .header("Host", "localhost")
        .body(Empty::<Bytes>::new())
        .context("failed to construct docker request")?;

    let res = sender.send_request(req).await.context("docker request failed")?;

    if !res.status().is_success() {
        anyhow::bail!("docker API returned {}", res.status());
    }

    let body = res
        .into_body()
        .collect()
        .await
        .context("failed to read docker response")?
        .to_bytes();

    let raw: Vec<DockerContainer> =
        serde_json::from_slice(&body).context("failed to parse docker container list")?;

    let mut containers = Vec::with_capacity(raw.len());
    for c in raw {
        // Docker returns names with leading slash, e.g. "/nginx"
        let name = c
            .names
            .first()
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_default();
        // Inspect each one for StartedAt / FinishedAt (local socket, tens of containers ~tens of ms)
        let rt = match inspect_container(&c.id).await {
            Ok(rt) => rt,
            Err(e) => {
                tracing::debug!(container = %c.id, error = %e, "Failed to inspect container, leaving uptime empty");
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
            // Usage filled in concurrently below
            mem_usage_bytes: 0,
            cpu_percent: 0.0,
        });
    }

    // Usage: `stats` needs an interval per sample (one-shot can return immediately), so only
    // get running containers, and run concurrently with 12 connection limit -- tens of containers
    // should be sampled within one second. Single container failure means this slot shows "--",
    // shouldn't affect entire snapshot.
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

/// Single-sample usage.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContainerUsage {
    /// Resident memory after page cache deduction (same algorithm as `docker stats`)
    pub mem_usage_bytes: u64,
    /// CPU usage percentage relative to all machine cores (same metric as process list)
    pub cpu_percent: f64,
}

/// stats is a query on demand, normally returns in milliseconds; if it hangs, don't drag down entire snapshot
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
    /// cgroup details: v1 is cache / total_inactive_file, v2 is inactive_file
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

/// stats response -> usage (pure function, easy to unit test).
///
/// CPU percentage uses docker CLI algorithm: delta of two samples / delta * cores.
/// For single sample (stream=false), `cpu_stats` and `precpu_stats` are exactly two consecutive ticks.
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

/// Get one sample of container usage. Old daemons (API < 1.41) don't recognize `one-shot`,
/// fall back to waiting one tick.
async fn container_usage(id: &str) -> anyhow::Result<ContainerUsage> {
    let body = match docker_get(&format!("/containers/{id}/stats?stream=false&one-shot=1")).await {
        Ok(b) => b,
        Err(_) => docker_get(&format!("/containers/{id}/stats?stream=false")).await?,
    };
    let parsed: StatsResponse = serde_json::from_slice(&body).context("failed to parse docker stats response")?;
    Ok(usage_from_stats(&parsed))
}

/// Get last N lines of container logs.
///
/// Docker non-TTY container logs are multiplexed streams of "8-byte frame header + payload",
/// need demultiplexing; TTY containers are raw streams. Detect automatically by whether
/// first frame header is valid.
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
    // First probe: byte0 ∈ {0,1,2}, byte1..4 = 0, and reasonable length
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

/// Read last N lines of a file (for non-container scenarios, e.g. nginx access.log)
pub fn read_file_tail(path: &str, tail: u32) -> anyhow::Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    // Roughly estimate lines from 200 bytes per line, read at most 1 MiB
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
        anyhow::bail!("docker socket not found");
    }
    let stream = tokio::net::UnixStream::connect(SOCKET).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("docker socket HTTP handshake")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri(format!("http://localhost{path}"))
        .header("Host", "localhost")
        .body(Empty::<Bytes>::new())?;
    let res = sender.send_request(req).await?;
    let status = res.status();
    let body = res.into_body().collect().await?.to_bytes();
    if !status.is_success() {
        // Docker's error response has its own message ("No such container: xxx", etc.).
        // Just returning "docker API returned 404" makes it impossible to tell why the container
        // couldn't be fetched -- common when getting logs/usage and the snapshot container was deleted.
        anyhow::bail!(
            "{}",
            docker_error_message(status.as_u16(), &String::from_utf8_lossy(&body))
        );
    }
    Ok(body.to_vec())
}

/// Docker calls that need write actions (start / stop / restart / remove).
/// Returns `(status code, response body)` -- on failure, response body has docker's own message, pass to user.
async fn docker_write(method: &str, path: &str) -> anyhow::Result<(u16, String)> {
    if !std::path::Path::new(SOCKET).exists() {
        anyhow::bail!("docker socket not found");
    }
    let stream = tokio::net::UnixStream::connect(SOCKET).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("docker socket HTTP handshake")?;
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

    /// Real `docker inspect` skeleton: containers started by compose have com.docker.compose.* labels
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

    /// Non-compose container: has Labels but not the compose keys
    const PLAIN_INSPECT: &str = r#"{
        "Id": "9a8b7c",
        "State": { "Status": "exited", "StartedAt": "0001-01-01T00:00:00Z", "FinishedAt": "2026-09-18T08:00:00Z" },
        "Config": { "Labels": { "org.opencontainers.image.title": "nginx" } }
    }"#;

    /// Old Docker version / abnormal response: entire Config missing
    const NO_CONFIG_INSPECT: &str = r#"{
        "Id": "111111",
        "State": { "Status": "running", "StartedAt": "0001-01-01T00:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z" }
    }"#;

    /// Real `docker stats?stream=false` skeleton (cgroup v2: inactive_file)
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

    /// Docker returns `"Labels": null` for containers without labels, not an empty object
    const NULL_LABELS_INSPECT: &str = r#"{
        "Id": "222222",
        "State": { "Status": "running", "StartedAt": "0001-01-01T00:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z" },
        "Config": { "Labels": null }
    }"#;

    fn runtime_of(raw: &str) -> ContainerRuntime {
        let parsed: InspectResponse = serde_json::from_str(raw).expect("inspect JSON should parse");
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
        // When deleting a running container, docker returns this -- user should see it, not just the status code
        assert_eq!(
            docker_error_message(409, r#"{"message":"You cannot remove a running container abc. Stop the container before attempting removal or force remove"}"#),
            "docker rejected this operation: You cannot remove a running container abc. Stop the container before attempting removal or force remove"
        );
    }

    #[test]
    fn docker_error_message_falls_back_to_the_status_code() {
        // No message / not JSON / empty object, shouldn't turn the response into blank
        assert_eq!(docker_error_message(500, ""), "docker API returned 500");
        assert_eq!(
            docker_error_message(404, "<html>Not Found</html>"),
            "docker API returned 404"
        );
        assert_eq!(docker_error_message(400, "{}"), "docker API returned 400");
        assert_eq!(
            docker_error_message(400, r#"{"message":"   "}"#),
            "docker API returned 400"
        );
    }

    #[test]
    fn limits_come_from_host_config() {
        // When cpus: "2" and mem_limit: 512m in compose, inspect looks like this
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
        // `--cpu-quota 50000 --cpu-period 100000` = half core, NanoCpus is empty
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
        // Old daemon / abnormal response: limits unknown, leave as 0 (UI shows "unlimited", not fake numbers)
        let rt = runtime_of(NO_CONFIG_INSPECT);
        assert_eq!(rt.mem_limit_bytes, 0);
        assert_eq!(rt.cpu_limit_nano, 0);
    }

    #[test]
    fn repeat_click_on_a_settled_container_is_not_an_error() {
        // Docker returns 304 for "already running + start" -- treating it as failure makes user think
        // "container won't stop/start" -- this is the most common collision with the 5-minute-old snapshot
        assert_eq!(
            already_message("nginx", "start"),
            "Container nginx already running (no change made this time)"
        );
        assert_eq!(
            already_message("nginx", "stop"),
            "Container nginx already stopped (no change made this time)"
        );
        assert_eq!(
            already_message("nginx", "restart"),
            "Container nginx just restarted (no change made this time)"
        );
    }

    #[test]
    fn usage_subtracts_page_cache_and_computes_cpu_percent() {
        let parsed: StatsResponse =
            serde_json::from_str(STATS_CGROUP_V2).expect("stats JSON should parse");
        let u = usage_from_stats(&parsed);
        // 100 MiB - 20 MiB inactive_file
        assert_eq!(u.mem_usage_bytes, 83886080);
        // (300e6-200e6)/(2000e6-1000e6) * 4 cores * 100 = 40%
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
        .expect("stats JSON should parse");
        let u = usage_from_stats(&parsed);
        // v1 uses cache (same as docker CLI), only falls back to total_inactive_file if missing
        assert_eq!(u.mem_usage_bytes, 104857600 - 41943040);
        // online_cpus missing -> use percpu_usage length; neither here -> can't compute percentage, don't lie
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
        .expect("stats JSON should parse");
        let u = usage_from_stats(&parsed);
        // (100e6/1000e6) * 2 cores * 100 = 20%
        assert!((u.cpu_percent - 20.0).abs() < 1e-9);
        assert_eq!(u.mem_usage_bytes, 0);
    }

    #[test]
    fn usage_of_a_stopped_container_is_all_zero() {
        // Stopped container: docker returns stats without memory/cpu details, not parsing failure or error
        let parsed: StatsResponse = serde_json::from_str(r#"{"read":"2026-09-23T10:00:00Z"}"#)
            .expect("stats JSON should parse");
        let u = usage_from_stats(&parsed);
        assert_eq!(u.mem_usage_bytes, 0);
        assert_eq!(u.cpu_percent, 0.0);
    }
}
