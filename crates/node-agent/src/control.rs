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

/// ACTION_FETCH_LOGS parameters (JSON, matches proto's FetchLogsParams fields)
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

/// ACTION_KILL_PROCESS parameters (JSON, matches proto's KillProcessParams fields)
#[derive(Debug, serde::Deserialize)]
struct KillProcessArgs {
    #[serde(default)]
    pid: i32,
    #[serde(default)]
    signal: String,
}

/// ACTION_CONTAINER_* parameters (JSON, matches proto's ContainerActionParams)
#[derive(Debug, serde::Deserialize)]
struct ContainerActionArgs {
    #[serde(default)]
    container: String,
    /// Only for container_remove: whether to force-delete running containers. Default false.
    #[serde(default)]
    force: bool,
}

/// ACTION_SCAN_CERTS parameters (JSON, matches proto's ScanCertsParams)
#[derive(Debug, serde::Deserialize)]
struct ScanCertsArgs {
    #[serde(default)]
    path: String,
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
fn action_allowed(action: Action) -> bool {
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
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
        let cmd = match Command::decode(&bytes[..]) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to decode command");
                continue;
            }
        };
        got = true;

        match verify(&cmd, state) {
            Ok(()) => {
                info!(command_id = %cmd.id, action = ?cmd.action, "Executing command");
                let result = execute(&cmd, state).await;
                if let Err(e) = submit_result(monitor, state, node_id, &cmd.id, result).await {
                    warn!(error = %e, "Failed to submit receipt");
                }
            }
            Err(e) => {
                // Signature verification failure must leave a trace: may indicate monitor is compromised forging commands
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
        }
    }
    Ok(got)
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

        Action::FetchLogs => {
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

        Action::KillProcess => {
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

        // Restart/shutdown always use `+1` (1 minute later) instead of `now`:
        // ① command can return normally, receipt and audit are persisted before power cut; ② one-minute cancellation window.
        Action::RestartHost => run_shutdown(&["-r", "+1"]).await,
        Action::ShutdownHost => run_shutdown(&["-h", "+1"]).await,

        Action::ContainerStart | Action::ContainerStop | Action::ContainerRestart => {
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

        // Remove container: force not passed by default, running containers are rejected by docker with
        // "stop it first" message -- not making the "force delete for you" decision for the user
        Action::ContainerRemove => {
            let p: ContainerActionArgs =
                serde_json::from_str(&cmd.params_json).context("failed to parse container params")?;
            let name = p.container.trim();
            if name.is_empty() {
                bail!("missing container (name or ID)");
            }
            let out = docker::container_remove(name, p.force).await?;
            Ok(out.into_bytes())
        }

        // Console clicked "rescan snapshot now": set flag, main loop sends inventory on next iteration,
        // don't wait for 5-minute cycle (needed after container start/stop for container list)
        Action::RefreshInventory => {
            state
                .inventory_due
                .store(true, std::sync::atomic::Ordering::Relaxed);
            Ok("snapshot rescan triggered".as_bytes().to_vec())
        }

        // Console "test" button: expand user's path with same rules, actually scan once,
        // results returned as JSON receipt, UI directly lists matched certs or failure reasons
        Action::ScanCerts => {
            let p: ScanCertsArgs =
                serde_json::from_str(&cmd.params_json).context("failed to parse scan_certs params")?;
            let path =
                zhiwei_common::certpath::normalize(&p.path).map_err(|e| anyhow::anyhow!(e))?;
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

        _ => bail!("unimplemented action"),
    }
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
                return Ok(format!("{bin} {} dispatched (executes in 1 minute)", argv.join(" ")).into_bytes());
            }
            Ok(out) => {
                last = format!("{bin}: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Err(e) => last = format!("{bin}: {e}"),
        }
    }
    bail!("shutdown/reboot dispatch failed: {last}")
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
        anyhow::bail!("receipt submission returned {status}: {}", String::from_utf8_lossy(&body));
    }
    Ok(())
}
