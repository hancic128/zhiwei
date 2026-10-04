//! Todo (`GET /v1/todo`) — default page.
//!
//! Design principle: the system gives you not curves, but "the few things I
//! need to handle today". This module combines multiple data sources into one
//! stream: metric alerts · service probes · certificate expiry · node offline.
//!
//! Design: `docs/superpowers/specs/2026-09-19-product-structure-design.md` §4.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde::Serialize;
use zhiwei_storage::alerts_repo::Alert;
use zhiwei_storage::node_repo::NodeRecord;

use crate::routes::{err, read_auth_ok};
use crate::state::AppState;

/// How long a node can go silent before considered offline — shares the same
/// constant with the background liveness check (see `alerts::NODE_OFFLINE_AFTER_MS`),
/// avoiding "console shows online / background already alerts" inconsistency.
pub use crate::alerts::NODE_OFFLINE_AFTER_MS;

/// Query parameters for todo endpoint pagination.
#[derive(Debug, Deserialize)]
pub struct TodoQuery {
    /// Cursor: `since_unix_nano` of the last item in the previous page.
    /// For recovered list, this is `resolved_at_unix_nano`.
    pub cursor: Option<i64>,
    /// Number of items per list (now, watch, recovered).
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    5
}

/// Buckets: by "do I need to act now", not by severity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// Already broken, needs action now
    Now,
    /// Not broken yet, but will be — keep an eye on it
    Watch,
}

/// Bucketing rules.
///
/// Key point: **doesn't look at severity, looks at "is it broken"**.
/// - `probe`: service already unreachable → act now (even if rule is "warning")
/// - `node_offline`: node already unreachable → act now
/// - `command_channel`: node online but control plane is broken (start / log
///   buttons are dead) → act now
/// - `cert` / `rule`: severity decides — but for these two sources the
///   severity semantics happen to be "is it broken": cert is "expired /
///   expiring soon", metric rule is "above threshold / approaching threshold"
pub fn bucket_of(source: &str, severity: &str) -> Bucket {
    match source {
        // Already broken: service unreachable / node unreachable / command
        // channel dead (clicks have no effect)
        "probe" | "node_offline" | "command_channel" => Bucket::Now,
        // Metric rules and certs: severity decided by the source itself
        // (semantics above)
        _ => {
            if severity == "critical" {
                Bucket::Now
            } else {
                Bucket::Watch
            }
        }
    }
}

/// Each todo item carries "what to do next" — returns an i18n key, the text
/// lives in the frontend's language pack.
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

/// Silence = "don't tell me about it for this period". Alerts within the
/// silence window don't enter the todo; they auto-return when the window ends.
/// Boundary: `until == now` is already expired, should return to the todo.
pub const fn is_silenced(silenced_until_unix_nano: Option<i64>, now_ns: i64) -> bool {
    matches!(silenced_until_unix_nano, Some(until) if until > now_ns)
}

/// One todo item. For `title` / `detail`, alert-class text comes from the
/// stored rule name and message; for things like node offline, the frontend
/// composes its own display via `hint_key` + time (so bilingual works).
#[derive(Debug, Serialize)]
pub struct TodoItem {
    pub id: String,
    /// rule | probe | cert | `node_offline` | `command_channel`
    pub source: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    /// What to do next (i18n key)
    pub hint_key: &'static str,
    pub node_id: String,
    pub hostname: String,
    pub since_unix_nano: i64,
    pub resolved_at_unix_nano: Option<i64>,
    /// Frontend "go look" target
    pub link: String,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub nodes_online: i64,
    pub nodes_total: i64,
    /// Number of healthy probes (per-probe, not per-service grouping)
    pub probes_healthy: i64,
    /// Total probes
    pub probes_total: i64,
    pub certs_total: i64,
    pub containers_total: i64,
    pub containers_failed: i64,
}

/// `GET /v1/todo` — default page: combines "things to handle today" into one
/// stream. Supports cursor-based pagination.
pub async fn todo_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TodoQuery>,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let now_ns = zhiwei_common::Timestamp::now().unix_nano();
    let now_epoch_ms = now_ns / 1_000_000;
    let limit = query.limit.clamp(1, 100);
    let cursor = query.cursor.unwrap_or(i64::MAX);

    let nodes = match state.storage.nodes().list_all().await {
        Ok(n) => n,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list nodes: {e}"),
            )
        }
    };

    let (services_healthy, services_total) = state
        .storage
        .probes()
        .probe_counts()
        .await
        .unwrap_or((0, 0));
    let (certs_total, containers_total, containers_failed) =
        inventory_counters(&state, &nodes).await;

    let open_alerts = state
        .storage
        .alerts()
        .open_alerts()
        .await
        .unwrap_or_default();
    // Use larger limit for pagination; we fetch all and slice on the backend side
    let resolved_alerts = state
        .storage
        .alerts()
        .resolved_alerts(1000) // Fetch enough for pagination
        .await
        .unwrap_or_default();

    let (mut now_items, mut watch_items, silenced) =
        bucket_open_alerts(&open_alerts, &nodes, now_ns);
    now_items.extend(command_channel_items(&state, &nodes, now_epoch_ms));

    // Urgent first; within the same bucket, longer-standing items come first
    sort_items(&mut now_items);
    sort_items(&mut watch_items);

    let all_recovered = recovered_items(&resolved_alerts, &nodes);

    // Apply cursor-based pagination: items with since_unix_nano < cursor (older)
    let (now_page, now_has_more) = paginate(now_items, cursor, limit);
    let (watch_page, watch_has_more) = paginate(watch_items, cursor, limit);
    let (recovered_page, recovered_has_more) = paginate_recovered(all_recovered, cursor, limit);

    let summary = Summary {
        nodes_online: nodes
            .iter()
            .filter(|n| is_online(n.last_seen_unix_nano, now_epoch_ms))
            .count()
            .try_into()
            .unwrap_or(i64::MAX),
        nodes_total: i64::try_from(nodes.len()).unwrap_or(i64::MAX),
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
            "now": now_page.len(),
            "watch": watch_page.len(),
            "recovered": recovered_page.len(),
            "silenced": silenced,
        },
        "pagination": {
            "has_more": {
                "now": now_has_more,
                "watch": watch_has_more,
                "recovered": recovered_has_more,
            },
            "next_cursor": {
                "now": now_page.last().map(|i| i.since_unix_nano),
                "watch": watch_page.last().map(|i| i.since_unix_nano),
                "recovered": recovered_page.last().map(|i| i.resolved_at_unix_nano.unwrap_or(i.since_unix_nano)),
            },
        },
        "now": now_page,
        "watch": watch_page,
        "recovered": recovered_page,
    }))
    .into_response()
}

/// Paginate items by since_unix_nano cursor.
fn paginate(mut items: Vec<TodoItem>, cursor: i64, limit: i64) -> (Vec<TodoItem>, bool) {
    // Filter items with since_unix_nano < cursor (older than cursor)
    items.retain(|i| i.since_unix_nano < cursor);
    let has_more = items.len() > limit as usize;
    items.truncate(usize::try_from(limit).unwrap_or(0));
    (items, has_more)
}

/// Paginate recovered items by resolved_at_unix_nano cursor.
fn paginate_recovered(mut items: Vec<TodoItem>, cursor: i64, limit: i64) -> (Vec<TodoItem>, bool) {
    // Filter items with resolved_at_unix_nano < cursor (older than cursor)
    items.retain(|i| {
        i.resolved_at_unix_nano
            .unwrap_or(i.since_unix_nano)
            < cursor
    });
    let has_more = items.len() > limit as usize;
    items.truncate(usize::try_from(limit).unwrap_or(0));
    (items, has_more)
}

/// Whether a node last seen at `last_seen_unix_nano` is considered online at `now_epoch_ms`.
fn is_online(last_seen_unix_nano: Option<i64>, now_epoch_ms: i64) -> bool {
    last_seen_unix_nano.is_some_and(|ts| now_epoch_ms - ts / 1_000_000 < NODE_OFFLINE_AFTER_MS)
}

/// Node names in the todo always use the "display name" (alias if set) —
/// hostnames tend to look like auto-generated cloud names and you can't tell
/// which machine it is; aliases are user-chosen names. When a node is deleted,
/// fall back to the hostname recorded when the alert was stored.
fn node_display(nodes: &[NodeRecord], id: &str, fallback: &str) -> String {
    nodes.iter().find(|n| n.id == id).map_or_else(
        || fallback.to_string(),
        |n| crate::routes::node_display_name(&n.alias, &n.hostname),
    )
}

/// Map a stored alert row into a todo item. `resolved` selects whether the
/// resolved timestamp is carried (open alerts have none).
fn alert_item(a: &Alert, nodes: &[NodeRecord], resolved: bool) -> TodoItem {
    TodoItem {
        id: format!("alert-{}", a.id),
        source: a.source.clone(),
        severity: a.severity.clone(),
        title: a.rule_name.clone(),
        detail: a.message.clone(),
        hint_key: hint_key(&a.source, &a.metric),
        node_id: a.node_id.clone(),
        hostname: node_display(nodes, &a.node_id, &a.hostname),
        since_unix_nano: a.started_at_unix_nano,
        resolved_at_unix_nano: if resolved {
            a.resolved_at_unix_nano
        } else {
            None
        },
        link: format!("/nodes/{}", a.node_id),
    }
}

/// Split open alerts into `now` / `watch` buckets, skipping silenced ones.
/// Returns the two item lists plus the silenced count.
fn bucket_open_alerts(
    open_alerts: &[Alert],
    nodes: &[NodeRecord],
    now_ns: i64,
) -> (Vec<TodoItem>, Vec<TodoItem>, i64) {
    let mut now_items = Vec::new();
    let mut watch_items = Vec::new();
    let mut silenced = 0i64;
    for a in open_alerts {
        // Within the silence window → don't enter the todo — silence means
        // "don't tell me about it for this period"
        if is_silenced(a.silenced_until_unix_nano, now_ns) {
            silenced += 1;
            continue;
        }
        let item = alert_item(a, nodes, false);
        match bucket_of(&a.source, &a.severity) {
            Bucket::Now => now_items.push(item),
            Bucket::Watch => watch_items.push(item),
        }
    }
    (now_items, watch_items, silenced)
}

/// Certificates and containers come from each node's latest snapshot.
async fn inventory_counters(state: &AppState, nodes: &[NodeRecord]) -> (i64, i64, i64) {
    let mut certs_total = 0i64;
    let mut containers_total = 0i64;
    let mut containers_failed = 0i64;
    for n in nodes {
        let id = zhiwei_common::NodeId::from_string(n.id.clone());
        let Ok(Some(row)) = state.storage.inventory().find(&id).await else {
            continue;
        };
        let certs: Vec<serde_json::Value> =
            serde_json::from_str(&row.certificates_json).unwrap_or_default();
        certs_total += i64::try_from(
            certs
                .iter()
                .filter(|c| {
                    !c.get("parse_error")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                })
                .count(),
        )
        .unwrap_or(i64::MAX);
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
    (certs_total, containers_total, containers_failed)
}

/// Command channel: node appears online but never polls for commands — start
/// / stop / logs buttons in the console do nothing while the Nodes page looks
/// normal. This is the hardest class of failure to spot in this project
/// (an old node without the ops pub key never sends a single request; the
/// log only has one warn line).
///
/// The "node offline" case is not raised here: it's already raised in
/// [`bucket_open_alerts`]; reporting the same root cause twice would be noise.
fn command_channel_items(
    state: &AppState,
    nodes: &[NodeRecord],
    now_epoch_ms: i64,
) -> Vec<TodoItem> {
    let mut items = Vec::new();
    for n in nodes {
        let last_poll_ms = state.control_polls.last(&n.id);
        let channel = crate::control_channel::channel_state(
            n.last_seen_unix_nano.map(|ns| ns / 1_000_000),
            last_poll_ms,
            now_epoch_ms - state.started_at_ms,
            now_epoch_ms,
        );
        if channel != crate::control_channel::Channel::Down {
            continue;
        }
        // "How long has it been broken": the last successful poll is the last
        // time the channel worked. Nodes that never polled (enrolled before
        // ops pub key distribution) use the enrollment timestamp — for them
        // the channel has never worked.
        let since_ms = last_poll_ms.unwrap_or(n.enrolled_at_unix_nano / 1_000_000);
        items.push(TodoItem {
            id: format!("command-channel-{}", n.id),
            source: "command_channel".into(),
            severity: "critical".into(),
            title: node_display(nodes, &n.id, &n.hostname),
            // The wording is composed by the frontend via hint_key, so both
            // en-US and zh-CN are covered.
            detail: String::new(),
            hint_key: "commandChannel",
            node_id: n.id.clone(),
            hostname: node_display(nodes, &n.id, &n.hostname),
            since_unix_nano: since_ms * 1_000_000,
            resolved_at_unix_nano: None,
            link: format!("/nodes/{}", n.id),
        });
    }
    items
}

/// Urgent first; within the same bucket, longer-standing items come first.
fn sort_items(items: &mut [TodoItem]) {
    items.sort_by(|a, b| {
        let rank = |s: &str| i32::from(s != "critical");
        rank(&a.severity)
            .cmp(&rank(&b.severity))
            .then(a.since_unix_nano.cmp(&b.since_unix_nano))
    });
}

fn recovered_items(resolved_alerts: &[Alert], nodes: &[NodeRecord]) -> Vec<TodoItem> {
    resolved_alerts
        .iter()
        .map(|a| alert_item(a, nodes, true))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_outage_is_act_now_even_when_severity_is_warning() {
        // A probe failure means the service is unreachable — it should not be
        // demoted to "watch" just because the rule is tagged warning.
        assert_eq!(bucket_of("probe", "warning"), Bucket::Now);
        assert_eq!(bucket_of("probe", "critical"), Bucket::Now);
    }

    #[test]
    fn node_offline_is_act_now() {
        assert_eq!(bucket_of("node_offline", "critical"), Bucket::Now);
    }

    #[test]
    fn expired_cert_is_act_now_but_expiring_soon_is_only_watch() {
        // The certificate-alert severity is literally "expired or not" — see alerts.rs:
        //   let severity = if expired { "critical" } else { "warning" };
        // Expired = HTTPS is broken right now -> handle now. Expiring soon =
        // not yet broken -> watch. Caught during browser QA: a cert expired
        // 961 days ago had landed in "watch" before this fix.
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
        // Node is online but the command channel is dead: start / stop / logs
        // are dead buttons; do not demote to "watch".
        assert_eq!(bucket_of("command_channel", "critical"), Bucket::Now);
        assert_eq!(bucket_of("command_channel", "warning"), Bucket::Now);
    }

    #[test]
    fn hint_narrows_metric_rules_by_metric() {
        assert_eq!(hint_key("rule", "host.disk.usage"), "disk");
        // The seeded alert rule's metric is host.mem.usage (see alerts.rs)
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
        // Exactly at the boundary: counts as already expired and should reappear in the todo
        assert!(!is_silenced(Some(now), now));
    }
}
