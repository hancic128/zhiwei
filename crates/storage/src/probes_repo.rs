//! 服务健康度存储：服务 / 探针 / 探针状态机 / 探针结果时序。
//!
//! 结构：services 1:N probes 1:1 probe_state（当前状态机）
//!                             1:N probe_results（历史明细，滚动保留 7 天）
//!
//! 探针由节点侧执行（`location = node`），monitor 只负责下发配置、收敛状态、
//! 触发告警。状态机规则见 [`ProbesRepo::record_result`]。

use sqlx::SqlitePool;

/// 状态常量：ok / degraded / down
pub const STATE_OK: &str = "ok";
pub const STATE_DEGRADED: &str = "degraded";
pub const STATE_DOWN: &str = "down";

#[derive(Debug, Clone, serde::Serialize)]
pub struct Service {
    pub id: String,
    pub name: String,
    pub description: String,
    pub group_name: String,
    pub tier: i64,
    pub enabled: bool,
    pub created_at_unix_nano: i64,
    pub updated_at_unix_nano: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Probe {
    pub id: String,
    pub service_id: String,
    pub service_name: String,
    pub name: String,
    pub kind: String,
    pub target_json: String,
    pub expect_json: String,
    pub interval_seconds: i64,
    pub timeout_ms: i64,
    pub failure_threshold: i64,
    pub node_id: Option<String>,
    pub node_hostname: Option<String>,
    pub location: String,
    pub enabled: bool,
    pub created_at_unix_nano: i64,
    pub updated_at_unix_nano: i64,
}

/// probe_state 行（探针当前状态）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeState {
    pub probe_id: String,
    pub state: String,
    pub consecutive_failures: i64,
    pub last_change_at_unix_nano: i64,
    pub last_check_at_unix_nano: i64,
    pub last_latency_ms: Option<f64>,
    pub last_error: String,
}

impl ProbeState {
    /// 从未检查过的探针：对外呈现为 unknown，内部按 ok 起算，
    /// 这样首次失败会走 ok → degraded 的正常收敛路径。
    fn unknown(probe_id: &str, now: i64) -> Self {
        Self {
            probe_id: probe_id.to_string(),
            state: "unknown".to_string(),
            consecutive_failures: 0,
            last_change_at_unix_nano: now,
            last_check_at_unix_nano: 0,
            last_latency_ms: None,
            last_error: String::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeResult {
    pub ts_unix_nano: i64,
    pub state: String,
    pub latency_ms: Option<f64>,
    pub status_code: Option<i64>,
    pub error: String,
}

/// 探针 + 当前状态（控制台服务页用）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeWithState {
    #[serde(flatten)]
    pub probe: Probe,
    pub state: ProbeState,
}

/// 服务 + 探针 + 服务健康汇总
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceWithProbes {
    #[serde(flatten)]
    pub service: Service,
    /// 服务健康：全部探针 ok 才 ok；任一 down 即 down；否则 degraded；无探针 unknown
    pub health: String,
    pub probes: Vec<ProbeWithState>,
}

/// 一次结果入库后的状态机输出（告警引擎据此决定开/关告警）
#[derive(Debug, Clone)]
pub struct StateTransition {
    pub probe_id: String,
    pub previous_state: String,
    pub new_state: String,
    pub consecutive_failures: i64,
    pub last_error: String,
    /// 是否发生了状态变化
    pub changed: bool,
}

/// 新建探针的入参
#[derive(Debug, Clone, Default)]
pub struct ProbeInput {
    pub service_id: String,
    pub name: String,
    pub kind: String,
    pub target_json: String,
    pub expect_json: String,
    pub interval_seconds: i64,
    pub timeout_ms: i64,
    pub failure_threshold: i64,
    pub node_id: Option<String>,
    pub location: String,
    pub enabled: bool,
}

/// 局部修改探针：None 表示不改
#[derive(Debug, Clone, Default)]
pub struct ProbePatch {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub target_json: Option<String>,
    pub expect_json: Option<String>,
    pub interval_seconds: Option<i64>,
    pub timeout_ms: Option<i64>,
    pub failure_threshold: Option<i64>,
    pub node_id: Option<Option<String>>,
    pub enabled: Option<bool>,
}

/// 局部修改服务
#[derive(Debug, Clone, Default)]
pub struct ServicePatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub group_name: Option<String>,
    pub tier: Option<i64>,
    pub enabled: Option<bool>,
}

type ServiceRow = (String, String, String, String, i64, i64, i64, i64);

#[allow(clippy::type_complexity)]
type ProbeRow = (
    String,         // p.id
    String,         // p.service_id
    String,         // s.name
    String,         // p.name
    String,         // p.kind
    String,         // p.target_json
    String,         // p.expect_json
    i64,            // p.interval_seconds
    i64,            // p.timeout_ms
    i64,            // p.failure_threshold
    Option<String>, // p.node_id
    String,         // p.location
    i64,            // p.enabled
    i64,            // p.created_at_unix_nano
    i64,            // p.updated_at_unix_nano
    Option<String>, // n.hostname
);

type StateRow = (String, String, i64, i64, i64, Option<f64>, String);

type ResultRow = (i64, String, Option<f64>, Option<i64>, String);

fn service_from_row(r: ServiceRow) -> Service {
    Service {
        id: r.0,
        name: r.1,
        description: r.2,
        group_name: r.3,
        tier: r.4,
        enabled: r.5 != 0,
        created_at_unix_nano: r.6,
        updated_at_unix_nano: r.7,
    }
}

fn state_from_row(r: StateRow) -> ProbeState {
    ProbeState {
        probe_id: r.0,
        state: r.1,
        consecutive_failures: r.2,
        last_change_at_unix_nano: r.3,
        last_check_at_unix_nano: r.4,
        last_latency_ms: r.5,
        last_error: r.6,
    }
}

fn probe_from_row(r: ProbeRow) -> Probe {
    Probe {
        id: r.0,
        service_id: r.1,
        service_name: r.2,
        name: r.3,
        kind: r.4,
        target_json: r.5,
        expect_json: r.6,
        interval_seconds: r.7,
        timeout_ms: r.8,
        failure_threshold: r.9,
        node_id: r.10,
        location: r.11,
        enabled: r.12 != 0,
        created_at_unix_nano: r.13,
        updated_at_unix_nano: r.14,
        node_hostname: r.15,
    }
}

const SERVICE_COLS: &str =
    "id, name, description, group_name, tier, enabled, created_at_unix_nano, updated_at_unix_nano";

const STATE_COLS: &str = "probe_id, state, consecutive_failures, last_change_at_unix_nano, \
     last_check_at_unix_nano, last_latency_ms, last_error";

const PROBE_COLS: &str =
    "p.id, p.service_id, s.name, p.name, p.kind, p.target_json, p.expect_json, \
     p.interval_seconds, p.timeout_ms, p.failure_threshold, p.node_id, p.location, p.enabled, \
     p.created_at_unix_nano, p.updated_at_unix_nano, n.hostname";

/// 最差状态聚合（服务健康 = 最差探针状态）
pub fn worst_state(states: &[String]) -> String {
    if states.is_empty() {
        return "unknown".into();
    }
    if states.iter().any(|s| s == STATE_DOWN) {
        return STATE_DOWN.into();
    }
    if states.iter().any(|s| s == STATE_DEGRADED) {
        return STATE_DEGRADED.into();
    }
    if states.iter().any(|s| s == "unknown") {
        return "unknown".into();
    }
    STATE_OK.into()
}

#[derive(Clone)]
pub struct ProbesRepo {
    pool: SqlitePool,
}

impl ProbesRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ---------- 服务 ----------

    pub async fn list_services(&self) -> anyhow::Result<Vec<Service>> {
        let rows: Vec<ServiceRow> = sqlx::query_as(&format!(
            "SELECT {SERVICE_COLS} FROM services ORDER BY name"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(service_from_row).collect())
    }

    pub async fn find_service(&self, id: &str) -> anyhow::Result<Option<Service>> {
        let row: Option<ServiceRow> =
            sqlx::query_as(&format!("SELECT {SERVICE_COLS} FROM services WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(service_from_row))
    }

    pub async fn create_service(
        &self,
        name: &str,
        description: &str,
        group_name: &str,
        tier: i64,
        now: i64,
    ) -> anyhow::Result<Service> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO services (id, name, description, group_name, tier, enabled, created_at_unix_nano, updated_at_unix_nano)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(description)
        .bind(group_name)
        .bind(tier)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.find_service(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("service 创建后读不到"))
    }

    pub async fn update_service(
        &self,
        id: &str,
        patch: &ServicePatch,
        now: i64,
    ) -> anyhow::Result<()> {
        let Some(mut svc) = self.find_service(id).await? else {
            anyhow::bail!("service 不存在");
        };
        if let Some(v) = &patch.name {
            svc.name = v.clone();
        }
        if let Some(v) = &patch.description {
            svc.description = v.clone();
        }
        if let Some(v) = &patch.group_name {
            svc.group_name = v.clone();
        }
        if let Some(v) = patch.tier {
            svc.tier = v;
        }
        if let Some(v) = patch.enabled {
            svc.enabled = v;
        }
        sqlx::query(
            "UPDATE services SET name = ?, description = ?, group_name = ?, tier = ?, enabled = ?,
                    updated_at_unix_nano = ? WHERE id = ?",
        )
        .bind(&svc.name)
        .bind(&svc.description)
        .bind(&svc.group_name)
        .bind(svc.tier)
        .bind(i64::from(svc.enabled))
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删除服务连带其探针、状态与结果明细
    pub async fn delete_service(&self, id: &str) -> anyhow::Result<()> {
        let probe_ids: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM probes WHERE service_id = ?")
                .bind(id)
                .fetch_all(&self.pool)
                .await?;
        for (pid,) in &probe_ids {
            self.delete_probe(pid).await?;
        }
        sqlx::query("DELETE FROM services WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- 探针 ----------

    pub async fn list_probes(&self) -> anyhow::Result<Vec<Probe>> {
        let rows: Vec<ProbeRow> = sqlx::query_as(&format!(
            "SELECT {PROBE_COLS} FROM probes p
             JOIN services s ON s.id = p.service_id
             LEFT JOIN nodes n ON n.id = p.node_id
             ORDER BY s.name, p.name"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(probe_from_row).collect())
    }

    /// 某节点要执行的探针：启用 + 归属该节点（或未指定节点）
    pub async fn probes_for_node(&self, node_id: &str) -> anyhow::Result<Vec<Probe>> {
        let rows: Vec<ProbeRow> = sqlx::query_as(&format!(
            "SELECT {PROBE_COLS} FROM probes p
             JOIN services s ON s.id = p.service_id
             LEFT JOIN nodes n ON n.id = p.node_id
             WHERE p.enabled = 1 AND s.enabled = 1 AND p.location = 'node'
               AND (p.node_id IS NULL OR p.node_id = ?)
             ORDER BY p.name"
        ))
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(probe_from_row).collect())
    }

    pub async fn find_probe(&self, id: &str) -> anyhow::Result<Option<Probe>> {
        let row: Option<ProbeRow> = sqlx::query_as(&format!(
            "SELECT {PROBE_COLS} FROM probes p
             JOIN services s ON s.id = p.service_id
             LEFT JOIN nodes n ON n.id = p.node_id
             WHERE p.id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(probe_from_row))
    }

    pub async fn create_probe(&self, input: &ProbeInput, now: i64) -> anyhow::Result<Probe> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO probes (id, service_id, name, kind, target_json, expect_json,
                                 interval_seconds, timeout_ms, failure_threshold, node_id, location,
                                 enabled, created_at_unix_nano, updated_at_unix_nano)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&input.service_id)
        .bind(&input.name)
        .bind(&input.kind)
        .bind(&input.target_json)
        .bind(&input.expect_json)
        .bind(input.interval_seconds)
        .bind(input.timeout_ms)
        .bind(input.failure_threshold)
        .bind(input.node_id.as_deref())
        .bind(&input.location)
        .bind(i64::from(input.enabled))
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.find_probe(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("probe 创建后读不到"))
    }

    pub async fn update_probe(&self, id: &str, patch: &ProbePatch, now: i64) -> anyhow::Result<()> {
        let Some(mut p) = self.find_probe(id).await? else {
            anyhow::bail!("probe 不存在");
        };
        if let Some(v) = &patch.name {
            p.name = v.clone();
        }
        if let Some(v) = &patch.kind {
            p.kind = v.clone();
        }
        if let Some(v) = &patch.target_json {
            p.target_json = v.clone();
        }
        if let Some(v) = &patch.expect_json {
            p.expect_json = v.clone();
        }
        if let Some(v) = patch.interval_seconds {
            p.interval_seconds = v;
        }
        if let Some(v) = patch.timeout_ms {
            p.timeout_ms = v;
        }
        if let Some(v) = patch.failure_threshold {
            p.failure_threshold = v;
        }
        if let Some(v) = &patch.node_id {
            p.node_id = v.clone();
        }
        if let Some(v) = patch.enabled {
            p.enabled = v;
        }
        sqlx::query(
            "UPDATE probes SET name = ?, kind = ?, target_json = ?, expect_json = ?,
                    interval_seconds = ?, timeout_ms = ?, failure_threshold = ?, node_id = ?,
                    enabled = ?, updated_at_unix_nano = ? WHERE id = ?",
        )
        .bind(&p.name)
        .bind(&p.kind)
        .bind(&p.target_json)
        .bind(&p.expect_json)
        .bind(p.interval_seconds)
        .bind(p.timeout_ms)
        .bind(p.failure_threshold)
        .bind(p.node_id.as_deref())
        .bind(i64::from(p.enabled))
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_probe(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM probe_results WHERE probe_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM probe_state WHERE probe_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM probes WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---------- 状态机 ----------

    pub async fn get_state(&self, probe_id: &str) -> anyhow::Result<Option<ProbeState>> {
        let row: Option<StateRow> = sqlx::query_as(&format!(
            "SELECT {STATE_COLS} FROM probe_state WHERE probe_id = ?"
        ))
        .bind(probe_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(state_from_row))
    }

    /// 落一条结果并推进状态机，返回本次状态变化（供告警引擎使用）。
    ///
    /// 规则：ok 立刻回 ok（清零失败计数）；非 ok 累加失败计数，
    /// 达到 `failure_threshold` 才 down，否则 degraded。
    #[allow(clippy::too_many_arguments)]
    pub async fn record_result(
        &self,
        probe: &Probe,
        node_id: &str,
        ts_unix_nano: i64,
        result_state: &str,
        latency_ms: Option<f64>,
        status_code: Option<i64>,
        error: &str,
    ) -> anyhow::Result<StateTransition> {
        sqlx::query(
            "INSERT INTO probe_results (probe_id, node_id, ts_unix_nano, state, latency_ms, status_code, error)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&probe.id)
        .bind(node_id)
        .bind(ts_unix_nano)
        .bind(result_state)
        .bind(latency_ms)
        .bind(status_code)
        .bind(error)
        .execute(&self.pool)
        .await?;

        let now = ts_unix_nano;
        let previous = self
            .get_state(&probe.id)
            .await?
            .unwrap_or_else(|| ProbeState::unknown(&probe.id, now));

        let (new_state, failures) = if result_state == STATE_OK {
            (STATE_OK.to_string(), 0)
        } else {
            let failures = previous.consecutive_failures + 1;
            let state = if failures >= probe.failure_threshold.max(1) {
                STATE_DOWN
            } else {
                STATE_DEGRADED
            };
            (state.to_string(), failures)
        };

        let changed = previous.state != new_state;
        let last_change = if changed {
            now
        } else {
            previous.last_change_at_unix_nano
        };

        sqlx::query(
            "INSERT INTO probe_state (probe_id, state, consecutive_failures, last_change_at_unix_nano,
                                      last_check_at_unix_nano, last_latency_ms, last_error, updated_at_unix_nano)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(probe_id) DO UPDATE SET
                state = excluded.state,
                consecutive_failures = excluded.consecutive_failures,
                last_change_at_unix_nano = excluded.last_change_at_unix_nano,
                last_check_at_unix_nano = excluded.last_check_at_unix_nano,
                last_latency_ms = excluded.last_latency_ms,
                last_error = excluded.last_error,
                updated_at_unix_nano = excluded.updated_at_unix_nano",
        )
        .bind(&probe.id)
        .bind(&new_state)
        .bind(failures)
        .bind(last_change)
        .bind(now)
        .bind(latency_ms)
        .bind(error)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(StateTransition {
            probe_id: probe.id.clone(),
            previous_state: previous.state,
            new_state,
            consecutive_failures: failures,
            last_error: error.to_string(),
            changed,
        })
    }

    pub async fn recent_results(
        &self,
        probe_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<ProbeResult>> {
        let rows: Vec<ResultRow> = sqlx::query_as(
            "SELECT ts_unix_nano, state, latency_ms, status_code, error
             FROM probe_results WHERE probe_id = ? ORDER BY ts_unix_nano DESC LIMIT ?",
        )
        .bind(probe_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| ProbeResult {
                ts_unix_nano: r.0,
                state: r.1,
                latency_ms: r.2,
                status_code: r.3,
                error: r.4,
            })
            .collect())
    }

    // ---------- 聚合视图 ----------

    /// 服务页数据：服务 → 探针 → 状态
    pub async fn services_with_probes(&self) -> anyhow::Result<Vec<ServiceWithProbes>> {
        let services = self.list_services().await?;
        let mut states = self.all_states().await?;
        let probes = self.list_probes().await?;

        let mut out = Vec::with_capacity(services.len());
        for svc in services {
            let mine: Vec<ProbeWithState> = probes
                .iter()
                .filter(|p| p.service_id == svc.id)
                .map(|p| ProbeWithState {
                    probe: p.clone(),
                    state: states
                        .remove(&p.id)
                        .unwrap_or_else(|| ProbeState::unknown(&p.id, 0)),
                })
                .collect();
            let names: Vec<String> = mine.iter().map(|p| p.state.state.clone()).collect();
            out.push(ServiceWithProbes {
                health: worst_state(&names),
                service: svc,
                probes: mine,
            });
        }
        Ok(out)
    }

    async fn all_states(&self) -> anyhow::Result<std::collections::HashMap<String, ProbeState>> {
        let rows: Vec<StateRow> = sqlx::query_as(&format!("SELECT {STATE_COLS} FROM probe_state"))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let st = state_from_row(r);
                (st.probe_id.clone(), st)
            })
            .collect())
    }

    /// 概览卡片用：服务健康数 / 总数
    pub async fn service_counts(&self) -> anyhow::Result<(i64, i64)> {
        let services = self.services_with_probes().await?;
        let total = services.len() as i64;
        let healthy = services.iter().filter(|s| s.health == STATE_OK).count() as i64;
        Ok((healthy, total))
    }

    // ---------- 维护 ----------

    /// 服务健康时间线：把探针结果按时间桶聚合，每个 (服务, 桶) 一行。
    ///
    /// 返回 `(service_id, bucket_start_unix_nano, ok_count, total_count)`。
    /// 图表画的是「这一格里有多少比例的探测是 ok 的」——比画单次探测的原始
    /// 结果更能看出趋势，也不会因为采样密度不同而忽高忽低。
    pub async fn health_buckets(
        &self,
        from_ns: i64,
        to_ns: i64,
        bucket_ns: i64,
    ) -> anyhow::Result<Vec<(String, i64, i64, i64)>> {
        let bucket_ns = bucket_ns.max(1);
        let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
            r#"
            SELECT p.service_id,
                   (r.ts_unix_nano / ?) * ? AS bucket_start,
                   SUM(CASE WHEN r.state = 'ok' THEN 1 ELSE 0 END) AS ok_count,
                   COUNT(*) AS total_count
            FROM probe_results r
            JOIN probes p ON p.id = r.probe_id
            WHERE r.ts_unix_nano >= ? AND r.ts_unix_nano <= ?
            GROUP BY p.service_id, bucket_start
            ORDER BY p.service_id, bucket_start
            "#,
        )
        .bind(bucket_ns)
        .bind(bucket_ns)
        .bind(from_ns)
        .bind(to_ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 删除早于 `cutoff_unix_nano` 的结果明细，返回删除行数
    pub async fn cleanup_results(&self, cutoff_unix_nano: i64) -> anyhow::Result<u64> {
        let res = sqlx::query("DELETE FROM probe_results WHERE ts_unix_nano < ?")
            .bind(cutoff_unix_nano)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_state_picks_the_most_severe() {
        assert_eq!(worst_state(&[]), "unknown");
        assert_eq!(worst_state(&["ok".into()]), "ok");
        assert_eq!(worst_state(&["ok".into(), "degraded".into()]), "degraded");
        assert_eq!(
            worst_state(&["ok".into(), "degraded".into(), "down".into()]),
            "down"
        );
        assert_eq!(worst_state(&["ok".into(), "unknown".into()]), "unknown");
    }
}
