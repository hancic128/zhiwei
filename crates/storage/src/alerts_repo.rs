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
    /// rule = 指标规则告警；probe = 服务探针状态告警；cert = 证书到期告警
    pub source: String,
    /// source = probe 时是 probe_id；source = cert 时是 `{source_id}:{证书路径}`
    pub source_ref: String,
}

/// 单个 (规则, 节点) 的评估状态
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
    /// `feishu` / `slack` / `webhook`
    pub kind: String,
    /// Slack / 通用 webhook 的投递地址；飞书不用（地址由 receive_id 决定）
    pub url: String,
    /// 按类型复用：飞书 = App Secret，通用 webhook = 投递 Token（Bearer）
    pub secret: String,
    /// 飞书应用 App ID
    pub app_id: String,
    /// 飞书接收 ID（群 chat_id / 用户 open_id 等）
    pub receive_id: String,
    /// 飞书的 receive_id_type：chat_id / open_id / user_id / union_id / email
    pub receive_id_type: String,
    pub enabled: bool,
    pub min_severity: String,
}

/// 新建渠道的入参。字段多、且大多只在某一种渠道类型下才有值，
/// 聚成一个结构比一长串位置参数好读（调用点也不用数顺序）。
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

/// alert_rules 行的裸形态（与 SELECT 列顺序一致）
type RuleRow = (i64, String, String, String, f64, i64, String, i64, i64, i64);

/// alerts 行的裸形态
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

/// alerts 行 → Alert（两处查询共用，避免列顺序写两遍）
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
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ---------- 规则 ----------

    pub async fn list_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        let rows: Vec<RuleRow> = sqlx::query_as(
            r#"SELECT id, name, metric, op, threshold, duration_seconds, severity, enabled,
                          created_at_unix_nano, updated_at_unix_nano
                   FROM alert_rules ORDER BY id"#,
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

    pub async fn enabled_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        Ok(self
            .list_rules()
            .await?
            .into_iter()
            .filter(|r| r.enabled)
            .collect())
    }

    pub async fn count_rules(&self) -> anyhow::Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alert_rules")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    #[allow(clippy::too_many_arguments)]
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
            r#"INSERT INTO alert_rules
               (name, metric, op, threshold, duration_seconds, severity, enabled,
                created_at_unix_nano, updated_at_unix_nano)
               VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)"#,
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

    pub async fn set_rule_enabled(&self, id: i64, enabled: bool, now: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE alert_rules SET enabled = ?, updated_at_unix_nano = ? WHERE id = ?")
            .bind(if enabled { 1 } else { 0 })
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        // 停用后评估会跳过该规则，若不处理，它的未解决告警会一直挂着
        if !enabled {
            self.resolve_open_alerts_of_rule(id, now).await?;
        }
        Ok(())
    }

    /// 关闭某条规则名下所有未解决的告警，并清空其评估状态
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

    pub async fn delete_rule(&self, id: i64, now: i64) -> anyhow::Result<()> {
        // 先把未解决告警收尾，避免删除规则后留下一堆永远开着的历史告警
        self.resolve_open_alerts_of_rule(id, now).await?;
        sqlx::query("DELETE FROM alert_rules WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- 评估状态 ----------

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
            r#"INSERT INTO alert_state
               (rule_id, node_id, breaching_since_unix_nano, firing, open_alert_id, last_value)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(rule_id, node_id) DO UPDATE SET
                 breaching_since_unix_nano = excluded.breaching_since_unix_nano,
                 firing = excluded.firing,
                 open_alert_id = excluded.open_alert_id,
                 last_value = excluded.last_value"#,
        )
        .bind(rule_id)
        .bind(node_id)
        .bind(breaching_since)
        .bind(if firing { 1 } else { 0 })
        .bind(open_alert_id)
        .bind(last_value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ---------- 告警实例 ----------

    #[allow(clippy::too_many_arguments)]
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
            r#"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'rule', '')"#,
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

    /// 服务探针状态告警：来源标记为 probe，`source_ref = probe_id`。
    /// rule_id 用 0（探针告警不走指标规则表）。
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
            r#"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, ?, ?, ?, 'probe.state', 'eq', 0, 1, ?, ?, 'probe', ?)"#,
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

    /// 关闭某个探针当前未解决的告警，返回关闭条数
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

    // ---------- 平台自身异常（source = platform） ----------

    /// 开一条平台自身异常的告警（留存清理失败等），node_id / hostname 为空。
    ///
    /// 这类告警不该每次失败都堆一条：同 `source_ref` 已有未解决的就直接返回它。
    /// 见 `docs/superpowers/specs/2026-09-19-product-structure-design.md` §8
    /// ——「留存任务失败必须出现在待办里」。
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
            r#"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, '', '', ?, 'platform.self', 'eq', 0, 1, ?, ?, 'platform', ?)"#,
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

    /// 关闭某类平台自身异常，返回关闭条数。
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

    // ---------- 证书到期告警（source = cert） ----------

    /// 新开一条证书到期告警。rule_id 用 0（不走指标规则表），
    /// `source_ref = {source_id}:{证书路径}`，一张证书一条。
    #[allow(clippy::too_many_arguments)]
    pub async fn open_cert_alert(
        &self,
        source_ref: &str,
        rule_name: &str,
        node_id: &str,
        hostname: &str,
        severity: &str,
        // 判定阈值 = 该来源配置的「到期前 N 天」（告警页直接显示这个数）
        threshold_days: f64,
        days_left: f64,
        message: &str,
        now: i64,
    ) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r#"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, ?, ?, ?, ?, 'cert.days_left', 'lte', ?, ?, ?, ?, 'cert', ?)"#,
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

    /// 更新已开告警的严重度与文案（证书从「临期」走到「已过期」时用）
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

    /// 某节点当前未解决的证书告警（评估时用来判断「是不是已经开过了」）
    pub async fn open_cert_alerts_for_node(&self, node_id: &str) -> anyhow::Result<Vec<Alert>> {
        let sql = r#"SELECT id, rule_id, rule_name, node_id, hostname, severity, metric, op, threshold,
                            value, message, started_at_unix_nano, resolved_at_unix_nano,
                            silenced_until_unix_nano, source, source_ref
                     FROM alerts
                     WHERE source = 'cert' AND resolved_at_unix_nano IS NULL AND node_id = ?
                     ORDER BY started_at_unix_nano DESC"#;
        let rows: Vec<AlertRow> = sqlx::query_as(sql)
            .bind(node_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(alert_from_row).collect())
    }

    /// 关闭某条来源（或整条来源下某张证书）的未解决证书告警，返回关闭条数。
    /// `cert_path` 为空表示整条来源。
    pub async fn resolve_open_cert_alerts(
        &self,
        source_id: &str,
        cert_path: Option<&str>,
        now: i64,
    ) -> anyhow::Result<u64> {
        // 用 instr 而不是 LIKE：证书路径里可能有 % / _ 这类通配字符
        let sql = if cert_path.is_some() {
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'cert' AND source_ref = ? AND resolved_at_unix_nano IS NULL"
        } else {
            "UPDATE alerts SET resolved_at_unix_nano = ?
             WHERE source = 'cert' AND instr(source_ref, ?) = 1 AND resolved_at_unix_nano IS NULL"
        };
        let key = match cert_path {
            Some(p) => format!("{source_id}:{p}"),
            None => format!("{source_id}:"),
        };
        let r = sqlx::query(sql)
            .bind(now)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    // ---------- 节点离线告警（source = node_offline） ----------

    /// 这台节点当前是否还有未解决的离线告警？
    /// 用 `source_ref = node_id` 做幂等键——一个节点同时只允许有一条离线告警开着。
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

    /// 开一条「节点离线」告警。
    ///
    /// 同一节点已有未解决的告警就**复用**并刷新 message / started_at_unix_nano，
    /// 不堆新行——和 `open_platform_alert` 同一套思路，节点反复进出时别把历史刷成
    /// 噪音。返回写入 / 命中行的 id。
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
            r#"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, '节点离线', ?, ?, ?, 'host.online', 'eq', 0, 0, ?, ?, 'node_offline', ?)"#,
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

    /// 关闭某节点所有未解决的离线告警（节点恢复上报时调用）。
    /// 返回关闭条数。
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

    pub async fn resolve_alert(&self, alert_id: i64, now: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE alerts SET resolved_at_unix_nano = ? WHERE id = ? AND resolved_at_unix_nano IS NULL")
            .bind(now)
            .bind(alert_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 未解决的告警，最新的在前
    pub async fn open_alerts(&self) -> anyhow::Result<Vec<Alert>> {
        self.query_alerts("WHERE resolved_at_unix_nano IS NULL ORDER BY started_at_unix_nano DESC")
            .await
    }

    /// 已解决的历史告警
    pub async fn resolved_alerts(&self, limit: i64) -> anyhow::Result<Vec<Alert>> {
        self.query_alerts(&format!(
            "WHERE resolved_at_unix_nano IS NOT NULL ORDER BY resolved_at_unix_nano DESC LIMIT {}",
            limit.clamp(1, 500)
        ))
        .await
    }

    async fn query_alerts(&self, tail: &str) -> anyhow::Result<Vec<Alert>> {
        let sql = format!(
            r#"SELECT id, rule_id, rule_name, node_id, hostname, severity, metric, op, threshold,
                      value, message, started_at_unix_nano, resolved_at_unix_nano,
                      silenced_until_unix_nano, source, source_ref
               FROM alerts {tail}"#
        );
        let rows: Vec<AlertRow> = sqlx::query_as(&sql).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(alert_from_row).collect())
    }

    pub async fn silence_alert(&self, id: i64, until_unix_nano: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE alerts SET silenced_until_unix_nano = ? WHERE id = ?")
            .bind(until_unix_nano)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- 通知渠道 ----------

    pub async fn create_channel(&self, ch: &NewChannel<'_>, now: i64) -> anyhow::Result<i64> {
        let r = sqlx::query(
            r#"INSERT INTO notify_channels (name, kind, url, secret, app_id, receive_id, receive_id_type, enabled, min_severity, created_at_unix_nano)
               VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)"#,
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

    pub async fn set_channel_enabled(&self, id: i64, enabled: bool) -> anyhow::Result<()> {
        sqlx::query("UPDATE notify_channels SET enabled = ? WHERE id = ?")
            .bind(if enabled { 1 } else { 0 })
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_channel(&self, id: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM notify_channels WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_channels(&self) -> anyhow::Result<Vec<NotifyChannel>> {
        let rows: Vec<(i64, String, String, String, String, String, String, String, i64, String)> =
            sqlx::query_as(
                "SELECT id, name, kind, url, secret, app_id, receive_id, receive_id_type, enabled, min_severity \
                 FROM notify_channels ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| NotifyChannel {
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
            })
            .collect())
    }
}

#[cfg(test)]
mod node_offline_tests {
    //! 离线告警专用测试。共用 alerts_repo 的字段（rule_id、source、source_ref），
    //! 但**不是**指标规则告警——所以 `rule_id = 0`，从 `rule_id NOT NULL` 约束里借
    //! 一个永远不被引用的特殊值。
    //!
    //! 关键不变量：
    //!   1. 同一节点同时只能有一条未解决的离线告警（重复 open 不堆行）；
    //!   2. resolve 只关 `source = node_offline` 的行，不会误伤其它告警；
    //!   3. resolve 后再次 open 会落新行（而不是去更新老行）。

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
            "同节点第二次 open 应该命中既有告警而不是新插一行"
        );

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE source = 'node_offline'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "alerts 表里只能有一条未解决的离线告警");

        // 再开第二个节点的离线告警：应该新插一行
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

        // 一条 node_offline + 一条 probe（人工插的行）共存，resolve 不该误伤 probe
        let off = repo
            .open_node_offline_alert("n1", "host-a", "critical", "offline", 100)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO alerts
               (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                message, started_at_unix_nano, source, source_ref)
               VALUES (0, 'probe probe', 'n1', 'host-a', 'warning', 'probe.state', 'eq', 0, 1,
                       'should not be touched', 100, 'probe', 'probe-1')"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let n = repo.resolve_node_offline_alerts("n1", 500).await.unwrap();
        assert_eq!(n, 1, "应该只关掉一条 node_offline");

        // node_offline 那条应已关闭
        let resolved: Option<i64> =
            sqlx::query_scalar("SELECT resolved_at_unix_nano FROM alerts WHERE id = ?")
                .bind(off)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(resolved, Some(500));

        // probe 那条应保持未解决
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
        assert_ne!(first, second, "resolve 后再 open 应该插新行而不是复活旧的");

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE source = 'node_offline'")
                .fetch_one(&pool)
                .await
                .unwrap();
        // 一条已恢复 + 一条新开 = 两条
        assert_eq!(count, 2);
    }
}
