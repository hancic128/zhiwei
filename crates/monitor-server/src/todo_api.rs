//! Todo (`GET /v1/todo`) — default page.
//!
//! Design principle: the system gives you not curves, but "the few things I
//! need to handle today". This module combines multiple data sources into one
//! stream: metric alerts · service probes · certificate expiry · node offline.
//!
//! Design: `docs/superpowers/specs/2026-09-19-product-structure-design.md` §4.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::routes::{err, read_auth_ok};
use crate::state::AppState;

/// How long a node can go silent before considered offline — shares the same
/// constant with the background liveness check (see alerts::NODE_OFFLINE_AFTER_MS),
/// avoiding "console shows online / background already alerts" inconsistency.
pub(crate) use crate::alerts::NODE_OFFLINE_AFTER_MS;

/// Number of recovered-history entries kept — history is for trust, doesn't
/// need to be long
const RECOVERED_LIMIT: i64 = 5;

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
pub fn is_silenced(silenced_until_unix_nano: Option<i64>, now_ns: i64) -> bool {
    matches!(silenced_until_unix_nano, Some(until) if until > now_ns)
}

/// One todo item. For `title` / `detail`, alert-class text comes from the
/// stored rule name and message; for things like node offline, the frontend
/// composes its own display via `hint_key` + time (so bilingual works).
#[derive(Debug, Serialize)]
pub struct TodoItem {
    pub id: String,
    /// rule | probe | cert | node_offline | command_channel
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
/// stream.
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

    // Node names in the todo always use the "display name" (alias if set) —
    // hostnames tend to look like auto-generated cloud names and you can't tell
    // which machine it is; aliases are user-chosen names. When a node is deleted,
    // fall back to the hostname recorded when the alert was stored.
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

    // Certificates and containers come from each node's latest snapshot
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
        // Within the silence window → don't enter the todo — silence means
        // "don't tell me about it for this period"
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

    // Node-offline: comes from real alerts in the alerts table (see
    // `alerts::on_node_liveness_change`). We do not synthesize a duplicate here —
    // real alerts flow through the notify channel, can be silenced by ID, and
    // show up under "recovered".

    // Command channel: node appears online but never polls for commands — start
    // / stop / logs buttons in the console do nothing while the Nodes page looks
    // normal. This is the hardest class of failure to spot in this project
    // (an old node without the ops pub key never sends a single request; the
    // log only has one warn line).
    // The "node offline" case is not raised here: it's already raised above;
    // reporting the same root cause twice would be noise.
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
        // "How long has it been broken": the last successful poll is the last
        // time the channel worked. Nodes that never polled (enrolled before
        // ops pub key distribution) use the enrollment timestamp — for them
        // the channel has never worked.
        let since_ms = last_poll_ms.unwrap_or(n.enrolled_at_unix_nano / 1_000_000);
        now_items.push(TodoItem {
            id: format!("command-channel-{}", n.id),
            source: "command_channel".into(),
            severity: "critical".into(),
            title: display_of(&n.id, &n.hostname),
            // The wording is composed by the frontend via hint_key, so both
            // en-US and zh-CN are covered.
            detail: String::new(),
            hint_key: "commandChannel",
            node_id: n.id.clone(),
            hostname: display_of(&n.id, &n.hostname),
            since_unix_nano: since_ms * 1_000_000,
            resolved_at_unix_nano: None,
            link: format!("/nodes/{}", n.id),
        });
    }

    // Urgent first; within the same bucket, longer-standing items come first
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
