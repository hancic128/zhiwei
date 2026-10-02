//! 待办（`GET /v1/todo`）——默认页。
//!
//! 定位（`docs/POSITIONING.md`）要求「认知轻」：系统给的不是曲线，是
//! 「今天要我处理的几件事」。本模块把多个数据源汇成一条流：
//! 指标告警 · 服务探活 · 证书到期 · 节点离线。
//!
//! 设计：`docs/superpowers/specs/2026-09-19-product-structure-design.md` §4。

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::routes::{err, read_auth_ok};
use crate::state::AppState;

/// 节点多久没上报算离线——和后台 liveness 巡检共用同一个常量（见 alerts::NODE_OFFLINE_AFTER_MS），
/// 避免出现「控制台显示在线 / 后台已经在告警」这种不一致。
pub(crate) use crate::alerts::NODE_OFFLINE_AFTER_MS;

/// 已恢复的留痕条数——留痕是建立信任用的，不需要长
const RECOVERED_LIMIT: i64 = 5;

/// 分档：按「要不要现在动手」，不按监控的严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// 已经坏了，现在就要处理
    Now,
    /// 还没坏，但会坏——要留意
    Watch,
}

/// 分档规则。
///
/// 关键点：**不看 severity，看「坏了没有」**。
/// - `probe`：服务已经探不到了 → 现在要处理（哪怕规则标的是 warning）
/// - `node_offline`：节点已经联系不上 → 现在要处理
/// - `command_channel`：节点在线但控制面是坏的（启停 / 日志全是死按钮）→ 现在要处理
/// - `cert` / `rule`：由 severity 决定——但这两个来源的 severity 语义恰好就是
///   「坏了没有」：证书是「已过期 / 快到期」，指标规则是「超阈值 / 接近阈值」
pub fn bucket_of(source: &str, severity: &str) -> Bucket {
    match source {
        // 已经坏了：服务探不到 / 节点联系不上 / 命令通道是死的（点了没反应）
        "probe" | "node_offline" | "command_channel" => Bucket::Now,
        // 指标规则与证书：严重程度由来源自己定（语义见上）
        _ => {
            if severity == "critical" {
                Bucket::Now
            } else {
                Bucket::Watch
            }
        }
    }
}

/// 每条待办要带「下一步做什么」——返回 i18n 键，文案在前端语言包里。
pub fn hint_key(source: &str, metric: &str) -> &'static str {
    match source {
        "probe" => "probe",
        "cert" => "cert",
        "node_offline" => "nodeOffline",
        "command_channel" => "commandChannel",
        "platform" => "platform",
        "container" => "container",
        _ => {
            if metric.starts_with("host.disk") {
                "disk"
            } else if metric.starts_with("host.mem") {
                "memory"
            } else if metric.starts_with("host.cpu") {
                "cpu"
            } else {
                "generic"
            }
        }
    }
}

/// 静默 = 「这段时间别告诉我」。静默期内的告警不进待办，到期后自动回来。
/// 边界：`until == now` 算已过期，应当回到待办。
pub fn is_silenced(silenced_until_unix_nano: Option<i64>, now_ns: i64) -> bool {
    matches!(silenced_until_unix_nano, Some(until) if until > now_ns)
}

/// 一条待办。`title` / `detail` 里，告警类的文案来自库里的规则名与 message；
/// 节点离线这类由前端用 `hint_key` + 时间自己拼（保证中英双语）。
#[derive(Debug, Serialize)]
pub struct TodoItem {
    pub id: String,
    /// rule | probe | cert | node_offline | command_channel
    pub source: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    /// 下一步做什么（i18n 键）
    pub hint_key: &'static str,
    pub node_id: String,
    pub hostname: String,
    pub since_unix_nano: i64,
    pub resolved_at_unix_nano: Option<i64>,
    /// 前端「去看看」的落点
    pub link: String,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub nodes_online: i64,
    pub nodes_total: i64,
    /// 健康探针数（按探针维度，不是按服务分组）
    pub probes_healthy: i64,
    /// 探针总数
    pub probes_total: i64,
    pub certs_total: i64,
    pub containers_total: i64,
    pub containers_failed: i64,
}

/// `GET /v1/todo` —— 默认页：把「今天要处理的事」汇成一条流。
pub async fn todo_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let now_ns = zhiwei_common::Timestamp::now().unix_nano();
    let now_ms = now_ns / 1_000_000;
    let is_online = |last_seen: Option<i64>| {
        last_seen
            .map(|ts| now_ms - ts / 1_000_000 < NODE_OFFLINE_AFTER_MS)
            .unwrap_or(false)
    };

    let nodes = match state.storage.nodes().list_all().await {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list nodes: {e}"),
            )
        }
    };

    // 待办里的节点名一律用「显示名」（有别名用别名）——主机名往往是
    // VM-16-12-opencloudos 这种，看不出是哪台；别名是用户自己起的名字。
    // 节点已删除时退回告警落库时记下的主机名。
    let display_of = |id: &str, fallback: &str| -> String {
        nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| crate::routes::node_display_name(&n.alias, &n.hostname))
            .unwrap_or_else(|| fallback.to_string())
    };

    let (services_healthy, services_total) = state
        .storage
        .probes()
        .probe_counts()
        .await
        .unwrap_or((0, 0));

    // 证书与容器来自各节点最新快照
    let mut certs_total = 0i64;
    let mut containers_total = 0i64;
    let mut containers_failed = 0i64;
    for n in &nodes {
        let id = zhiwei_common::NodeId::from_string(n.id.clone());
        let row = match state.storage.inventory().find(&id).await {
            Ok(Some(r)) => r,
            _ => continue,
        };
        let certs: Vec<serde_json::Value> =
            serde_json::from_str(&row.certificates_json).unwrap_or_default();
        certs_total += certs
            .iter()
            .filter(|c| {
                !c.get("parse_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .count() as i64;
        let containers: Vec<serde_json::Value> =
            serde_json::from_str(&row.containers_json).unwrap_or_default();
        for c in &containers {
            containers_total += 1;
            let st = c.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let status = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if crate::probes_api::container_is_failed(st, status) {
                containers_failed += 1;
            }
        }
    }

    let open_alerts = state
        .storage
        .alerts()
        .open_alerts()
        .await
        .unwrap_or_default();
    let resolved_alerts = state
        .storage
        .alerts()
        .resolved_alerts(RECOVERED_LIMIT)
        .await
        .unwrap_or_default();

    let mut now_items: Vec<TodoItem> = Vec::new();
    let mut watch_items: Vec<TodoItem> = Vec::new();
    let mut silenced = 0i64;

    for a in &open_alerts {
        // 静默期内的不进待办——静默就是「这段时间别告诉我」
        if is_silenced(a.silenced_until_unix_nano, now_ns) {
            silenced += 1;
            continue;
        }
        let item = TodoItem {
            id: format!("alert-{}", a.id),
            source: a.source.clone(),
            severity: a.severity.clone(),
            title: a.rule_name.clone(),
            detail: a.message.clone(),
            hint_key: hint_key(&a.source, &a.metric),
            node_id: a.node_id.clone(),
            hostname: display_of(&a.node_id, &a.hostname),
            since_unix_nano: a.started_at_unix_nano,
            resolved_at_unix_nano: None,
            link: format!("/nodes/{}", a.node_id),
        };
        match bucket_of(&a.source, &a.severity) {
            Bucket::Now => now_items.push(item),
            _ => watch_items.push(item),
        }
    }

    // 节点离线：来自 alerts 表的真实告警（见 `alerts::on_node_liveness_change`）。
    // 这里不再合成——真实告警的好处是：能走通知渠道、能按 ID 静默、历史在「已恢复」里能看到。

    // 命令通道：节点在线，却一直不来拉命令 —— 控制台上的启停 / 重启 / 日志
    // 点下去没有任何反应，而节点页看起来一切正常。这是本项目最难自查的一类
    // 故障（没有 ops 公钥的老节点一个请求都不发，日志里只有一行 warn）。
    // 节点离线的那种不在这里报：上面那条已经说了，同一个根因别报两遍。
    for n in &nodes {
        let last_poll_ms = state.control_polls.last(&n.id);
        let channel = crate::control_channel::channel_state(
            n.last_seen_unix_nano.map(|ns| ns / 1_000_000),
            last_poll_ms,
            now_ms - state.started_at_ms,
            now_ms,
        );
        if channel != crate::control_channel::Channel::Down {
            continue;
        }
        // 「坏了多久」：最后一次拉取就是最后一次能用；一次都没拉过的
        // （入网早于 ops 公钥分发）用入网时刻——对它来说通道从来没通过。
        let since_ms = last_poll_ms.unwrap_or(n.enrolled_at_unix_nano / 1_000_000);
        now_items.push(TodoItem {
            id: format!("command-channel-{}", n.id),
            source: "command_channel".into(),
            severity: "critical".into(),
            title: display_of(&n.id, &n.hostname),
            // 文案由前端按 hint_key 拼，保证中英双语
            detail: String::new(),
            hint_key: "commandChannel",
            node_id: n.id.clone(),
            hostname: display_of(&n.id, &n.hostname),
            since_unix_nano: since_ms * 1_000_000,
            resolved_at_unix_nano: None,
            link: format!("/nodes/{}", n.id),
        });
    }

    // 急的先看；同档里拖得久的排前面
    let sort_items = |items: &mut Vec<TodoItem>| {
        items.sort_by(|a, b| {
            let rank = |s: &str| if s == "critical" { 0 } else { 1 };
            rank(&a.severity)
                .cmp(&rank(&b.severity))
                .then(a.since_unix_nano.cmp(&b.since_unix_nano))
        });
    };
    sort_items(&mut now_items);
    sort_items(&mut watch_items);

    let recovered: Vec<TodoItem> = resolved_alerts
        .iter()
        .map(|a| TodoItem {
            id: format!("alert-{}", a.id),
            source: a.source.clone(),
            severity: a.severity.clone(),
            title: a.rule_name.clone(),
            detail: a.message.clone(),
            hint_key: hint_key(&a.source, &a.metric),
            node_id: a.node_id.clone(),
            hostname: display_of(&a.node_id, &a.hostname),
            since_unix_nano: a.started_at_unix_nano,
            resolved_at_unix_nano: a.resolved_at_unix_nano,
            link: format!("/nodes/{}", a.node_id),
        })
        .collect();

    let summary = Summary {
        nodes_online: nodes
            .iter()
            .filter(|n| is_online(n.last_seen_unix_nano))
            .count() as i64,
        nodes_total: nodes.len() as i64,
        probes_healthy: services_healthy,
        probes_total: services_total,
        certs_total,
        containers_total,
        containers_failed,
    };

    Json(serde_json::json!({
        "generated_at_unix_nano": now_ns,
        "summary": summary,
        "counts": {
            "now": now_items.len(),
            "watch": watch_items.len(),
            "recovered": recovered.len(),
            "silenced": silenced,
        },
        "now": now_items,
        "watch": watch_items,
        "recovered": recovered,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_outage_is_act_now_even_when_severity_is_warning() {
        // 探活失败意味着服务已经不可用，不该因为规则标了 warning 就降级成「留意」
        assert_eq!(bucket_of("probe", "warning"), Bucket::Now);
        assert_eq!(bucket_of("probe", "critical"), Bucket::Now);
    }

    #[test]
    fn node_offline_is_act_now() {
        assert_eq!(bucket_of("node_offline", "critical"), Bucket::Now);
    }

    #[test]
    fn expired_cert_is_act_now_but_expiring_soon_is_only_watch() {
        // 证书告警的 severity 语义就是「过没过期」——见 alerts.rs:
        //   let severity = if expired { "critical" } else { "warning" };
        // 已过期 = HTTPS 现在就是坏的 → 现在要处理；快到期 = 还没坏 → 留意。
        // 这条是浏览器验收时抓出来的：把已过期 961 天的证书放进了「留意」。
        assert_eq!(bucket_of("cert", "critical"), Bucket::Now);
        assert_eq!(bucket_of("cert", "warning"), Bucket::Watch);
    }

    #[test]
    fn metric_rule_follows_severity() {
        assert_eq!(bucket_of("rule", "critical"), Bucket::Now);
        assert_eq!(bucket_of("rule", "warning"), Bucket::Watch);
    }

    #[test]
    fn unknown_source_and_empty_severity_default_to_watch() {
        assert_eq!(bucket_of("rule", ""), Bucket::Watch);
        assert_eq!(bucket_of("something_new", "warning"), Bucket::Watch);
    }

    #[test]
    fn hint_follows_the_source() {
        assert_eq!(hint_key("probe", ""), "probe");
        assert_eq!(hint_key("cert", ""), "cert");
        assert_eq!(hint_key("node_offline", ""), "nodeOffline");
        assert_eq!(hint_key("command_channel", ""), "commandChannel");
    }

    #[test]
    fn dead_command_channel_is_act_now() {
        // 节点在线却拉不动命令：启停 / 日志全是死按钮，别降级成「留意」
        assert_eq!(bucket_of("command_channel", "critical"), Bucket::Now);
        assert_eq!(bucket_of("command_channel", "warning"), Bucket::Now);
    }

    #[test]
    fn hint_narrows_metric_rules_by_metric() {
        assert_eq!(hint_key("rule", "host.disk.usage"), "disk");
        // 播种规则里的真实指标名是 host.mem.usage（见 alerts.rs）
        assert_eq!(hint_key("rule", "host.mem.usage"), "memory");
        assert_eq!(hint_key("rule", "host.cpu.usage"), "cpu");
    }

    #[test]
    fn hint_falls_back_for_unknown_metric() {
        assert_eq!(hint_key("rule", "some.new.metric"), "generic");
    }

    #[test]
    fn silenced_alerts_stay_out_of_the_todo_until_the_window_ends() {
        let now = 1_000_000_000_000i64;
        assert!(!is_silenced(None, now));
        assert!(is_silenced(Some(now + 1), now));
        assert!(!is_silenced(Some(now - 1), now));
        // 刚好到点：算已过期，应当回到待办
        assert!(!is_silenced(Some(now), now));
    }
}
