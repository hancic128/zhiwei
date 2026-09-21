//! 告警评估引擎。
//!
//! 在每次 telemetry 落库后运行：对每条启用规则、比对 batch 里的指标，
//! 维护「持续 N 秒越界才告警」的状态机，状态翻转时开/关告警并推送通知。

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tracing::{info, warn};
use zhiwei_common::{NodeId, Timestamp};
use zhiwei_proto::telemetry::TelemetryBatch;
use zhiwei_storage::alerts_repo::AlertRule;

use crate::state::AppState;

/// 越界判定
fn breaching(value: f64, op: &str, threshold: f64) -> bool {
    match op {
        "gt" => value > threshold,
        "gte" => value >= threshold,
        "lt" => value < threshold,
        "lte" => value <= threshold,
        "eq" => (value - threshold).abs() < f64::EPSILON,
        _ => false,
    }
}

fn op_symbol(op: &str) -> &str {
    match op {
        "gt" => ">",
        "gte" => ">=",
        "lt" => "<",
        "lte" => "<=",
        "eq" => "==",
        _ => op,
    }
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 2,
        "warning" => 1,
        _ => 0,
    }
}

/// 对一批 telemetry 跑一遍全部启用规则。
pub async fn evaluate(state: &AppState, node_id: &NodeId, hostname: &str, batch: &TelemetryBatch) {
    let rules = match state.storage.alerts().enabled_rules().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "读取告警规则失败");
            return;
        }
    };
    if rules.is_empty() {
        return;
    }

    let now = Timestamp::now().unix_nano();
    let repo = state.storage.alerts();

    for rule in rules {
        // 用统一的抽取口径：派生指标（内存百分比、网络合计）也能被规则用上。
        // 这条曾经只查 `metrics[]`——播种规则「内存使用率过高」用的 host.mem.usage
        // 节点从不上报，于是那条规则永远不会触发。
        let Some(value) = crate::routes::extract_metric(batch, &rule.metric) else {
            continue;
        };
        let hit = breaching(value, &rule.op, rule.threshold);

        let st = match repo.get_state(rule.id, node_id.as_str()).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "读取告警状态失败");
                continue;
            }
        };

        if hit && !st.firing {
            // 首次越界：记下起点；已持续足够久才开告警
            let since = st.breaching_since_unix_nano.unwrap_or(now);
            let held_ns = now.saturating_sub(since);
            let need_ns = rule.duration_seconds.saturating_mul(1_000_000_000);

            if held_ns >= need_ns {
                let message = format!(
                    "{} 在 {hostname} 上 {} {}{} （当前 {:.1}）",
                    rule.metric,
                    op_symbol(&rule.op),
                    rule.threshold,
                    if rule.duration_seconds > 0 {
                        format!("，已持续 {}s", rule.duration_seconds)
                    } else {
                        String::new()
                    },
                    value
                );
                match repo
                    .open_alert(&rule, node_id.as_str(), hostname, value, &message, now)
                    .await
                {
                    Ok(alert_id) => {
                        info!(rule = %rule.name, %node_id, alert_id, "告警触发");
                        let _ = repo
                            .upsert_state(
                                rule.id,
                                node_id.as_str(),
                                Some(since),
                                true,
                                Some(alert_id),
                                Some(value),
                            )
                            .await;
                        notify(state, &rule, hostname, &message, now).await;
                    }
                    Err(e) => warn!(error = %e, "开告警失败"),
                }
            } else {
                let _ = repo
                    .upsert_state(
                        rule.id,
                        node_id.as_str(),
                        Some(since),
                        false,
                        st.open_alert_id,
                        Some(value),
                    )
                    .await;
            }
        } else if !hit {
            // 恢复正常：若在告警中则关闭
            if st.firing {
                if let Some(alert_id) = st.open_alert_id {
                    if let Err(e) = repo.resolve_alert(alert_id, now).await {
                        warn!(error = %e, "关闭告警失败");
                    } else {
                        info!(rule = %rule.name, %node_id, alert_id, "告警恢复");
                    }
                }
            }
            let _ = repo
                .upsert_state(rule.id, node_id.as_str(), None, false, None, Some(value))
                .await;
        } else {
            // 持续越界中：只更新最新值
            let _ = repo
                .upsert_state(
                    rule.id,
                    node_id.as_str(),
                    st.breaching_since_unix_nano.or(Some(now)),
                    st.firing,
                    st.open_alert_id,
                    Some(value),
                )
                .await;
        }
    }
}

/// 按严重度投递到启用的通知渠道（当前仅 webhook）。
async fn notify(state: &AppState, rule: &AlertRule, hostname: &str, message: &str, now: i64) {
    let channels = match state.storage.alerts().list_channels().await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "读取通知渠道失败");
            return;
        }
    };

    for ch in channels
        .into_iter()
        .filter(|c| c.enabled && severity_rank(&c.min_severity) <= severity_rank(&rule.severity))
    {
        let body = channel_body(&ch.kind, rule, hostname, message, now);

        if let Err(e) = post_webhook(&ch.url, &ch.secret, &body).await {
            warn!(channel = %ch.name, kind = %ch.kind, error = %e, "通知投递失败");
        }
    }
}

/// 通知渠道支持的种类。前三个是各家 IM 的「自定义机器人」webhook，
/// 只是 JSON 外壳不同——统一在这里组装，投递链路（HTTP POST）完全复用。
pub const CHANNEL_KINDS: &[&str] = &["webhook", "feishu", "dingtalk", "slack"];

/// 组装通知体。`webhook` 给结构化 JSON（喂自己的接收端），其余三种按各自协议包一层。
pub fn channel_body(
    kind: &str,
    rule: &AlertRule,
    hostname: &str,
    message: &str,
    now: i64,
) -> String {
    match kind {
        "feishu" => serde_json::json!({
            "msg_type": "text",
            "content": { "text": plain_text(rule, hostname, message) },
        })
        .to_string(),
        "dingtalk" => serde_json::json!({
            "msgtype": "text",
            "text": { "content": plain_text(rule, hostname, message) },
        })
        .to_string(),
        "slack" => serde_json::json!({
            "text": plain_text(rule, hostname, message),
        })
        .to_string(),
        // webhook（含未知 kind，按 webhook 处理）：结构化 JSON
        //
        // `title` + `text` 是 Bluebird / 通用 webhook 接收端约定：
        // 收到 payload 后用 `title` 作为通知标题、`text` 作为正文；
        // 缺这两个字段的 webhook 会被通用接收端 `ignored` 掉（蓝鸟实测验证）。
        // 同时保留 `rule`/`severity`/... 让自建接收端也能消费。
        _ => serde_json::json!({
            "title": plain_text(rule, hostname, message).lines().next().unwrap_or("").to_string(),
            "text": plain_text(rule, hostname, message),
            "rule": rule.name,
            "severity": rule.severity,
            "hostname": hostname,
            "metric": rule.metric,
            "value": message,
            "at_unix_nano": now,
        })
        .to_string(),
    }
}

/// 「测试通知」用的一条假规则——只为把通知体组出来，不落库、不参与求值。
pub fn test_rule() -> AlertRule {
    AlertRule {
        id: 0,
        name: "测试通知".into(),
        metric: "host.cpu.usage".into(),
        op: "gt".into(),
        threshold: 90.0,
        duration_seconds: 0,
        severity: "warning".into(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// IM 文本通知：一眼能看出「哪台机器、多严重、什么事」。
fn plain_text(rule: &AlertRule, hostname: &str, message: &str) -> String {
    let level = if rule.severity == "critical" {
        "严重"
    } else {
        "警告"
    };
    format!("[{level}] {}\n主机：{hostname}\n{message}", rule.name)
}

/// 服务探针状态翻转 → 开/关告警（`source = probe`），并按严重度投递通知。
///
/// 通知策略：只有进入 `down`（critical）与从非 ok 恢复到 `ok` 才投递；
/// `degraded` 只反映在界面上，避免偶发抖动刷屏。
pub async fn on_probe_transition(
    state: &AppState,
    probe: &zhiwei_storage::probes_repo::Probe,
    transition: &zhiwei_storage::probes_repo::StateTransition,
    node_id: &str,
    hostname: &str,
) {
    use zhiwei_storage::probes_repo::{STATE_DOWN, STATE_OK};

    if !transition.changed {
        return;
    }
    let repo = state.storage.alerts();
    let now = Timestamp::now().unix_nano();
    let rule_name = format!("服务 {} · {}", probe.service_name, probe.name);

    if transition.new_state == STATE_OK {
        match repo.resolve_open_probe_alerts(&probe.id, now).await {
            Ok(n) if n > 0 => {
                info!(probe = %probe.name, "服务探针恢复，告警关闭");
                let rule = probe_alert_rule(&rule_name, "warning");
                let message = format!(
                    "服务 {} 的探针 {} 已恢复（此前 {:?}）",
                    probe.service_name, probe.name, transition.previous_state
                );
                notify(state, &rule, hostname, &message, now).await;
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "关闭服务探针告警失败"),
        }
        return;
    }

    if transition.new_state != STATE_DOWN {
        return;
    }

    let severity = "critical";
    let detail = if transition.last_error.is_empty() {
        format!("连续 {} 次检查失败", transition.consecutive_failures)
    } else {
        transition.last_error.clone()
    };
    let message = format!(
        "服务 {} 的探针 {} 异常（down）：{detail}",
        probe.service_name, probe.name
    );

    match repo
        .open_probe_alert(
            &probe.id,
            &rule_name,
            // 探针可能未绑定节点，此时记上报结果的这台节点
            probe.node_id.as_deref().unwrap_or(node_id),
            hostname,
            severity,
            &message,
            now,
        )
        .await
    {
        Ok(alert_id) => {
            info!(probe = %probe.name, alert_id, "服务探针告警触发");
            let rule = probe_alert_rule(&rule_name, severity);
            notify(state, &rule, hostname, &message, now).await;
        }
        Err(e) => warn!(error = %e, "开服务探针告警失败"),
    }
}

/// 探针告警没有指标规则行，这里造一个只用于「命名 + 严重度」的载体，
/// 让既有 notify() 的严重度过滤逻辑可以原样复用。
fn probe_alert_rule(name: &str, severity: &str) -> AlertRule {
    AlertRule {
        id: 0,
        name: name.to_string(),
        metric: "probe.state".to_string(),
        op: "eq".to_string(),
        threshold: 0.0,
        duration_seconds: 0,
        severity: severity.to_string(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// 证书到期评估：每次节点交回快照后跑一遍。
///
/// 判定口径：
///   * 只看**配了通知开关**的来源（`enabled && notify_enabled`）；
///   * 剩余天数 ≤ `notify_days_before` 即告警：已过期 critical、临期 warning；
///   * 一张证书一条告警，`source_ref = {source_id}:{证书路径}`；
///   * 续签（剩余天数回到阈值内）/ 路径改了 / 来源删了 → 自动 resolved。
///
/// 之所以跟着快照走而不是单独起定时任务：证书只在快照里出现，趁数据最新时
/// 判定最省事，也不会出现「快照换了、告警还停在旧值」的错配。
pub async fn evaluate_cert_expiry(
    state: &AppState,
    node_id: &str,
    hostname: &str,
    certs_json: &str,
) {
    let repo = state.storage.alerts();
    let sources = match state
        .storage
        .cert_sources()
        .list_for_node(node_id, true)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "读取证书来源失败");
            return;
        }
    };

    let now = Timestamp::now().unix_nano();
    let open = repo
        .open_cert_alerts_for_node(node_id)
        .await
        .unwrap_or_default();
    let mut open_by_ref: std::collections::HashMap<String, i64> =
        open.iter().map(|a| (a.source_ref.clone(), a.id)).collect();

    let certs: Vec<serde_json::Value> = serde_json::from_str(certs_json).unwrap_or_default();

    for source in sources.iter().filter(|s| s.notify_enabled) {
        for cert in &certs {
            if !cert_belongs_to(source, cert) {
                continue;
            }
            if cert
                .get("parse_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let Some(not_after) = cert.get("not_after_unix_nano").and_then(|v| v.as_i64()) else {
                continue;
            };
            let path = cert.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let days_left = (not_after - now) as f64 / 86_400_000_000_000.0;
            if days_left > source.notify_days_before as f64 {
                continue;
            }

            let subject = cert
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let domains: Vec<String> = cert
                .get("domains")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|d| d.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let name = domains.first().cloned().unwrap_or_else(|| subject.clone());
            let expired = days_left < 0.0;
            let severity = if expired { "critical" } else { "warning" };
            let message = if expired {
                format!(
                    "证书 {name}（{path}）已于 {} 天前过期",
                    (-days_left).floor().max(0.0) as i64
                )
            } else {
                format!(
                    "证书 {name}（{path}）还有 {} 天到期（阈值 {} 天）",
                    days_left.floor().max(0.0) as i64,
                    source.notify_days_before
                )
            };
            let source_ref = format!("{}:{}", source.id, path);

            match open_by_ref.remove(&source_ref) {
                // 已经开过：只在严重度/文案变化时更新，避免每次快照都写库
                Some(id) => {
                    if let Some(existing) = open.iter().find(|a| a.id == id) {
                        if existing.severity != severity || existing.message != message {
                            let _ = repo
                                .update_alert_message(id, severity, days_left, &message)
                                .await;
                        }
                    }
                }
                None => {
                    let rule_name = format!("证书到期 · {name}");
                    match repo
                        .open_cert_alert(
                            &source_ref,
                            &rule_name,
                            node_id,
                            hostname,
                            severity,
                            source.notify_days_before as f64,
                            days_left,
                            &message,
                            now,
                        )
                        .await
                    {
                        Ok(id) => {
                            info!(cert = %name, %severity, id, "证书到期告警触发");
                            let rule = cert_alert_rule(&rule_name, severity);
                            notify(state, &rule, hostname, &message, now).await;
                        }
                        Err(e) => warn!(error = %e, "开证书告警失败"),
                    }
                }
            }
        }
    }

    // 剩下的都是「不再成立」的：证书续签了、文件删了、来源停用/删除了
    for (source_ref, id) in open_by_ref {
        if let Err(e) = repo.resolve_alert(id, now).await {
            warn!(error = %e, %source_ref, "关闭证书告警失败");
        } else {
            info!(%source_ref, "证书告警已恢复");
        }
    }
}

/// 证书来源是否"拥有"这份证书（与 certs_api 的判定保持一致）
fn cert_belongs_to(
    source: &zhiwei_storage::cert_sources_repo::CertSource,
    cert: &serde_json::Value,
) -> bool {
    let sid = cert.get("source_id").and_then(|v| v.as_str()).unwrap_or("");
    if !sid.is_empty() {
        return sid == source.id;
    }
    cert.get("path")
        .and_then(|v| v.as_str())
        .is_some_and(|p| zhiwei_common::certpath::matches(&source.path, p))
}

fn cert_alert_rule(name: &str, severity: &str) -> AlertRule {
    AlertRule {
        id: 0,
        name: name.to_string(),
        metric: "cert.days_left".to_string(),
        op: "lte".to_string(),
        threshold: 0.0,
        duration_seconds: 0,
        severity: severity.to_string(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// 投递一个通知体到指定地址。
///
/// `token` 非空时带 `Authorization: Bearer <token>`——自建接收端可以用它做鉴权；
/// 飞书 / 钉钉 / Slack 的机器人地址本来就是「地址即凭据」，留空即可。
/// （`notify_channels.secret` 这一列从建表起就存在，但一直没被用上——这里补上。）
pub async fn post_webhook(url: &str, token: &str, body: &str) -> anyhow::Result<()> {
    let url = url.to_string();
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| anyhow::anyhow!("仅支持 http(s) webhook"))?;
    let tls = url.starts_with("https://");
    let (host_port, path) = match authority.find('/') {
        Some(i) => (&authority[..i], &authority[i..]),
        None => (authority, "/"),
    };

    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(if tls { 443 } else { 80 })),
        None => (host_port, if tls { 443 } else { 80 }),
    };

    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("连接超时"))??;

    // 飞书 / 钉钉 / Slack 的机器人地址都是 https；自建接收端多半是内网明文 http
    if tls {
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config()?));
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| anyhow::anyhow!("非法主机名 {host}: {e}"))?;
        let tls_stream = tokio::time::timeout(
            Duration::from_secs(5),
            connector.connect(server_name, stream),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TLS 握手超时"))?
        .map_err(|e| anyhow::anyhow!("TLS 握手失败：{e}"))?;
        send_post(tls_stream, host, path, token, body).await
    } else {
        send_post(stream, host, path, token, body).await
    }
}

/// 系统根证书（webhook 目标都是正经证书：飞书 / 钉钉 / Slack 或用户自己的域名）
fn tls_config() -> anyhow::Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// POST 一个 JSON body 并读回响应状态。http 与 https 共用（TcpStream 与 TlsStream 都实现同一组 trait）。
async fn send_post<S>(
    stream: S,
    host: &str,
    path: &str,
    token: &str,
    body: &str,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("Host", host)
        .header("Content-Type", "application/json");
    if !token.trim().is_empty() {
        builder = builder.header("Authorization", format!("Bearer {}", token.trim()));
    }
    let req = builder.body(Full::new(Bytes::from(body.to_string())))?;

    let res = tokio::time::timeout(Duration::from_secs(5), sender.send_request(req))
        .await
        .map_err(|_| anyhow::anyhow!("请求超时"))??;
    let status = res.status();
    let _ = res.into_body().collect().await;
    if !status.is_success() {
        anyhow::bail!("webhook 返回 {status}");
    }
    Ok(())
}

/// 首次启动时写入几条默认规则，避免告警页空着。
pub async fn seed_default_rules(state: &AppState) -> anyhow::Result<()> {
    let repo = state.storage.alerts();
    if repo.count_rules().await? > 0 {
        return Ok(());
    }
    let now = Timestamp::now().unix_nano();
    let defaults: [(&str, &str, &str, f64, i64, &str); 4] = [
        (
            "CPU 持续高负载",
            "host.cpu.usage",
            "gt",
            90.0,
            300,
            "warning",
        ),
        (
            "CPU 长时间打满",
            "host.cpu.usage",
            "gt",
            98.0,
            60,
            "critical",
        ),
        (
            "内存使用率过高",
            "host.mem.usage",
            "gt",
            90.0,
            300,
            "warning",
        ),
        (
            "磁盘使用率过高",
            "host.disk.usage",
            "gt",
            85.0,
            600,
            "warning",
        ),
    ];
    for (name, metric, op, threshold, duration, severity) in defaults {
        repo.create_rule(name, metric, op, threshold, duration, severity, now)
            .await?;
    }
    info!("已写入 {} 条默认告警规则", defaults.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(severity: &str) -> AlertRule {
        AlertRule {
            id: 1,
            name: "磁盘使用率过高".into(),
            metric: "host.disk.usage".into(),
            op: "gt".into(),
            threshold: 85.0,
            duration_seconds: 600,
            severity: severity.into(),
            enabled: true,
            created_at_unix_nano: 0,
            updated_at_unix_nano: 0,
        }
    }

    #[test]
    fn webhook_channel_keeps_structured_body() {
        let body = channel_body("webhook", &rule("warning"), "shark-9", "磁盘 91%", 42);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["hostname"], "shark-9");
        assert_eq!(v["at_unix_nano"], 42);
        // 通用 webhook 接收端约定（Bluebird 等）
        assert_eq!(v["title"], "[警告] 磁盘使用率过高");
        assert!(v["text"].as_str().unwrap().contains("主机：shark-9"));
        assert!(v["text"].as_str().unwrap().contains("磁盘 91%"));
    }

    #[test]
    fn im_channels_wrap_text_per_protocol() {
        let feishu: serde_json::Value = serde_json::from_str(&channel_body(
            "feishu",
            &rule("critical"),
            "bj",
            "站点不可用",
            0,
        ))
        .unwrap();
        assert_eq!(feishu["msg_type"], "text");
        assert!(feishu["content"]["text"]
            .as_str()
            .unwrap()
            .contains("[严重] 磁盘使用率过高"));
        assert!(feishu["content"]["text"]
            .as_str()
            .unwrap()
            .contains("主机：bj"));

        let dingtalk: serde_json::Value =
            serde_json::from_str(&channel_body("dingtalk", &rule("warning"), "bj", "x", 0))
                .unwrap();
        assert_eq!(dingtalk["msgtype"], "text");
        assert!(dingtalk["text"]["content"]
            .as_str()
            .unwrap()
            .contains("主机：bj"));

        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &rule("warning"), "bj", "x", 0)).unwrap();
        assert!(slack["text"].as_str().unwrap().starts_with("[警告]"));
    }

    /// 未知 kind 按 webhook 处理：老库里可能存着历史值，不能因此丢通知
    #[test]
    fn unknown_kind_falls_back_to_webhook() {
        let body = channel_body("something-new", &rule("warning"), "bj", "x", 7);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rule"], "磁盘使用率过高");
    }
}
