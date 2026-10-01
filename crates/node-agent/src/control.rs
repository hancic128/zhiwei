//! 控制通道：拉取命令 → 验签 → 执行 → 签名回执。
//!
//! 安全要点（对应设计稿 D7）：命令必须由 **ops 私钥**签名，节点用 enroll
//! 时拿到的 ops 公钥验签。monitor 只负责转发，因此它被攻陷也伪造不了命令。

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

/// ACTION_FETCH_LOGS 的参数（JSON，与 proto 的 FetchLogsParams 字段一致）
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

/// ACTION_KILL_PROCESS 的参数（JSON，与 proto 的 KillProcessParams 字段一致）
#[derive(Debug, serde::Deserialize)]
struct KillProcessArgs {
    #[serde(default)]
    pid: i32,
    #[serde(default)]
    signal: String,
}

/// ACTION_CONTAINER_* 的参数（JSON，与 proto 的 ContainerActionParams 一致）
#[derive(Debug, serde::Deserialize)]
struct ContainerActionArgs {
    #[serde(default)]
    container: String,
    /// 仅 container_remove 用：是否强制删除运行中的容器。默认 false。
    #[serde(default)]
    force: bool,
}

/// ACTION_SCAN_CERTS 的参数（JSON，与 proto 的 ScanCertsParams 一致）
#[derive(Debug, serde::Deserialize)]
struct ScanCertsArgs {
    #[serde(default)]
    path: String,
}

/// 命令长轮询的挂起秒数（monitor 端上限 25s，这里取相同值）。
///
/// 之前是「每 10 秒拉一次」：点一次删除容器 / 测试证书路径，最多要等 10 秒
/// 命令才被取走。改成挂起式长轮询后，命令一签发（monitor 立刻唤醒）就到手。
const POLL_WAIT_SECS: u64 = 25;
/// 兜底轮询间隔（秒）：对端是旧版 monitor（不认识 `wait`、立刻返回空）或请求
/// 出错时，退回定时轮询——否则会忙轮询把对端打满。
const POLL_INTERVAL: u64 = 10;

/// 允许执行的动作白名单。节点能力集（P2-3 后续）会在此基础上再收敛。
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
        warn!("未持有 ops 公钥，控制通道不会拉取命令（安全侧默认拒绝）");
        return;
    }

    loop {
        let started = std::time::Instant::now();
        match poll_once(&monitor, &state, &node_id).await {
            // 拿到了命令：立刻再拉一次，命令成串时不必等下一轮
            Ok(true) => {}
            Ok(false) => {
                // 长轮询被「拒收」（旧版 monitor 不认 wait、立刻空手返回）时，
                // 退回 POLL_INTERVAL 定时轮询；否则这里会变成忙轮询。
                if started.elapsed() < Duration::from_secs(POLL_WAIT_SECS / 2) {
                    tokio::time::sleep(Duration::from_secs(POLL_INTERVAL)).await;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "命令轮询失败");
                tokio::time::sleep(Duration::from_secs(POLL_INTERVAL)).await;
            }
        }
    }
}

/// 拉一次命令，返回「本轮是否取到命令」。
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
        anyhow::bail!("拉取命令返回 {status}");
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
                warn!(error = %e, "解码命令失败");
                continue;
            }
        };
        got = true;

        match verify(&cmd, state) {
            Ok(()) => {
                info!(command_id = %cmd.id, action = ?cmd.action, "执行命令");
                let result = execute(&cmd, state).await;
                if let Err(e) = submit_result(monitor, state, node_id, &cmd.id, result).await {
                    warn!(error = %e, "回执提交失败");
                }
            }
            Err(e) => {
                // 验签失败必须留痕：可能是 monitor 被攻陷在伪造命令
                warn!(command_id = %cmd.id, error = %e, "命令验签失败，已拒绝执行");
                let _ = submit_result(
                    monitor,
                    state,
                    node_id,
                    &cmd.id,
                    Err(anyhow::anyhow!("验签失败：{e}")),
                )
                .await;
            }
        }
    }
    Ok(got)
}

/// 校验 ops 签名 + TTL。nonce 去重由 TTL 覆盖（TTL ≤ 60s）。
fn verify(cmd: &Command, state: &NodeState) -> anyhow::Result<()> {
    let Some(pub_b64) = state.ops_public_key.as_deref() else {
        bail!("未持有 ops 公钥");
    };
    let pub_bytes = base64::engine::general_purpose::STANDARD.decode(pub_b64.trim())?;
    let public_key = PublicKey(pub_bytes);

    let now = Timestamp::now().unix_nano();
    let age_ns = now.saturating_sub(cmd.issued_at_unix_nano);
    if cmd.ttl_seconds > 0 && age_ns > cmd.ttl_seconds.saturating_mul(1_000_000_000) {
        bail!(
            "命令已过期（签发于 {}ns 前，TTL {}s）",
            age_ns,
            cmd.ttl_seconds
        );
    }
    if age_ns < 0 {
        bail!("命令签发时间在未来");
    }

    let mut unsigned = cmd.clone();
    let sig = Signature(std::mem::take(&mut unsigned.signature));
    let mut preimage = Vec::new();
    unsigned.encode(&mut preimage)?;

    KeyPair::verify(&public_key, &preimage, &sig).context("ops 签名不匹配")?;
    Ok(())
}

async fn execute(cmd: &Command, state: &NodeState) -> anyhow::Result<Vec<u8>> {
    let action = Action::try_from(cmd.action).unwrap_or(Action::Unspecified);
    if !action_allowed(action) {
        bail!("动作 {action:?} 不在白名单内");
    }

    match action {
        Action::Noop => Ok(format!("noop ok at {}", Timestamp::now().unix_nano()).into_bytes()),

        Action::FetchLogs => {
            let p: FetchLogsArgs =
                serde_json::from_str(&cmd.params_json).context("解析 fetch_logs 参数失败")?;
            let tail = if p.tail == 0 { 200 } else { p.tail };

            let text = match p.source.as_str() {
                "container" => {
                    if p.container.is_empty() {
                        bail!("source=container 需要 container");
                    }
                    docker::container_logs(&p.container, tail, p.timestamps).await?
                }
                "file" => {
                    if p.path.is_empty() {
                        bail!("source=file 需要 path");
                    }
                    docker::read_file_tail(&p.path, tail)?
                }
                other => bail!("未知 source: {other}"),
            };
            Ok(text.into_bytes())
        }

        Action::KillProcess => {
            let p: KillProcessArgs =
                serde_json::from_str(&cmd.params_json).context("解析 kill_process 参数失败")?;
            // pid 1 是 init，杀它等于整机失联；0 / 负数同样拒绝
            if p.pid <= 1 {
                bail!("pid 非法（必须大于 1）");
            }
            let flag = match p.signal.as_str() {
                "" | "term" => "-TERM",
                "kill" => "-KILL",
                other => bail!("未知信号 {other}（只支持 term / kill）"),
            };
            let out = tokio::process::Command::new("kill")
                .arg(flag)
                .arg(p.pid.to_string())
                .output()
                .await
                .context("执行 kill 失败")?;
            if !out.status.success() {
                bail!(
                    "kill {flag} {} 失败：{}",
                    p.pid,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(format!(
                "已向 pid {} 发送 {}{}",
                p.pid,
                flag,
                if flag == "-TERM" {
                    "（优雅退出）"
                } else {
                    "（强杀）"
                }
            )
            .into_bytes())
        }

        // 重启 / 关机一律用 `+1`（1 分钟后执行）而不是 `now`：
        // ① 命令能正常返回，回执与审计落库不被断电截断；② 留出一分钟的取消窗口。
        Action::RestartHost => run_shutdown(&["-r", "+1"]).await,
        Action::ShutdownHost => run_shutdown(&["-h", "+1"]).await,

        Action::ContainerStart | Action::ContainerStop | Action::ContainerRestart => {
            let p: ContainerActionArgs =
                serde_json::from_str(&cmd.params_json).context("解析 container 参数失败")?;
            let name = p.container.trim();
            if name.is_empty() {
                bail!("缺少 container（容器名或 ID）");
            }
            let op = match action {
                Action::ContainerStart => "start",
                Action::ContainerStop => "stop",
                _ => "restart",
            };
            let out = docker::container_action(name, op).await?;
            Ok(out.into_bytes())
        }

        // 删除容器：默认不传 force，运行中的容器会被 docker 拒绝并回一句
        // 「先停掉它」——不替用户做「顺手强删」这个决定
        Action::ContainerRemove => {
            let p: ContainerActionArgs =
                serde_json::from_str(&cmd.params_json).context("解析 container 参数失败")?;
            let name = p.container.trim();
            if name.is_empty() {
                bail!("缺少 container（容器名或 ID）");
            }
            let out = docker::container_remove(name, p.force).await?;
            Ok(out.into_bytes())
        }

        // 控制台点了「立刻重采快照」：置位后主循环下一拍就发一次 inventory，
        // 不必等 5 分钟周期（容器列表的启动/关闭之后要用到）
        Action::RefreshInventory => {
            state
                .inventory_due
                .store(true, std::sync::atomic::Ordering::Relaxed);
            Ok("已触发快照重采".as_bytes().to_vec())
        }

        // 控制台「测试」按钮：把用户填的路径按同一套规则展开后真扫一遍，
        // 结果以 JSON 回执，界面直接列出命中的证书或失败原因
        Action::ScanCerts => {
            let p: ScanCertsArgs =
                serde_json::from_str(&cmd.params_json).context("解析 scan_certs 参数失败")?;
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

        _ => bail!("未实现的动作"),
    }
}

/// 关机 / 重启：`shutdown` 优先，退化到 `systemctl`（容器或精简系统里可能没有 shutdown）。
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
                return Ok(format!("{bin} {} 已下发（1 分钟后执行）", argv.join(" ")).into_bytes());
            }
            Ok(out) => {
                last = format!("{bin}: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Err(e) => last = format!("{bin}: {e}"),
        }
    }
    bail!("关机/重启下发失败：{last}")
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

    // 节点私钥签名，防止回执在传输途或服务端被篡改
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
        anyhow::bail!("回执提交返回 {status}: {}", String::from_utf8_lossy(&body));
    }
    Ok(())
}
