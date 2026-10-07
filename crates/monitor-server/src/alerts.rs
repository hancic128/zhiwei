//! Alert evaluation engine.
//!
//! Runs after each telemetry batch lands: for each enabled rule, compares metrics in the batch,
//! maintains the "alert only after sustained N seconds breaching" state machine, opens/closes
//! alerts on state transitions and pushes notifications.

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tracing::{info, warn};
use zhiwei_common::{NodeId, Timestamp};
use zhiwei_proto::telemetry::TelemetryBatch;
use zhiwei_storage::alerts_repo::{AlertRule, AlertsRepo, EvalState};

use crate::state::AppState;

/// Breach detection
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

/// Metric name → (human label, unit). Rules can use any metric name; unknown ones are shown as-is —
///// better to show the raw key like `host.disk.usage` plus `92.3` than a half-cooked abstraction.
fn metric_label(metric: &str) -> (&str, &str) {
    match metric {
        "host.cpu.usage" => ("CPU Usage", "%"),
        "host.mem.usage" => ("Memory Usage", "%"),
        "host.disk.usage" => ("Disk Usage", "%"),
        "host.disk.used_bytes" => ("Disk Used", " B"),
        "host.mem.used_bytes" => ("Memory Used", " B"),
        "host.net.rx_bytes" => ("Network RX", " B"),
        "host.net.tx_bytes" => ("Network TX", " B"),
        other => (other, ""),
    }
}

/// The "facts" of an alert — the sole input for channel rendering.
///
/// Four alert sources (metrics / node online-offline / service probes / certificate expiry) each
/// carry different data: metrics have current value and threshold, probes have service name, certs
/// have days remaining. So we don't keep each source's domain fields here — only "what rendering
/// needs": who, firing or resolved, how many card columns, the body sentence. Each call site fills
/// in what it knows; the channel side doesn't need to understand four sets of semantics.
pub struct AlertFacts {
    /// Node display name (alias preferred)
    pub node: String,
    /// true = alert firing, false = resolved. Title text and color both depend on this
    pub firing: bool,
    /// Card columns, order is display order (Feishu shows two per row)
    pub fields: Vec<(&'static str, String)>,
    /// The body sentence: explains "what happened, how severe"
    pub detail: String,
}

/// The single source of truth for "alert level → display style".
///
/// The same alert must look the same in four places: the Feishu card title bar, the Slack sidebar
/// color, the severity field in the payload, the emoji in the plain-text title. Previously each
/// place had its own copy (card template, emoji text, webhook just passed `severity` through) —
/// adding a new tier meant editing three match blocks. Now everyone reads from this one table,
/// and the channel side doesn't need to know severity values.
///
/// Colors follow the Bluebird palette naming convention: Feishu wants enum names, Slack wants hex,
/// both are translated from here.
struct LevelStyle {
    /// Stable key: Bluebird generic source's `event`, generic webhook's `level` (also the key for color lookup)
    key: &'static str,
    /// English display label
    label: &'static str,
    emoji: &'static str,
    /// Feishu message card `header.template` enum name
    feishu: &'static str,
    /// Slack `attachment.color`
    slack: &'static str,
}

/// Four levels: critical / warning / info / resolved.
fn level_style(rule: &AlertRule, firing: bool) -> LevelStyle {
    if !firing {
        LevelStyle {
            key: "resolved",
            label: "Resolved",
            emoji: "✅",
            feishu: "green",
            slack: "#2da44e",
        }
    } else if rule.severity == "critical" {
        LevelStyle {
            key: "critical",
            label: "Critical",
            emoji: "🔴",
            feishu: "red",
            slack: "#cf222e",
        }
    } else if rule.severity == "info" {
        LevelStyle {
            key: "info",
            label: "Info",
            emoji: "🔵",
            feishu: "blue",
            slack: "#0969da",
        }
    } else {
        LevelStyle {
            key: "warning",
            label: "Warning",
            emoji: "🟠",
            feishu: "orange",
            slack: "#d93f0b",
        }
    }
}

/// Notification title line — **all channels share the same text**.
///
/// When push shows only the title (phone notification bar, IM collapsed state, Bark), it must
/// still answer "which machine, how severe, what happened" — so node and level are crammed into
/// this one line, and the body can elaborate on details.
fn title_line(rule: &AlertRule, facts: &AlertFacts) -> String {
    let lv = level_style(rule, facts.firing);
    format!("{} {} · {} ({})", lv.emoji, lv.label, rule.name, facts.node)
}

/// JSON form of card columns (used by generic webhook / Bluebird payload).
fn fields_json(facts: &AlertFacts) -> Vec<serde_json::Value> {
    facts
        .fields
        .iter()
        .map(|(label, value)| serde_json::json!({ "label": label, "value": value }))
        .collect()
}

/// Neutralization of dynamic values before they go into `lark_md`.
///
/// Only `<` can open a `lark_md` tag, and node aliases are **user input without character
/// validation** (see `routes::normalize_alias`, which only trims + limits length). If an alias
/// contains `<at id=all></at>` it will genuinely @everyone. We replace it with fullwidth `＜`
/// rather than deleting it: that blocks tag parsing while not breaking legitimate content like
/// "threshold `> 90%`" the way character removal would (`>` is not a `lark_md` marker and is left alone).
fn md_escape(raw: &str) -> String {
    raw.replace('<', "＜").replace(['\n', '\r'], " ")
}

/// Neutralization of dynamic values before Slack `mrkdwn`: only blocks `<`, preserves newlines.
///
/// Slack's `<@U123>` / `<!channel>` will genuinely @ people — same class of issue as `lark_md`;
/// but Slack body text (probe failure reasons, etc.) can be multi-line, so we can't collapse it
/// to one line the way `lark_md` card columns do.
fn slack_escape(raw: &str) -> String {
    raw.replace('<', "＜").replace('\r', "")
}

/// Card footer source line (body and title already convey info; this just labels the source)
const CARD_SOURCE: &str = "zhiwei monitor";

/// Feishu message card (the `content` of a `msg_type: interactive` message).
///
/// Using card 1.0 not 2.0: 2.0 messages sent via the message API fall back to fallback text on
/// old clients ("Please upgrade to the latest client to view content") — hit this in practice.
/// 1.0's `header.template` can still apply color, and "collapsed state should show at a glance
/// whether it's an alert or recovery" is fully satisfied by 1.0.
///
/// Returns only the **bare card object**: when sending via im/v1/messages it gets stringified
/// into `content`; wrapping it in another `{"msg_type":..,"card":..}` (custom bot webhook
/// envelope) will be rejected.
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
    // Body uses plain_text: probe failure reasons are uncontrolled content and must not be parsed as lark_md tags
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

/// Slack Block Kit card: colored sidebar + header + columns + body + source.
///
/// Sidebar color follows the level's hex from [`level_style`] — when only one line of plain
/// text is sent, the collapsed state doesn't show whether it's an alert or recovery. Top-level
/// `text` is still kept: it's the fallback for clients without block support, and the field that
/// legacy receivers have always read — removing it would break existing integrations.
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
        // Slack max 10 fields per section
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
        // Top-level `text` is parsed as mrkdwn by Slack, dynamic values must also be neutralized
        "text": slack_escape(&plain_text(rule, facts)),
        "blocks": blocks,
        "attachments": [{ "color": lv.slack, "text": "" }],
    })
}

/// Run all enabled rules against a batch of telemetry.
pub async fn evaluate(state: &AppState, node_id: &NodeId, hostname: &str, batch: &TelemetryBatch) {
    let all_rules = load_enabled_rules(state).await;
    if all_rules.is_empty() {
        return;
    }
    let now = Timestamp::now().unix_nano();
    let repo = state.storage.alerts();
    for rule in all_rules {
        evaluate_rule(state, &repo, &rule, node_id, hostname, batch, now).await;
    }
}

/// Load user-defined + builtin metric rules (`cpu_high`, `mem_high`, `disk_high`) in one list.
async fn load_enabled_rules(state: &AppState) -> Vec<AlertRule> {
    let rules = match state.storage.alerts().enabled_rules().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Failed to read alert rules");
            return Vec::new();
        }
    };
    let builtin_rules = match state.storage.alerts().enabled_builtin_rules().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Failed to read builtin rules");
            Vec::new()
        }
    };
    rules.into_iter().chain(builtin_rules).collect()
}

/// Extract this rule's metric from the batch and dispatch to breach / recovery handling.
async fn evaluate_rule(
    state: &AppState,
    repo: &AlertsRepo,
    rule: &AlertRule,
    node_id: &NodeId,
    hostname: &str,
    batch: &TelemetryBatch,
    now: i64,
) {
    // Use a unified extraction path: derived metrics (memory percentage, network totals)
    // can also be used by rules. This used to only query `metrics[]` — the seeded rule
    // "Memory usage high" uses host.mem.usage, which nodes never report, so that rule
    // would never fire.
    let Some(value) = crate::routes::extract_metric(batch, &rule.metric) else {
        return;
    };
    let hit = breaching(value, &rule.op, rule.threshold);

    let st = match repo.get_state(rule.id, node_id.as_str()).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Failed to read alert state");
            return;
        }
    };

    if hit {
        handle_breach(state, repo, rule, node_id, hostname, value, now, &st).await;
    } else {
        handle_recovery(repo, rule, node_id, value, now, &st).await;
    }
}

/// A breaching sample. First breach is held until `duration_seconds` elapse before the alert is
/// opened; subsequent breaching samples only refresh the latest value.
#[allow(clippy::too_many_arguments)] // the state machine needs all of rule/state/value; grouping would only shuffle names
async fn handle_breach(
    state: &AppState,
    repo: &zhiwei_storage::alerts_repo::AlertsRepo,
    rule: &AlertRule,
    node_id: &NodeId,
    hostname: &str,
    value: f64,
    now: i64,
    st: &EvalState,
) {
    if st.firing {
        // Still breaching: only update the latest value
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
        return;
    }

    // First breach: record the start; only open the alert after sustained long enough
    let since = st.breaching_since_unix_nano.unwrap_or(now);
    let held_ns = now.saturating_sub(since);
    let need_ns = rule.duration_seconds.saturating_mul(1_000_000_000);
    if held_ns < need_ns {
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
        return;
    }
    fire_metric_alert(state, repo, rule, node_id, hostname, value, now, since).await;
}

/// Open + notify an alert for a rule that has been breaching long enough.
#[allow(clippy::too_many_arguments)] // mirrors handle_breach; a struct would just relocate the fields
async fn fire_metric_alert(
    state: &AppState,
    repo: &zhiwei_storage::alerts_repo::AlertsRepo,
    rule: &AlertRule,
    node_id: &NodeId,
    hostname: &str,
    value: f64,
    now: i64,
    since: i64,
) {
    // The text should answer at a glance "which machine, what metric, what value now" —
    // node name is in the notification title line (see plain_text); here we give metric and value.
    let (label, unit) = metric_label(&rule.metric);
    let sym = op_symbol(&rule.op);
    let threshold = rule.threshold;
    let duration = if rule.duration_seconds > 0 {
        format!(", sustained {}s", rule.duration_seconds)
    } else {
        String::new()
    };
    let message = format!("{label} {sym}{threshold}{unit} (current: {value:.1}{unit}{duration})");
    if !open_metric_alert(repo, rule, node_id, hostname, value, &message, now, since).await {
        return;
    }
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Metric", label.to_string()),
            ("Current", format!("{value:.1}{unit}")),
            ("Threshold", format!("{sym}{threshold}{unit}")),
        ],
        detail: message,
    };
    notify(state, rule, &facts, now).await;
}

/// Open the alert and mark the eval state as firing. Returns whether it opened.
#[allow(clippy::too_many_arguments)]
async fn open_metric_alert(
    repo: &zhiwei_storage::alerts_repo::AlertsRepo,
    rule: &AlertRule,
    node_id: &NodeId,
    hostname: &str,
    value: f64,
    message: &str,
    now: i64,
    since: i64,
) -> bool {
    let alert_id = match repo
        .open_alert(rule, node_id.as_str(), hostname, value, message, now)
        .await
    {
        Ok(alert_id) => alert_id,
        Err(e) => {
            warn!(error = %e, "Failed to open alert");
            return false;
        }
    };
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
    true
}

/// A non-breaching sample: close a firing alert if present, then clear the state.
async fn handle_recovery(
    repo: &zhiwei_storage::alerts_repo::AlertsRepo,
    rule: &AlertRule,
    node_id: &NodeId,
    value: f64,
    now: i64,
    st: &EvalState,
) {
    // Recovered: if currently firing, close it
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
}

/// Deliver to enabled notification channels by severity.
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
        deliver_to_channel(&ch, rule, facts, now).await;
    }
}

/// Validate + deliver to a single channel; incomplete config logs once instead of recurring
/// delivery failures.
async fn deliver_to_channel(
    ch: &zhiwei_storage::alerts_repo::NotifyChannel,
    rule: &AlertRule,
    facts: &AlertFacts,
    now: i64,
) {
    // Channels with incomplete config (old DB rows may lack fields) only log once, don't repeatedly spam delivery failures
    if let Err(msg) = validate_channel(
        &ch.kind,
        &ch.url,
        &ch.secret,
        &ch.app_id,
        &ch.receive_id,
        &ch.receive_id_type,
    ) {
        warn!(channel = %ch.name, kind = %ch.kind, reason = %msg, "Channel config incomplete, skipping");
        return;
    }
    if let Err(e) = deliver(ch, rule, facts, now).await {
        warn!(channel = %ch.name, kind = %ch.kind, error = %e, "Failed to deliver notification");
    }
}

/// Supported notification channel kinds (field design aligns with Bluebird's delivery channels).
///
/// Each kind requires different fields:
/// - `feishu`: uses the official app API — App ID + App Secret exchange for `tenant_access_token`,
///   then sends via `receive_id` (group / user), no bot address needed;
/// - `slack`: Incoming Webhook URL, the URL itself is the credential;
/// - `bluebird`: Bluebird notification gateway's **generic source** URL `…/hooks/<source ID>`
///   plus a Token — hands the alert to it for concurrent delivery to Bark / Feishu / `WeCom`, etc.;
///   `ZhiWei` no longer integrates those directly;
/// - `webhook`: our own receiver, optional Token for auth.
pub const CHANNEL_KINDS: &[&str] = &["feishu", "slack", "bluebird", "webhook"];

/// Whitelist of Feishu `receive_id_type` values (consistent with im/v1/messages on open.feishu.cn).
/// Feishu returns 99992402 if wrong — better to catch it before saving.
pub const FEISHU_RECEIVE_ID_TYPES: &[&str] =
    &["chat_id", "open_id", "user_id", "union_id", "email"];

/// Feishu open platform base URL (can point to Lark international / self-hosted proxy / test stub via env var)
fn feishu_base() -> String {
    match std::env::var("ZHIWEI_FEISHU_BASE") {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => "https://open.feishu.cn".into(),
    }
}

/// Parameter validation before save / test, returns an error message directly displayable to the user.
///
/// Three channel kinds need different fields, and the frontend shows different forms per type —
/// this is the last gate. Old console, hand-written curl, migrated legacy data all pass through here.
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
            "unsupported notify channel type {} (supported: {})",
            kind,
            CHANNEL_KINDS.join(" / ")
        ));
    }
    match kind {
        "feishu" => {
            if app_id.trim().is_empty() {
                return Err("Feishu: App ID is required".into());
            }
            if secret.trim().is_empty() {
                return Err("Feishu: App Secret is required".into());
            }
            if receive_id.trim().is_empty() {
                return Err("Feishu: receive ID is required (chat_id / user open_id)".into());
            }
            if !FEISHU_RECEIVE_ID_TYPES.contains(&receive_id_type) {
                return Err(format!(
                    "unknown Feishu receive ID type {} (supported: {})",
                    receive_id_type,
                    FEISHU_RECEIVE_ID_TYPES.join(" / ")
                ));
            }
            Ok(())
        }
        "slack" => {
            if url.trim().is_empty() {
                return Err("Slack: Webhook URL is required".into());
            }
            Ok(())
        }
        // Bluebird source is fail-closed: no token always 401,
        // so treat token as required to avoid exposing error only at delivery time
        "bluebird" => {
            if url.trim().is_empty() {
                return Err("Bluebird: source URL is required (.../hooks/<source_id>)".into());
            }
            if secret.trim().is_empty() {
                return Err("Bluebird: access token is required".into());
            }
            Ok(())
        }
        _ => {
            if url.trim().is_empty() {
                return Err("generic webhook: URL is required".into());
            }
            Ok(())
        }
    }
}

/// Temporary channel for the "Test" button: only fills in what the console currently has, not saved to DB.
/// `receive_id_type` defaults to `chat_id`, matching Feishu's own default.
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
        name: "Test".into(),
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

/// Deliver one notification to the specified channel. Real alerts and the console's "Test" button
/// share this path — otherwise no one would notice "test passed but real events don't send".
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
        // Slack's Incoming Webhook URL is the credential itself, no auth header needed
        "slack" => post_json(&ch.url, "", &body).await.map(|_| ()),
        // Bluebird generic source: URL + Token (Authorization: Bearer), same path as generic webhook
        "bluebird" => post_json(&ch.url, &ch.secret, &body).await.map(|_| ()),
        // Generic webhook: if `secret` is non-empty, include Authorization: Bearer; receiver uses it for auth
        _ => post_json(&ch.url, &ch.secret, &body).await.map(|_| ()),
    }
}

/// Assemble the notification body. Feishu gets a message card (which is already the bare card
/// object that `content` expects); the rest are wrapped per their respective protocols.
pub fn channel_body(kind: &str, rule: &AlertRule, facts: &AlertFacts, now: i64) -> String {
    match kind {
        // Feishu uses message cards: colored title bar + columns. When only the title bar shows in
        // collapsed state, the color itself signals "critical / warning / resolved" — which plain text can't.
        "feishu" => feishu_card(rule, facts).to_string(),
        // Slack also gets a colored card (sidebar color), top-level text is kept as fallback
        "slack" => slack_payload(rule, facts).to_string(),
        // Bluebird generic source: accepts `{"title","body","event","repo"}` or plain text.
        // - `event` is Bluebird's palette key; ZhiWei sends the level key directly (critical/warning/resolved),
        //   Bluebird knows these three keys and colors by level; if it doesn't know them it just falls
        //   back to neutral color, which doesn't affect delivery;
        // - `fields` / `severity` are currently ignored by Bluebird, kept for receivers that support columns;
        //   they're not part of the protocol contract.
        // Worth noting: Bluebird generic source event whitelist only takes effect when explicitly configured,
        // and the console doesn't expose whitelist config for generic sources, so custom events sent here
        // won't be silently dropped.
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
        // webhook (including unknown kind, treated as webhook): structured JSON
        //
        // `title` + `body` is the convention for Bluebird / generic webhook receivers: after receiving
        // the payload they use `title` as notification title and `body` as content (receivers missing
        // these fields will drop the message). But existing self-hosted receivers expect `text`, so
        // we send both — no one has to change.
        _ => {
            let lv = level_style(rule, facts.firing);
            serde_json::json!({
                "title": title_line(rule, facts),
                "text": plain_text(rule, facts),
                "body": plain_text(rule, facts),
                // Level style: receiver uses `level` to look up its own palette, or directly uses `color` (hex)
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

/// Feishu im/v1/messages send URL.
pub fn feishu_messages_url(base: &str, receive_id_type: &str) -> String {
    format!("{base}/open-apis/im/v1/messages?receive_id_type={receive_id_type}")
}

/// Feishu im/v1/messages request body.
///
/// `content` expects "the stringified JSON of the card object"; wrapping it again in `card` will be
/// rejected by Feishu (error code 9499, hit in production), so here we only escape, no envelope added.
pub fn feishu_message_body(receive_id: &str, card: &str) -> String {
    serde_json::json!({
        "receive_id": receive_id,
        "msg_type": "interactive",
        "content": card,
    })
    .to_string()
}

/// Cache key (`app_id`, `app_secret`); value (`tenant_access_token`, `expires_at_unix_secs`).
type FeishuTokenCache = std::collections::HashMap<(String, String), (String, i64)>;

/// `tenant_access_token` cache: 2h validity, keyed by (`app_id`, `app_secret`).
/// During an alert storm, dozens of notifications should only trigger one token exchange
/// (the exchange API also has rate limits).
static FEISHU_TOKENS: std::sync::OnceLock<std::sync::Mutex<FeishuTokenCache>> =
    std::sync::OnceLock::new();

fn feishu_tokens() -> &'static std::sync::Mutex<FeishuTokenCache> {
    FEISHU_TOKENS.get_or_init(Default::default)
}

fn now_unix_secs() -> i64 {
    Timestamp::now().unix_nano() / 1_000_000_000
}

/// Exchange (or reuse) the Feishu app's `tenant_access_token`.
async fn feishu_token(app_id: &str, app_secret: &str) -> anyhow::Result<String> {
    {
        let cache = feishu_tokens()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((token, expire_at)) = cache.get(&(app_id.to_string(), app_secret.to_string())) {
            // Treat as expired 60s early — don't let the first notification hit 401 on the boundary
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
    let v: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|e| anyhow::anyhow!("Feishu response is not JSON: {e}"))?;
    let token = v
        .get("tenant_access_token")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    if token.is_empty() {
        let why = body_error(&raw).unwrap_or_else(|| body_detail(&raw));
        anyhow::bail!("Failed to exchange Feishu tenant_access_token{why}");
    }
    let expire = v
        .get("expire")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(7200);
    feishu_tokens()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            (app_id.to_string(), app_secret.to_string()),
            (token.clone(), now_unix_secs() + expire),
        );
    Ok(token)
}

/// A fake rule for "Test notification" — only used to assemble the notification body, not saved or evaluated.
pub fn test_rule() -> AlertRule {
    AlertRule {
        id: 0,
        name: "Test Notification".into(),
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

/// Fake facts for "Test notification".
///
/// The console's "Test" button must be able to **preview what a real alert looks like** (with
/// card columns), otherwise users can't judge the styling in the group — they only see it when
/// something actually goes wrong. So this deliberately fills in a metric alert.
pub fn test_facts() -> AlertFacts {
    AlertFacts {
        node: "zhiwei-test".into(),
        firing: true,
        fields: vec![
            ("Node", "zhiwei-test".into()),
            ("Metric", "CPU Usage".into()),
            ("Current", "92.3%".into()),
            ("Threshold", "> 90%".into()),
        ],
        detail: "This is a test notification — receiving it means this channel works.".into(),
    }
}

/// IM text notification: at a glance shows "which machine, how severe, what happened".
///
/// The first line is the shared [`title_line`] (level + rule name + node) — phone/desktop push
/// that only shows the title line must still identify which node it is. Recovery uses "Resolved"
/// rather than reusing "Warning", otherwise on phone it looks like another alert fired.
/// Second line is the specific detail for this alert (metric value / probe reason).
fn plain_text(rule: &AlertRule, facts: &AlertFacts) -> String {
    format!("{}\n{}", title_line(rule, facts), facts.detail)
}

/// Service probe state transition → open/close alert (`source = probe`), deliver notifications by severity.
///
/// Notification policy: only push when entering `down` (critical) or recovering from non-ok to `ok`;
/// `degraded` only reflects in the UI to avoid spammy alerts from occasional blips.
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
    // Disabled probes no longer produce any alerts / notifications (the disable close action
    // happens in the PATCH handler; this is a fallback: before the node receives the disable
    // config, don't let the last result reopen the alert)
    if !probe.enabled {
        return;
    }
    let repo = state.storage.alerts();
    let now = Timestamp::now().unix_nano();
    let rule_name = probe.name.clone();

    if transition.new_state == STATE_OK {
        let resolved =
            handle_probe_recovered(state, &repo, probe, transition, hostname, &rule_name, now)
                .await;
        // A probe whose very first result is healthy has no "recovery" to announce, so the normal
        // path stays silent. Announce it once as "healthy" (newly configured probe). `previous_state`
        // is per-probe (not per-node), so this fires exactly once even for multi-node probes.
        if resolved == 0 && transition.previous_state == "unknown" {
            notify_probe_joined(state, &repo, probe, hostname, &rule_name, now).await;
        }
        return;
    }

    if transition.new_state != STATE_DOWN {
        return;
    }
    handle_probe_down(
        state, &repo, probe, transition, node_id, hostname, &rule_name, now,
    )
    .await;
}

/// Probe returned to `ok`: close any open alert, then (if the `service_online` toggle allows)
/// push a "back online" notification.
async fn handle_probe_recovered(
    state: &AppState,
    repo: &AlertsRepo,
    probe: &zhiwei_storage::probes_repo::Probe,
    transition: &zhiwei_storage::probes_repo::StateTransition,
    hostname: &str,
    rule_name: &str,
    now: i64,
) -> u64 {
    let resolved = match repo.resolve_open_probe_alerts(&probe.id, now).await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "Failed to close service probe alert");
            return 0;
        }
    };
    if resolved == 0 {
        return 0;
    }
    info!(probe = %probe.name, "Service probe recovered, alert closed");
    // The "back online" push is controlled by the builtin toggle service_online; closing
    // the alert itself is unaffected — the person came back, leaving the old alert up
    // would be misleading.
    if !builtin_enabled_or_default(repo, "service_online").await {
        return resolved;
    }
    let rule = probe_alert_rule(rule_name, "warning");
    let message = format!(
        "Probe {} recovered (previously {:?})",
        probe.name, transition.previous_state
    );
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: false,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Probe", probe.name.clone()),
        ],
        detail: message,
    };
    notify(state, &rule, &facts, now).await;
    resolved
}

/// A probe's first observed state is healthy: no recovery happened, so announce it once as
/// "healthy" (a newly configured probe). Controlled by the `service_joined` builtin toggle.
async fn notify_probe_joined(
    state: &AppState,
    repo: &AlertsRepo,
    probe: &zhiwei_storage::probes_repo::Probe,
    hostname: &str,
    rule_name: &str,
    now: i64,
) {
    if !builtin_enabled_or_default(repo, "service_joined").await {
        return;
    }
    let rule = probe_alert_rule(rule_name, "info");
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Probe", probe.name.clone()),
            ("Status", "healthy".to_string()),
        ],
        detail: format!("Probe {} is healthy", probe.name),
    };
    notify(state, &rule, &facts, now).await;
    info!(probe = %probe.name, "Probe joined (first healthy) notification sent");
}

/// Probe transitioned to `down`: if the `service_offline` toggle allows, open an alert and notify.
async fn handle_probe_down(
    state: &AppState,
    repo: &AlertsRepo,
    probe: &zhiwei_storage::probes_repo::Probe,
    transition: &zhiwei_storage::probes_repo::StateTransition,
    node_id: &str,
    hostname: &str,
    rule_name: &str,
    now: i64,
) {
    // Service offline: opening alerts and sending notifications are both controlled by the
    // builtin toggle service_offline. Disabled = completely untouched (no requirement that the
    // service comes back up to clear red).
    if !builtin_enabled_or_default(repo, "service_offline").await {
        return;
    }

    let severity = "critical";
    let detail = if transition.last_error.is_empty() {
        format!(
            "{} consecutive check failures",
            transition.consecutive_failures
        )
    } else {
        transition.last_error.clone()
    };
    let message = format!("Probe {} is down: {detail}", probe.name);

    let opened = repo
        .open_probe_alert(
            &probe.id, rule_name,
            // Alert recorded on the node that "reported this result": the probe may be bound to
            // multiple nodes (or any node), recording the reporting node shows at a glance which
            // machine detected it
            node_id, hostname, severity, &message, now,
        )
        .await;
    let alert_id = match opened {
        Ok(alert_id) => alert_id,
        Err(e) => {
            warn!(error = %e, "Failed to open service probe alert");
            return;
        }
    };
    info!(probe = %probe.name, alert_id, "Service probe alert firing");
    let rule = probe_alert_rule(rule_name, severity);
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Probe", probe.name.clone()),
            (
                "Consecutive failures",
                format!("{} times", transition.consecutive_failures),
            ),
        ],
        detail: message,
    };
    notify(state, &rule, &facts, now).await;
}

/// Read a builtin toggle; on error log and fall back to enabled (fail-open), matching the
/// startup default.
async fn builtin_enabled_or_default(repo: &AlertsRepo, id: &str) -> bool {
    match repo.builtin_rule_enabled(id).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, %id, "Failed to check builtin toggle, enabling by default");
            true
        }
    }
}

/// Probe alerts have no metric rule row; this creates a carrier for just "name + severity",
/// so the existing `notify()` severity filter logic can be reused as-is.
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

/// Carrier for node offline alerts — like `probe_alert_rule`, purely for feeding `notify()`'s
/// "severity filter + naming". `metric = host.online` is to clearly distinguish from probe/cert
/// alert metric columns (prefix `host.`), won't conflict with real metrics.
fn node_offline_alert_rule(severity: &str) -> AlertRule {
    AlertRule {
        id: 0,
        name: "Node offline".to_string(),
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

/// Carrier for node online events — pairs with `node_offline_alert_rule`, different text/severity.
/// Same namespace `host.online`, sharing one notification template with offline.
fn node_online_alert_rule() -> AlertRule {
    AlertRule {
        id: 0,
        name: "Node online".to_string(),
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

/// Carrier for "node joined" events — distinct from `node_online_alert_rule` (recovery).
fn node_joined_alert_rule() -> AlertRule {
    AlertRule {
        id: 0,
        name: "Node joined".to_string(),
        metric: "host.online".to_string(),
        op: "eq".to_string(),
        threshold: 1.0,
        duration_seconds: 0,
        severity: "info".to_string(),
        enabled: true,
        created_at_unix_nano: 0,
        updated_at_unix_nano: 0,
    }
}

/// Node online/offline events — feed in a batch of node state changes at once.
///
/// Design: a background task scans all nodes every ~30s (see `spawn_node_liveness_watcher`):
/// `last_seen` online / still online now = skip; `last_seen` online / offline now = open alert + notify;
/// `last_seen` offline / online now = close old offline alert + send "Node X is online" notification;
/// others (never reported / new observation window) = skip.
///
/// One node is allowed at most one unresolved offline alert at a time (checked inside
/// `open_node_offline_alert` via `open_node_offline_alert_id`), so repeated triggers won't pile up history.
///
/// Both notifications are controlled by the enabled flag in `builtin_alert_rules`:
/// `node_offline` / `node_online`. Disabling one skips the corresponding alert-open / notification-send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Online,
    Offline,
}

/// Evaluate the latest liveness of a batch of nodes, write transitions to the alerts table and deliver notifications.
///
/// Callers only need to "compute online / offline from `last_seen`" — everything else
/// (deduplication, closing old alerts, severity, notification text, builtin enable toggles) is contained here.
pub async fn on_node_liveness_change(state: &AppState, transitions: &[(String, String, Liveness)]) {
    for (node_id, hostname, status) in transitions {
        let now = Timestamp::now().unix_nano();
        let repo = state.storage.alerts();
        match status {
            Liveness::Offline => handle_node_offline(state, &repo, node_id, hostname, now).await,
            Liveness::Online => handle_node_online(state, &repo, node_id, hostname, now).await,
        }
    }
}

/// A genuinely-new node (enrolled within the warmup window) appeared. Unlike `node_online`
/// (which fires on the Offline→Online recovery transition), this fires exactly once, when the
/// node first becomes visible to the watcher. Controlled by the `node_joined` builtin toggle.
///
/// `compute_transitions` only lists a node here when its `enrolled_at` is recent — a monitor
/// restart must not replay "Node X joined" for every node already in the fleet.
pub async fn on_node_joined(state: &AppState, joined: &[(String, String)]) {
    let repo = state.storage.alerts();
    for (node_id, hostname) in joined {
        let now = Timestamp::now().unix_nano();
        notify_node_joined(state, &repo, node_id, hostname, now).await;
    }
}

async fn notify_node_joined(
    state: &AppState,
    repo: &AlertsRepo,
    node_id: &str,
    hostname: &str,
    now: i64,
) {
    if !builtin_enabled_or_default(repo, "node_joined").await {
        return;
    }
    let rule = node_joined_alert_rule();
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Status", "joined".to_string()),
        ],
        detail: format!("Node {hostname} joined the cluster"),
    };
    notify(state, &rule, &facts, now).await;
    info!(%node_id, "Node joined notification sent");
}

/// Node went offline: open (or refresh) the single unresolved offline alert and notify once.
async fn handle_node_offline(
    state: &AppState,
    repo: &AlertsRepo,
    node_id: &str,
    hostname: &str,
    now: i64,
) {
    // If the builtin rule is off, do nothing — neither open alert nor send notification
    if !builtin_enabled_or_default(repo, "node_offline").await {
        return;
    }
    // Notify only when this call *first* opened the alert (see `ensure_node_offline_alert`)
    if let Some(alert_id) = ensure_node_offline_alert(repo, node_id, hostname, now).await {
        notify_node_offline(state, node_id, hostname, alert_id, now).await;
    }
}

/// Open (or refresh) the node's offline alert. Returns `Some(alert_id)` only when it was newly
/// opened — an already-open alert is merely refreshed to avoid repeated pushes in the 30s cycle.
async fn ensure_node_offline_alert(
    repo: &AlertsRepo,
    node_id: &str,
    hostname: &str,
    now: i64,
) -> Option<i64> {
    let existing = match repo.open_node_offline_alert_id(node_id).await {
        Ok(id) => id,
        Err(e) => {
            warn!(%node_id, error = %e, "Failed to query node offline alert");
            return None;
        }
    };
    let message = format!("Node {hostname} is offline");
    let opened = repo
        .open_node_offline_alert(node_id, hostname, "critical", &message, now)
        .await;
    let alert_id = match opened {
        Ok(id) => id,
        Err(e) => {
            warn!(%node_id, error = %e, "Failed to open node offline alert");
            return None;
        }
    };
    existing.is_none().then_some(alert_id)
}

async fn notify_node_offline(
    state: &AppState,
    node_id: &str,
    hostname: &str,
    alert_id: i64,
    now: i64,
) {
    let rule = node_offline_alert_rule("critical");
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Status", "offline".to_string()),
        ],
        detail: format!("Node {hostname} is offline"),
    };
    notify(state, &rule, &facts, now).await;
    info!(alert_id, %node_id, "Node offline alert firing");
}

/// Node came back online: close any lingering offline alert and (per the `node_online` toggle) notify.
async fn handle_node_online(
    state: &AppState,
    repo: &AlertsRepo,
    node_id: &str,
    hostname: &str,
    now: i64,
) {
    let resolved = close_node_offline_alerts(repo, node_id, now).await;
    notify_node_online(state, repo, node_id, hostname, now).await;
    if resolved > 0 {
        info!(count = resolved, %node_id, "Node offline alert closed");
    }
}

/// Close the node's unresolved offline alerts. This is independent of the builtin toggle: the
/// person came back online, leaving the old alert up would be misleading to ops.
async fn close_node_offline_alerts(repo: &AlertsRepo, node_id: &str, now: i64) -> u64 {
    match repo.resolve_node_offline_alerts(node_id, now).await {
        Ok(n) => n,
        Err(e) => {
            warn!(%node_id, error = %e, "Failed to close node offline alert");
            0
        }
    }
}

/// "Online" is a separate event: as long as the transition goes from Offline to Online we send a
/// notification, not dependent on the alert close count — sending is a separate path.
async fn notify_node_online(
    state: &AppState,
    repo: &AlertsRepo,
    node_id: &str,
    hostname: &str,
    now: i64,
) {
    if !builtin_enabled_or_default(repo, "node_online").await {
        return;
    }
    let rule = node_online_alert_rule();
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Status", "online".to_string()),
        ],
        detail: format!("Node {hostname} is online"),
    };
    notify(state, &rule, &facts, now).await;
    info!(%node_id, "Node online notification sent");
}

/// Certificate expiry evaluation: runs once each time a node sends back a snapshot.
///
/// Decision criteria:
///   * Only look at sources with **notification enabled** (`enabled && notify_enabled`);
///   * Days remaining ≤ `notify_days_before` triggers alert: expired = critical, nearing = warning;
///   * One alert per certificate, `source_ref = {source_id}:{cert_path}`;
///   * Renewal (days remaining goes back above threshold) / path changed / source deleted → auto resolved.
///
/// Round a day count for display (clamped at 0). The `f64` → `i64` cast is safe:
/// certificate expiry windows are at most a few decades, far below 2^53.
#[allow(clippy::cast_possible_truncation)]
fn days_for_display(days: f64) -> i64 {
    days.floor().max(0.0) as i64
}

/// Following snapshots rather than a separate scheduled task: certificates only appear in snapshots,
/// judging while the data is freshest is simplest and avoids the mismatch of "snapshot changed but
/// alert stuck on old value".
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
            evaluate_cert(
                state,
                &repo,
                source,
                cert,
                node_id,
                hostname,
                now,
                &open,
                &mut open_by_ref,
            )
            .await;
        }
    }

    // Everything left is "no longer valid": certificate renewed, file deleted, source disabled/deleted
    for (source_ref, id) in open_by_ref {
        resolve_cert_alert(&repo, id, &source_ref, now).await;
    }
}

/// Evaluate a single `(source, cert)` pair and open / update / resolve as needed.
#[allow(clippy::too_many_arguments)] // per-cert evaluation legitimately spans source, node, state and repo
async fn evaluate_cert(
    state: &AppState,
    repo: &AlertsRepo,
    source: &zhiwei_storage::cert_sources_repo::CertSource,
    cert: &serde_json::Value,
    node_id: &str,
    hostname: &str,
    now: i64,
    open: &[zhiwei_storage::alerts_repo::Alert],
    open_by_ref: &mut std::collections::HashMap<String, i64>,
) {
    if !cert_belongs_to(source, cert) {
        return;
    }
    if cert
        .get("parse_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    let Some(not_after) = cert
        .get("not_after_unix_nano")
        .and_then(serde_json::Value::as_i64)
    else {
        return;
    };
    let path = cert.get("path").and_then(|v| v.as_str()).unwrap_or("");
    // Truncate to integer days first (cert alerting only cares about day granularity),
    // then convert via f32 -- stays within f32 mantissa precision for any reasonable window.
    let days_i64 = (not_after - now) / 86_400_000_000_000;
    let days_left = f64::from(i32::try_from(days_i64).unwrap_or(i32::MAX));
    let notify_days = f64::from(i32::try_from(source.notify_days_before).unwrap_or(0));
    if days_left > notify_days {
        return;
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
    let message = cert_message(&name, path, source.notify_days_before, days_left, expired);
    let source_ref = format!("{}:{}", source.id, path);

    // Global toggle: cert alerts are split by severity into two tiers (cert_expiring / cert_expired),
    // forming an AND with each source's own `notify_enabled`. Disabling a tier both stops opening
    // new alerts and closes already-open ones — being above the local threshold means we wouldn't
    // enter this loop in the first place, so getting here means "should it report" is already true.
    let gate = if expired {
        "cert_expired"
    } else {
        "cert_expiring"
    };
    if !builtin_enabled_or_default(repo, gate).await {
        if let Some(id) = open_by_ref.remove(&source_ref) {
            if let Err(e) = repo.resolve_alert(id, now).await {
                warn!(error = %e, %source_ref, "Failed to close cert alert");
            }
        }
        return;
    }

    let Some(id) = open_by_ref.remove(&source_ref) else {
        open_cert_alert(
            state,
            repo,
            &name,
            &source_ref,
            severity,
            notify_days,
            days_left,
            &message,
            node_id,
            hostname,
            source.notify_days_before,
            expired,
            now,
        )
        .await;
        return;
    };
    // Already opened: only update on severity/text change, avoid writing DB on every snapshot
    if let Some(existing) = open.iter().find(|a| a.id == id) {
        if existing.severity != severity || existing.message != message {
            let _ = repo
                .update_alert_message(id, severity, days_left, &message)
                .await;
        }
    }
}

fn cert_message(
    name: &str,
    path: &str,
    notify_days_before: i64,
    days_left: f64,
    expired: bool,
) -> String {
    if expired {
        format!(
            "Certificate {name} ({path}) expired {} days ago",
            days_for_display(-days_left)
        )
    } else {
        format!(
            "Certificate {name} ({path}) expires in {} days (threshold {notify_days_before} days)",
            days_for_display(days_left)
        )
    }
}

/// Open a fresh cert alert and push the notification.
#[allow(clippy::too_many_arguments)]
async fn open_cert_alert(
    state: &AppState,
    repo: &AlertsRepo,
    name: &str,
    source_ref: &str,
    severity: &str,
    notify_days: f64,
    days_left: f64,
    message: &str,
    node_id: &str,
    hostname: &str,
    notify_days_before: i64,
    expired: bool,
    now: i64,
) {
    let rule_name = format!("Certificate expiry · {name}");
    let opened = repo
        .open_cert_alert(
            source_ref,
            &rule_name,
            node_id,
            hostname,
            severity,
            notify_days,
            days_left,
            message,
            now,
        )
        .await;
    let id = match opened {
        Ok(id) => id,
        Err(e) => {
            warn!(error = %e, "Failed to open cert alert");
            return;
        }
    };
    info!(cert = %name, %severity, id, "Certificate expiry alert firing");
    let rule = cert_alert_rule(&rule_name, severity);
    let days_field = if expired {
        format!("Expired {} days ago", days_for_display(-days_left))
    } else {
        format!("{} days", days_for_display(days_left))
    };
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Certificate", name.to_string()),
            ("Remaining", days_field),
            ("Notify threshold", format!("{notify_days_before} days")),
        ],
        detail: message.to_string(),
    };
    notify(state, &rule, &facts, now).await;
}

async fn resolve_cert_alert(repo: &AlertsRepo, id: i64, source_ref: &str, now: i64) {
    if let Err(e) = repo.resolve_alert(id, now).await {
        warn!(error = %e, %source_ref, "Failed to close cert alert");
    } else {
        info!(%source_ref, "Cert alert resolved");
    }
}

/// Whether a certificate source "owns" this certificate (consistent with `certs_api`'s judgment)
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

/// Carrier for container start/stop events — purely for feeding `notify()`'s severity filter + naming,
/// same pattern as `probe_alert_rule` / `node_offline_alert_rule`.
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

/// One container event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerEvent {
    Started { id: String, name: String },
    Stopped { id: String, name: String },
}

fn container_running(state: &str) -> bool {
    state.eq_ignore_ascii_case("running")
}

/// Snapshot diff (pure function, easy to unit-test): two container snapshot JSON → event list.
///
/// Main criterion is **state (running / non-running)**; timestamps only supplement "restart within cycle":
/// old node-agent versions don't report `started_at` / `finished_at` (proto3 default is 0), using timestamps
/// as the main criterion would cause all existing containers to be misreported as "started / stopped" after agent upgrade.
///
/// - Appearing in snapshot and running → `Started` (new container, exited→running, restart within cycle);
///   "restart within cycle" requires the previous `started_at` to be known and updated (> 0),
///   to avoid treating "just learned to read start time" as "just started";
/// - running→non-running, or disappearing from snapshot (rm / cleanup) → `Stopped`;
/// - Others (non-running→non-running, new but already exited) don't emit events — one-shot containers
///   (exit immediately, `--rm`) shouldn't repeat the same state every snapshot.
///
/// Return order: stopped first, started last, stable and testable.
fn diff_containers(previous_json: &str, current_json: &str) -> Vec<ContainerEvent> {
    let prev = parse_container_map(previous_json);
    let cur = parse_container_map(current_json);
    if prev.is_empty() && cur.is_empty() {
        return Vec::new();
    }
    let mut stopped = Vec::new();
    let mut started = Vec::new();

    // 1) Disappearing from snapshot = stopped (cleanup / rm)
    for (id, (name, _started, _running)) in &prev {
        if !cur.contains_key(id) {
            stopped.push(ContainerEvent::Stopped {
                id: id.clone(),
                name: name.clone(),
            });
        }
    }

    // 2) Containers still in snapshot: judge start/stop by state
    for (id, (name, cur_started, cur_running)) in &cur {
        match prev.get(id) {
            // New container: only count as "started" if actually running (one-shot containers that exit immediately don't report)
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
                    // Restart within cycle: state stayed running, only start time changed
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

/// Container IDs that a *same-named running* container has replaced this cycle.
///
/// `docker compose up -d` (every deploy) recreates containers with fresh IDs: the old ID stops
/// and a new ID starts in the same snapshot. The new ID's `Started` event only resolves its own
/// row, so the old ID's "stopped" alert would stay open until manually closed. These are the old
/// IDs to close in that case — matched by name against a currently-running container.
///
/// The replaced container must have actually left the snapshot (`prev - cur`): a still-present
/// exited container with the same name (e.g. `docker rename`) keeps its alert.
fn replaced_container_ids(previous_json: &str, current_json: &str) -> Vec<String> {
    let prev = parse_container_map(previous_json);
    let cur = parse_container_map(current_json);
    let running_names: std::collections::HashSet<&str> = cur
        .values()
        .filter(|(_, _, running)| *running)
        .map(|(name, _, _)| name.as_str())
        .collect();
    prev.iter()
        .filter(|(id, (name, _, _))| {
            !cur.contains_key(*id) && running_names.contains(name.as_str())
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// Container start/stop event detection: compare the previous and current container snapshots for the
/// same node, convert the diff into events.
///
/// Diff is computed directly on the latest snapshot from `node_inventory` (same time every 5 minutes);
/// baseline is stored in the database, so node / monitor restarts don't lose it. When there's no
/// historical snapshot (`previous_json = None`), only establish the baseline, don't emit events —
/// avoids flashing a screen of "started" for existing containers after upgrade / fresh install.
/// Diff criteria see [`diff_containers`].
///
/// Both notifications are controlled by `builtin_alert_rules`: `container_started` /
/// `container_stopped`. "Stop" opens a `source = container` alert (visible in todo, can be silenced),
/// "start" only closes the alert + sends notification.
pub async fn on_container_events(
    state: &AppState,
    node_id: &str,
    hostname: &str,
    previous_json: Option<&str>,
    current_json: &str,
) {
    let Some(previous) = previous_json else {
        // First snapshot for this node: no baseline to diff against. Announce the containers we
        // just discovered (not their "started" transition — that would re-fire on the next cycle);
        // suppressed for nodes already in the fleet, so a monitor upgrade doesn't flood.
        notify_newly_discovered_containers(state, node_id, hostname, current_json).await;
        return;
    };
    let events = diff_containers(previous, current_json);
    // A recreated container (`docker compose up -d`) keeps its name but gets a new ID, so the new
    // ID's Started event never resolves the old ID's "stopped" alert. Collect those old IDs here.
    let replaced = replaced_container_ids(previous, current_json);
    if events.is_empty() && replaced.is_empty() {
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
                handle_container_stopped(state, &repo, &id, &name, &short, node_id, hostname, now)
                    .await;
            }
            ContainerEvent::Started { .. } => {
                handle_container_started(state, &repo, &id, &name, &short, node_id, hostname, now)
                    .await;
            }
        }
    }

    for old_id in &replaced {
        if let Err(e) = repo.resolve_open_container_alerts(old_id, now).await {
            warn!(container_id = %old_id, error = %e, "Failed to close replaced container alert");
        }
    }
}

/// Container stopped: open a `source = container` alert and notify.
#[allow(clippy::too_many_arguments)] // event payload + node context; wrapping adds no clarity
async fn handle_container_stopped(
    state: &AppState,
    repo: &AlertsRepo,
    id: &str,
    name: &str,
    short: &str,
    node_id: &str,
    hostname: &str,
    now: i64,
) {
    if !builtin_enabled_or_default(repo, "container_stopped").await {
        return;
    }
    let message = format!("Container {name} ({short}) stopped");
    let opened = repo
        .open_container_alert(
            id,
            "Container stopped",
            node_id,
            hostname,
            "warning",
            &message,
            now,
        )
        .await;
    let alert_id = match opened {
        Ok(alert_id) => alert_id,
        Err(e) => {
            warn!(error = %e, "Failed to open container stopped alert");
            return;
        }
    };
    info!(container = %name, alert_id, %node_id, "Container stopped alert firing");
    let rule = container_event_rule("Container stopped", "warning");
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Container", name.to_string()),
            ("ID", short.to_string()),
        ],
        detail: message,
    };
    notify(state, &rule, &facts, now).await;
}

/// Container started: close the unresolved stopped alert, then (per toggle) notify.
#[allow(clippy::too_many_arguments)] // event payload + node context; wrapping adds no clarity
async fn handle_container_started(
    state: &AppState,
    repo: &AlertsRepo,
    id: &str,
    name: &str,
    short: &str,
    node_id: &str,
    hostname: &str,
    now: i64,
) {
    // Close this container's unresolved stopped alert - even if started toggle is off:
    // if container is up, the old "stopped" alert would be misleading
    if let Err(e) = repo.resolve_open_container_alerts(id, now).await {
        warn!(error = %e, "Failed to close container stopped alert");
    }
    if !builtin_enabled_or_default(repo, "container_started").await {
        return;
    }
    let message = format!("Container {name} ({short}) started");
    let rule = container_event_rule("Container started", "info");
    let facts = AlertFacts {
        node: hostname.to_string(),
        firing: true,
        fields: vec![
            ("Node", hostname.to_string()),
            ("Container", name.to_string()),
            ("ID", short.to_string()),
        ],
        detail: message,
    };
    notify(state, &rule, &facts, now).await;
    info!(container = %name, %node_id, "Container started notification sent");
}

/// Announce containers discovered on a node's very first snapshot. Unlike `handle_container_started`
/// (which fires on a running-transition), each running container is announced once, when first seen.
/// The whole path is suppressed for nodes already in the fleet (see [`should_announce_joins`]) so a
/// monitor upgrade / fresh install doesn't replay every container. Controlled by `container_joined`.
async fn notify_newly_discovered_containers(
    state: &AppState,
    node_id: &str,
    hostname: &str,
    current_json: &str,
) {
    let repo = state.storage.alerts();
    if !should_announce_joins(state, node_id).await {
        return;
    }
    if !builtin_enabled_or_default(&repo, "container_joined").await {
        return;
    }
    for (id, (name, _started, running)) in parse_container_map(current_json) {
        if !running {
            continue;
        }
        let short = id.chars().take(12).collect::<String>();
        let rule = container_event_rule("Container discovered", "info");
        let facts = AlertFacts {
            node: hostname.to_string(),
            firing: true,
            fields: vec![
                ("Node", hostname.to_string()),
                ("Container", name.clone()),
                ("ID", short.clone()),
            ],
            detail: format!("Container {name} ({short}) discovered"),
        };
        notify(state, &rule, &facts, Timestamp::now().unix_nano()).await;
        info!(container = %name, %node_id, "Container discovered notification sent");
    }
}

/// Suppress the "joined / discovered" family (node / container / service) for nodes that were
/// already in the fleet when this monitor started. `node_joined` is filtered upstream via
/// `enrolled_at`; containers and services have no such origin timestamp, so they use this
/// node-level proxy: a node enrolled within [`NODE_JOIN_WINDOW_MS`] is genuinely new.
async fn should_announce_joins(state: &AppState, node_id: &str) -> bool {
    let Ok(Some(n)) = state
        .storage
        .nodes()
        .find_by_id(&zhiwei_common::NodeId::from_string(node_id.to_string()))
        .await
    else {
        return false;
    };
    let now_ms = Timestamp::now().unix_nano() / 1_000_000;
    let enrolled_age_ms = now_ms - n.enrolled_at_unix_nano / 1_000_000;
    enrolled_age_ms < NODE_JOIN_WINDOW_MS
}

/// Snapshot JSON → { `container_id`: (name, `started_at_unix_nano`, running) }.
/// Field names may not exactly match node-agent versions, treat missing values as 0 / false.
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
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let running = c
            .get("state")
            .and_then(|v| v.as_str())
            .is_some_and(container_running);
        out.insert(id.to_string(), (name, started, running));
    }
    out
}

/// Deliver a JSON request body, return the response body.
///
/// When `token` is non-empty, includes `Authorization: Bearer <token>` — Feishu app API uses it
/// to carry `tenant_access_token`, self-hosted receivers can use it for auth; Slack's Incoming
/// Webhook URL is itself the "URL is the credential", just leave it empty.
pub async fn post_json(url: &str, token: &str, body: &str) -> anyhow::Result<Vec<u8>> {
    let url = url.to_string();
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| anyhow::anyhow!("only http(s) webhooks are supported"))?;
    let tls = url.starts_with("https://");
    let (host_port, path) = authority
        .find('/')
        .map_or((authority, "/"), |i| (&authority[..i], &authority[i..]));

    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(if tls { 443 } else { 80 })),
        None => (host_port, if tls { 443 } else { 80 }),
    };

    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connection timed out"))??;

    // Feishu / Slack URLs are all https; self-hosted receivers are mostly internal plaintext http
    if tls {
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(tls_config()));
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| anyhow::anyhow!("invalid hostname {host}: {e}"))?;
        let tls_stream = tokio::time::timeout(
            Duration::from_secs(5),
            connector.connect(server_name, stream),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TLS handshake timed out"))?
        .map_err(|e| anyhow::anyhow!("TLS handshake failed: {e}"))?;
        send_post(tls_stream, host, path, token, body).await
    } else {
        send_post(stream, host, path, token, body).await
    }
}

/// System root certificates (webhook targets all have proper certs: Feishu / Slack or user's own domain)
fn tls_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// POST a JSON body and read back the response. Shared by http and https (`TcpStream` and `TlsStream` both implement the same set of traits).
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

    let resp = tokio::time::timeout(Duration::from_secs(5), sender.send_request(req))
        .await
        .map_err(|_| anyhow::anyhow!("request timed out"))??;
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("API returned {status}{}", body_detail(&body));
    }
    // Some channels still return 2xx on failure: error code is in body, ignoring it would treat failure as successful delivery
    if let Some(e) = body_error(&body) {
        anyhow::bail!("API returned {status}, {e}");
    }
    Ok(body.to_vec())
}

/// Dig out "why it failed" from the response body.
///
/// Feishu puts the error code in body (10003/10014 credentials wrong, 230001 receive ID doesn't match type,
/// 99991672 permission not enabled), HTTP status is often still 200 — checking only the status treats failure as success.
fn body_error(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    // Feishu code/msg; Slack success body is plain text "ok", can't be parsed as JSON
    let code = v.get("code").or_else(|| v.get("errcode"))?.as_i64()?;
    if code == 0 {
        return None;
    }
    let msg = v
        .get("msg")
        .or_else(|| v.get("errmsg"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    Some(format!("error code {code} ({msg}){}", channel_hint(code)))
}

/// Include the response body on failure (truncated to 200 chars), helps self-hosted receiver troubleshooting
fn body_detail(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!(": {}", text.chars().take(200).collect::<String>())
    }
}

/// Known error codes -> troubleshooting hints. Only covers Feishu since its error codes are opaque.
/// Slack / self-hosted webhooks: if something's wrong, the HTTP status already tells you.
#[allow(clippy::unreadable_literal)] // Feishu error codes are opaque identifiers, not quantities
const fn channel_hint(code: i64) -> &'static str {
    match code {
        10003 => ": App ID / App Secret is incomplete or invalid",
        10014 => ": App ID or App Secret is incorrect",
        230001 => ": receive ID is invalid — check if ID type matches (group uses oc_ prefix chat_id)",
        99992402 => ": receive ID type is invalid, optional chat_id / open_id / user_id / union_id / email",
        99991672 => ": app lacks permission — go to developer console and enable im:message:send_as_bot, then publish",
        11232 => ": Feishu rate limit hit (50 req/s per app)",
        _ => "",
    }
}

/// Insert a few default rules on first start to avoid an empty alerts page.
pub async fn seed_default_rules(state: &AppState) -> anyhow::Result<()> {
    let repo = state.storage.alerts();
    if repo.count_rules().await? > 0 {
        return Ok(());
    }
    let now = Timestamp::now().unix_nano();
    let defaults: [(&str, &str, &str, f64, i64, &str); 4] = [
        (
            "CPU sustained high load",
            "host.cpu.usage",
            "gt",
            90.0,
            300,
            "warning",
        ),
        (
            "CPU sustained maximum",
            "host.cpu.usage",
            "gt",
            98.0,
            60,
            "critical",
        ),
        (
            "Memory usage high",
            "host.mem.usage",
            "gt",
            90.0,
            300,
            "warning",
        ),
        (
            "Disk usage high",
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

/// "How long without reporting" counts as offline — same criterion as the frontend's
/// `livenessOf()` (see ui/src/lib/utils.ts). 60s means "just alive"; beyond that, each alert rule's
/// own duration decides whether to alert.
pub const NODE_OFFLINE_AFTER_MS: i64 = 60_000;

/// Background liveness scan interval.
///
/// 30s is the balance: too short hammers the DB; too long makes the "offline → alert" chain feel
/// sluggish (worst case needs a full cycle to enter todo). Given telemetry defaults to once per 10s,
/// the 30s interval guarantees any node that's lost contact triggers an alert within ≤ 90s
/// (= 60s threshold + 30s scan interval).
const LIVENESS_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// "False alarm suppression" window after monitor restart.
///
/// During restart, no node reports — their `last_seen` looks ≥ 60s threshold. Running the first
/// scan would alert on all of them as "offline", and they'd auto-recover when they reconnect —
/// flooding the alerts page with fake alerts. 120s is empirical: restarts longer than this are
/// usually not normal rolling restarts but the whole cluster being down, and alerting then is fine.
///
/// During this window, don't read `last_seen_state`, don't send any transitions: the first real
/// "first round" happens after warmup, logic stays the same (None→Offline still really alerts).
pub const LIVENESS_WARMUP_MS: i64 = 120_000;

/// A node first seen in this monitor process whose `enrolled_at` is within this window counts as
/// newly joined (announced via `node_joined`). 24h keeps it robust to a monitor restart hours
/// after enrollment; a node that was already in the fleet before the window is never re-announced.
pub const NODE_JOIN_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// One liveness scan: (offline/online transitions, newly-joined nodes). Both carry `(id, display)`.
type Transitions = (Vec<(String, String, Liveness)>, Vec<(String, String)>);

/// Spawn a long-running task that periodically scans all nodes' `last_seen_unix_nano`,
/// writing transitions to the alerts table.
///
/// First scan doesn't notify — during cold start the tens of seconds where every node has "no
/// pull command / no report" isn't a fault; we only act on alerts / notifications when
/// "we once recorded a state, and this time it changed".
///
/// By design doesn't touch telemetry / probes: while a node stays online its "most recent" is always
/// updated, but this task only cares about whether 60s threshold has passed, friendly to cold starts
/// and single blips.
pub fn spawn_node_liveness_watcher(state: AppState) {
    tokio::spawn(async move {
        // Wait 5s after startup before first scan: give telemetry reporting time, avoid false alarms during cold start.
        tokio::time::sleep(Duration::from_secs(5)).await;
        // Use HashMap to record "what state we considered each node in last round", skip unrecorded ones —
        // "never reported" nodes (newly joined / offline a long time) shouldn't be reported offline immediately.
        let mut last_seen_state: std::collections::HashMap<String, Liveness> =
            std::collections::HashMap::new();

        loop {
            // warmup check moved into compute_transitions: the function itself gets
            // monitor_uptime_ms and the whole thing decides whether to send transitions.
            // This way the call site is just a clean "run one round + record state + notify".
            let transitions =
                match compute_transitions(&state, &last_seen_state, state.started_at_ms).await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = %e, "Node liveness check failed");
                        tokio::time::sleep(LIVENESS_POLL_INTERVAL).await;
                        continue;
                    }
                };
            let (changes, joined) = transitions;
            // Record state first then notify, to avoid same transitions arriving again during notify
            for (id, _, liveness) in &changes {
                last_seen_state.insert(id.clone(), *liveness);
            }
            // Joined nodes are already in `last_seen_state` (every listed node carries a liveness
            // transition when it is Offline; when Online it does not — record it here so it isn't
            // reported as "joined" again next round).
            for (id, _) in &joined {
                last_seen_state
                    .entry(id.clone())
                    .or_insert(Liveness::Online);
            }
            if !joined.is_empty() {
                on_node_joined(&state, &joined).await;
            }
            if !changes.is_empty() {
                on_node_liveness_change(&state, &changes).await;
            }
            tokio::time::sleep(LIVENESS_POLL_INTERVAL).await;
        }
    });
}

/// Compute one round of "state transitions": for each node, get the latest `last_seen` and compute
/// Liveness, then compare with the previous round — only report when "both previous and current rounds can confirm it".
///
/// `monitor_started_at_unix_ms` is the Unix milliseconds when the monitor process started, used to
/// suppress cold-start false alarms after restart: see [`LIVENESS_WARMUP_MS`].
/// During warmup **no transitions are sent** — including the "first round pushes all offline nodes
/// once" path; otherwise during restart all nodes' `last_seen` would look ≥ 60s threshold and
/// still flood a screen of fake offline alerts.
async fn compute_transitions(
    state: &AppState,
    last_seen_state: &std::collections::HashMap<String, Liveness>,
    monitor_started_at_unix_ms: i64,
) -> anyhow::Result<Transitions> {
    let nodes = state.storage.nodes().list_all().await?;
    let now_ms = Timestamp::now().unix_nano() / 1_000_000;
    let mut out = Vec::new();
    // Nodes seen for the first time in this monitor *process* whose enrollment is recent —
    // a genuine new node. Reported once; the offline/online transition list is separate.
    let mut joined = Vec::new();
    // Suppress cold-start after monitor restart: all transitions are held first, wait for nodes
    // to reconnect and stabilize. `monitor_started_at_unix_ms` comes from the monitor startup Unix ms;
    // old binaries / tests may pass 0, saturating_sub treats it as "started a long time ago" and
    // runs normal logic, won't get stuck in warmup.
    let monitor_uptime_ms = now_ms.saturating_sub(monitor_started_at_unix_ms);
    if monitor_uptime_ms < LIVENESS_WARMUP_MS {
        tracing::debug!(
            monitor_uptime_ms,
            warmup_ms = LIVENESS_WARMUP_MS,
            "Skipping liveness transition (warmup)"
        );
        return Ok((out, joined));
    }
    for n in nodes {
        // Nodes that haven't reported yet (newly joined / offline a long time) skip this round: give one more cycle window.
        let Some(last_seen) = n.last_seen_unix_nano else {
            continue;
        };
        let age_ms = now_ms - last_seen / 1_000_000;
        let current = if age_ms < NODE_OFFLINE_AFTER_MS {
            Liveness::Online
        } else {
            Liveness::Offline
        };
        let display = if n.alias.is_empty() {
            n.hostname.clone()
        } else {
            n.alias.clone()
        };
        // Only report when "previous round recorded, and different from now", to avoid cold-start noise.
        match last_seen_state.get(&n.id) {
            Some(prev) if *prev != current => {
                out.push((n.id, display, current));
            }
            None => {
                // First time this process sees the node. Report offline nodes (pick them up after a
                // long monitor restart). Don't report "online" — that's `node_online`'s recovery
                // transition, and a restart would otherwise replay it for the whole fleet.
                //
                // Separately: if the node enrolled recently (within the join window), it's a genuine
                // arrival — tell the user. A node already in the fleet simply isn't mentioned.
                let enrolled_age_ms = now_ms - n.enrolled_at_unix_nano / 1_000_000;
                if enrolled_age_ms < NODE_JOIN_WINDOW_MS {
                    joined.push((n.id.clone(), display.clone()));
                }
                if current == Liveness::Offline {
                    out.push((n.id, display, current));
                }
            }
            _ => {}
        }
    }
    Ok((out, joined))
}

#[cfg(test)]
mod tests {
    // Test fixtures use raw Unix timestamps (1700000000 = 2023-11-14) and
    // container IDs; underscores would obscure the intended value.
    #![allow(clippy::unreadable_literal)]

    use super::*;

    fn rule(severity: &str) -> AlertRule {
        AlertRule {
            id: 1,
            name: "Disk usage high".into(),
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

    /// Test helper: create alert facts matching the test rule
    fn facts(firing: bool) -> AlertFacts {
        AlertFacts {
            node: "shark-9".into(),
            firing,
            fields: vec![
                ("Node", "shark-9".into()),
                ("Metric", "Disk usage".into()),
                ("Value", "91.0%".into()),
                ("Threshold", "> 85%".into()),
            ],
            detail: "Disk usage >85% (current 91.0%)".into(),
        }
    }

    #[test]
    fn webhook_channel_keeps_structured_body() {
        let body = channel_body("webhook", &rule("warning"), &facts(true), 42);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["hostname"], "shark-9");
        assert_eq!(v["at_unix_nano"], 42);
        assert_eq!(v["title"], "🟠 Warning · Disk usage high (shark-9)");
        assert!(v["text"].as_str().unwrap().contains("(shark-9)"));
        assert!(v["text"].as_str().unwrap().contains("Disk usage"));
        // `body` is the contract field for receivers like Bluebird, `text` is what self-hosted receivers expect
        assert_eq!(v["body"], v["text"]);
        // fields and color included, receiver doesn't need to parse title
        assert_eq!(v["fields"][0]["label"], "Node");
        assert_eq!(v["fields"][0]["value"], "shark-9");
        assert_eq!(v["level"], "warning");
        assert_eq!(v["color"], "#d93f0b");
    }

    /// Recovery notification should not look like "another alert": copy changes to "Recovered", level and color follow
    #[test]
    fn recovery_notifications_say_recovered() {
        let body = channel_body("webhook", &rule("warning"), &facts(false), 0);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["title"], "✅ Resolved · Disk usage high (shark-9)");
        assert_eq!(v["level"], "resolved");
        assert_eq!(v["color"], "#2da44e");
    }

    /// Bluebird generic source contract: `title` + `body`, level via `event`
    /// (Bluebird uses it to look up its palette). Token via Authorization: Bearer, same path as generic webhook.
    #[test]
    fn bluebird_channel_follows_generic_source_contract() {
        let v: serde_json::Value = serde_json::from_str(&channel_body(
            "bluebird",
            &rule("critical"),
            &facts(true),
            8,
        ))
        .unwrap();
        assert_eq!(v["title"], "🔴 Critical · Disk usage high (shark-9)");
        assert_eq!(v["body"], "Disk usage >85% (current 91.0%)");
        assert_eq!(v["event"], "critical");
        assert_eq!(v["fields"][2]["label"], "Value");
        assert_eq!(v["at_unix_nano"], 8);

        // Body shouldn't repeat the title line: Bluebird uses title as card header, body as content
        let body = v["body"].as_str().unwrap();
        assert!(!body.contains("Critical"), "{body}");
        assert!(!body.contains("Disk usage high"), "{body}");
    }

    /// Feishu uses colored cards, title bar colored by "Critical / Warning / Recovered" three levels
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
            "🔴 Critical · Disk usage high (shark-9)"
        );

        assert_eq!(card("warning", true)["header"]["template"], "orange");

        let recovered = card("critical", false);
        assert_eq!(recovered["header"]["template"], "green");
        assert_eq!(
            recovered["header"]["title"]["content"],
            "✅ Resolved · Disk usage high (shark-9)"
        );
    }

    /// When sending via im/v1/messages, card must be wrapped in `content` string, and must NOT wrap in another `card`
    /// (custom bot wrapper — Feishu returns 9499 if present)
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
        // content is the stringified card object (Feishu requires this double-layer escaping)
        let content: serde_json::Value =
            serde_json::from_str(body["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["header"]["template"], "orange");
        assert!(
            content.get("card").is_none(),
            "should not wrap in another `card`"
        );

        assert_eq!(
            feishu_messages_url("https://open.feishu.cn", "chat_id"),
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id"
        );
    }

    /// Card spreads facts into columns, rather than cramming back into one line of text
    #[test]
    fn feishu_card_lays_facts_into_fields() {
        let v: serde_json::Value =
            serde_json::from_str(&channel_body("feishu", &rule("critical"), &facts(true), 0))
                .unwrap();
        let elements = v["elements"].as_array().unwrap();
        let fields = elements[0]["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0]["text"]["content"], "**Node**\nshark-9");
        assert_eq!(fields[2]["text"]["content"], "**Value**\n91.0%");
        // Threshold's `>` is normal content, must not be escaped (once became " 85%")
        assert_eq!(fields[3]["text"]["content"], "**Threshold**\n> 85%");
        // Body uses plain_text: probe failure reasons are uncontrolled, must not be parsed as lark_md
        assert_eq!(elements[2]["text"]["tag"], "plain_text");
    }

    /// Alias has no character validation (`routes::normalize_alias` only trims + limits length),
    /// injecting into `lark_md` would @mention everyone — `<` that can open tags must be neutralized
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
            "alias tag opening not neutralized: {content}"
        );
        // Original text must still be recognizable (neutralization, not deletion)
        assert!(content.contains("＜at id=all"));
    }

    /// Slack: upgraded from "one line of plain text" to a Block Kit card colored by level.
    /// Top-level `text` must be kept — clients without block support and legacy plain-text receivers both read it.
    #[test]
    fn slack_channel_sends_colored_card() {
        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &rule("warning"), &facts(true), 0))
                .unwrap();
        assert!(slack["text"].as_str().unwrap().starts_with("🟠 Warning ·"));
        assert!(slack["text"].as_str().unwrap().contains("(shark-9)"));
        // Sidebar color by level
        assert_eq!(slack["attachments"][0]["color"], "#d93f0b");

        let blocks = slack["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(
            blocks[0]["text"]["text"],
            "🟠 Warning · Disk usage high (shark-9)"
        );
        let fields = blocks[1]["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0]["text"], "*Node*\nshark-9");
        // body and source
        assert_eq!(blocks[2]["text"]["text"], "Disk usage >85% (current 91.0%)");
        assert_eq!(blocks[3]["elements"][0]["text"], "zhiwei monitor");
    }

    /// Slack's mrkdwn parses `<@U123>` / `<!channel>` as real @ mentions,
    /// but node aliases are user input without character validation — must neutralize.
    #[test]
    fn slack_card_neutralizes_markup_in_alias() {
        let mut f = facts(true);
        f.node = "<!channel>".into();
        f.fields[0].1 = "<!channel>".into();
        let slack: serde_json::Value =
            serde_json::from_str(&channel_body("slack", &rule("critical"), &f, 0)).unwrap();

        // Both fallback text (mrkdwn) and columns (mrkdwn) need neutralization
        let fallback = slack["text"].as_str().unwrap();
        assert!(!fallback.contains("<!channel>"), "{fallback}");
        let field = slack["blocks"][1]["fields"][0]["text"].as_str().unwrap();
        assert!(!field.contains("<!channel>"), "{field}");
        assert!(field.contains("＜!channel>"), "{field}");

        // header is plain_text, Slack doesn't parse it as tags
        assert_eq!(slack["blocks"][0]["text"]["type"], "plain_text");
    }

    /// Each channel type has different required fields — must validate before save
    #[test]
    fn channel_params_are_validated_per_kind() {
        // Feishu: App ID / App Secret / receive ID are all required, type must be in whitelist
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
            .contains("receive ID"));
        assert!(
            validate_channel("feishu", "", "s", "cli_x", "oc_1", "chat-id")
                .unwrap_err()
                .contains("receive ID type")
        );
        // Feishu doesn't need URL (address determined by receive_id)
        assert!(validate_channel("feishu", "", "s", "cli_x", "oc_1", "chat_id").is_ok());

        // Slack / generic webhook: URL required; webhook token optional
        assert!(validate_channel("slack", "", "", "", "", "").is_err());
        assert!(validate_channel("slack", "https://hooks.slack.com/x", "", "", "", "").is_ok());
        assert!(validate_channel("webhook", "", "", "", "", "").is_err());
        assert!(validate_channel("webhook", "http://10.0.0.1/hook", "", "", "", "").is_ok());
        assert!(validate_channel("webhook", "http://10.0.0.1/hook", "tok", "", "", "").is_ok());

        // Bluebird: URL and Token both required (no token = 401, fail fast)
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
        .contains("access token"));
        assert!(validate_channel(
            "bluebird",
            "https://bb.example.com/hooks/zhiwei",
            "tok",
            "",
            "",
            ""
        )
        .is_ok());

        // Deprecated types should be rejected
        for kind in ["dingtalk", "bark", "wecom", "wechat"] {
            assert!(
                validate_channel(kind, "https://x/y", "", "", "", "").is_err(),
                "{kind} should be rejected"
            );
        }
    }

    /// Temporary channel for the Test button: even with only url / kind filled, must be constructable (`receive_id_type` filled with default)
    #[test]
    fn test_channel_fills_defaults() {
        let ch = test_channel("feishu", "", " secret ", " cli_x ", " oc_1 ", "");
        assert_eq!(ch.kind, "feishu");
        assert_eq!(ch.receive_id_type, "chat_id");
        assert_eq!(ch.secret, "secret");
        assert_eq!(ch.app_id, "cli_x");
        assert_eq!(ch.receive_id, "oc_1");
    }

    /// Unknown kind falls back to webhook: old DB may have historical values, must not lose notifications
    #[test]
    fn unknown_kind_falls_back_to_webhook() {
        let body = channel_body("something-new", &rule("warning"), &facts(true), 7);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["rule"], "Disk usage high");
    }

    /// Delivery failure reasons are extracted from body: Feishu puts error code in body (HTTP may still be 200),
    /// so checking only the status would treat failure as success
    #[test]
    fn delivery_errors_surface_channel_codes() {
        // Success states are not failures: Feishu code=0, Slack plain "ok"
        assert_eq!(body_error(br#"{"code":0,"msg":"success"}"#), None);
        assert_eq!(body_error(br#"{"errcode":0,"errmsg":"ok"}"#), None);
        assert_eq!(body_error(b"ok"), None);

        let bad_secret = body_error(br#"{"code":10014,"msg":"app secret invalid"}"#).unwrap();
        assert!(bad_secret.contains("10014"), "{bad_secret}");
        assert!(bad_secret.contains("App ID or App Secret"), "{bad_secret}");

        let bad_receive = body_error(br#"{"code":230001,"msg":"receive_id invalid"}"#).unwrap();
        assert!(bad_receive.contains("chat_id"), "{bad_receive}");

        // Non-2xx: include the response body, don't leave self-hosted receivers in the dark
        assert!(body_detail(b"nope").contains("nope"));
        assert_eq!(body_detail(b"   "), "");
    }

    /// Card color and text are pure functions, but "who counts as severe" must align with `severity_rank`
    #[test]
    fn level_palette_follows_severity() {
        assert_eq!(level_style(&rule("critical"), true).feishu, "red");
        assert_eq!(level_style(&rule("warning"), true).feishu, "orange");
        assert_eq!(level_style(&rule("info"), true).feishu, "blue");
        assert_eq!(level_style(&rule("critical"), false).feishu, "green");
    }

    /// "Level → style" has only one source: Feishu's enum name, Slack's hex, the key and emoji in the payload
    /// must come from the same source, otherwise switching channels shows contradictions like "Feishu red, Slack orange"
    #[test]
    fn channels_agree_on_level_style() {
        for (severity, firing, key, feishu, hex, emoji) in [
            ("critical", true, "critical", "red", "#cf222e", "🔴"),
            ("warning", true, "warning", "orange", "#d93f0b", "🟠"),
            ("info", true, "info", "blue", "#0969da", "🔵"),
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
            assert_eq!(bb["severity"], severity, "raw severity preserved");
        }
    }

    /// Title line is shared by all channels: when push shows only the title (phone notification bar / Bark / IM collapsed)
    /// must also tell "which machine, how severe, what happened"
    #[test]
    fn title_line_is_shared_across_channels() {
        let r = rule("critical");
        let f = facts(true);
        let title = "🔴 Critical · Disk usage high (shark-9)";
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

    /// Alert text uses human-readable metric names: known ones should show human label and unit, unknown keep raw name without panic
    #[test]
    fn metric_label_humanizes_known_metrics() {
        assert_eq!(metric_label("host.disk.usage"), ("Disk Usage", "%"));
        assert_eq!(metric_label("host.mem.usage"), ("Memory Usage", "%"));
        assert_eq!(metric_label("host.cpu.usage"), ("CPU Usage", "%"));
        assert_eq!(metric_label("some.new.metric"), ("some.new.metric", ""));
    }

    /// Build a container snapshot JSON: `(id, name, started_at, state)`
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

    /// Same snapshot twice → no events (the core of "no spam": every-5-minute same shot is just the same state)
    #[test]
    fn identical_snapshots_yield_no_events() {
        let s = snap(&[("c1", "web", 1000, "running"), ("c2", "db", 500, "exited")]);
        assert!(diff_containers(&s, &s).is_empty());
    }

    /// New running container = started; disappearing from snapshot = stopped
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

    /// running → exited counts as stopped; exited → running counts as started
    #[test]
    fn running_state_transitions_are_events() {
        let up = snap(&[("c1", "web", 1000, "running")]);
        let down = snap(&[("c1", "web", 1000, "exited")]);
        assert_eq!(diff_containers(&up, &down), vec![stopped("c1", "web")]);
        assert_eq!(diff_containers(&down, &up), vec![started("c1", "web")]);

        // dead / restarting both count as non-running
        let dead = snap(&[("c1", "web", 1000, "dead")]);
        assert_eq!(diff_containers(&up, &dead), vec![stopped("c1", "web")]);
    }

    /// Restart within one cycle: state stays running, only `started_at` gets newer → counts as started
    #[test]
    fn restart_within_one_cycle_reports_start_only() {
        let prev = snap(&[("c1", "web", 1000, "running")]);
        let cur = snap(&[("c1", "web", 3000, "running")]);
        assert_eq!(diff_containers(&prev, &cur), vec![started("c1", "web")]);
    }

    /// Old node-agent doesn't report start time (0): getting the real time doesn't mean "just started",
    /// can't misreport existing containers as a screen of "started" (spam after agent upgrade)
    #[test]
    fn unknown_previous_start_time_does_not_report_a_burst() {
        let old = snap(&[("c1", "web", 0, "running"), ("c2", "db", 0, "running")]);
        let new = snap(&[
            ("c1", "web", 1700000000, "running"),
            ("c2", "db", 1700000000, "running"),
        ]);
        assert!(diff_containers(&old, &new).is_empty());

        // But actual state changes still report
        let down = snap(&[("c1", "web", 1700000000, "exited")]);
        assert_eq!(
            diff_containers(&snap(&[("c1", "web", 0, "running")]), &down),
            vec![stopped("c1", "web")]
        );
    }

    /// One-shot containers (exit immediately) shouldn't repeat every snapshot: new but non-running doesn't report,
    /// exited → exited also doesn't report
    #[test]
    fn one_shot_containers_do_not_alert() {
        let empty = snap(&[]);
        let done = snap(&[("c1", "migrate", 1000, "exited")]);
        assert!(diff_containers(&empty, &done).is_empty());
        assert!(diff_containers(&done, &done).is_empty());
    }

    /// Recreate (compose up / deploy): same name, new ID. The old ID left the snapshot while a
    /// running container with the same name exists → its stopped alert must be closed.
    #[test]
    fn replaced_container_ids_matches_renamed_id() {
        let prev = snap(&[("old123", "zhiwei-monitor", 1000, "running")]);
        let cur = snap(&[("new456", "zhiwei-monitor", 2000, "running")]);
        assert_eq!(
            replaced_container_ids(&prev, &cur),
            vec!["old123".to_string()]
        );
        // diff also reports the old container stopped and the new one started
        assert_eq!(
            diff_containers(&prev, &cur),
            vec![
                stopped("old123", "zhiwei-monitor"),
                started("new456", "zhiwei-monitor")
            ]
        );
    }

    /// No same-named running replacement → nothing to close. Covers: no change at all, a genuinely
    /// removed container (no successor), and a same-name container that is not running.
    #[test]
    fn replaced_container_ids_needs_a_running_successor() {
        let same = snap(&[("c1", "web", 1000, "running")]);
        assert!(replaced_container_ids(&same, &same).is_empty());

        let before = snap(&[("c1", "web", 1000, "running")]);
        let gone = snap(&[]);
        assert!(replaced_container_ids(&before, &gone).is_empty());

        let exited_successor = snap(&[("c2", "web", 1000, "exited")]);
        assert!(replaced_container_ids(&before, &exited_successor).is_empty());
    }

    /// Dirty JSON / missing fields: don't panic, don't false-alert
    #[test]
    fn malformed_snapshots_are_ignored() {
        assert!(diff_containers("not json", "not json").is_empty());
        assert!(diff_containers("[]", "[]").is_empty());
        // Entries missing id are skipped
        let cur = r#"[{"name":"x","state":"running"}]"#;
        assert!(diff_containers("[]", cur).is_empty());
        // Missing state: treat as non-running, don't report "started"
        let old = r#"[{"id":"c1","name":"web"}]"#;
        assert!(diff_containers(old, old).is_empty());
    }
}
