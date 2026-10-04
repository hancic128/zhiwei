use sqlx::SqlitePool;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AlertRule {
    pub id: i64,
    pub name: String,
    pub metric: String,
    /// gt | gte | lt | lte | eq
    pub op: String,
    pub threshold: f64,
    pub duration_seconds: i64,
    /// warning | critical
    pub severity: String,
    pub enabled: bool,
    pub created_at_unix_nano: i64,
    pub updated_at_unix_nano: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct AlertRuleRow {
    id: i64,
    name: String,
    metric: String,
    op: String,
    threshold: f64,
    duration_seconds: i64,
    severity: String,
    enabled: i32,
    created_at_unix_nano: i64,
    updated_at_unix_nano: i64,
}

impl From<AlertRuleRow> for AlertRule {
    fn from(r: AlertRuleRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            metric: r.metric,
            op: r.op,
            threshold: r.threshold,
            duration_seconds: r.duration_seconds,
            severity: r.severity,
            enabled: r.enabled != 0,
            created_at_unix_nano: r.created_at_unix_nano,
            updated_at_unix_nano: r.updated_at_unix_nano,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Alert {
    pub id: i64,
    pub rule_id: i64,
    pub rule_name: String,
    pub node_id: String,
    pub hostname: String,
    pub severity: String,
    pub metric: String,
    pub op: String,
    pub threshold: f64,
    pub value: f64,
    pub message: String,
    pub started_at_unix_nano: i64,
    pub resolved_at_unix_nano: Option<i64>,
    pub silenced_until_unix_nano: Option<i64>,
    /// rule = metric rule alert; probe = service probe state alert; cert = cert expiry alert
    pub source: String,
    /// when source = probe: `probe_id`; when source = cert: `{source_id}:{cert path}`
    pub source_ref: String,
}

/// Evaluation state for a single (rule, node) pair
#[derive(Debug, Clone)]
pub struct EvalState {
    pub breaching_since_unix_nano: Option<i64>,
    pub firing: bool,
    pub open_alert_id: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NotifyChannel {
    pub id: i64,
    pub name: String,
    /// `feishu` / `slack` / `bluebird` / `webhook`
    pub kind: String,
    /// Slack/generic webhook delivery address; Feishu doesn't use this (address determined by `receive_id`)
    pub url: String,
    /// Reused by type: Feishu = App Secret, generic webhook = delivery Token (Bearer)
    pub secret: String,
    /// Feishu app App ID
    pub app_id: String,
    /// Feishu receive ID (group `chat_id` / user `open_id`, etc.)
    pub receive_id: String,
    /// Feishu's `receive_id_type`: `chat_id` / `open_id` / `user_id` / `union_id` / email
    pub receive_id_type: String,
    pub enabled: bool,
    pub min_severity: String,
}

/// Input for creating a new channel.
///
/// # Detailed
///
/// Many fields, most only apply to certain channel types; grouping into one
/// struct is more readable than a long list of positional arguments (call sites
/// don't need to count positions either).
pub struct NewChannel<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub url: &'a str,
    pub secret: &'a str,
    pub app_id: &'a str,
    pub receive_id: &'a str,
    pub receive_id_type: &'a str,
    pub min_severity: &'a str,
}

/// Bare form of `alert_rules` rows (matches SELECT column order)
type RuleRow = (i64, String, String, String, f64, i64, String, i64, i64, i64);

/// External form of builtin rules (node online/offline/service probes/containers/certs).
///
/// id is a stable string ('`node_offline`' / '`node_online`', etc.), frontend uses it as the toggle key.
/// Supports editing threshold and `duration_seconds`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BuiltinAlertRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub threshold: f64,
    pub duration_seconds: i64,
    pub updated_at_unix_nano: i64,
}

/// Bare form of `builtin_alert_rules` rows (matches SELECT column order)
type BuiltinRow = (String, String, i64, f64, i64, i64);

fn builtin_from_row(r: BuiltinRow) -> BuiltinAlertRule {
    BuiltinAlertRule {
        id: r.0,
        name: r.1,
        enabled: r.2 != 0,
        threshold: r.3,
        duration_seconds: r.4,
        updated_at_unix_nano: r.5,
    }
}

/// Bare form of `notify_channels` rows (matches SELECT column order)
type ChannelRow = (
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
);

const CHANNEL_COLS: &str =
    "id, name, kind, url, secret, app_id, receive_id, receive_id_type, enabled, min_severity";

fn channel_from_row(r: ChannelRow) -> NotifyChannel {
    NotifyChannel {
        id: r.0,
        name: r.1,
        kind: r.2,
        url: r.3,
        secret: r.4,
        app_id: r.5,
        receive_id: r.6,
        receive_id_type: r.7,
        enabled: r.8 != 0,
        min_severity: r.9,
    }
}

/// Bare form of alerts rows
#[allow(clippy::type_complexity)]
type AlertRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    f64,
    f64,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    String,
    String,
);

/// alerts row -> Alert (shared by two queries, avoid writing column order twice)
fn alert_from_row(r: AlertRow) -> Alert {
    Alert {
        id: r.0,
        rule_id: r.1,
        rule_name: r.2,
        node_id: r.3,
        hostname: r.4,
        severity: r.5,
        metric: r.6,
        op: r.7,
        threshold: r.8,
        value: r.9,
        message: r.10,
        started_at_unix_nano: r.11,
        resolved_at_unix_nano: r.12,
        silenced_until_unix_nano: r.13,
        source: r.14,
        source_ref: r.15,
    }
}

#[derive(Clone)]
pub struct AlertsRepo {
    pool: SqlitePool,
}

impl AlertsRepo {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ---------- Rules ----------

    /// List all alert rules.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the database query fails.
    pub async fn list_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        let rows: Vec<RuleRow> = sqlx::query_as(
            r"SELECT id, name, metric, op, threshold, duration_seconds, severity, enabled,
                          created_at_unix_nano, updated_at_unix_nano
                   FROM alert_rules ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| AlertRule {
                id: r.0,
                name: r.1,
                metric: r.2,
                op: r.3,
                threshold: r.4,
                duration_seconds: r.5,
                severity: r.6,
                enabled: r.7 != 0,
                created_at_unix_nano: r.8,
                updated_at_unix_nano: r.9,
            })
            .collect())
    }

    /// List enabled rule definitions for the evaluator.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the underlying query fails.
    pub async fn enabled_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        Ok(self
            .list_rules()
            .await?
            .into_iter()
            .filter(|r| r.enabled)
            .collect())
    }

    /// Total number of user-defined alert rules.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the count query fails.
    pub async fn count_rules(&self) -> anyhow::Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_rules")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    #[allow(clippy::too_many_arguments)]
    /// Insert a new user-defined rule and return its id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails.
    pub async fn create_rule(
        &self,
        name: &str,
        metric: &str,
        op: &str,
        threshold: f64,
        duration_seconds: i64,
        severity: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r"INSERT INTO alert_rules
               (name, metric, op, threshold, duration_seconds, severity, enabled,
                created_at_unix_nano, updated_at_unix_nano)
               VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(name)
        .bind(metric)
        .bind(op)
        .bind(threshold)
        .bind(duration_seconds)
        .bind(severity)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Toggle a rule. Disabling also closes any open alerts for it.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update or alert-resolution query fails.
    pub async fn set_rule_enabled(&self, id: i64, enabled: bool, now: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE alert_rules SET enabled = ?, updated_at_unix_nano = ? WHERE id = ?")
            .bind(i32::from(enabled))
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        // After disabling, evaluation skips this rule; if not handled, its unresolved alerts stay open
        if !enabled {
            self.resolve_open_alerts_of_rule(id, now).await?;
        }
        Ok(())
    }

    /// Look up a rule by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn get_rule(&self, id: i64) -> anyhow::Result<Option<AlertRule>> {
        let row = sqlx::query_as::<_, AlertRuleRow>(
            "SELECT id, name, metric, op, threshold, duration_seconds, severity, enabled, created_at_unix_nano, updated_at_unix_nano FROM alert_rules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(std::convert::Into::into))
    }

    /// Replace rule definition by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn update_rule(&self, id: i64, rule: &AlertRule) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE alert_rules SET name = ?, metric = ?, op = ?, threshold = ?, duration_seconds = ?, severity = ?, enabled = ?, updated_at_unix_nano = ? WHERE id = ?",
        )
        .bind(&rule.name)
        .bind(&rule.metric)
        .bind(&rule.op)
        .bind(rule.threshold)
        .bind(rule.duration_seconds)
        .bind(&rule.severity)
        .bind(i32::from(rule.enabled))
        .bind(rule.updated_at_unix_nano)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Close all unresolved alerts for a rule and clear its evaluation state
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update or delete query fails.
    pub async fn resolve_open_alerts_of_rule(&self, rule_id: i64, now: i64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE rule_id = ? AND resolved_at_unix_nano IS NULL",
        )
        .bind(now)
        .bind(rule_id)
        .execute(&self.pool)
        .await?;
        sqlx::query("DELETE FROM alert_state WHERE rule_id = ?")
            .bind(rule_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a rule. Open alerts for the rule are closed first to avoid orphans.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the alert-resolution or delete query fails.
    pub async fn delete_rule(&self, id: i64, now: i64) -> anyhow::Result<()> {
        // First close unresolved alerts to avoid leaving behind forever-open historical alerts after deleting the rule
        self.resolve_open_alerts_of_rule(id, now).await?;
        sqlx::query("DELETE FROM alert_rules WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- Builtin rules (node online/offline) ----------

    /// List all builtin rules (ordered by id lexicographically, UI display order is stable)
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn list_builtin_rules(&self) -> anyhow::Result<Vec<BuiltinAlertRule>> {
        let rows: Vec<BuiltinRow> = sqlx::query_as(
            "SELECT id, name, enabled, COALESCE(threshold, 0) as threshold,
                    COALESCE(duration_seconds, 300) as duration_seconds, updated_at_unix_nano
             FROM builtin_alert_rules ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(builtin_from_row).collect())
    }

    /// List enabled builtin metric rules (`cpu_high`, `mem_high`, `disk_high`, etc.)
    /// These are returned as `AlertRule` struct for compatibility with `evaluate()`.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn enabled_builtin_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        let rows: Vec<BuiltinRow> = sqlx::query_as(
            "SELECT id, name, enabled, COALESCE(threshold, 0) as threshold,
                    COALESCE(duration_seconds, 300) as duration_seconds, updated_at_unix_nano
             FROM builtin_alert_rules
             WHERE enabled = 1 AND (id = 'cpu_high' OR id = 'mem_high' OR id = 'disk_high')",
        )
        .fetch_all(&self.pool)
        .await?;

        // Convert BuiltinAlertRule to AlertRule for evaluate()
        let rules: Vec<AlertRule> = rows
            .into_iter()
            .map(|r| {
                let (id_str, name, enabled, threshold, duration_seconds, updated_at) =
                    (r.0.clone(), r.1.clone(), r.2 != 0, r.3, r.4, r.5);
                AlertRule {
                    id: 0, // metric rules use 0 as placeholder
                    name,
                    metric: match id_str.as_str() {
                        "cpu_high" => "host.cpu.usage".to_string(),
                        "mem_high" => "host.mem.usage".to_string(),
                        "disk_high" => "host.disk.usage".to_string(),
                        _ => id_str,
                    },
                    op: "gt".to_string(),
                    threshold,
                    duration_seconds,
                    severity: "warning".to_string(),
                    enabled,
                    created_at_unix_nano: updated_at,
                    updated_at_unix_nano: updated_at,
                }
            })
            .collect();
        Ok(rules)
    }

    /// Current enabled state of a single builtin rule; if not exists (migration not run) defaults to true,
    /// so old databases (table not created yet) still send node offline alerts.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn builtin_rule_enabled(&self, id: &str) -> anyhow::Result<bool> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT enabled FROM builtin_alert_rules WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map_or(true, |(e,)| e != 0))
    }

    /// Toggle builtin rule enabled state. Returns new value; if id doesn't exist returns None (UI should handle as 404).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update or alert-resolution query fails.
    pub async fn set_builtin_rule_enabled(
        &self,
        id: &str,
        enabled: bool,
        now: i64,
    ) -> anyhow::Result<Option<bool>> {
        let n = sqlx::query(
            "UPDATE builtin_alert_rules
             SET enabled = ?, updated_at_unix_nano = ?
             WHERE id = ?",
        )
        .bind(i32::from(enabled))
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Ok(None);
        }

        // When disabling a builtin rule type, also close its unresolved alerts to avoid red alerts
        // after disabling. Examples:
        // - node_offline: node offline alerts are instances of this event type;
        // - service_offline: probe alerts (source = probe) all arise from "service unreachable";
        // - container_stopped: container stop alerts (source = container);
        // - cert_expiring / cert_expired: cert alerts split by severity.
        if !enabled {
            let resolve_sql = match id {
                "node_offline" => Some(
                    "UPDATE alerts SET resolved_at_unix_nano = ?
                     WHERE source = 'node_offline' AND resolved_at_unix_nano IS NULL",
                ),
                "service_offline" => Some(
                    "UPDATE alerts SET resolved_at_unix_nano = ?
                     WHERE source = 'probe' AND resolved_at_unix_nano IS NULL",
                ),
                "container_stopped" => Some(
                    "UPDATE alerts SET resolved_at_unix_nano = ?
                     WHERE source = 'container' AND resolved_at_unix_nano IS NULL",
                ),
                "cert_expiring" => Some(
                    "UPDATE alerts SET resolved_at_unix_nano = ?
                     WHERE source = 'cert' AND severity = 'warning' AND resolved_at_unix_nano IS NULL",
                ),
                "cert_expired" => Some(
                    "UPDATE alerts SET resolved_at_unix_nano = ?
                     WHERE source = 'cert' AND severity = 'critical' AND resolved_at_unix_nano IS NULL",
                ),
                _ => None,
            };
            if let Some(sql) = resolve_sql {
                sqlx::query(sql).bind(now).execute(&self.pool).await?;
            }
        }
        Ok(Some(enabled))
    }

    /// Update builtin alert rule settings (threshold, `duration_seconds`, enabled).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update or lookup query fails.
    pub async fn update_builtin_rule(
        &self,
        id: &str,
        threshold: f64,
        duration_seconds: i64,
        enabled: bool,
        now: i64,
    ) -> anyhow::Result<Option<BuiltinAlertRule>> {
        let n = sqlx::query(
            "UPDATE builtin_alert_rules
             SET threshold = ?, duration_seconds = ?, enabled = ?, updated_at_unix_nano = ?
             WHERE id = ?",
        )
        .bind(threshold)
        .bind(duration_seconds)
        .bind(i32::from(enabled))
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if n == 0 {
            return Ok(None);
        }
        let row: Option<BuiltinRow> = sqlx::query_as(
            "SELECT id, name, enabled, COALESCE(threshold, 0), COALESCE(duration_seconds, 300), updated_at_unix_nano
             FROM builtin_alert_rules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(builtin_from_row))
    }

    // ---------- Evaluation state ----------

    /// Load evaluation state for `(rule_id, node_id)`.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn get_state(&self, rule_id: i64, node_id: &str) -> anyhow::Result<EvalState> {
        let row: Option<(Option<i64>, i64, Option<i64>)> = sqlx::query_as(
            "SELECT breaching_since_unix_nano, firing, open_alert_id FROM alert_state WHERE rule_id = ? AND node_id = ?",
        )
        .bind(rule_id)
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some((since, firing, open)) => EvalState {
                breaching_since_unix_nano: since,
                firing: firing != 0,
                open_alert_id: open,
            },
            None => EvalState {
                breaching_since_unix_nano: None,
                firing: false,
                open_alert_id: None,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    /// Upsert evaluation state for `(rule_id, node_id)`.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn upsert_state(
        &self,
        rule_id: i64,
        node_id: &str,
        breaching_since: Option<i64>,
        firing: bool,
        open_alert_id: Option<i64>,
        last_value: Option<f64>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r"INSERT INTO alert_state
               (rule_id, node_id, breaching_since_unix_nano, firing, open_alert_id, last_value)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(rule_id, node_id) DO UPDATE SET
                 breaching_since_unix_nano = excluded.breaching_since_unix_nano,
                 firing = excluded.firing,
                 open_alert_id = excluded.open_alert_id,
                 last_value = excluded.last_value",
        )
        .bind(rule_id)
        .bind(node_id)
        .bind(breaching_since)
        .bind(i32::from(firing))
        .bind(open_alert_id)
        .bind(last_value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------- Alert instances ----------

    #[allow(clippy::too_many_arguments)]
    /// Insert a new alert row for a user-defined rule. Returns the alert id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails.
    pub async fn open_alert(
        &self,
        rule: &AlertRule,
        node_id: &str,
        hostname: &str,
        value: f64,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'rule', '')",
        )
        .bind(rule.id)
        .bind(&rule.name)
        .bind(node_id)
        .bind(hostname)
        .bind(&rule.severity)
        .bind(&rule.metric)
        .bind(&rule.op)
        .bind(rule.threshold)
        .bind(value)
        .bind(message)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Service probe state alert: source marked as probe, `source_ref = probe_id`.
    /// `rule_id` uses 0 (probe alerts don't go through the metric rule table).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_probe_alert(
        &self,
        probe_id: &str,
        rule_name: &str,
        node_id: &str,
        hostname: &str,
        severity: &str,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, ?, ?, ?, 'probe.state', 'eq', 0, 1, ?, ?, 'probe', ?)",
        )
        .bind(rule_name)
        .bind(node_id)
        .bind(hostname)
        .bind(severity)
        .bind(message)
        .bind(now)
        .bind(probe_id)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Close current unresolved alert for a probe, returns number of closed alerts
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn resolve_open_probe_alerts(&self, probe_id: &str, now: i64) -> anyhow::Result<u64> {
        let r = sqlx::query(
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'probe' AND source_ref = ? AND resolved_at_unix_nano IS NULL",
        )
        .bind(now)
        .bind(probe_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    // ---------- Platform self anomalies (source = platform) ----------

    /// Open an alert for platform self anomalies (retention cleanup failures, etc.), `node_id` / hostname empty.
    ///
    /// These alerts should not stack on every failure: if the same `source_ref` already has an unresolved alert,
    /// just return it. See `docs/superpowers/specs/2026-09-19-product-structure-design.md` §8
    /// -- "retention task failure must appear in the todo list".
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn open_platform_alert(
        &self,
        source_ref: &str,
        rule_name: &str,
        severity: &str,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM alerts
             WHERE source = 'platform' AND source_ref = ? AND resolved_at_unix_nano IS NULL
             ORDER BY id DESC LIMIT 1",
        )
        .bind(source_ref)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(id) = existing {
            sqlx::query("UPDATE alerts SET message = ?, severity = ? WHERE id = ?")
                .bind(message)
                .bind(severity)
                .bind(id)
                .execute(&self.pool)
                .await?;
            return Ok(id);
        }

        let r = sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, '', '', ?, 'platform.self', 'eq', 0, 1, ?, ?, 'platform', ?)",
        )
        .bind(rule_name)
        .bind(severity)
        .bind(message)
        .bind(now)
        .bind(source_ref)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Close platform self anomalies of a given type, returns number closed.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn resolve_platform_alerts(&self, source_ref: &str, now: i64) -> anyhow::Result<u64> {
        let r = sqlx::query(
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'platform' AND source_ref = ? AND resolved_at_unix_nano IS NULL",
        )
        .bind(now)
        .bind(source_ref)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    // ---------- Certificate expiry alerts (source = cert) ----------

    /// Open a new certificate expiry alert. `rule_id` uses 0 (not through metric rule table),
    /// `source_ref = {source_id}:{cert path}`, one alert per certificate.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_cert_alert(
        &self,
        source_ref: &str,
        rule_name: &str,
        node_id: &str,
        hostname: &str,
        severity: &str,
        // Threshold = "days before expiry" from source config (alert page directly shows this number)
        threshold_days: f64,
        days_left: f64,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, ?, ?, ?, 'cert.days_left', 'lte', ?, ?, ?, ?, 'cert', ?)",
        )
        .bind(rule_name)
        .bind(node_id)
        .bind(hostname)
        .bind(severity)
        .bind(threshold_days)
        .bind(days_left)
        .bind(message)
        .bind(now)
        .bind(source_ref)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Update severity and message of an existing alert (used when cert goes from "approaching" to "expired")
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn update_alert_message(
        &self,
        id: i64,
        severity: &str,
        value: f64,
        message: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE alerts SET severity = ?, value = ?, message = ? WHERE id = ?")
            .bind(severity)
            .bind(value)
            .bind(message)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Current unresolved cert alerts for a node (used during evaluation to check "has this already been opened")
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn open_cert_alerts_for_node(&self, node_id: &str) -> anyhow::Result<Vec<Alert>> {
        let sql = r"SELECT id, rule_id, rule_name, node_id, hostname, severity, metric, op, threshold,
                            value, message, started_at_unix_nano, resolved_at_unix_nano,
                            silenced_until_unix_nano, source, source_ref
                     FROM alerts
                     WHERE source = 'cert' AND resolved_at_unix_nano IS NULL AND node_id = ?
                     ORDER BY started_at_unix_nano DESC";
        let rows: Vec<AlertRow> = sqlx::query_as(sql)
            .bind(node_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(alert_from_row).collect())
    }

    /// Close unresolved cert alerts for a source (or a specific certificate under a source), returns count closed.
    /// `cert_path` empty means the entire source.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn resolve_open_cert_alerts(
        &self,
        source_id: &str,
        cert_path: Option<&str>,
        now: i64,
    ) -> anyhow::Result<u64> {
        // Uses instr instead of LIKE: cert paths may contain % / _ wildcards
        let sql = if cert_path.is_some() {
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'cert' AND source_ref = ? AND resolved_at_unix_nano IS NULL"
        } else {
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'cert' AND instr(source_ref, ?) = 1 AND resolved_at_unix_nano IS NULL"
        };
        let key = cert_path.map_or_else(|| format!("{source_id}:"), |p| format!("{source_id}:{p}"));
        let r = sqlx::query(sql)
            .bind(now)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    // ---------- Node offline alerts (source = node_offline) ----------

    /// Does this node currently have an unresolved offline alert?
    /// Uses `source_ref = node_id` as idempotency key -- only one offline alert allowed per node at a time.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn open_node_offline_alert_id(&self, node_id: &str) -> anyhow::Result<Option<i64>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM alerts
             WHERE source = 'node_offline' AND source_ref = ? AND resolved_at_unix_nano IS NULL
             ORDER BY id DESC LIMIT 1",
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(id)
    }

    /// Open a "node offline" alert.
    ///
    /// If the same node already has an unresolved alert, **reuse** it and refresh message / `started_at_unix_nano`,
    /// don't stack new rows -- same pattern as `open_platform_alert`, avoid turning node flapping
    /// into noisy history. Returns the written/hit row id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update or insert query fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_node_offline_alert(
        &self,
        node_id: &str,
        hostname: &str,
        severity: &str,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        if let Some(existing) = self.open_node_offline_alert_id(node_id).await? {
            sqlx::query(
                "UPDATE alerts
                 SET message = ?, severity = ?, started_at_unix_nano = ?
                 WHERE id = ?",
            )
            .bind(message)
            .bind(severity)
            .bind(now)
            .bind(existing)
            .execute(&self.pool)
            .await?;
            return Ok(existing);
        }
        let r = sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, 'Node Offline', ?, ?, ?, 'host.online', 'eq', 0, 0, ?, ?, 'node_offline', ?)",
        )
        .bind(node_id)
        .bind(hostname)
        .bind(severity)
        .bind(message)
        .bind(now)
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Close all unresolved offline alerts for a node (called when node resumes reporting).
    /// Returns number closed.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn resolve_node_offline_alerts(
        &self,
        node_id: &str,
        now: i64,
    ) -> anyhow::Result<u64> {
        let r = sqlx::query(
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'node_offline' AND source_ref = ? AND resolved_at_unix_nano IS NULL",
        )
        .bind(now)
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    // ---------- Container start/stop alerts (source = container) ----------

    /// Does this container currently have an unresolved start/stop alert?
    /// Uses `source_ref = container_id` as idempotency key -- only one alert allowed per container at a time.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn open_container_alert_id(&self, container_id: &str) -> anyhow::Result<Option<i64>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM alerts
             WHERE source = 'container' AND source_ref = ? AND resolved_at_unix_nano IS NULL
             ORDER BY id DESC LIMIT 1",
        )
        .bind(container_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(id)
    }

    /// Open a "container stopped" alert.
    ///
    /// If the same container already has an unresolved alert, **reuse** it and refresh message / `started_at_unix_nano`,
    /// don't stack new rows -- same idempotency pattern as `open_node_offline_alert`. When the container
    /// starts again, `resolve_open_container_alerts` closes it.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update or insert query fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_container_alert(
        &self,
        container_id: &str,
        rule_name: &str,
        node_id: &str,
        hostname: &str,
        severity: &str,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        if let Some(existing) = self.open_container_alert_id(container_id).await? {
            sqlx::query(
                "UPDATE alerts
                 SET message = ?, severity = ?, started_at_unix_nano = ?
                 WHERE id = ?",
            )
            .bind(message)
            .bind(severity)
            .bind(now)
            .bind(existing)
            .execute(&self.pool)
            .await?;
            return Ok(existing);
        }
        let r = sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, ?, ?, ?, 'container.state', 'eq', 1, 1, ?, ?, 'container', ?)",
        )
        .bind(rule_name)
        .bind(node_id)
        .bind(hostname)
        .bind(severity)
        .bind(message)
        .bind(now)
        .bind(container_id)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Close current unresolved start/stop alert for a container (called when container starts again), returns count closed.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn resolve_open_container_alerts(
        &self,
        container_id: &str,
        now: i64,
    ) -> anyhow::Result<u64> {
        let r = sqlx::query(
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'container' AND source_ref = ? AND resolved_at_unix_nano IS NULL",
        )
        .bind(now)
        .bind(container_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// Resolve a single alert by id (no-op if already resolved).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn resolve_alert(&self, alert_id: i64, now: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE alerts SET resolved_at_unix_nano = ? WHERE id = ? AND resolved_at_unix_nano IS NULL")
            .bind(now)
            .bind(alert_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Unresolved alerts, newest first
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn open_alerts(&self) -> anyhow::Result<Vec<Alert>> {
        self.query_alerts("WHERE resolved_at_unix_nano IS NULL ORDER BY started_at_unix_nano DESC")
            .await
    }

    /// Resolved historical alerts
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn resolved_alerts(&self, limit: i64) -> anyhow::Result<Vec<Alert>> {
        self.query_alerts(&format!(
            "WHERE resolved_at_unix_nano IS NOT NULL ORDER BY resolved_at_unix_nano DESC LIMIT {}",
            limit.clamp(1, 500)
        ))
        .await
    }

    async fn query_alerts(&self, tail: &str) -> anyhow::Result<Vec<Alert>> {
        let sql = format!(
            r"SELECT id, rule_id, rule_name, node_id, hostname, severity, metric, op, threshold,
                      value, message, started_at_unix_nano, resolved_at_unix_nano,
                      silenced_until_unix_nano, source, source_ref
               FROM alerts {tail}"
        );
        let rows: Vec<AlertRow> = sqlx::query_as(&sql).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(alert_from_row).collect())
    }

    /// Silence an alert until `until_unix_nano` (used for "I'm handling this").
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn silence_alert(&self, id: i64, until_unix_nano: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE alerts SET silenced_until_unix_nano = ? WHERE id = ?")
            .bind(until_unix_nano)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- Notify channels ----------

    /// Insert a new notify channel. Returns the row id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails.
    pub async fn create_channel(&self, ch: &NewChannel<'_>, now: i64) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r"INSERT INTO notify_channels (name, kind, url, secret, app_id, receive_id, receive_id_type, enabled, min_severity, created_at_unix_nano)
               VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(ch.name)
        .bind(ch.kind)
        .bind(ch.url)
        .bind(ch.secret)
        .bind(ch.app_id)
        .bind(ch.receive_id)
        .bind(ch.receive_id_type)
        .bind(ch.min_severity)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    /// Enable / disable a notify channel.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn set_channel_enabled(&self, id: i64, enabled: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE notify_channels SET enabled = ? WHERE id = ?")
            .bind(i32::from(enabled))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a notify channel by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the delete fails.
    pub async fn delete_channel(&self, id: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM notify_channels WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Look up a channel by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn find_channel(&self, id: i64) -> anyhow::Result<Option<NotifyChannel>> {
        let row: Option<ChannelRow> = sqlx::query_as(&format!(
            "SELECT {CHANNEL_COLS} FROM notify_channels WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(channel_from_row))
    }

    /// Full-field channel update (kind cannot be changed: if type changes, required fields change;
    /// delete and recreate is clearer). Returns whether row existed (not exists = caller handles as 404).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn update_channel(
        &self,
        id: i64,
        ch: &NewChannel<'_>,
        enabled: bool,
    ) -> anyhow::Result<bool> {
        let n = sqlx::query(
            "UPDATE notify_channels SET name = ?, kind = ?, url = ?, secret = ?, app_id = ?,
                    receive_id = ?, receive_id_type = ?, enabled = ?, min_severity = ?
             WHERE id = ?",
        )
        .bind(ch.name)
        .bind(ch.kind)
        .bind(ch.url)
        .bind(ch.secret)
        .bind(ch.app_id)
        .bind(ch.receive_id)
        .bind(ch.receive_id_type)
        .bind(i32::from(enabled))
        .bind(ch.min_severity)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n > 0)
    }

    /// List all notify channels.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn list_channels(&self) -> anyhow::Result<Vec<NotifyChannel>> {
        let rows: Vec<ChannelRow> = sqlx::query_as(&format!(
            "SELECT {CHANNEL_COLS} FROM notify_channels ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(channel_from_row).collect())
    }
}

#[cfg(test)]
mod node_offline_tests {
    //! Offline alert tests. Shares `alerts_repo` fields (`rule_id`, source, `source_ref`),
    //! but these are NOT metric rule alerts -- so `rule_id = 0`, borrowing from `rule_id NOT NULL`
    //! constraint a special value that is never referenced.
    //!
    //! Key invariants:
    //!   1. Only one unresolved offline alert per node at a time (repeated opens don't stack rows);
    //!   2. resolve only closes rows with `source = node_offline`, won't accidentally affect other alerts;
    //!   3. After resolve, open again creates a new row (not reviving the old one).

    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        crate::migrations::run(&pool).await.expect("migrations");
        pool
    }

    #[tokio::test]
    async fn open_node_offline_alert_is_idempotent_per_node() {
        let pool = fresh_pool().await;
        let repo = AlertsRepo::new(pool.clone());

        let id_a = repo
            .open_node_offline_alert("n1", "host-a", "critical", "first", 100)
            .await
            .unwrap();
        let id_b = repo
            .open_node_offline_alert("n1", "host-a", "critical", "second", 200)
            .await
            .unwrap();
        assert_eq!(
            id_a, id_b,
            "Second open on same node should hit existing alert, not insert a new row"
        );

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE source = 'node_offline'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            count, 1,
            "alerts table should have only one unresolved offline alert"
        );

        // Open a second node's offline alert: should insert a new row
        let id_c = repo
            .open_node_offline_alert("n2", "host-b", "critical", "third", 300)
            .await
            .unwrap();
        assert_ne!(id_a, id_c);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE source = 'node_offline'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn resolve_node_offline_alerts_only_touches_node_offline_source() {
        let pool = fresh_pool().await;
        let repo = AlertsRepo::new(pool.clone());

        // One node_offline + one probe (manually inserted) coexist, resolve should not accidentally affect probe
        let off = repo
            .open_node_offline_alert("n1", "host-a", "critical", "offline", 100)
            .await
            .unwrap();
        sqlx::query(
            r"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, 'probe probe', 'n1', 'host-a', 'warning', 'probe.state', 'eq', 0, 1,
                       'should not be touched', 100, 'probe', 'probe-1')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let n = repo.resolve_node_offline_alerts("n1", 500).await.unwrap();
        assert_eq!(n, 1, "should only close one node_offline");

        // node_offline should be closed
        let resolved: Option<i64> =
            sqlx::query_scalar("SELECT resolved_at_unix_nano FROM alerts WHERE id = ?")
                .bind(off)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(resolved, Some(500));

        // probe should remain unresolved
        let probe_resolved: Option<i64> =
            sqlx::query_scalar("SELECT resolved_at_unix_nano FROM alerts WHERE source = 'probe'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(probe_resolved, None);
    }

    #[tokio::test]
    async fn reopen_after_resolve_inserts_new_row() {
        let pool = fresh_pool().await;
        let repo = AlertsRepo::new(pool.clone());

        let first = repo
            .open_node_offline_alert("n1", "host-a", "critical", "first", 100)
            .await
            .unwrap();
        repo.resolve_node_offline_alerts("n1", 200).await.unwrap();
        let second = repo
            .open_node_offline_alert("n1", "host-a", "critical", "second", 300)
            .await
            .unwrap();
        assert_ne!(
            first, second,
            "open after resolve should insert new row, not revive old"
        );

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE source = 'node_offline'")
                .fetch_one(&pool)
                .await
                .unwrap();
        // One resolved + one new = two
        assert_eq!(count, 2);
    }
}

#[cfg(test)]
mod container_builtin_channel_tests {
    //! Container start/stop alerts, builtin alert toggle cleanup semantics, channel full-field update.
    //!
    //! Key invariants:
    //!   1. Migration 019 seeds all new builtin alerts, enabled by default;
    //!   2. Only one unresolved start/stop alert per container; resolve only closes its own;
    //!   3. Disabling a builtin toggle closes its corresponding unresolved alerts (`service_offline` -> probe,
    //!      `container_stopped` -> container);
    //!   4. `update_channel` overwrites all fields, unknown id returns false.

    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        crate::migrations::run(&pool).await.expect("migrations");
        pool
    }

    async fn open_alert_count(pool: &sqlx::SqlitePool, source: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM alerts WHERE source = ? AND resolved_at_unix_nano IS NULL",
        )
        .bind(source)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn migration_019_seeds_new_builtin_alerts() {
        let repo = AlertsRepo::new(fresh_pool().await);
        let list = repo.list_builtin_rules().await.unwrap();
        let ids: Vec<&str> = list.iter().map(|r| r.id.as_str()).collect();
        for want in [
            "node_offline",
            "node_online",
            "service_offline",
            "service_online",
            "container_started",
            "container_stopped",
            "cert_expiring",
            "cert_expired",
        ] {
            assert!(ids.contains(&want), "missing builtin alert {want}: {ids:?}");
        }
        assert!(
            list.iter().all(|r| r.enabled),
            "new builtin alerts should be enabled by default"
        );
    }

    #[tokio::test]
    async fn container_alert_is_idempotent_and_resolve_only_touches_its_own() {
        let pool = fresh_pool().await;
        let repo = AlertsRepo::new(pool.clone());

        let a = repo
            .open_container_alert(
                "cid-1",
                "Container Stopped",
                "n1",
                "host-a",
                "warning",
                "stopped",
                100,
            )
            .await
            .unwrap();
        let b = repo
            .open_container_alert(
                "cid-1",
                "Container Stopped",
                "n1",
                "host-a",
                "warning",
                "stopped",
                200,
            )
            .await
            .unwrap();
        assert_eq!(
            a, b,
            "second open on same container should hit existing row"
        );
        assert_eq!(open_alert_count(&pool, "container").await, 1);

        // Another container's alert should not be accidentally closed
        repo.open_container_alert(
            "cid-2",
            "Container Stopped",
            "n1",
            "host-a",
            "warning",
            "stopped",
            200,
        )
        .await
        .unwrap();
        assert_eq!(
            repo.resolve_open_container_alerts("cid-1", 300)
                .await
                .unwrap(),
            1
        );
        assert_eq!(open_alert_count(&pool, "container").await, 1);
        assert_eq!(
            repo.resolve_open_container_alerts("cid-1", 400)
                .await
                .unwrap(),
            0,
            "already-closed should not be closed again"
        );

        // open after resolve creates new row (history preserved)
        let c = repo
            .open_container_alert(
                "cid-1",
                "Container Stopped",
                "n1",
                "host-a",
                "warning",
                "stopped",
                500,
            )
            .await
            .unwrap();
        assert_ne!(a, c);
    }

    #[tokio::test]
    async fn disabling_builtin_closes_its_open_alerts() {
        let pool = fresh_pool().await;
        let repo = AlertsRepo::new(pool.clone());
        repo.open_probe_alert(
            "p1",
            "Service web http",
            "n1",
            "host-a",
            "critical",
            "down",
            100,
        )
        .await
        .unwrap();
        repo.open_container_alert(
            "cid-1",
            "Container Stopped",
            "n1",
            "host-a",
            "warning",
            "stopped",
            100,
        )
        .await
        .unwrap();

        // Disable "container stopped": only clears container's
        assert_eq!(
            repo.set_builtin_rule_enabled("container_stopped", false, 200)
                .await
                .unwrap(),
            Some(false)
        );
        assert_eq!(open_alert_count(&pool, "container").await, 0);
        assert_eq!(open_alert_count(&pool, "probe").await, 1);

        // Disable "service offline": clears probe's
        assert_eq!(
            repo.set_builtin_rule_enabled("service_offline", false, 300)
                .await
                .unwrap(),
            Some(false)
        );
        assert_eq!(open_alert_count(&pool, "probe").await, 0);

        // Unknown id -> None (caller handles as 404)
        assert_eq!(
            repo.set_builtin_rule_enabled("nope", false, 400)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn update_channel_overwrites_every_field_and_reports_missing_row() {
        let repo = AlertsRepo::new(fresh_pool().await);
        let now = 1;
        let id = repo
            .create_channel(
                &NewChannel {
                    name: "a",
                    kind: "webhook",
                    url: "http://x",
                    secret: "tok",
                    app_id: "",
                    receive_id: "",
                    receive_id_type: "chat_id",
                    min_severity: "warning",
                },
                now,
            )
            .await
            .unwrap();

        let update = NewChannel {
            name: "b",
            kind: "webhook",
            url: "http://y",
            secret: "tok2",
            app_id: "",
            receive_id: "",
            receive_id_type: "chat_id",
            min_severity: "critical",
        };
        assert!(repo.update_channel(id, &update, false).await.unwrap());

        let got = repo.find_channel(id).await.unwrap().unwrap();
        assert_eq!(got.name, "b");
        assert_eq!(got.url, "http://y");
        assert_eq!(got.secret, "tok2");
        assert_eq!(got.min_severity, "critical");
        assert!(!got.enabled);

        assert!(!repo.update_channel(9999, &update, true).await.unwrap());
        assert!(repo.find_channel(9999).await.unwrap().is_none());
    }
}
