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

/// 「告警级别 → 展示样式」的唯一出处。
///
/// 同一条告警要在四个地方长成一样：飞书卡片的标题栏、Slack 的侧栏色、载荷里的
/// 级别字段、纯文本标题的 emoji。以前这几处各写各的（卡片一套 template、文案一套
/// emoji、webhook 只透传 `severity`），想加一档就得改三处 match。现在只认这张表，
/// 渠道侧也不必再理解 severity 的取值。
///
/// 配色沿用青鸟（bluebird）调色板的命名：飞书要枚举名，Slack 要 hex，都从这里翻译。
struct LevelStyle {
    /// 稳定 key：青鸟通用来源的 `event`、通用 webhook 的 `level`（也是配色查表的键）
    key: &'static str,
    /// 中文档位名
    label: &'static str,
    emoji: &'static str,
    /// 飞书消息卡片 `header.template` 的枚举名
    feishu: &'static str,
    /// Slack `attachment.color`
    slack: &'static str,
}

/// 三档：严重 / 警告 / 已恢复。
///
/// 恢复单独成一档是有意的：以前恢复通知复用 warning 模板，手机上只显示
/// 「[警告] 服务 X · Y」，看着像又告警了一次。
fn level_style(rule: &AlertRule, firing: bool) -> LevelStyle {
    if !firing {
        LevelStyle {
            key: "resolved",
            label: "已恢复",
            emoji: "✅",
            feishu: "green",
            slack: "#2da44e",
        }
    } else if rule.severity == "critical" {
        LevelStyle {
            key: "critical",
            label: "严重",
            emoji: "🔴",
            feishu: "red",
            slack: "#cf222e",
        }
    } else {
        LevelStyle {
            key: "warning",
            label: "警告",
            emoji: "🟠",
            feishu: "orange",
            slack: "#d93f0b",
        }
    }
}

/// 通知标题行——**所有渠道共用同一份文案**。
///
/// 推送只展示标题时（手机通知栏、IM 折叠态、Bark）也得能回答「哪台机器、多严重、
/// 什么事」，所以节点与级别都压在这一行里，正文才可以说细节。
fn title_line(rule: &AlertRule, facts: &AlertFacts) -> String {
    let lv = level_style(rule, facts.firing);
    format!(
        "{} {} · {}（{}）",
        lv.emoji, lv.label, rule.name, facts.node
    )
}

/// 卡片分栏的 JSON 形态（通用 webhook / 青鸟载荷用）。
fn fields_json(facts: &AlertFacts) -> Vec<serde_json::Value> {
    facts
        .fields
        .iter()
        .map(|(label, value)| serde_json::json!({ "label": label, "value": value }))
        .collect()
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

/// 动态值进 Slack `mrkdwn` 前的中和：只挡 `<`，保留换行。
///
/// Slack 的 `<@U123>` / `<!channel>` 会真的 @ 人，和 lark_md 是同一类问题；
/// 但 Slack 的正文（探针失败原因等）可能是多行，不能像 lark_md 分栏那样压成一行。
fn slack_escape(raw: &str) -> String {
    raw.replace('<', "＜").replace('\r', "")
}

/// 卡片底部来源行（正文与标题都给了信息，这里只标来源）
const CARD_SOURCE: &str = "zhiwei 节点监控";

/// 飞书消息卡片（`msg_type: interactive` 的 `content`）。
///
/// 用卡片 1.0 而不是 2.0：2.0 经消息接口下发时会落到旧客户端的兜底文案
/// （「请升级至最新版本客户端，以查看内容」），实测踩过。1.0 的 `header.template`
/// 一样能上色，「折叠态也能一眼看出是告警还是恢复」这个诉求用 1.0 就能满足。
///
/// 这里只返回**裸卡片对象**：走 im/v1/messages 时它要被字符串化塞进 `content`，
/// 外面那层 `{"msg_type":..,"card":..}` 是自定义机器人 webhook 的壳，加了会被拒。
fn feishu_card(rule: &AlertRule, facts: &AlertFacts) -> serde_json::Value {
    let lv = level_style(rule, facts.firing);

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
        "config": { "wide_screen_mode": true },
        "header": {
            "template": lv.feishu,
            "title": {
                "tag": "plain_text",
                "content": title_line(rule, facts),
            },
        },
        "elements": elements,
    })
}

/// Slack Block Kit 卡片：彩色侧栏 + header + 分栏 + 正文 + 来源。
///
/// 侧栏色按级别走 [`level_style`] 的 hex——只发一行纯文本时，折叠态看不出是告警还是
/// 恢复。顶层 `text` 仍然保留：它是不支持 blocks 的客户端的回退文案，也是老接收端
/// 一直认的字段，去掉等于把既有集成弄坏。
fn slack_payload(rule: &AlertRule, facts: &AlertFacts) -> serde_json::Value {
    let lv = level_style(rule, facts.firing);

    let mut blocks = vec![serde_json::json!({
        "type": "header",
        "text": {
            "type": "plain_text",
            "text": title_line(rule, facts),
            "emoji": true,
        },
    })];
    if !facts.fields.is_empty() {
        let mut fields: Vec<serde_json::Value> = facts
            .fields
            .iter()
            .map(|(label, value)| {
                serde_json::json!({
                    "type": "mrkdwn",
                    "text": format!("*{label}*\n{}", slack_escape(value)),
                })
            })
            .collect();
        // Slack 一节最多 10 格
        fields.truncate(10);
        blocks.push(serde_json::json!({ "type": "section", "fields": fields }));
    }
    blocks.push(serde_json::json!({
        "type": "section",
        "text": { "type": "mrkdwn", "text": slack_escape(&facts.detail) },
    }));
    blocks.push(serde_json::json!({
        "type": "context",
        "elements": [{ "type": "mrkdwn", "text": CARD_SOURCE }],
    }));

    serde_json::json!({
        // 顶层 `text` 在 Slack 里是按 mrkdwn 解析的，动态值同样要中和
        "text": slack_escape(&plain_text(rule, facts)),
        "blocks": blocks,
        "attachments": [{ "color": lv.slack, "text": "" }],
    })
}

/// 对一批 telemetry 跑一遍全部启用规则。
pub async fn evaluate(state: &AppState, node_id: &NodeId, hostname: &str, batch: &TelemetryBatch) {
    let rules = match state.storage.alerts().enabled_rules().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Failed to read alert rules");
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
                warn!(error = %e, "Failed to read alert state");
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
                        info!(rule = %rule.name, %node_id, alert_id, "Alert firing");
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
                    Err(e) => warn!(error = %e, "Failed to open alert"),
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
                        warn!(error = %e, "Failed to close alert");
                    } else {
                        info!(rule = %rule.name, %node_id, alert_id, "Alert resolved");
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
            warn!(error = %e, "Failed to read notify channels");
            return;
        }
    };

    for ch in channels
        .into_iter()
        .filter(|c| c.enabled && severity_rank(&c.min_severity) <= severity_rank(&rule.severity))
    {
        // 配置不完整的渠道（老库里可能存着缺字段的行）只记一条日志，不反复刷投递失败
        if let Err(msg) = validate_channel(
            &ch.kind,
            &ch.url,
            &ch.secret,
            &ch.app_id,
            &ch.receive_id,
            &ch.receive_id_type,
        ) {
            warn!(channel = %ch.name, kind = %ch.kind, reason = %msg, "Channel config incomplete, skipping");
            continue;
        }
        if let Err(e) = deliver(&ch, rule, facts, now).await {
            warn!(channel = %ch.name, kind = %ch.kind, error = %e, "Failed to deliver notification");
        }
    }
}

/// 通知渠道支持的种类（字段设计对齐 bluebird 的分发渠道）。
///
/// 各自要填的东西不同：
/// - `feishu`：走官方应用接口——App ID + App Secret 换 tenant_access_token，
///   再按 receive_id（群 / 用户）发消息，不需要机器人地址；
/// - `slack`：Incoming Webhook 地址，地址本身即凭据；
/// - `bluebird`：青鸟（bluebird）通知网关的**通用来源**地址 `…/hooks/<来源 ID>`
///   加一枚 Token——把告警交给它去并发分发到 Bark / 飞书 / 企业微信等，zhiwei 不再自己接；
/// - `webhook`：自己的接收端，可选一个 Token 做鉴权。
pub const CHANNEL_KINDS: &[&str] = &["feishu", "slack", "bluebird", "webhook"];

/// 飞书的 receive_id_type 白名单（与 open.feishu.cn 的 im/v1/messages 一致）。
/// 填错时飞书回 99992402，不如在保存前就拦下来。
pub const FEISHU_RECEIVE_ID_TYPES: &[&str] =
    &["chat_id", "open_id", "user_id", "union_id", "email"];

/// 飞书开放平台的地址（可用环境变量指到 Lark 国际版 / 自建代理 / 测试桩）
fn feishu_base() -> String {
    match std::env::var("ZHIWEI_FEISHU_BASE") {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => "https://open.feishu.cn".into(),
    }
}

/// 保存 / 测试之前的参数校验，返回可直接展示给用户的错误信息。
///
/// 三种渠道要填的字段不同，前端也会按类型显示表单；这里是最后一道闸——
/// 老控制台、手写 curl、迁移过来的老数据都会从这条路上过。
pub fn validate_channel(
    kind: &str,
    url: &str,
    secret: &str,
    app_id: &str,
    receive_id: &str,
    receive_id_type: &str,
) -> Result<(), String> {
    if !CHANNEL_KINDS.contains(&kind) {
        return Err(format!(
            "不支持的通知类型 {}（可选：{}）",
            kind,
            CHANNEL_KINDS.join(" / ")
        ));
    }
    match kind {
        "feishu" => {
            if app_id.trim().is_empty() {
                return Err("飞书需要填写 App ID".into());
            }
            if secret.trim().is_empty() {
                return Err("飞书需要填写 App Secret".into());
            }
            if receive_id.trim().is_empty() {
                return Err("飞书需要填写接收 ID（群 chat_id / 用户 open_id 等）".into());
            }
            if !FEISHU_RECEIVE_ID_TYPES.contains(&receive_id_type) {
                return Err(format!(
                    "未知的飞书接收 ID 类型 {}（可选：{}）",
                    receive_id_type,
                    FEISHU_RECEIVE_ID_TYPES.join(" / ")
                ));
            }
            Ok(())
        }
        "slack" => {
            if url.trim().is_empty() {
                return Err("Slack 需要填写 Webhook URL".into());
            }
            Ok(())
        }
        // 青鸟的通用来源是 fail-closed 的：没配 Token 一律 401，
        // 所以这里把 Token 也当成必填，免得提示到投递失败里才暴露
        "bluebird" => {
            if url.trim().is_empty() {
                return Err("青鸟需要填写通用来源地址（…/hooks/<来源 ID>）".into());
            }
            if secret.trim().is_empty() {
                return Err("青鸟需要填写 Token（通用来源的访问令牌）".into());
            }
            Ok(())
        }
        _ => {
            if url.trim().is_empty() {
                return Err("通用 webhook 需要填写 URL".into());
            }
            Ok(())
        }
    }
}

/// 「测试」按钮用的临时渠道：只装控制台当前填的那几项，不入库。
/// `receive_id_type` 缺省成 `chat_id`，与飞书自己的默认一致。
pub fn test_channel(
    kind: &str,
    url: &str,
    secret: &str,
    app_id: &str,
    receive_id: &str,
    receive_id_type: &str,
) -> zhiwei_storage::alerts_repo::NotifyChannel {
    let rid_type = receive_id_type.trim();
    zhiwei_storage::alerts_repo::NotifyChannel {
        id: 0,
        name: "测试".into(),
        kind: kind.trim().to_string(),
        url: url.trim().to_string(),
        secret: secret.trim().to_string(),
        app_id: app_id.trim().to_string(),
        receive_id: receive_id.trim().to_string(),
        receive_id_type: if rid_type.is_empty() {
            "chat_id".into()
        } else {
            rid_type.to_string()
        },
        enabled: true,
        min_severity: "warning".into(),
    }
}

/// 投递一条通知到指定渠道。真告警与控制台的「测试」按钮共用这一条路径——
/// 否则「测试通过、真出事不发」这种偏差没人发现得了。
pub async fn deliver(
    ch: &zhiwei_storage::alerts_repo::NotifyChannel,
    rule: &AlertRule,
    facts: &AlertFacts,
    now: i64,
) -> anyhow::Result<()> {
    let body = channel_body(&ch.kind, rule, facts, now);
    match ch.kind.as_str() {
        "feishu" => {
            let token = feishu_token(ch.app_id.trim(), ch.secret.trim()).await?;
            let url = feishu_messages_url(&feishu_base(), ch.receive_id_type.trim());
            let payload = feishu_message_body(ch.receive_id.trim(), &body);
            post_json(&url, &token, &payload).await.map(|_| ())
        }
        // Slack 的 Incoming Webhook 地址本身即凭据，不带认证头
        "slack" => post_json(&ch.url, "", &body).await.map(|_| ()),
        // 青鸟通用来源：地址 + Token（Authorization: Bearer），与通用 webhook 同一条路
        "bluebird" => post_json(&ch.url, &ch.secret, &body).await.map(|_| ()),
        // 通用 webhook：`secret` 非空则带 Authorization: Bearer，接收端据此鉴权
        _ => post_json(&ch.url, &ch.secret, &body).await.map(|_| ()),
    }
}

/// 组装通知体。飞书给消息卡片（原本就是 `content` 要的裸卡片对象），
/// 其余按各自协议包一层。
pub fn channel_body(kind: &str, rule: &AlertRule, facts: &AlertFacts, now: i64) -> String {
    match kind {
        // 飞书走消息卡片：彩色标题栏 + 分栏。折叠态只剩标题栏时，颜色本身就是
        // 「严重 / 警告 / 已恢复」的信号，这是纯文本做不到的。
        "feishu" => feishu_card(rule, facts).to_string(),
        // Slack 同样给彩色卡片（侧栏色），顶层 text 留作回退
        "slack" => slack_payload(rule, facts).to_string(),
        // 青鸟（bluebird）通用来源：收 `{"title","body","event","repo"}` 或纯文本。
        // - `event` 是青鸟调色板的键，zhiwei 直接送级别 key（critical/warning/resolved），
        //   青鸟侧认识这三个键就按级别上色，不认识也只是回落成中性色，不影响投递；
        // - `fields` / `severity` 青鸟当前忽略，留给支持分栏的接收端，不算协议的一部分。
        // 顺带一提：青鸟通用来源的事件白名单只在显式配置时生效，而面板不给通用来源配白名单，
        // 所以这里送自定义 event 不会被静默丢掉。
        "bluebird" => {
            let lv = level_style(rule, facts.firing);
            serde_json::json!({
                "title": title_line(rule, facts),
                "body": facts.detail,
                "event": lv.key,
                "color": lv.slack,
                "fields": fields_json(facts),
                "severity": rule.severity,
                "hostname": facts.node,
                "at_unix_nano": now,
            })
            .to_string()
        }
        // webhook（含未知 kind，按 webhook 处理）：结构化 JSON
        //
        // `title` + `body` 是青鸟 / 通用 webhook 接收端的约定：收到 payload 后用
        // `title` 当通知标题、`body` 当正文（缺这两个字段的接收端会把消息丢掉）。
        // 但既有的自建接收端认的是 `text`，两个都给，谁也不必改。
        _ => {
            let lv = level_style(rule, facts.firing);
            serde_json::json!({
                "title": title_line(rule, facts),
                "text": plain_text(rule, facts),
                "body": plain_text(rule, facts),
                // 级别样式：接收端拿 `level` 查自己的配色，或直接用 `color`（hex）
                "level": lv.key,
                "color": lv.slack,
                "emoji": lv.emoji,
                "fields": fields_json(facts),
                "rule": rule.name,
                "severity": rule.severity,
                "hostname": facts.node,
                "metric": rule.metric,
                "value": facts.detail,
                "at_unix_nano": now,
            })
            .to_string()
        }
    }
}

/// 飞书 im/v1/messages 的发消息地址。
pub fn feishu_messages_url(base: &str, receive_id_type: &str) -> String {
    format!("{base}/open-apis/im/v1/messages?receive_id_type={receive_id_type}")
}

/// 飞书 im/v1/messages 的请求体。
///
/// `content` 要的是「卡片对象字符串化后的 JSON」；再包一层 `card` 会被飞书拒掉
/// （错误码 9499，现场踩过），所以这里只做转义、不加外壳。
pub fn feishu_message_body(receive_id: &str, card: &str) -> String {
    serde_json::json!({
        "receive_id": receive_id,
        "msg_type": "interactive",
        "content": card,
    })
    .to_string()
}

/// tenant_access_token 缓存：有效期 2h，按 (app_id, app_secret) 存。
/// 告警风暴里几十条通知只该换一次 token（换取的接口也有频率限制）。
static FEISHU_TOKENS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<(String, String), (String, i64)>>,
> = std::sync::OnceLock::new();

fn feishu_tokens(
) -> &'static std::sync::Mutex<std::collections::HashMap<(String, String), (String, i64)>> {
    FEISHU_TOKENS.get_or_init(Default::default)
}

fn now_unix_secs() -> i64 {
    Timestamp::now().unix_nano() / 1_000_000_000
}

/// 换取（或复用）飞书应用的 tenant_access_token。
async fn feishu_token(app_id: &str, app_secret: &str) -> anyhow::Result<String> {
    {
        let cache = feishu_tokens().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((token, expire_at)) = cache.get(&(app_id.to_string(), app_secret.to_string())) {
            // 提前 60s 当作过期，别卡在边界上让第一条通知撞 401
            if expire_at - 60 > now_unix_secs() {
                return Ok(token.clone());
            }
        }
    }

    let url = format!(
        "{}/open-apis/auth/v3/tenant_access_token/internal",
        feishu_base()
    );
    let body = serde_json::json!({ "app_id": app_id, "app_secret": app_secret }).to_string();
    let raw = post_json(&url, "", &body).await?;
    let v: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|e| anyhow::anyhow!("飞书返回的不是 JSON：{e}"))?;
    let token = v
        .get("tenant_access_token")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        let why = body_error(&raw).unwrap_or_else(|| body_detail(&raw));
        anyhow::bail!("换取飞书 tenant_access_token 失败{why}");
    }
    let expire = v.get("expire").and_then(|e| e.as_i64()).unwrap_or(7200);
    feishu_tokens()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            (app_id.to_string(), app_secret.to_string()),
            (token.clone(), now_unix_secs() + expire),
        );
    Ok(token)
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
/// 第一行就是所有渠道共用的 [`title_line`]（级别 + 规则名 + 节点）——手机 / 桌面
/// 推送只展示标题行时，也必须能看出是哪台节点。恢复用「已恢复」而不是复用「警告」，
/// 否则手机上看着像又告警了一次。第二行是这一条的具体细节（指标量值 / 探针原因）。
fn plain_text(rule: &AlertRule, facts: &AlertFacts) -> String {
    format!("{}\n{}", title_line(rule, facts), facts.detail)
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
    // 停用探针不再产生任何告警 / 通知（停用时的关闭动作在 PATCH 处理器里做，
    // 这里兜底：节点还没拿到停用配置时，最后一拍结果别把告警重新打开）
    if !probe.enabled {
        return;
    }
    let repo = state.storage.alerts();
    let now = Timestamp::now().unix_nano();
    let rule_name = format!("服务 {} · {}", probe.service_name, probe.name);

    if transition.new_state == STATE_OK {
        match repo.resolve_open_probe_alerts(&probe.id, now).await {
            Ok(n) if n > 0 => {
                info!(probe = %probe.name, "Service probe recovered, alert closed");
                // 「恢复上线」的推送受内置开关 service_online 控制；关告警本身不受
                // 影响——人都回来了，之前那条告警再挂着会误导。
                let notify_on = match repo.builtin_rule_enabled("service_online").await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "Failed to check service_online builtin toggle, enabling by default");
                        true
                    }
                };
                if !notify_on {
                    return;
                }
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
            Err(e) => warn!(error = %e, "Failed to close service probe alert"),
        }
        return;
    }

    if transition.new_state != STATE_DOWN {
        return;
    }

    // 服务离线：开告警与发通知都受内置开关 service_offline 控制。
    // 停用 = 完全不碰（不要求服务又活过来才能消红）。
    let enabled = match repo.builtin_rule_enabled("service_offline").await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to check service_offline builtin toggle, enabling by default");
            true
        }
    };
    if !enabled {
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
            &probe.id, &rule_name,
            // 告警记在「上报这台结果」的节点上：探针可能绑了多台（或任意节点），
            // 记上报节点才能一眼看出是哪台机器探到的
            node_id, hostname, severity, &message, now,
        )
        .await
    {
        Ok(alert_id) => {
            info!(probe = %probe.name, alert_id, "Service probe alert firing");
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
        Err(e) => warn!(error = %e, "Failed to open service probe alert"),
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

/// 节点离线告警的载体——和 `probe_alert_rule` 一样，纯用来喂 notify() 的
/// 「严重度过滤 + 命名」。`metric = host.online` 是为了和探针/证书告警的
/// 指标列做明显区分（前缀 `host.`），不会和真实指标冲突。
fn node_offline_alert_rule(severity: &str) -> AlertRule {
    AlertRule {
        id: 0,
        name: "节点离线".to_string(),
        metric: "host.online".to_string(),
        op: "eq".to_string(),
        threshold: 0.0,
        duration_seconds: 0,
        severity: severity.to_string(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// 节点上线事件的载体——和 `node_offline_alert_rule` 对仗，文案/严重度不同。
/// 命名空间同样用 `host.online`，与离线共享一行通知模板。
fn node_online_alert_rule() -> AlertRule {
    AlertRule {
        id: 0,
        name: "节点上线".to_string(),
        metric: "host.online".to_string(),
        op: "eq".to_string(),
        threshold: 1.0,
        duration_seconds: 0,
        severity: "warning".to_string(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// 节点上下线事件——一次性把一组节点的状态变化喂进来。
///
/// 设计上由后台任务每 ~30 秒扫一遍所有节点算出来（见 `spawn_node_liveness_watcher`）：
/// 上次在线 / 现在仍在线 = 跳过；上次在线 / 现在离线 = 开告警 + 通知；
/// 上次离线 / 现在在线 = 关旧离线告警 + 发「节点 X 已上线」通知；
/// 其它（从未上报 / 刚入网观察期） = 跳过。
///
/// 一个节点同时只允许一条未解决的离线告警（`open_node_offline_alert` 内部
/// 检查 `open_node_offline_alert_id`），所以重复触发不会堆历史。
///
/// 两条通知都受 `builtin_alert_rules` 里的 enabled 控制：
/// `node_offline` / `node_online`。停用某条就跳过对应的开告警 / 发通知。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Online,
    Offline,
}

/// 评估一组节点的最新 liveness，把翻转写入 alerts 表并按渠道投递通知。
///
/// 调用方要做的只是「拿 `last_seen` 算 online / offline」，别的事
/// （去重、关旧告警、严重度、通知文案、builtin 启用开关）都收在这里。
pub async fn on_node_liveness_change(state: &AppState, transitions: &[(String, String, Liveness)]) {
    for (node_id, hostname, status) in transitions {
        let now = Timestamp::now().unix_nano();
        let repo = state.storage.alerts();
        match status {
            Liveness::Offline => {
                // 内置规则关着就什么都不做——既不开告警也不发通知
                let enabled = match repo.builtin_rule_enabled("node_offline").await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(%node_id, error = %e, "Failed to check node_offline builtin toggle, enabling by default");
                        true
                    }
                };
                if !enabled {
                    continue;
                }

                // 已经在开着就只刷 message / started_at，不再通知（避免每 30 秒刷屏）
                let existing = match repo.open_node_offline_alert_id(node_id).await {
                    Ok(id) => id,
                    Err(e) => {
                        warn!(%node_id, error = %e, "Failed to query node offline alert");
                        continue;
                    }
                };
                let message = format!("Node {hostname} is offline");
                let severity = "critical";
                let alert_id = match repo
                    .open_node_offline_alert(node_id, hostname, severity, &message, now)
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        warn!(%node_id, error = %e, "Failed to open node offline alert");
                        continue;
                    }
                };
                if existing.is_none() {
                    // 只在「第一次开」时通知，避免 30s 周期里反复推
                    let rule = node_offline_alert_rule(severity);
                    let facts = AlertFacts {
                        node: hostname.clone(),
                        firing: true,
                        fields: vec![("Node", hostname.clone()), ("Status", "offline".to_string())],
                        detail: message,
                    };
                    notify(state, &rule, &facts, now).await;
                    info!(alert_id, %node_id, "Node offline alert firing");
                }
            }
            Liveness::Online => {
                // 关掉遗留的离线告警（这条不受 builtin 开关影响：人都上线了，
                // 之前那条告警再挂着会误导运维）
                let resolved = match repo.resolve_node_offline_alerts(node_id, now).await {
                    Ok(n) => n,
                    Err(e) => {
                        warn!(%node_id, error = %e, "Failed to close node offline alert");
                        0
                    }
                };

                // 「上线」是独立事件：只要从 Offline 翻转到 Online 就发通知，
                // 不依赖 resolved > 0——resolve 是为了清旧告警，与发通知是两条路。
                let enabled = match repo.builtin_rule_enabled("node_online").await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(%node_id, error = %e, "Failed to check node_online builtin toggle, enabling by default");
                        true
                    }
                };
                if enabled {
                    let rule = node_online_alert_rule();
                    let facts = AlertFacts {
                        node: hostname.clone(),
                        firing: true,
                        fields: vec![("节点", hostname.clone()), ("状态", "已上线".to_string())],
                        detail: format!("Node {hostname} is online"),
                    };
                    notify(state, &rule, &facts, now).await;
                    info!(%node_id, "Node online notification sent");
                }
                if resolved > 0 {
                    info!(count = resolved, %node_id, "Node offline alert closed");
                }
            }
        }
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
            warn!(error = %e, "Failed to read cert sources");
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

            // 全局开关：证书告警按严重度拆两档（cert_expiring / cert_expired），
            // 与每条来源自己的 `notify_enabled` 是「都开才报」的与关系。
            // 停用那一档时，既不开新的，也把已经开的关掉——本地阈值之上
            // 本来就不进这个循环，能走到这里说明「该不该报」已经成立。
            let gate = if expired {
                "cert_expired"
            } else {
                "cert_expiring"
            };
            let gate_enabled = match repo.builtin_rule_enabled(gate).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, %gate, "Failed to check builtin toggle, enabling by default");
                    true
                }
            };
            if !gate_enabled {
                if let Some(id) = open_by_ref.remove(&source_ref) {
                    if let Err(e) = repo.resolve_alert(id, now).await {
                        warn!(error = %e, %source_ref, "Failed to close cert alert");
                    }
                }
                continue;
            }

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
                            info!(cert = %name, %severity, id, "Certificate expiry alert firing");
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
                        Err(e) => warn!(error = %e, "Failed to open cert alert"),
                    }
                }
            }
        }
    }

    // 剩下的都是「不再成立」的：证书续签了、文件删了、来源停用/删除了
    for (source_ref, id) in open_by_ref {
        if let Err(e) = repo.resolve_alert(id, now).await {
            warn!(error = %e, %source_ref, "Failed to close cert alert");
        } else {
            info!(%source_ref, "Cert alert resolved");
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

/// 容器启停事件的载体——纯用来喂 notify() 的严重度过滤 + 命名，
/// 与 `probe_alert_rule` / `node_offline_alert_rule` 同一个套路。
fn container_event_rule(name: &str, severity: &str) -> AlertRule {
    AlertRule {
        id: 0,
        name: name.to_string(),
        metric: "container.state".to_string(),
        op: "eq".to_string(),
        threshold: 1.0,
        duration_seconds: 0,
        severity: severity.to_string(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// 一条容器事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerEvent {
    Started { id: String, name: String },
    Stopped { id: String, name: String },
}

fn container_running(state: &str) -> bool {
    state.eq_ignore_ascii_case("running")
}

/// 快照差异（纯函数，便于单测）：两次容器快照 JSON → 事件列表。
///
/// 主判据是 **state（running / 非 running）**，时间戳只用来补「周期内重启」：
/// 老版本 node-agent 不上报 started_at / finished_at（proto3 缺省即 0），
/// 拿时间戳当主判据会在 agent 升级后把所有存量容器误报一遍「已启动 / 已停止」。
///
/// - 出现在快照里且 running → `Started`（新容器、exited→running、周期内重启）；
///   其中「周期内重启」要求上次的 started_at 已知且本次更新（> 0），
///   避免把「刚学会读启动时间」当成「刚启动」；
/// - running→非 running，或从快照里消失（被 rm / 清理）→ `Stopped`；
/// - 其余（非 running→非 running、新增但已退出）不发事件——一次性容器
///   （跑完即退、`--rm`）不该每次同拍都刷一遍。
///
/// 返回顺序：停止在前、启动在后，稳定可测。
fn diff_containers(previous_json: &str, current_json: &str) -> Vec<ContainerEvent> {
    let prev = parse_container_map(previous_json);
    let cur = parse_container_map(current_json);
    if prev.is_empty() && cur.is_empty() {
        return Vec::new();
    }
    let mut stopped = Vec::new();
    let mut started = Vec::new();

    // 1) 快照里消失 = 已停止（被清理 / rm）
    for (id, (name, _started, _running)) in &prev {
        if !cur.contains_key(id) {
            stopped.push(ContainerEvent::Stopped {
                id: id.clone(),
                name: name.clone(),
            });
        }
    }

    // 2) 仍在快照里的容器：按 state 判定启停
    for (id, (name, cur_started, cur_running)) in &cur {
        match prev.get(id) {
            // 新容器：只有真的在跑才算「启动」（跑完即退的一次性容器不报）
            None => {
                if *cur_running {
                    started.push(ContainerEvent::Started {
                        id: id.clone(),
                        name: name.clone(),
                    });
                }
            }
            Some((_, prev_started, prev_running)) => {
                if !*prev_running && *cur_running {
                    started.push(ContainerEvent::Started {
                        id: id.clone(),
                        name: name.clone(),
                    });
                } else if *prev_running && !*cur_running {
                    stopped.push(ContainerEvent::Stopped {
                        id: id.clone(),
                        name: name.clone(),
                    });
                } else if *prev_running
                    && *cur_running
                    && *prev_started > 0
                    && *cur_started > *prev_started
                {
                    // 周期内重启：状态一直是 running，只有启动时间变了
                    started.push(ContainerEvent::Started {
                        id: id.clone(),
                        name: name.clone(),
                    });
                }
            }
        }
    }

    stopped.extend(started);
    stopped
}

/// 容器启停事件检测：对比同一节点上次与本次的容器快照，把差异转成事件。
///
/// 直接在 node_inventory 的最新一份快照上做差（每 5 分钟同拍一次），
/// 基线存在数据库里，节点 / monitor 重启都不会丢。没有任何历史快照
/// （`previous_json = None`）时只立基线、不发事件——避免升级 / 初装时
/// 把存量容器刷成一屏「已启动」。差异口径见 [`diff_containers`]。
///
/// 两条通知都受 `builtin_alert_rules` 控制：`container_started` /
/// `container_stopped`。「停止」开一条 `source = container` 的告警（待办里
/// 能看到、可静默），「启动」只关告警 + 发通知。
pub async fn on_container_events(
    state: &AppState,
    node_id: &str,
    hostname: &str,
    previous_json: Option<&str>,
    current_json: &str,
) {
    let Some(previous) = previous_json else {
        return; // 还没有历史基线，这次先立起来
    };
    let events = diff_containers(previous, current_json);
    if events.is_empty() {
        return;
    }

    let repo = state.storage.alerts();
    let now = Timestamp::now().unix_nano();

    for event in events {
        let (id, name) = match &event {
            ContainerEvent::Started { id, name } | ContainerEvent::Stopped { id, name } => {
                (id.clone(), name.clone())
            }
        };
        let short = id.chars().take(12).collect::<String>();

        match event {
            ContainerEvent::Stopped { .. } => {
                let enabled = match repo.builtin_rule_enabled("container_stopped").await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "Failed to check container_stopped builtin toggle, enabling by default");
                        true
                    }
                };
                if !enabled {
                    continue;
                }
                let message = format!("Container {name} ({short}) stopped");
                match repo
                    .open_container_alert(
                        &id,
                        "容器停止",
                        node_id,
                        hostname,
                        "warning",
                        &message,
                        now,
                    )
                    .await
                {
                    Ok(alert_id) => {
                        info!(container = %name, alert_id, %node_id, "Container stopped alert firing");
                        let rule = container_event_rule("容器停止", "warning");
                        let facts = AlertFacts {
                            node: hostname.to_string(),
                            firing: true,
                            fields: vec![
                                ("节点", hostname.to_string()),
                                ("容器", name.clone()),
                                ("ID", short.clone()),
                            ],
                            detail: message,
                        };
                        notify(state, &rule, &facts, now).await;
                    }
                    Err(e) => warn!(error = %e, "Failed to open container stopped alert"),
                }
            }
            ContainerEvent::Started { .. } => {
                // Close this container's unresolved stopped alert - even if started toggle is off:
                // if container is up, the old "stopped" alert would be misleading
                if let Err(e) = repo.resolve_open_container_alerts(&id, now).await {
                    warn!(error = %e, "Failed to close container stopped alert");
                }
                let enabled = match repo.builtin_rule_enabled("container_started").await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "Failed to check container_started builtin toggle, enabling by default");
                        true
                    }
                };
                if !enabled {
                    continue;
                }
                let message = format!("Container {name} ({short}) started");
                let rule = container_event_rule("容器启动", "info");
                let facts = AlertFacts {
                    node: hostname.to_string(),
                    firing: true,
                    fields: vec![
                        ("节点", hostname.to_string()),
                        ("容器", name.clone()),
                        ("ID", short.clone()),
                    ],
                    detail: message,
                };
                notify(state, &rule, &facts, now).await;
                info!(container = %name, %node_id, "Container started notification sent");
            }
        }
    }
}

/// 快照 JSON → { container_id: (name, started_at_unix_nano, running) }。
/// 字段可能与 node-agent 版本不完全一致，取不到就按 0 / false 处理。
fn parse_container_map(json: &str) -> std::collections::HashMap<String, (String, i64, bool)> {
    let mut out = std::collections::HashMap::new();
    let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return out;
    };
    for c in list {
        let Some(id) = c.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let name = c
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let started = c
            .get("started_at_unix_nano")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let running = c
            .get("state")
            .and_then(|v| v.as_str())
            .is_some_and(container_running);
        out.insert(id.to_string(), (name, started, running));
    }
    out
}

/// 投递一个 JSON 请求体，返回响应体。
///
/// `token` 非空时带 `Authorization: Bearer <token>`——飞书应用接口用它带
/// tenant_access_token，自建接收端可以用它做鉴权；Slack 的 Incoming Webhook
/// 地址本身就是「地址即凭据」，留空即可。
pub async fn post_json(url: &str, token: &str, body: &str) -> anyhow::Result<Vec<u8>> {
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

    // 飞书 / Slack 的地址都是 https；自建接收端多半是内网明文 http
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

/// 系统根证书（webhook 目标都是正经证书：飞书 / Slack 或用户自己的域名）
fn tls_config() -> anyhow::Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// POST 一个 JSON body 并读回响应体。http 与 https 共用（TcpStream 与 TlsStream 都实现同一组 trait）。
async fn send_post<S>(
    stream: S,
    host: &str,
    path: &str,
    token: &str,
    body: &str,
) -> anyhow::Result<Vec<u8>>
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
        anyhow::bail!("接口返回 {status}{}", body_detail(&body));
    }
    // 也有渠道失败仍返回 2xx：错误码在 body 里，不看就会把失败当成投递成功
    if let Some(e) = body_error(&body) {
        anyhow::bail!("接口返回 {status}，{e}");
    }
    Ok(body.to_vec())
}

/// 从响应体里挖出「为什么失败」。
///
/// 飞书把错误码放在 body（10003/10014 凭据不对、230001 接收 ID 与类型不匹配、
/// 99991672 权限没开），HTTP 状态往往还是 200——只看状态就把失败当成功。
fn body_error(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    // 飞书 code/msg；Slack 成功时 body 是纯文本 "ok"，解析不出 JSON
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

/// 已知错误码 → 排查方向。只覆盖飞书：它的错误码光看数字没法动手，
/// 而 Slack / 自建接收端出错多半就是地址不对，HTTP 状态已经说明问题。
fn channel_hint(code: i64) -> &'static str {
    match code {
        10003 => "：App ID / App Secret 不完整或不合法",
        10014 => "：App ID 或 App Secret 不正确",
        230001 => "：接收 ID 无效——核对接收 ID 与类型是否匹配（群是 oc_ 开头的 chat_id）",
        99992402 => "：接收 ID 类型不合法，可选 chat_id / open_id / user_id / union_id / email",
        99991672 => "：应用权限不足，请到开发者后台开通 im:message:send_as_bot 并发布版本",
        11232 => "：触发飞书限流（同一应用 50 次/秒）",
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
    info!("Inserted {} default alert rules", defaults.len());
    Ok(())
}

/// 「多久没上报」算离线——和前端 `livenessOf()` 同口径（见 ui/src/lib/utils.ts）。
/// 60s 是「刚刚还活着」；再往后由各告警规则各自的 duration 决定要不要告警。
pub const NODE_OFFLINE_AFTER_MS: i64 = 60_000;

/// 后台 liveness 巡检的周期。
///
/// 30s 是平衡值：太短会疯狂查库；太长会让「离线 → 告警」这条链感觉拖沓
/// （最坏情况要等一个完整周期才能进待办）。考虑到 telemetry 默认 10s 一次，
/// 30s 周期能保证任何节点失联后 ≤ 90s 内（= 60s 阈值 + 30s 巡检周期）出告警。
const LIVENESS_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// monitor 重启后的「误报压住」窗口。
///
/// 重启耗时段里所有节点都不会上报，它们的 `last_seen` 看起来 ≥ 60s 阈值。
/// 直接跑第一轮就会按「离线」全部告警一次，节点重连之后又自动恢复——告警
/// 页面会被刷一屏假的。120s 是经验值：超过这个时长的重启通常不是普通的
/// rolling restart，而是真的整个集群挂了，那时再报警也来得及。
///
/// 期间不读 last_seen_state、不发任何 transition：第一轮真正的「首轮」
/// 在 warmup 之后做，逻辑保持不变（None→Offline 仍会真告警）。
pub const LIVENESS_WARMUP_MS: i64 = 120_000;

/// 起一个常驻任务，周期扫所有节点的 `last_seen_unix_nano`，把翻转写到 alerts 表。
///
/// 第一次跑不通知——冷启动那几十秒里每个节点都「没拉过命令 / 没上报」，并不是故障；
/// 只在「曾经记过某个状态、这次变了」时才动 alerts / 通知。
///
/// 设计上不动 telemetry / 探针：节点持续在线时「最近一次」会一直更新，但本任务只看
/// 是否过了 60s 阈值，对冷启动 / 单次抖动都友好。
pub fn spawn_node_liveness_watcher(state: AppState) {
    tokio::spawn(async move {
        // 启动 5s 后再跑第一轮：给 telemetry 上报留时间，避免误报冷启动期。
        tokio::time::sleep(Duration::from_secs(5)).await;
        // 用 HashMap 记「上一轮认为它处于什么状态」，没记过的就跳过——
        // 「从未上报」的节点（刚入网 / 离线很久）不该一上来就报离线。
        let mut last_seen_state: std::collections::HashMap<String, Liveness> =
            std::collections::HashMap::new();

        loop {
            // warmup 检查搬到 compute_transitions 里面了：函数自己拿到
            // monitor_uptime_ms，整段决定要不要发 transition。
            // 这样调用点只剩一个干净的「跑一轮 + 记状态 + 通知」。
            let transitions =
                match compute_transitions(&state, &last_seen_state, state.started_at_ms).await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = %e, "Node liveness check failed");
                        tokio::time::sleep(LIVENESS_POLL_INTERVAL).await;
                        continue;
                    }
                };
            // 先把状态记下来再通知，避免 notify 期间又来一遍相同的转换
            for (id, _, liveness) in &transitions {
                last_seen_state.insert(id.clone(), *liveness);
            }
            if !transitions.is_empty() {
                on_node_liveness_change(&state, &transitions).await;
            }
            tokio::time::sleep(LIVENESS_POLL_INTERVAL).await;
        }
    });
}

/// 算一次「状态翻转」：每个节点拿最新 last_seen 算 Liveness，
/// 再和上一轮的对照——只有「上一轮 + 这一轮都能确定」才报。
///
/// `monitor_started_at_unix_ms` 是 monitor 进程启动的 Unix 毫秒，用来
/// 抑制重启后的冷启动误报：见 [`LIVENESS_WARMUP_MS`]。
/// warmup 期内**任何 transition 都不发**——包括「首轮就把所有 offline
/// 节点 push 一次」那条路径；否则重启耗时段里所有节点的 last_seen 都
/// 看起来 ≥ 60s 阈值，照样会刷一屏假离线。
async fn compute_transitions(
    state: &AppState,
    last_seen_state: &std::collections::HashMap<String, Liveness>,
    monitor_started_at_unix_ms: i64,
) -> anyhow::Result<Vec<(String, String, Liveness)>> {
    let nodes = state.storage.nodes().list_all().await?;
    let now_ms = Timestamp::now().unix_nano() / 1_000_000;
    let mut out = Vec::new();
    // monitor 重启后 cold-start 压住：所有 transition 都先不发，等节点
    // 重连稳下来再说。`monitor_started_at_unix_ms` 来自 monitor 启动时刻的
    // Unix ms；老的二进制/测试可能传 0，用 saturating_sub 让它直接当成「
    // 启动非常久」跑正常逻辑，不会被 stuck 在 warmup 里。
    let monitor_uptime_ms = now_ms.saturating_sub(monitor_started_at_unix_ms);
    if monitor_uptime_ms < LIVENESS_WARMUP_MS {
        tracing::debug!(
            monitor_uptime_ms,
            warmup_ms = LIVENESS_WARMUP_MS,
            "跳过 liveness transition 计算（warmup）"
        );
        return Ok(out);
    }
    for n in nodes {
        // 还没上报过的节点（刚入网 / 离线很久）跳过本轮：再给一个周期的窗口。
        let Some(last_seen) = n.last_seen_unix_nano else {
            continue;
        };
        let age_ms = now_ms - last_seen / 1_000_000;
        let current = if age_ms < NODE_OFFLINE_AFTER_MS {
            Liveness::Online
        } else {
            Liveness::Offline
        };
        // 仅在「上一轮记过、且与现在不同」时才报，避免冷启动噪声。
        match last_seen_state.get(&n.id) {
            Some(prev) if *prev != current => {
                let hostname = if !n.alias.is_empty() {
                    n.alias.clone()
                } else {
                    n.hostname.clone()
                };
                out.push((n.id, hostname, current));
            }
            None => {
                // 第一轮：记一下当前状态但不通知——避免重启后报一屏「刚启动就恢复」。
                // 但 offline 的节点**第一轮就报**：monitor 长时间重启之后就该接住。
                // 注意：warmup 期我们已经在函数顶上提前 return 了；走到这里说明
                // 已经过了 warmup，monitor 起码跑了 LIVENESS_WARMUP_MS，正常行为。
                if current == Liveness::Offline {
                    let hostname = if !n.alias.is_empty() {
                        n.alias.clone()
                    } else {
                        n.hostname.clone()
                    };
                    out.push((n.id, hostname, current));
                }
            }
            _ => {}
        }
    }
    Ok(out)
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
        assert_eq!(v["title"], "🟠 警告 · 磁盘使用率过高（shark-9）");
        assert!(v["text"].as_str().unwrap().contains("（shark-9）"));
        assert!(v["text"].as_str().unwrap().contains("磁盘使用率"));
        // `body` 是青鸟那类接收端的正文约定字段，`text` 是既有自建接收端认的，两个都要有
        assert_eq!(v["body"], v["text"]);
        // 分栏与配色一并给出，接收端不必再自己解析 title
        assert_eq!(v["fields"][0]["label"], "节点");
        assert_eq!(v["fields"][0]["value"], "shark-9");
        assert_eq!(v["level"], "warning");
        assert_eq!(v["color"], "#d93f0b");
    }

    /// 恢复通知不能长成「又告警一次」：文案换「已恢复」，级别与配色跟着变
    #[test]
    fn recovery_notifications_say_recovered() {
        let body = channel_body("webhook", &rule("warning"), &facts(false), 0);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "✅ 已恢复 · 磁盘使用率过高（shark-9）");
        assert_eq!(v["level"], "resolved");
        assert_eq!(v["color"], "#2da44e");
    }

    /// 青鸟（bluebird）通用来源的契约：`title` + `body`，级别走 `event`
    /// （青鸟拿它查自己的调色板）。Token 走 Authorization: Bearer，与通用 webhook 同路。
    #[test]
    fn bluebird_channel_follows_generic_source_contract() {
        let v: serde_json::Value = serde_json::from_str(&channel_body(
            "bluebird",
            &rule("critical"),
            &facts(true),
            8,
        ))
        .unwrap();
        assert_eq!(v["title"], "🔴 严重 · 磁盘使用率过高（shark-9）");
        assert_eq!(v["body"], "磁盘使用率 >85%（当前 91.0%）");
        assert_eq!(v["event"], "critical");
        assert_eq!(v["fields"][2]["label"], "当前值");
        assert_eq!(v["at_unix_nano"], 8);

        // 正文里不该再重复一遍标题行：青鸟会把 title 当卡片头、body 当正文
        let body = v["body"].as_str().unwrap();
        assert!(!body.contains("严重"), "{body}");
        assert!(!body.contains("磁盘使用率过高"), "{body}");
    }

    /// 飞书换成消息卡片，标题栏按「严重 / 警告 / 已恢复」三档上色
    #[test]
    fn feishu_channel_sends_colored_card() {
        let card = |severity: &str, firing: bool| -> serde_json::Value {
            serde_json::from_str(&channel_body("feishu", &rule(severity), &facts(firing), 0))
                .unwrap()
        };

        let critical = card("critical", true);
        assert_eq!(critical["header"]["template"], "red");
        assert_eq!(
            critical["header"]["title"]["content"],
            "🔴 严重 · 磁盘使用率过高（shark-9）"
        );

        assert_eq!(card("warning", true)["header"]["template"], "orange");

        let recovered = card("critical", false);
        assert_eq!(recovered["header"]["template"], "green");
        assert_eq!(
            recovered["header"]["title"]["content"],
            "✅ 已恢复 · 磁盘使用率过高（shark-9）"
        );
    }

    /// 走 im/v1/messages 时卡片要塞进 `content` 字符串，且**不能**再包一层 `card`
    /// （自定义机器人的壳，加了飞书直接回 9499）
    #[test]
    fn feishu_message_wraps_bare_card_into_content() {
        let card = channel_body("feishu", &rule("warning"), &facts(true), 0);
        assert!(
            card.starts_with('{') && !card.contains("\"msg_type\""),
            "{card}"
        );

        let body: serde_json::Value =
            serde_json::from_str(&feishu_message_body("oc_abc", &card)).unwrap();
        assert_eq!(body["receive_id"], "oc_abc");
        assert_eq!(body["msg_type"], "interactive");
        // content 是字符串化的卡片对象（飞书要的就是这种双层转义）
        let content: serde_json::Value =
            serde_json::from_str(body["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["header"]["template"], "orange");
        assert!(content.get("card").is_none(), "不该再包一层 card");

        assert_eq!(
            feishu_messages_url("https://open.feishu.cn", "chat_id"),
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id"
        );
    }

    /// 卡片把事实摊成分栏，而不是塞回一行文字
    #[test]
    fn feishu_card_lays_facts_into_fields() {
        let v: serde_json::Value =
            serde_json::from_str(&channel_body("feishu", &rule("critical"), &facts(true), 0))
                .unwrap();
        let elements = v["elements"].as_array().unwrap();
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
        let content = v["elements"][0]["fields"][0]["text"]["content"]
            .as_str()
            .unwrap();
        assert!(
            !content.contains('<'),
            "别名里的标签起始符没被中和：{content}"
        );
        // 原文本还得能认出来（是中和，不是删掉）
        assert!(content.contains("＜at id=all"));
    }

    /// Slack：从「一行纯文本」升级成按级别上色的 Block Kit 卡片。
    /// 顶层 `text` 必须留着——不支持 blocks 的客户端和老的纯文本接收端都读它。
    #[test]
    fn slack_channel_sends_colored_card() {
        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &rule("warning"), &facts(true), 0))
                .unwrap();
        assert!(slack["text"].as_str().unwrap().starts_with("🟠 警告 ·"));
        assert!(slack["text"].as_str().unwrap().contains("（shark-9）"));
        // 侧栏配色按级别
        assert_eq!(slack["attachments"][0]["color"], "#d93f0b");

        let blocks = slack["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(
            blocks[0]["text"]["text"],
            "🟠 警告 · 磁盘使用率过高（shark-9）"
        );
        let fields = blocks[1]["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0]["text"], "*节点*\nshark-9");
        // 正文与来源
        assert_eq!(blocks[2]["text"]["text"], "磁盘使用率 >85%（当前 91.0%）");
        assert_eq!(blocks[3]["elements"][0]["text"], "zhiwei 节点监控");
    }

    /// Slack 的 mrkdwn 会把 `<@U123>` / `<!channel>` 解析成真的 @，
    /// 而节点别名是没有字符校验的用户输入——凡是要按 mrkdwn 渲染的位置都必须中和。
    /// （header 只收 plain_text，不作解析，保持原样反而更好读。）
    #[test]
    fn slack_card_neutralizes_markup_in_alias() {
        let mut f = facts(true);
        f.node = "<!channel>".into();
        f.fields[0].1 = "<!channel>".into();
        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &rule("critical"), &f, 0)).unwrap();

        // 回退文本（mrkdwn）与分栏（mrkdwn）都要中和
        let fallback = slack["text"].as_str().unwrap();
        assert!(!fallback.contains("<!channel>"), "{fallback}");
        let field = slack["blocks"][1]["fields"][0]["text"].as_str().unwrap();
        assert!(!field.contains("<!channel>"), "{field}");
        assert!(field.contains("＜!channel>"), "{field}");

        // header 是 plain_text，Slack 不作标签解析
        assert_eq!(slack["blocks"][0]["text"]["type"], "plain_text");
    }

    /// 每种渠道要填的字段不同，缺哪个都要在保存前就说清楚
    #[test]
    fn channel_params_are_validated_per_kind() {
        // 飞书：App ID / App Secret / 接收 ID 缺一不可，类型要在白名单里
        assert!(validate_channel("feishu", "", "s", "cli_x", "oc_1", "chat_id").is_ok());
        assert!(validate_channel("feishu", "", "s", "", "oc_1", "chat_id")
            .unwrap_err()
            .contains("App ID"));
        assert!(
            validate_channel("feishu", "", "", "cli_x", "oc_1", "chat_id")
                .unwrap_err()
                .contains("App Secret")
        );
        assert!(validate_channel("feishu", "", "s", "cli_x", "", "chat_id")
            .unwrap_err()
            .contains("接收 ID"));
        assert!(
            validate_channel("feishu", "", "s", "cli_x", "oc_1", "chat-id")
                .unwrap_err()
                .contains("接收 ID 类型")
        );
        // 飞书不需要 url，缺了也放行（地址由 receive_id 决定）
        assert!(validate_channel("feishu", "", "s", "cli_x", "oc_1", "chat_id").is_ok());

        // Slack / 通用 webhook：必须要地址；webhook 的 Token 可选
        assert!(validate_channel("slack", "", "", "", "", "").is_err());
        assert!(validate_channel("slack", "https://hooks.slack.com/x", "", "", "", "").is_ok());
        assert!(validate_channel("webhook", "", "", "", "", "").is_err());
        assert!(validate_channel("webhook", "http://10.0.0.1/hook", "", "", "", "").is_ok());
        assert!(validate_channel("webhook", "http://10.0.0.1/hook", "tok", "", "", "").is_ok());

        // 青鸟：地址和 Token 都必填（通用来源没 Token 一律 401，早说早好）
        assert!(validate_channel("bluebird", "", "", "", "", "").is_err());
        assert!(validate_channel(
            "bluebird",
            "https://bb.example.com/hooks/zhiwei",
            "",
            "",
            "",
            ""
        )
        .unwrap_err()
        .contains("Token"));
        assert!(validate_channel(
            "bluebird",
            "https://bb.example.com/hooks/zhiwei",
            "tok",
            "",
            "",
            ""
        )
        .is_ok());

        // 下线的类型不能再进来（钉钉等）
        for kind in ["dingtalk", "bark", "wecom", "wechat"] {
            assert!(
                validate_channel(kind, "https://x/y", "", "", "", "").is_err(),
                "{kind} 不该被接受"
            );
        }
    }

    /// 测试按钮用的临时渠道：只填 url / kind 时也要能构出一条（receive_id_type 补默认值）
    #[test]
    fn test_channel_fills_defaults() {
        let ch = test_channel("feishu", "", " secret ", " cli_x ", " oc_1 ", "");
        assert_eq!(ch.kind, "feishu");
        assert_eq!(ch.receive_id_type, "chat_id");
        assert_eq!(ch.secret, "secret");
        assert_eq!(ch.app_id, "cli_x");
        assert_eq!(ch.receive_id, "oc_1");
    }

    /// 未知 kind 按 webhook 处理：老库里可能存着历史值，不能因此丢通知
    #[test]
    fn unknown_kind_falls_back_to_webhook() {
        let body = channel_body("something-new", &rule("warning"), &facts(true), 7);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rule"], "磁盘使用率过高");
    }

    /// 投递失败的原因得从 body 里挖：飞书把错误码放这里（HTTP 还是 200），
    /// 只看状态就会把失败当成功
    #[test]
    fn delivery_errors_surface_channel_codes() {
        // 成功形态都不算失败：飞书 code=0、Slack 纯文本 "ok"
        assert_eq!(body_error(br#"{"code":0,"msg":"success"}"#), None);
        assert_eq!(body_error(br#"{"errcode":0,"errmsg":"ok"}"#), None);
        assert_eq!(body_error(b"ok"), None);

        let bad_secret = body_error(br#"{"code":10014,"msg":"app secret invalid"}"#).unwrap();
        assert!(bad_secret.contains("10014"), "{bad_secret}");
        assert!(bad_secret.contains("App ID 或 App Secret"), "{bad_secret}");

        let bad_receive = body_error(br#"{"code":230001,"msg":"receive_id invalid"}"#).unwrap();
        assert!(bad_receive.contains("chat_id"), "{bad_receive}");

        // 非 2xx 时把响应体带上，别让自建接收端摸黑
        assert!(body_detail(b"nope").contains("nope"));
        assert_eq!(body_detail(b"   "), "");
    }

    /// 卡片的颜色与文案是纯函数，但「谁算严重」这条得跟 severity_rank 对齐
    #[test]
    fn level_palette_follows_severity() {
        assert_eq!(level_style(&rule("critical"), true).feishu, "red");
        assert_eq!(level_style(&rule("warning"), true).feishu, "orange");
        assert_eq!(level_style(&rule("info"), true).feishu, "orange");
        assert_eq!(level_style(&rule("critical"), false).feishu, "green");
    }

    /// 「级别 → 样式」只有一份：飞书的枚举名、Slack 的 hex、载荷里的 key 与 emoji
    /// 必须同源，否则换个渠道就会出现「飞书红、Slack 橙」这种自相矛盾
    #[test]
    fn channels_agree_on_level_style() {
        for (severity, firing, key, feishu, hex, emoji) in [
            ("critical", true, "critical", "red", "#cf222e", "🔴"),
            ("warning", true, "warning", "orange", "#d93f0b", "🟠"),
            ("critical", false, "resolved", "green", "#2da44e", "✅"),
        ] {
            let r = rule(severity);
            let f = facts(firing);
            let lv = level_style(&r, firing);
            assert_eq!(
                (lv.key, lv.feishu, lv.slack, lv.emoji),
                (key, feishu, hex, emoji)
            );

            let feishu_card: serde_json::Value =
                serde_json::from_str(&channel_body("feishu", &r, &f, 0)).unwrap();
            assert_eq!(feishu_card["header"]["template"], feishu);

            let slack: serde_json::Value =
                serde_json::from_str(&channel_body("slack", &r, &f, 0)).unwrap();
            assert_eq!(slack["attachments"][0]["color"], hex);

            for (kind, level_key) in [("webhook", "level"), ("bluebird", "event")] {
                let v: serde_json::Value =
                    serde_json::from_str(&channel_body(kind, &r, &f, 0)).unwrap();
                assert_eq!(v[level_key], key, "{kind}.{level_key}");
                assert_eq!(v["color"], hex, "{kind}.color");
            }
            let bb: serde_json::Value =
                serde_json::from_str(&channel_body("bluebird", &r, &f, 0)).unwrap();
            assert_eq!(bb["severity"], severity, "原始 severity 一并保留");
        }
    }

    /// 标题行所有渠道共用一份文案：推送只显示标题时（手机通知栏 / Bark / IM 折叠态）
    /// 也要能看出「哪台机器、多严重、什么事」
    #[test]
    fn title_line_is_shared_across_channels() {
        let r = rule("critical");
        let f = facts(true);
        let title = "🔴 严重 · 磁盘使用率过高（shark-9）";
        assert_eq!(title_line(&r, &f), title);

        let webhook: serde_json::Value =
            serde_json::from_str(&channel_body("webhook", &r, &f, 0)).unwrap();
        assert_eq!(webhook["title"], title);
        let bluebird: serde_json::Value =
            serde_json::from_str(&channel_body("bluebird", &r, &f, 0)).unwrap();
        assert_eq!(bluebird["title"], title);
        let feishu: serde_json::Value =
            serde_json::from_str(&channel_body("feishu", &r, &f, 0)).unwrap();
        assert_eq!(feishu["header"]["title"]["content"], title);
        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &r, &f, 0)).unwrap();
        assert_eq!(slack["blocks"][0]["text"]["text"], title);
    }

    /// 告警文案里的人话指标名：认识的要带中文名与单位，不认识的保留原名不崩
    #[test]
    fn metric_label_humanizes_known_metrics() {
        assert_eq!(metric_label("host.disk.usage"), ("磁盘使用率", "%"));
        assert_eq!(metric_label("host.mem.usage"), ("内存使用率", "%"));
        assert_eq!(metric_label("host.cpu.usage"), ("CPU 使用率", "%"));
        assert_eq!(metric_label("some.new.metric"), ("some.new.metric", ""));
    }

    /// 构造一份容器快照 JSON：`(id, name, started_at, state)`
    fn snap(entries: &[(&str, &str, i64, &str)]) -> String {
        let list: Vec<serde_json::Value> = entries
            .iter()
            .map(|(id, name, started, state)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "started_at_unix_nano": started,
                    "state": state,
                })
            })
            .collect();
        serde_json::to_string(&list).unwrap()
    }

    fn started(id: &str, name: &str) -> ContainerEvent {
        ContainerEvent::Started {
            id: id.into(),
            name: name.into(),
        }
    }
    fn stopped(id: &str, name: &str) -> ContainerEvent {
        ContainerEvent::Stopped {
            id: id.into(),
            name: name.into(),
        }
    }

    /// 两次快照一样 → 没有事件（「不刷屏」的核心：每 5 分钟同拍只是同一份状态）
    #[test]
    fn identical_snapshots_yield_no_events() {
        let s = snap(&[("c1", "web", 1000, "running"), ("c2", "db", 500, "exited")]);
        assert!(diff_containers(&s, &s).is_empty());
    }

    /// running 的新容器 = 已启动；从快照里消失 = 已停止
    #[test]
    fn new_running_container_starts_and_vanished_container_stops() {
        let prev = snap(&[("c1", "web", 1000, "running")]);
        let cur = snap(&[
            ("c1", "web", 1000, "running"),
            ("c2", "db", 2000, "running"),
        ]);
        assert_eq!(diff_containers(&prev, &cur), vec![started("c2", "db")]);

        let after = snap(&[("c2", "db", 2000, "running")]);
        assert_eq!(diff_containers(&cur, &after), vec![stopped("c1", "web")]);
    }

    /// running → exited 算停止；exited → running 算启动
    #[test]
    fn running_state_transitions_are_events() {
        let up = snap(&[("c1", "web", 1000, "running")]);
        let down = snap(&[("c1", "web", 1000, "exited")]);
        assert_eq!(diff_containers(&up, &down), vec![stopped("c1", "web")]);
        assert_eq!(diff_containers(&down, &up), vec![started("c1", "web")]);

        // dead / restarting 都算非 running
        let dead = snap(&[("c1", "web", 1000, "dead")]);
        assert_eq!(diff_containers(&up, &dead), vec![stopped("c1", "web")]);
    }

    /// 周期内重启：状态一直是 running，只有 started_at 变新 → 算启动
    #[test]
    fn restart_within_one_cycle_reports_start_only() {
        let prev = snap(&[("c1", "web", 1000, "running")]);
        let cur = snap(&[("c1", "web", 3000, "running")]);
        assert_eq!(diff_containers(&prev, &cur), vec![started("c1", "web")]);
    }

    /// 老版本 node-agent 不上报启动时间（0）：拿到真实时间不等于「刚启动」，
    /// 不能把存量容器误报成一片「已启动」（agent 升级后的刷屏）
    #[test]
    fn unknown_previous_start_time_does_not_report_a_burst() {
        let old = snap(&[("c1", "web", 0, "running"), ("c2", "db", 0, "running")]);
        let new = snap(&[
            ("c1", "web", 1700000000, "running"),
            ("c2", "db", 1700000000, "running"),
        ]);
        assert!(diff_containers(&old, &new).is_empty());

        // 但「状态真的变了」照旧要报
        let down = snap(&[("c1", "web", 1700000000, "exited")]);
        assert_eq!(
            diff_containers(&snap(&[("c1", "web", 0, "running")]), &down),
            vec![stopped("c1", "web")]
        );
    }

    /// 一次性容器（跑完即退）不该每次同拍都刷：新增但非 running 不报事件，
    /// exited → exited 也不报
    #[test]
    fn one_shot_containers_do_not_alert() {
        let empty = snap(&[]);
        let done = snap(&[("c1", "migrate", 1000, "exited")]);
        assert!(diff_containers(&empty, &done).is_empty());
        assert!(diff_containers(&done, &done).is_empty());
    }

    /// 脏 JSON / 缺字段不 panic、不误报
    #[test]
    fn malformed_snapshots_are_ignored() {
        assert!(diff_containers("not json", "not json").is_empty());
        assert!(diff_containers("[]", "[]").is_empty());
        // 缺 id 的条目被跳过
        let cur = r#"[{"name":"x","state":"running"}]"#;
        assert!(diff_containers("[]", cur).is_empty());
        // 缺 state：按非 running 处理，不报「已启动」
        let old = r#"[{"id":"c1","name":"web"}]"#;
        assert!(diff_containers(old, old).is_empty());
    }
}
