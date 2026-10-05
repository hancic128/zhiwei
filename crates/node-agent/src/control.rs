//! Control channel: pull commands -> verify signature -> execute -> sign receipt.
//!
//! Security points (corresponding to design doc D7): commands must be signed by **ops private key**, node
//! uses ops public key obtained during enroll to verify. Monitor only forwards, so even if compromised
//! it cannot forge commands.

use std::time::Duration;

use anyhow::{bail, Context};
use base64::Engine;
use prost::Message as _;
use tracing::{info, warn};
use zhiwei_common::{KeyPair, PublicKey, Signature, Timestamp};
use zhiwei_proto::control::{Action, Command, CommandResult};

use std::sync::Arc;

use crate::docker;
use crate::NodeState;

/// `ACTION_FETCH_LOGS` parameters (JSON, matches proto's `FetchLogsParams` fields)
#[derive(Debug, serde::Deserialize)]
struct FetchLogsArgs {
    #[serde(default)]
    source: String,
    #[serde(default)]
    container: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    tail: u32,
    #[serde(default)]
    timestamps: bool,
}

/// `ACTION_KILL_PROCESS` parameters (JSON, matches proto's `KillProcessParams` fields)
#[derive(Debug, serde::Deserialize)]
struct KillProcessArgs {
    #[serde(default)]
    pid: i32,
    #[serde(default)]
    signal: String,
}

/// `ACTION_CONTAINER`_* parameters (JSON, matches proto's `ContainerActionParams`)
#[derive(Debug, serde::Deserialize)]
struct ContainerActionArgs {
    #[serde(default)]
    container: String,
    /// Only for `container_remove`: whether to force-delete running containers. Default false.
    #[serde(default)]
    force: bool,
}

/// `ACTION_SCAN_CERTS` parameters (JSON, matches proto's `ScanCertsParams`)
#[derive(Debug, serde::Deserialize)]
struct ScanCertsArgs {
    #[serde(default)]
    path: String,
}

/// `ACTION_UPGRADE_AGENT` parameters (JSON, matches proto's `UpgradeAgentParams`)
#[derive(Debug, serde::Deserialize)]
struct UpgradeAgentArgs {
    version: String,
    download_url: String,
    sha256: String,
    #[serde(default = "default_restart_true")]
    restart: bool,
}

const fn default_restart_true() -> bool {
    true
}

/// Seconds to block on long-poll (monitor side limit is 25s, using same value here).
///
/// Previously "pull every 10 seconds": clicking delete container / test cert path took up to 10 seconds
/// for command to be pulled. Changed to blocking long-poll, command arrives immediately after signing
/// (monitor wakes up right away).
const POLL_WAIT_SECS: u64 = 25;
/// Fallback poll interval (seconds): when peer is old monitor (doesn't know `wait`, returns empty immediately)
/// or on request error, fall back to timed polling -- otherwise would busy-poll and saturate the peer.
const POLL_INTERVAL: u64 = 10;

/// Allowlist of executable actions. Node capabilities (P2-3 later) will further constrain on this base.
const fn action_allowed(action: Action) -> bool {
    matches!(
        action,
        Action::Noop
            | Action::FetchLogs
            | Action::KillProcess
            | Action::RestartHost
            | Action::ShutdownHost
            | Action::ContainerStart
            | Action::ContainerStop
            | Action::ContainerRestart
            | Action::ContainerRemove
            | Action::RefreshInventory
            | Action::ScanCerts
            | Action::UpgradeAgent
    )
}

pub async fn run_poll_loop(monitor: String, state: Arc<NodeState>, node_id: String) {
    if state.ops_public_key.is_none() {
        warn!("No ops public key held, command channel will not pull commands (secure default)");
        return;
    }

    loop {
        let started = std::time::Instant::now();
        match poll_once(&monitor, &state, &node_id).await {
            // Got a command: pull again immediately, don't wait for next round if commands come in batches
            Ok(true) => {}
            Ok(false) => {
                // When long-poll is "rejected" (old monitor doesn't know wait, returns empty immediately),
                // fall back to POLL_INTERVAL timed polling; otherwise would become busy polling.
                if started.elapsed() < Duration::from_secs(POLL_WAIT_SECS / 2) {
                    tokio::time::sleep(Duration::from_secs(POLL_INTERVAL)).await;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "Command poll failed");
                tokio::time::sleep(Duration::from_secs(POLL_INTERVAL)).await;
            }
        }
    }
}

/// Pull once, returns "whether any command was fetched this round".
async fn poll_once(monitor: &str, state: &NodeState, node_id: &str) -> anyhow::Result<bool> {
    let t = crate::transport(monitor, state)?;
    let (status, raw) = t
        .request(
            "GET",
            &format!("/v1/commands?node_id={node_id}&wait={POLL_WAIT_SECS}"),
            &[],
            node_id,
            &state.signing_key,
        )
        .await?;
    if status != 200 {
        anyhow::bail!("pull command returned {status}");
    }
    if raw.is_empty() {
        return Ok(false);
    }

    let v: serde_json::Value = serde_json::from_slice(&raw)?;
    let Some(list) = v.get("commands").and_then(|c| c.as_array()) else {
        return Ok(false);
    };

    let mut got = false;
    for b64 in list.iter().filter_map(|x| x.as_str()) {
        got |= handle_command(b64, monitor, state, node_id).await?;
    }
    Ok(got)
}

/// Decode one command, verify it, run it and submit the receipt.
///
/// Returns `false` when the payload could not be decoded (bad base64), so it
/// does not count as "fetched a command"; a decode failure on a later entry
/// still propagates as an error.
async fn handle_command(
    b64: &str,
    monitor: &str,
    state: &NodeState,
    node_id: &str,
) -> anyhow::Result<bool> {
    let Some(cmd) = decode_command(b64)? else {
        return Ok(false);
    };

    match verify(&cmd, state) {
        Ok(()) => accept_command(&cmd, monitor, state, node_id).await,
        Err(e) => reject_command(&cmd, monitor, state, node_id, e).await,
    }
    Ok(true)
}

/// Decode a base64 command payload; `Ok(None)` = decode failure for this entry.
fn decode_command(b64: &str) -> anyhow::Result<Option<Command>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
    match Command::decode(&bytes[..]) {
        Ok(c) => Ok(Some(c)),
        Err(e) => {
            warn!(error = %e, "Failed to decode command");
            Ok(None)
        }
    }
}

/// Verified command: execute it, then submit the signed receipt.
async fn accept_command(cmd: &Command, monitor: &str, state: &NodeState, node_id: &str) {
    info!(command_id = %cmd.id, action = ?cmd.action, "Executing command");
    let result = execute(cmd, state).await;
    if let Err(e) = submit_result(monitor, state, node_id, &cmd.id, result).await {
        warn!(error = %e, "Failed to submit receipt");
    }
}

/// Signature verification failure must leave a trace: may indicate monitor is
/// compromised and forging commands.
async fn reject_command(
    cmd: &Command,
    monitor: &str,
    state: &NodeState,
    node_id: &str,
    e: anyhow::Error,
) {
    warn!(command_id = %cmd.id, error = %e, "Command signature verification failed, rejected");
    let _ = submit_result(
        monitor,
        state,
        node_id,
        &cmd.id,
        Err(anyhow::anyhow!("signature verification failed: {e}")),
    )
    .await;
}

/// Verify ops signature + TTL. Nonce deduplication covered by TTL (TTL ≤ 60s).
fn verify(cmd: &Command, state: &NodeState) -> anyhow::Result<()> {
    let Some(pub_b64) = state.ops_public_key.as_deref() else {
        bail!("no ops public key held");
    };
    let pub_bytes = base64::engine::general_purpose::STANDARD.decode(pub_b64.trim())?;
    let public_key = PublicKey(pub_bytes);

    let now = Timestamp::now().unix_nano();
    let age_ns = now.saturating_sub(cmd.issued_at_unix_nano);
    if cmd.ttl_seconds > 0 && age_ns > cmd.ttl_seconds.saturating_mul(1_000_000_000) {
        bail!(
            "command expired (issued {}ns ago, TTL {}s)",
            age_ns,
            cmd.ttl_seconds
        );
    }
    if age_ns < 0 {
        bail!("command issued time is in the future");
    }

    let mut unsigned = cmd.clone();
    let sig = Signature(std::mem::take(&mut unsigned.signature));
    let mut preimage = Vec::new();
    unsigned.encode(&mut preimage)?;

    KeyPair::verify(&public_key, &preimage, &sig).context("ops signature mismatch")?;
    Ok(())
}

async fn execute(cmd: &Command, state: &NodeState) -> anyhow::Result<Vec<u8>> {
    let action = Action::try_from(cmd.action).unwrap_or(Action::Unspecified);
    if !action_allowed(action) {
        bail!("action {action:?} not in allowlist");
    }

    match action {
        Action::Noop => Ok(format!("noop ok at {}", Timestamp::now().unix_nano()).into_bytes()),

        Action::FetchLogs => execute_fetch_logs(cmd).await,

        Action::KillProcess => execute_kill_process(cmd).await,

        // Restart/shutdown always use `+1` (1 minute later) instead of `now`:
        // ① command can return normally, receipt and audit are persisted before power cut; ② one-minute cancellation window.
        Action::RestartHost => run_shutdown(&["-r", "+1"]).await,
        Action::ShutdownHost => run_shutdown(&["-h", "+1"]).await,

        Action::ContainerStart | Action::ContainerStop | Action::ContainerRestart => {
            execute_container_lifecycle(cmd, action).await
        }

        // Remove container: force not passed by default, running containers are rejected by docker with
        // "stop it first" message -- not making the "force delete for you" decision for the user
        Action::ContainerRemove => execute_container_remove(cmd).await,

        // Console clicked "rescan snapshot now": set flag, main loop sends inventory on next iteration,
        // don't wait for 5-minute cycle (needed after container start/stop for container list)
        Action::RefreshInventory => {
            state
                .inventory_due
                .store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(b"snapshot rescan triggered".to_vec())
        }

        // Console "test" button: expand user's path with same rules, actually scan once,
        // results returned as JSON receipt, UI directly lists matched certs or failure reasons
        Action::ScanCerts => execute_scan_certs(cmd),

        // Remote upgrade: download new binary, verify, backup, replace, restart
        Action::UpgradeAgent => execute_upgrade_agent(cmd).await,

        Action::Unspecified => bail!("unimplemented action"),
    }
}

async fn execute_fetch_logs(cmd: &Command) -> anyhow::Result<Vec<u8>> {
    let p: FetchLogsArgs =
        serde_json::from_str(&cmd.params_json).context("failed to parse fetch_logs params")?;
    let tail = if p.tail == 0 { 200 } else { p.tail };

    let text = match p.source.as_str() {
        "container" => {
            if p.container.is_empty() {
                bail!("source=container requires container");
            }
            docker::container_logs(&p.container, tail, p.timestamps).await?
        }
        "file" => {
            if p.path.is_empty() {
                bail!("source=file requires path");
            }
            docker::read_file_tail(&p.path, tail)?
        }
        other => bail!("unknown source: {other}"),
    };
    Ok(text.into_bytes())
}

async fn execute_kill_process(cmd: &Command) -> anyhow::Result<Vec<u8>> {
    let p: KillProcessArgs =
        serde_json::from_str(&cmd.params_json).context("failed to parse kill_process params")?;
    // pid 1 is init, killing it cuts off the whole machine; 0 / negative also rejected
    if p.pid <= 1 {
        bail!("invalid pid (must be greater than 1)");
    }
    let flag = match p.signal.as_str() {
        "" | "term" => "-TERM",
        "kill" => "-KILL",
        other => bail!("unknown signal {other} (only term / kill supported)"),
    };
    let out = tokio::process::Command::new("kill")
        .arg(flag)
        .arg(p.pid.to_string())
        .output()
        .await
        .context("kill failed")?;
    if !out.status.success() {
        bail!(
            "kill {flag} {} failed: {}",
            p.pid,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(format!(
        "sent {}{} to pid {}",
        p.pid,
        flag,
        if flag == "-TERM" {
            " (graceful)"
        } else {
            " (force)"
        }
    )
    .into_bytes())
}

async fn execute_container_lifecycle(cmd: &Command, action: Action) -> anyhow::Result<Vec<u8>> {
    let p: ContainerActionArgs =
        serde_json::from_str(&cmd.params_json).context("failed to parse container params")?;
    let name = p.container.trim();
    if name.is_empty() {
        bail!("missing container (name or ID)");
    }
    let op = match action {
        Action::ContainerStart => "start",
        Action::ContainerStop => "stop",
        _ => "restart",
    };
    let out = docker::container_action(name, op).await?;
    Ok(out.into_bytes())
}

async fn execute_container_remove(cmd: &Command) -> anyhow::Result<Vec<u8>> {
    let p: ContainerActionArgs =
        serde_json::from_str(&cmd.params_json).context("failed to parse container params")?;
    let name = p.container.trim();
    if name.is_empty() {
        bail!("missing container (name or ID)");
    }
    let out = docker::container_remove(name, p.force).await?;
    Ok(out.into_bytes())
}

fn execute_scan_certs(cmd: &Command) -> anyhow::Result<Vec<u8>> {
    let p: ScanCertsArgs =
        serde_json::from_str(&cmd.params_json).context("failed to parse scan_certs params")?;
    let path = zhiwei_common::certpath::normalize(&p.path).map_err(|e| anyhow::anyhow!(e))?;
    let (patterns, entries) = crate::certs::scan_path(&path);
    let certs: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "path": e.info.path,
                "subject": e.info.subject,
                "issuer": e.info.issuer,
                "domains": e.info.domains,
                "not_after_unix_nano": e.info.not_after_unix_nano,
                "parse_error": e.info.parse_error,
                "error": e.error,
            })
        })
        .collect();
    let out = serde_json::json!({
        "path": path,
        "patterns": patterns,
        "matched": certs.len(),
        "certs": certs,
    });
    Ok(out.to_string().into_bytes())
}

/// Shutdown/reboot: prefer `shutdown`, fall back to `systemctl` (may not exist in containers or minimal systems).
async fn run_shutdown(args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let fallback: &[&str] = if args[0] == "-r" {
        &["reboot"]
    } else {
        &["poweroff"]
    };
    let mut last = String::new();
    for (bin, argv) in [("shutdown", args), ("systemctl", fallback)] {
        match tokio::process::Command::new(bin).args(argv).output().await {
            Ok(out) if out.status.success() => {
                return Ok(
                    format!("{bin} {} dispatched (executes in 1 minute)", argv.join(" "))
                        .into_bytes(),
                );
            }
            Ok(out) => {
                last = format!("{bin}: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Err(e) => last = format!("{bin}: {e}"),
        }
    }
    bail!("shutdown/reboot dispatch failed: {last}")
}

/// Upgrade the node-agent binary.
///
/// Downloads the new binary from the given URL, verifies its SHA256 checksum,
/// backs up the current version, atomically replaces it, and optionally restarts
/// the agent via systemd.
#[allow(clippy::cognitive_complexity)]
async fn execute_upgrade_agent(cmd: &Command) -> anyhow::Result<Vec<u8>> {
    let p: UpgradeAgentArgs =
        serde_json::from_str(&cmd.params_json).context("failed to parse upgrade_agent params")?;

    info!(
        version = %p.version,
        url = %p.download_url,
        "Starting agent upgrade"
    );

    // 1. Download the new binary to a temporary location
    let temp_path = format!("/tmp/node-agent-{}.bin", p.version);
    download_file(&p.download_url, &temp_path).await?;

    // 2. Verify SHA256 checksum
    let hash = sha256_file(&temp_path)?;
    if hash != p.sha256 {
        let _ = std::fs::remove_file(&temp_path);
        anyhow::bail!("SHA256 mismatch: expected {}, got {}", p.sha256, hash);
    }

    // 3. Get the current binary path
    let current_exe = std::env::current_exe().context("failed to get current executable path")?;

    // 4. Backup directory
    let backup_dir = std::path::PathBuf::from("/var/lib/zhiwei-agent/backup");
    std::fs::create_dir_all(&backup_dir).context("failed to create backup directory")?;
    let backup_path = backup_dir.join(format!("node-agent.{}", p.version));

    // 5. Backup current version (keep the version we're upgrading FROM)
    // If we're already at the target version, this is a no-op
    if !backup_path.exists() {
        std::fs::copy(&current_exe, &backup_path).context("failed to backup current binary")?;
        info!(backup_path = %backup_path.display(), "Current binary backed up");
    }

    // 6. Atomically replace the binary
    // On Unix, rename() is atomic if src and dst are on the same filesystem
    std::fs::rename(&temp_path, &current_exe).context("failed to replace binary")?;

    // 7. Ensure executable permission
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&current_exe)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)?;
    }

    info!("Binary replaced successfully");

    // 8. Restart agent via systemd if requested
    if p.restart {
        let output = tokio::process::Command::new("systemctl")
            .args(["restart", "node-agent"])
            .output()
            .await
            .context("failed to restart node-agent via systemctl")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("systemctl restart failed: {stderr}");
        }

        info!("Agent restart triggered via systemd");
    }

    Ok(format!(
        "upgraded to {}, backup at {}",
        p.version,
        backup_path.display()
    )
    .into_bytes())
}

/// Download a file from URL to local path using the agent's HTTP client.
async fn download_file(url: &str, dest_path: &str) -> anyhow::Result<()> {
    use std::io::Write;

    let response = reqwest::get(url)
        .await
        .with_context(|| format!("failed to download from {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("download failed with status {}: {}", response.status(), url);
    }

    let bytes = response
        .bytes()
        .await
        .context("failed to read response body")?;

    let mut file = std::fs::File::create(dest_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;

    Ok(())
}

/// Calculate SHA256 hash of a file.
fn sha256_file(path: &str) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

async fn submit_result(
    monitor: &str,
    state: &NodeState,
    node_id: &str,
    command_id: &str,
    outcome: anyhow::Result<Vec<u8>>,
) -> anyhow::Result<()> {
    let (ok, error, payload) = match outcome {
        Ok(p) => (true, String::new(), p),
        Err(e) => (false, e.to_string(), Vec::new()),
    };

    let mut result = CommandResult {
        command_id: command_id.to_string(),
        node_id: node_id.to_string(),
        ok,
        error,
        payload,
        finished_at_unix_nano: Timestamp::now().unix_nano(),
        signature: Vec::new(),
    };

    // Sign with node private key to prevent receipt tampering in transit or on server
    let mut preimage = Vec::new();
    result.encode(&mut preimage)?;
    result.signature = state.signing_key.sign(&preimage).0;

    let mut buf = Vec::new();
    result.encode(&mut buf)?;
    let t = crate::transport(monitor, state)?;
    let (status, body) = t
        .request(
            "POST",
            &format!("/v1/commands/{command_id}/result"),
            &buf,
            node_id,
            &state.signing_key,
        )
        .await?;
    if status != 204 {
        anyhow::bail!(
            "receipt submission returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    Ok(())
}
