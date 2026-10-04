//! Probe health storage: probes / probe state machine / probe result time series.
//!
//! Structure: probes 1:1 `probe_state` (current state machine)
//!                     1:N `probe_results` (historical details, rolling 7-day retention)
//!
//! Probes are executed by the node side (`location = node`), monitor only handles
//! config distribution, state aggregation, and alert triggering. State machine rules
//! are documented in [`ProbesRepo::record_result`].

use sqlx::SqlitePool;

/// State constants: ok / degraded / down
pub const STATE_OK: &str = "ok";
pub const STATE_DEGRADED: &str = "degraded";
pub const STATE_DOWN: &str = "down";

#[derive(Debug, Clone, serde::Serialize)]
pub struct Probe {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub target_json: String,
    pub expect_json: String,
    pub interval_seconds: i64,
    pub timeout_ms: i64,
    pub failure_threshold: i64,
    /// Bound execution nodes; empty = any node (every node runs this probe)
    pub node_ids: Vec<String>,
    /// Display names of bound nodes, in same order as `node_ids` (alias preferred, fallback to hostname)
    pub node_labels: Vec<String>,
    pub location: String,
    pub enabled: bool,
    pub created_at_unix_nano: i64,
    pub updated_at_unix_nano: i64,
}

/// `probe_state` row (probe's current state)
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
    /// A probe that has never been checked: presents as unknown externally, treated as ok internally,
    /// so the first failure follows the normal ok -> degraded convergence path.
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

/// Probe + current state (used by console service page)
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeWithState {
    #[serde(flatten)]
    pub probe: Probe,
    pub state: ProbeState,
}

/// State machine output after one result is stored (alert engine uses this to decide open/close)
#[derive(Debug, Clone)]
pub struct StateTransition {
    pub probe_id: String,
    pub previous_state: String,
    pub new_state: String,
    pub consecutive_failures: i64,
    pub last_error: String,
    /// Whether a state change occurred
    pub changed: bool,
}

/// Input for creating a new probe
#[derive(Debug, Clone, Default)]
pub struct ProbeInput {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub target_json: String,
    pub expect_json: String,
    pub interval_seconds: i64,
    pub timeout_ms: i64,
    pub failure_threshold: i64,
    /// Empty = any node
    pub node_ids: Vec<String>,
    pub location: String,
    pub enabled: bool,
}

/// Partial probe modification: None means don't change
#[derive(Debug, Clone, Default)]
pub struct ProbePatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub kind: Option<String>,
    pub target_json: Option<String>,
    pub expect_json: Option<String>,
    pub interval_seconds: Option<i64>,
    pub timeout_ms: Option<i64>,
    pub failure_threshold: Option<i64>,
    /// Some = replace all bound nodes as a whole (empty array = revert to "any node")
    pub node_ids: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[allow(clippy::type_complexity)]
type ProbeRow = (
    String,  // p.id
    String,  // p.name
    String,  // p.description
    String,  // p.kind
    String,  // p.target_json
    String,  // p.expect_json
    i64,     // p.interval_seconds
    i64,     // p.timeout_ms
    i64,     // p.failure_threshold
    String,  // p.node_ids_json (JSON array, empty = any node)
    String,  // p.location
    i64,     // p.enabled
    i64,     // p.created_at_unix_nano
    i64,     // p.updated_at_unix_nano
);

type StateRow = (String, String, i64, i64, i64, Option<f64>, String);

type ResultRow = (i64, String, Option<f64>, Option<i64>, String);

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
        name: r.1,
        description: r.2,
        kind: r.3,
        target_json: r.4,
        expect_json: r.5,
        interval_seconds: r.6,
        timeout_ms: r.7,
        failure_threshold: r.8,
        node_ids: parse_node_ids(&r.9),
        node_labels: Vec::new(),
        location: r.10,
        enabled: r.11 != 0,
        created_at_unix_nano: r.12,
        updated_at_unix_nano: r.13,
    }
}

/// Parse `probes.node_ids_json`; bad values treated as "any node", one bad row shouldn't break the whole page
fn parse_node_ids(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Serialize bound nodes (empty = any node, stored as `[]` not NULL, queries only compare strings)
fn node_ids_json(ids: &[String]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".into())
}

const STATE_COLS: &str = "probe_id, state, consecutive_failures, last_change_at_unix_nano, \
     last_check_at_unix_nano, last_latency_ms, last_error";

const PROBE_COLS: &str =
    "p.id, p.name, p.description, p.kind, p.target_json, p.expect_json, \
     p.interval_seconds, p.timeout_ms, p.failure_threshold, p.node_ids_json, p.location, p.enabled, \
     p.created_at_unix_nano, p.updated_at_unix_nano";

/// Aggregate worst state (probe health = worst probe state)
#[must_use]
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
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ---------- Probes ----------

    /// List all probes (ordered by name).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query or label lookup fails.
    pub async fn list_probes(&self) -> anyhow::Result<Vec<Probe>> {
        let rows: Vec<ProbeRow> = sqlx::query_as(&format!(
            "SELECT {PROBE_COLS} FROM probes p ORDER BY p.name"
        ))
        .fetch_all(&self.pool)
        .await?;
        let mut probes: Vec<Probe> = rows.into_iter().map(probe_from_row).collect();
        self.fill_node_labels(&mut probes).await?;
        Ok(probes)
    }

    /// Probes a specific node should execute: enabled + bound to this node (or no node specified)
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn probes_for_node(&self, node_id: &str) -> anyhow::Result<Vec<Probe>> {
        let rows: Vec<ProbeRow> = sqlx::query_as(&format!(
            "SELECT {PROBE_COLS} FROM probes p
             WHERE p.enabled = 1 AND p.location = 'node'
               AND (p.node_ids_json = '[]'
                    OR EXISTS (SELECT 1 FROM json_each(p.node_ids_json) WHERE json_each.value = ?))
             ORDER BY p.name"
        ))
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(probe_from_row).collect())
    }

    /// Look up a probe by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query or label lookup fails.
    pub async fn find_probe(&self, id: &str) -> anyhow::Result<Option<Probe>> {
        let row: Option<ProbeRow> = sqlx::query_as(&format!(
            "SELECT {PROBE_COLS} FROM probes p WHERE p.id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let mut probe = probe_from_row(r);
                self.fill_node_labels(std::slice::from_mut(&mut probe))
                    .await?;
                Ok(Some(probe))
            }
            None => Ok(None),
        }
    }

    /// Fill in display names for bound nodes: alias preferred, fallback to hostname (consistent with node list)
    async fn fill_node_labels(&self, probes: &mut [Probe]) -> anyhow::Result<()> {
        if probes.iter().all(|p| p.node_ids.is_empty()) {
            return Ok(());
        }
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT id, alias, hostname FROM nodes")
                .fetch_all(&self.pool)
                .await?;
        let names: std::collections::HashMap<String, String> = rows
            .into_iter()
            .map(|(id, alias, hostname)| {
                let alias = alias.trim();
                let label = if alias.is_empty() {
                    hostname
                } else {
                    alias.to_string()
                };
                (id, label)
            })
            .collect();
        for p in probes.iter_mut() {
            p.node_labels = p
                .node_ids
                .iter()
                .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone()))
                .collect();
        }
        Ok(())
    }

    /// Insert a new probe and return the resulting row.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert or lookup fails, or `anyhow::Error`
    /// if the row is missing after a successful insert.
    pub async fn create_probe(&self, input: &ProbeInput, now: i64) -> anyhow::Result<Probe> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO probes (id, name, description, kind, target_json, expect_json,
                                 interval_seconds, timeout_ms, failure_threshold, node_ids_json, location,
                                 enabled, created_at_unix_nano, updated_at_unix_nano)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.kind)
        .bind(&input.target_json)
        .bind(&input.expect_json)
        .bind(input.interval_seconds)
        .bind(input.timeout_ms)
        .bind(input.failure_threshold)
        .bind(node_ids_json(&input.node_ids))
        .bind(&input.location)
        .bind(i64::from(input.enabled))
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.find_probe(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("probe not found after creation"))
    }

    /// Apply a partial update to a probe (PATCH semantics).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the lookup or update fails, or `anyhow::Error`
    /// if no probe matches the given id.
    pub async fn update_probe(&self, id: &str, patch: &ProbePatch, now: i64) -> anyhow::Result<()> {
        let Some(mut p) = self.find_probe(id).await? else {
            anyhow::bail!("probe not found");
        };
        if let Some(v) = &patch.name {
            p.name.clone_from(v);
        }
        if let Some(v) = &patch.description {
            p.description.clone_from(v);
        }
        if let Some(v) = &patch.kind {
            p.kind.clone_from(v);
        }
        if let Some(v) = &patch.target_json {
            p.target_json.clone_from(v);
        }
        if let Some(v) = &patch.expect_json {
            p.expect_json.clone_from(v);
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
        if let Some(v) = &patch.node_ids {
            let mut seen = std::collections::HashSet::new();
            p.node_ids = v
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && seen.insert(s.clone()))
                .collect();
        }
        if let Some(v) = patch.enabled {
            p.enabled = v;
        }
        sqlx::query(
            "UPDATE probes SET name = ?, description = ?, kind = ?, target_json = ?, expect_json = ?,
                    interval_seconds = ?, timeout_ms = ?, failure_threshold = ?, node_ids_json = ?,
                    enabled = ?, updated_at_unix_nano = ? WHERE id = ?",
        )
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.kind)
        .bind(&p.target_json)
        .bind(&p.expect_json)
        .bind(p.interval_seconds)
        .bind(p.timeout_ms)
        .bind(p.failure_threshold)
        .bind(node_ids_json(&p.node_ids))
        .bind(i64::from(p.enabled))
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a probe and its associated results and state.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if any of the delete queries fail.
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

    // ---------- State machine ----------

    /// Load current state for a probe (returns `None` if it has never been checked).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn get_state(&self, probe_id: &str) -> anyhow::Result<Option<ProbeState>> {
        let row: Option<StateRow> = sqlx::query_as(&format!(
            "SELECT {STATE_COLS} FROM probe_state WHERE probe_id = ?"
        ))
        .bind(probe_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(state_from_row))
    }

    /// Store one result and advance the state machine, return this state change (for alert engine use).
    ///
    /// Rules: ok immediately returns ok (clears failure count); non-ok accumulates failure count,
    /// only goes down when `failure_threshold` is reached, otherwise degraded.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if any of the queries fail.
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
        let last_change = if changed { now } else { previous.last_change_at_unix_nano };

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

    /// Most recent probe results for a probe (newest first).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
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

    // ---------- Aggregate views ----------

    /// Overview card data: healthy / total probe count.
    ///
    /// Disabled probes (`p.enabled = 0`) are excluded: disabled probes won't be dispatched anymore,
    /// shouldn't expect them to work, their down shouldn't count toward cluster failures.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if any underlying query fails.
    pub async fn probe_counts(&self) -> anyhow::Result<(i64, i64)> {
        let rows: Vec<StateRow> = sqlx::query_as(&format!("SELECT {STATE_COLS} FROM probe_state"))
            .fetch_all(&self.pool)
            .await?;
        let probes = self.list_probes().await?;
        let states: std::collections::HashMap<String, ProbeState> = rows
            .into_iter()
            .map(state_from_row)
            .map(|s| (s.probe_id.clone(), s))
            .collect();

        let mut total = 0i64;
        let mut healthy = 0i64;
        for p in &probes {
            if !p.enabled {
                continue;
            }
            total += 1;
            if let Some(state) = states.get(&p.id) {
                if state.state == STATE_OK {
                    healthy += 1;
                }
            }
        }
        Ok((healthy, total))
    }

    /// Probe health timeline: aggregate probe results by time bucket, one row per (probe, bucket).
    ///
    /// Returns `(probe_id, bucket_start_unix_nano, ok_count, total_count)`.
    /// Charts show "what proportion of checks in this bucket were ok" -- better than showing single
    /// probe raw results, smoother trend, not affected by varying sampling density.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the aggregation query fails.
    pub async fn health_buckets(
        &self,
        from_ns: i64,
        to_ns: i64,
        bucket_ns: i64,
    ) -> anyhow::Result<Vec<(String, i64, i64, i64)>> {
        let bucket_ns = bucket_ns.max(1);
        let sql = format!(
            r"
            SELECT r.probe_id AS grp,
                   (r.ts_unix_nano / ?) * ? AS bucket_start,
                   SUM(CASE WHEN r.state = 'ok' THEN 1 ELSE 0 END) AS ok_count,
                   COUNT(*) AS total_count
            FROM probe_results r
            WHERE r.ts_unix_nano >= ? AND r.ts_unix_nano <= ?
            GROUP BY grp, bucket_start
            ORDER BY grp, bucket_start
            "
        );
        let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(&sql)
            .bind(bucket_ns)
            .bind(bucket_ns)
            .bind(from_ns)
            .bind(to_ns)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    // ---------- Maintenance ----------

    /// Delete result details older than `cutoff_unix_nano`, returns rows deleted
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the delete fails.
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
    fn node_ids_round_trip() {
        assert_eq!(node_ids_json(&[]), "[]");
        let ids = vec!["n-1".to_string(), "n-2".to_string()];
        assert_eq!(node_ids_json(&ids), r#"["n-1","n-2"]"#);
        assert_eq!(parse_node_ids(r#"["n-1","n-2"]"#), ids);
        // Bad data treated as "any node", page shouldn't break due to one bad JSON row
        assert_eq!(parse_node_ids("oops"), Vec::<String>::new());
        assert_eq!(parse_node_ids(""), Vec::<String>::new());
    }

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

    /// Disabled probes excluded from counts: both overview count exclude them.
    /// This is a regression test for "probes after disabling are not counted in totals/failures".
    #[tokio::test]
    async fn disabled_probes_are_excluded_from_counts() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        crate::migrations::run(&pool).await.expect("migrations");
        let repo = ProbesRepo::new(pool);

        let now = 1_000_000_000;
        let mk = |name: &str, enabled: bool| ProbeInput {
            name: name.to_string(),
            description: "".to_string(),
            kind: "http".into(),
            target_json: "{}".into(),
            expect_json: "{}".into(),
            interval_seconds: 30,
            timeout_ms: 1000,
            failure_threshold: 1,
            node_ids: vec![],
            location: "node".into(),
            enabled,
        };
        let ok = repo.create_probe(&mk("ok", true), now).await.unwrap();
        let bad = repo.create_probe(&mk("bad", true), now).await.unwrap();
        let off = repo.create_probe(&mk("off", false), now).await.unwrap();

        repo.record_result(&ok, "n1", now, STATE_OK, Some(5.0), Some(200), "")
            .await
            .unwrap();
        repo.record_result(&off, "n1", now, "timeout", None, None, "boom")
            .await
            .unwrap();

        // The disabled "off" probe first goes to down (simulating "residual state after disabling"),
        // but it shouldn't count toward totals
        assert_eq!(
            repo.get_state(&off.id).await.unwrap().unwrap().state,
            STATE_DOWN
        );
        let (healthy, total) = repo.probe_counts().await.unwrap();
        assert_eq!(
            (healthy, total),
            (1, 2),
            "disabled probe not counted in total (bad is enabled but never probed, counts as unknown, in total)"
        );

        // The enabled "bad" probe goes to down
        repo.record_result(&bad, "n1", now, "timeout", None, None, "boom")
            .await
            .unwrap();

        // Disable bad -> counts return to (1, 1)
        repo.update_probe(
            &bad.id,
            &ProbePatch {
                enabled: Some(false),
                ..Default::default()
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(repo.probe_counts().await.unwrap(), (1, 1));
    }
}
