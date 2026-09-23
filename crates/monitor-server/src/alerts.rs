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

/// 指标名 →（人话名称，单位）。规则可以填任意指标名，认不出的原样展示——
/// 好过把 `host.disk.usage` 这种内部键和没头没尾的 `92.3` 丢给用户。
fn metric_label(metric: &str) -> (&str, &str) {
    match metric {
        "host.cpu.usage" => ("CPU 使用率", "%"),
        "host.mem.usage" => ("内存使用率", "%"),
        "host.disk.usage" => ("磁盘使用率", "%"),
        "host.disk.used_bytes" => ("磁盘占用", " B"),
        "host.mem.used_bytes" => ("内存占用", " B"),
        "host.net.rx_bytes" => ("网络接收", " B"),
        "host.net.tx_bytes" => ("网络发送", " B"),
        other => (other, ""),
    }
}

/// 一条告警的「事实」——渠道渲染的唯一输入。
///
/// 四类告警源（指标 / 节点上下线 / 服务探针 / 证书到期）能拿出来的东西并不一样：
/// 指标有当前值与阈值，探针有服务名，证书有剩余天数。所以这里不放各自的业务字段，
/// 只放「渲染需要什么」：谁、是触发还是恢复、卡片分几栏、正文那句话。各调用点把
/// 自己知道的填进来，渠道侧不必理解四套语义。
pub struct AlertFacts {
    /// 节点显示名（别名优先）
    pub node: String,
    /// true = 告警中，false = 已恢复。标题文案与配色都由它决定
    pub firing: bool,
    /// 卡片分栏，顺序即展示顺序（飞书每行两栏）
    pub fields: Vec<(&'static str, String)>,
    /// 正文那句话：说清「什么事、多严重」
    pub detail: String,
}

/// 严重度 + 触发/恢复 →（文案，标题 emoji）。
///
/// 恢复单独成一档是有意的：以前恢复通知复用 warning 模板，手机上只显示
/// 「[警告] 服务 X · Y」，看着像又告警了一次。
fn level_text(rule: &AlertRule, firing: bool) -> (&'static str, &'static str) {
    if !firing {
        ("已恢复", "✅")
    } else if rule.severity == "critical" {
        ("严重", "🔴")
    } else {
        ("警告", "🟠")
    }
}

/// 卡片标题栏配色：绿=已恢复，红=严重，橙=其余。
fn header_template(rule: &AlertRule, firing: bool) -> &'static str {
    if !firing {
        "green"
    } else if rule.severity == "critical" {
        "red"
    } else {
        "orange"
    }
}

/// 动态值进 `lark_md` 前的中和。
///
/// 只有 `<` 能开启 lark_md 标签，而节点别名是**没有字符校验**的用户输入
/// （见 `routes::normalize_alias`，只 trim + 限长），别名填 `<at id=all></at>`
/// 就会真的 @所有人。这里换成全角 `＜` 而不是删掉：既挡掉标签解析，又不会像删字符
/// 那样把「阈值 `> 90%`」这类正常内容弄坏（`>` 在 lark_md 里不是标记，原样留着）。
fn md_escape(raw: &str) -> String {
    raw.replace('<', "＜").replace('\n', " ").replace('\r', " ")
}

/// 卡片底部来源行（正文与标题都给了信息，这里只标来源）
const CARD_SOURCE: &str = "zhiwei 节点监控";

/// 飞书消息卡片（`msg_type: interactive`）。
///
/// 用卡片 1.0 而不是 2.0：2.0 经消息接口下发时会落到旧客户端的兜底文案
/// （「请升级至最新版本客户端，以查看内容」），实测踩过。1.0 的 `header.template`
/// 一样能上色，「折叠态也能一眼看出是告警还是恢复」这个诉求用 1.0 就能满足。
fn feishu_card(rule: &AlertRule, facts: &AlertFacts) -> serde_json::Value {
    let (level, emoji) = level_text(rule, facts.firing);

    let mut elements: Vec<serde_json::Value> = Vec::new();
    if !facts.fields.is_empty() {
        let fields: Vec<serde_json::Value> = facts
            .fields
            .iter()
            .map(|(label, value)| {
                serde_json::json!({
                    "is_short": true,
                    "text": {
                        "tag": "lark_md",
                        "content": format!("**{label}**\n{}", md_escape(value)),
                    },
                })
            })
            .collect();
        elements.push(serde_json::json!({ "tag": "div", "fields": fields }));
        elements.push(serde_json::json!({ "tag": "hr" }));
    }
    // 正文用 plain_text：探针失败原因这类内容不可控，不能被当成 lark_md 标签解析
    elements.push(serde_json::json!({
        "tag": "div",
        "text": { "tag": "plain_text", "content": facts.detail },
    }));
    elements.push(serde_json::json!({
        "tag": "note",
        "elements": [{ "tag": "plain_text", "content": CARD_SOURCE }],
    }));

    serde_json::json!({
        "msg_type": "interactive",
        "card": {
            "config": { "wide_screen_mode": true },
            "header": {
                "template": header_template(rule, facts.firing),
                "title": {
                    "tag": "plain_text",
                    "content": format!("{emoji} {level} · {}", rule.name),
                },
            },
            "elements": elements,
        },
    })
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
                // 文案要能一眼回答「哪台机器、什么指标、现在多少」——
                // 节点名在通知标题行（见 plain_text），这里给出指标与量值。
                let (label, unit) = metric_label(&rule.metric);
                let sym = op_symbol(&rule.op);
                let threshold = rule.threshold;
                let duration = if rule.duration_seconds > 0 {
                    format!("，已持续 {}s", rule.duration_seconds)
                } else {
                    String::new()
                };
                let message =
                    format!("{label} {sym}{threshold}{unit}（当前 {value:.1}{unit}{duration}）");
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
                        let facts = AlertFacts {
                            node: hostname.to_string(),
                            firing: true,
                            fields: vec![
                                ("节点", hostname.to_string()),
                                ("指标", label.to_string()),
                                ("当前值", format!("{value:.1}{unit}")),
                                ("阈值", format!("{sym}{threshold}{unit}")),
                            ],
                            detail: message.clone(),
                        };
                        notify(state, &rule, &facts, now).await;
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

/// 按严重度投递到启用的通知渠道。
async fn notify(state: &AppState, rule: &AlertRule, facts: &AlertFacts, now: i64) {
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
        let body = channel_body(&ch.kind, rule, facts, now);

        if let Err(e) = post_webhook(&ch.url, &ch.secret, &body).await {
            warn!(channel = %ch.name, kind = %ch.kind, error = %e, "通知投递失败");
        }
    }
}

/// 通知渠道支持的种类。前三个是各家 IM 的「自定义机器人」webhook，
/// 只是 JSON 外壳不同——统一在这里组装，投递链路（HTTP POST）完全复用。
pub const CHANNEL_KINDS: &[&str] = &["webhook", "feishu", "dingtalk", "slack"];

/// 组装通知体。`webhook` 给结构化 JSON（喂自己的接收端），其余三种按各自协议包一层。
pub fn channel_body(kind: &str, rule: &AlertRule, facts: &AlertFacts, now: i64) -> String {
    match kind {
        // 飞书走消息卡片：彩色标题栏 + 分栏。折叠态只剩标题栏时，颜色本身就是
        // 「严重 / 警告 / 已恢复」的信号，这是纯文本做不到的。
        "feishu" => feishu_card(rule, facts).to_string(),
        "dingtalk" => serde_json::json!({
            "msgtype": "text",
            "text": { "content": plain_text(rule, facts) },
        })
        .to_string(),
        "slack" => serde_json::json!({
            "text": plain_text(rule, facts),
        })
        .to_string(),
        // webhook（含未知 kind，按 webhook 处理）：结构化 JSON
        //
        // `title` + `text` 是 Bluebird / 通用 webhook 接收端约定：
        // 收到 payload 后用 `title` 作为通知标题、`text` 作为正文；
        // 缺这两个字段的 webhook 会被通用接收端 `ignored` 掉（蓝鸟实测验证）。
        // 同时保留 `rule`/`severity`/... 让自建接收端也能消费。
        _ => serde_json::json!({
            "title": plain_text(rule, facts).lines().next().unwrap_or("").to_string(),
            "text": plain_text(rule, facts),
            "rule": rule.name,
            "severity": rule.severity,
            "hostname": facts.node,
            "metric": rule.metric,
            "value": facts.detail,
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

/// 「测试通知」用的假事实。
///
/// 控制台的「测试」按钮要能**预演真实告警的样子**（含分栏），否则用户没法在群里
/// 判断样式、只能等真出事才看见。所以这里刻意填成一条指标告警。
pub fn test_facts() -> AlertFacts {
    AlertFacts {
        node: "zhiwei-test".into(),
        firing: true,
        fields: vec![
            ("节点", "zhiwei-test".into()),
            ("指标", "CPU 使用率".into()),
            ("当前值", "92.3%".into()),
            ("阈值", "> 90%".into()),
        ],
        detail: "这是一条测试通知，收到说明该渠道可用。".into(),
    }
}

/// IM 文本通知：一眼能看出「哪台机器、多严重、什么事」。
///
/// 第一行同时充当通用 webhook 的 `title`（见 [`channel_body`]）：手机 / 桌面
/// 推送只展示标题行时，也必须能看出是哪台节点，所以节点名放在这里而不是正文。
/// 恢复单独用「已恢复」而不是复用「警告」——否则手机上看着像又告警了一次。
fn plain_text(rule: &AlertRule, facts: &AlertFacts) -> String {
    let (level, _) = level_text(rule, facts.firing);
    format!(
        "[{level}] {}（{}）\n{}",
        rule.name, facts.node, facts.detail
    )
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
                let facts = AlertFacts {
                    node: hostname.to_string(),
                    firing: false,
                    fields: vec![
                        ("节点", hostname.to_string()),
                        ("服务", probe.service_name.clone()),
                        ("探针", probe.name.clone()),
                    ],
                    detail: message,
                };
                notify(state, &rule, &facts, now).await;
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
            let facts = AlertFacts {
                node: hostname.to_string(),
                firing: true,
                fields: vec![
                    ("节点", hostname.to_string()),
                    ("服务", probe.service_name.clone()),
                    ("探针", probe.name.clone()),
                    (
                        "连续失败",
                        format!("{} 次", transition.consecutive_failures),
                    ),
                ],
                detail: message,
            };
            notify(state, &rule, &facts, now).await;
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
                            let days_field = if expired {
                                format!("已过期 {} 天", (-days_left).floor().max(0.0) as i64)
                            } else {
                                format!("{} 天", days_left.floor().max(0.0) as i64)
                            };
                            let facts = AlertFacts {
                                node: hostname.to_string(),
                                firing: true,
                                fields: vec![
                                    ("节点", hostname.to_string()),
                                    ("证书", name.clone()),
                                    ("剩余", days_field),
                                    ("提醒阈值", format!("{} 天", source.notify_days_before)),
                                ],
                                detail: message,
                            };
                            notify(state, &rule, &facts, now).await;
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
    let body = res
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("webhook 返回 {status}{}", body_detail(&body));
    }
    // 也有渠道失败仍返回 2xx：错误码在 body 里，不看就会把失败当成投递成功
    if let Some(e) = body_error(&body) {
        anyhow::bail!("webhook 返回 {status}，{e}");
    }
    Ok(())
}

/// 从响应体里挖出「为什么失败」。
///
/// 飞书把错误码放在 body（9499 卡片结构错、19024 关键词不匹配、19021 签名失败、
/// 19022 IP 不在白名单），HTTP 状态常常就是 400——只看状态就只剩一句
/// 「webhook 返回 400 Bad Request」，卡片 JSON 写错时完全没法定位。
fn body_error(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    // 飞书 code/msg、钉钉 errcode/errmsg；Slack 成功时 body 是纯文本 "ok"，解析不出 JSON
    let code = v.get("code").or_else(|| v.get("errcode"))?.as_i64()?;
    if code == 0 {
        return None;
    }
    let msg = v
        .get("msg")
        .or_else(|| v.get("errmsg"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    Some(format!("错误码 {code}（{msg}）{}", channel_hint(code)))
}

/// 失败时把响应体带上（截断到 200 字符），便于自建接收端排查
fn body_detail(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("：{}", text.chars().take(200).collect::<String>())
    }
}

/// 已知错误码 → 排查方向。只覆盖飞书：自定义机器人的「JSON 写错」是日常故障，
/// 而 Slack / 钉钉的 webhook 地址本身即凭据，出错多半是地址不对。
fn channel_hint(code: i64) -> &'static str {
    match code {
        9499 => "：请求体结构不对，卡片 JSON 大概率有问题",
        19001 => "：卡片内容非法",
        19021 => "：机器人开了签名校验，zhiwei 还不支持加签，请先关掉或改用 IP 白名单 / 关键词",
        19022 => "：来源 IP 不在机器人的白名单里",
        19024 => "：消息没命中机器人的自定义关键词",
        11232 => "：触发飞书限流（单机器人 5 次/秒、100 次/分）",
        _ => "",
    }
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

    /// 一条指标告警的事实（形状与 `evaluate` 里的调用点一致）
    fn facts(firing: bool) -> AlertFacts {
        AlertFacts {
            node: "shark-9".into(),
            firing,
            fields: vec![
                ("节点", "shark-9".into()),
                ("指标", "磁盘使用率".into()),
                ("当前值", "91.0%".into()),
                ("阈值", "> 85%".into()),
            ],
            detail: "磁盘使用率 >85%（当前 91.0%）".into(),
        }
    }

    #[test]
    fn webhook_channel_keeps_structured_body() {
        let body = channel_body("webhook", &rule("warning"), &facts(true), 42);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["hostname"], "shark-9");
        assert_eq!(v["at_unix_nano"], 42);
        // 通用 webhook 接收端约定（Bluebird 等）
        assert_eq!(v["title"], "[警告] 磁盘使用率过高（shark-9）");
        assert!(v["text"].as_str().unwrap().contains("（shark-9）"));
        assert!(v["text"].as_str().unwrap().contains("磁盘使用率"));
    }

    /// 恢复通知不能长成「又告警一次」：文案换「已恢复」，通用 webhook 的 title 跟着变
    /// （蓝鸟那侧直接拿 title 当推送标题）
    #[test]
    fn recovery_notifications_say_recovered() {
        let body = channel_body("webhook", &rule("warning"), &facts(false), 0);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "[已恢复] 磁盘使用率过高（shark-9）");
    }

    /// 飞书换成消息卡片，标题栏按「严重 / 警告 / 已恢复」三档上色
    #[test]
    fn feishu_channel_sends_colored_card() {
        let card = |severity: &str, firing: bool| -> serde_json::Value {
            serde_json::from_str(&channel_body("feishu", &rule(severity), &facts(firing), 0))
                .unwrap()
        };

        let critical = card("critical", true);
        assert_eq!(critical["msg_type"], "interactive");
        assert_eq!(critical["card"]["header"]["template"], "red");
        assert_eq!(
            critical["card"]["header"]["title"]["content"],
            "🔴 严重 · 磁盘使用率过高"
        );

        assert_eq!(
            card("warning", true)["card"]["header"]["template"],
            "orange"
        );

        let recovered = card("critical", false);
        assert_eq!(recovered["card"]["header"]["template"], "green");
        assert_eq!(
            recovered["card"]["header"]["title"]["content"],
            "✅ 已恢复 · 磁盘使用率过高"
        );
    }

    /// 卡片把事实摊成分栏，而不是塞回一行文字
    #[test]
    fn feishu_card_lays_facts_into_fields() {
        let v: serde_json::Value =
            serde_json::from_str(&channel_body("feishu", &rule("critical"), &facts(true), 0))
                .unwrap();
        let elements = v["card"]["elements"].as_array().unwrap();
        let fields = elements[0]["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0]["text"]["content"], "**节点**\nshark-9");
        assert_eq!(fields[2]["text"]["content"], "**当前值**\n91.0%");
        // 阈值里的 `>` 是正常内容，不能被转义吃掉（曾经把「> 85%」变成「 85%」）
        assert_eq!(fields[3]["text"]["content"], "**阈值**\n> 85%");
        // 正文走 plain_text：探针失败原因这类内容不可控，不能当 lark_md 解析
        assert_eq!(elements[2]["text"]["tag"], "plain_text");
    }

    /// 别名没有字符校验（routes::normalize_alias 只 trim + 限长），塞进 lark_md
    /// 会真的 @所有人——动态值里能开启标签的 `<` 必须中和
    #[test]
    fn feishu_card_neutralizes_markup_in_alias() {
        let mut f = facts(true);
        f.node = "<at id=all></at>".into();
        f.fields[0].1 = "<at id=all></at>".into();
        let v: serde_json::Value =
            serde_json::from_str(&channel_body("feishu", &rule("critical"), &f, 0)).unwrap();
        let content = v["card"]["elements"][0]["fields"][0]["text"]["content"]
            .as_str()
            .unwrap();
        assert!(
            !content.contains('<'),
            "别名里的标签起始符没被中和：{content}"
        );
        // 原文本还得能认出来（是中和，不是删掉）
        assert!(content.contains("＜at id=all"));
    }

    #[test]
    fn im_channels_wrap_text_per_protocol() {
        let dingtalk: serde_json::Value =
            serde_json::from_str(&channel_body("dingtalk", &rule("warning"), &facts(true), 0))
                .unwrap();
        assert_eq!(dingtalk["msgtype"], "text");
        assert!(dingtalk["text"]["content"]
            .as_str()
            .unwrap()
            .contains("（shark-9）"));

        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &rule("warning"), &facts(true), 0))
                .unwrap();
        assert!(slack["text"].as_str().unwrap().starts_with("[警告]"));
    }

    /// 未知 kind 按 webhook 处理：老库里可能存着历史值，不能因此丢通知
    #[test]
    fn unknown_kind_falls_back_to_webhook() {
        let body = channel_body("something-new", &rule("warning"), &facts(true), 7);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rule"], "磁盘使用率过高");
    }

    /// 投递失败的原因得从 body 里挖：飞书把错误码放这里，只看 HTTP 状态的话
    /// 卡片 JSON 写错也只会看到一句「400 Bad Request」
    #[test]
    fn delivery_errors_surface_channel_codes() {
        // 成功形态都不算失败：飞书 code=0、钉钉 errcode=0、Slack 纯文本 "ok"
        assert_eq!(body_error(br#"{"code":0,"msg":"success"}"#), None);
        assert_eq!(body_error(br#"{"errcode":0,"errmsg":"ok"}"#), None);
        assert_eq!(body_error(b"ok"), None);

        let feishu = body_error(br#"{"code":19021,"msg":"sign match fail"}"#).unwrap();
        assert!(feishu.contains("19021"), "{feishu}");
        assert!(feishu.contains("加签"), "{feishu}");

        let dingtalk =
            body_error(br#"{"errcode":310000,"errmsg":"keywords not in content"}"#).unwrap();
        assert!(dingtalk.contains("310000") && dingtalk.contains("keywords not in content"));

        // 非 2xx 时把响应体带上，别让自建接收端摸黑
        assert!(body_detail(b"nope").contains("nope"));
        assert_eq!(body_detail(b"   "), "");
    }

    /// 卡片的颜色与文案是纯函数，但「谁算严重」这条得跟 severity_rank 对齐
    #[test]
    fn header_template_follows_severity() {
        assert_eq!(header_template(&rule("critical"), true), "red");
        assert_eq!(header_template(&rule("warning"), true), "orange");
        assert_eq!(header_template(&rule("info"), true), "orange");
        assert_eq!(header_template(&rule("critical"), false), "green");
    }

    /// 告警文案里的人话指标名：认识的要带中文名与单位，不认识的保留原名不崩
    #[test]
    fn metric_label_humanizes_known_metrics() {
        assert_eq!(metric_label("host.disk.usage"), ("磁盘使用率", "%"));
        assert_eq!(metric_label("host.mem.usage"), ("内存使用率", "%"));
        assert_eq!(metric_label("host.cpu.usage"), ("CPU 使用率", "%"));
        assert_eq!(metric_label("some.new.metric"), ("some.new.metric", ""));
    }
}
