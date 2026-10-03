use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use zhiwei_common::NodeId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecord {
    pub id: String,
    pub hostname: String,
    pub labels_json: String,
    pub client_cert_pem: String,
    pub enrolled_at_unix_nano: i64,
    pub last_seen_unix_nano: Option<i64>,
    /// Basic host info (JSON, from telemetry's `HostInfo`)
    pub host_info_json: String,
    /// Node Ed25519 public key (base64). Used for request signature verification, replacing the original mTLS client cert.
    pub public_key: String,
    /// Short alias given by admin (≤10 chars); empty string means not set
    pub alias: String,
    /// Admin-provided tags (JSON array, ≤10 items)
    pub tags_json: String,
}

/// Bare form of `SQLite` row: one-to-one correspondence with SELECT column order.
type NodeRow = (
    String,      // id
    String,      // hostname
    String,      // labels_json
    String,      // client_cert_pem (historical column, no longer read/written)
    i64,         // enrolled_at_unix_nano
    Option<i64>, // last_seen_unix_nano
    String,      // host_info_json
    String,      // public_key (base64)
    String,      // alias
    String,      // tags_json
);

#[derive(Clone)]
pub struct NodeRepo {
    pool: SqlitePool,
}

impl NodeRepo {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a new node record.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails (e.g. duplicate `id`).
    pub async fn insert(&self, record: &NodeRecord) -> anyhow::Result<()> {
        sqlx::query(
            r"
            INSERT INTO nodes (id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, public_key, alias, tags_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&record.id)
        .bind(&record.hostname)
        .bind(&record.labels_json)
        .bind(&record.client_cert_pem)
        .bind(record.enrolled_at_unix_nano)
        .bind(record.last_seen_unix_nano)
        .bind(&record.public_key)
        .bind(&record.alias)
        .bind(&record.tags_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up a node by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn find_by_id(&self, id: &NodeId) -> anyhow::Result<Option<NodeRecord>> {
        let row: Option<NodeRow> =
            sqlx::query_as(
                r"
            SELECT id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, host_info_json, public_key, alias, tags_json
            FROM nodes WHERE id = ?
            ",
            )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(
                id,
                hostname,
                labels_json,
                client_cert_pem,
                enrolled_at_unix_nano,
                last_seen_unix_nano,
                host_info_json,
                public_key,
                alias,
                tags_json,
            )| NodeRecord {
                id,
                hostname,
                labels_json,
                client_cert_pem,
                enrolled_at_unix_nano,
                last_seen_unix_nano,
                host_info_json,
                public_key,
                alias,
                tags_json,
            },
        ))
    }

    /// Stamp `last_seen_unix_nano = now` for a node.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn touch_last_seen(&self, id: &NodeId) -> anyhow::Result<()> {
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query("UPDATE nodes SET last_seen_unix_nano = ? WHERE id = ?")
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Allow node to report a new hostname (e.g., first report had system hostname as "bogon",
    /// later uses `LocalHostName` or --node-name). Only updates if name is non-empty and actually different.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn update_hostname(&self, id: &NodeId, hostname: &str) -> anyhow::Result<()> {
        if hostname.trim().is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE nodes SET hostname = ? WHERE id = ? AND hostname <> ?")
            .bind(hostname.trim())
            .bind(id.as_str())
            .bind(hostname.trim())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Overwrite node basic info with the latest reported `HostInfo`.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn update_host_info(&self, id: &NodeId, host_info_json: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE nodes SET host_info_json = ? WHERE id = ?")
            .bind(host_info_json)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Overwrite admin-maintained alias and tags (node reporting doesn't touch these columns).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the update fails.
    pub async fn update_meta(
        &self,
        id: &NodeId,
        alias: &str,
        tags_json: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE nodes SET alias = ?, tags_json = ? WHERE id = ?")
            .bind(alias)
            .bind(tags_json)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// List all enrolled nodes, newest first.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn list_all(&self) -> anyhow::Result<Vec<NodeRecord>> {
        let rows: Vec<NodeRow> =
            sqlx::query_as(
                r"
            SELECT id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, host_info_json, public_key, alias, tags_json
            FROM nodes ORDER BY enrolled_at_unix_nano DESC
            ",
            )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    hostname,
                    labels_json,
                    client_cert_pem,
                    enrolled_at_unix_nano,
                    last_seen_unix_nano,
                    host_info_json,
                    public_key,
                    alias,
                    tags_json,
                )| NodeRecord {
                    id,
                    hostname,
                    labels_json,
                    client_cert_pem,
                    enrolled_at_unix_nano,
                    last_seen_unix_nano,
                    host_info_json,
                    public_key,
                    alias,
                    tags_json,
                },
            )
            .collect())
    }

    /// Delete a node: clean up this row along with all its referencing data.
    ///
    /// Design choices:
    ///   - **Direct SQL, not wrapped in transactions**: `SQLite` uses file-level locking, there's no
    ///     real concurrent transaction boundary. Adding BEGIN/COMMIT just makes people think there's
    ///     rollback capability. Failures bubble up directly to callers.
    ///   - **Synchronously delete strong references**: `telemetry_batches` / `node_inventory` have FKs,
    ///     must be deleted first. `telemetry_hourly` / `probe_results` / `cert_sources` / alerts /
    ///     `alert_state` have no FKs, but must also be cleared when node is deleted -- without the
    ///     node, historical metrics and "auto-close alerts" make no sense.
    ///   - **`probes.node_ids_json` is a string array**: need to remove this node's id from all probes,
    ///     can't just DELETE directly, otherwise remaining probes will keep treating it as a bound node.
    ///     Here we modify the JSON array in the database with a SQL expression, a single UPDATE
    ///     removes all references.
    ///   - **commands deleted together**: issued commands are all "targeting this node", meaningless
    ///     without the node. The only thing to preserve is `audit_log`, but `audit_log` has no `node_id` column.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if any of the queries or transaction operations fail.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;

        // Remove node id from all probes' node_ids_json. Node deletion is low-frequency,
        // doing it in application layer is more intuitive than writing nested json_remove expressions.
        let affected_probes: Vec<(String, String)> = {
            use sqlx::Row;
            let rows = sqlx::query(
                "SELECT id, node_ids_json FROM probes
                 WHERE EXISTS (SELECT 1 FROM json_each(node_ids_json) WHERE json_each.value = ?)",
            )
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
            rows.into_iter()
                .map(|row| {
                    let probe_id: String = row.try_get("id")?;
                    let raw: String = row.try_get("node_ids_json")?;
                    Ok::<_, anyhow::Error>((probe_id, raw))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        for (probe_id, raw) in affected_probes {
            let updated: Vec<String> = serde_json::from_str::<Vec<String>>(&raw)
                .unwrap_or_default()
                .into_iter()
                .filter(|n| n != id)
                .collect();
            let updated_json = serde_json::to_string(&updated).unwrap_or_else(|_| "[]".into());
            sqlx::query(
                "UPDATE probes SET node_ids_json = ?, updated_at_unix_nano = ? WHERE id = ?",
            )
            .bind(updated_json)
            .bind(now)
            .bind(probe_id)
            .execute(&mut *tx)
            .await?;
        }

        // telemetry_batches / node_inventory have FK references, must be deleted first.
        // audit_log is preserved: operational audit is for post-incident review, should still be visible after node deletion.
        for tbl in [
            "telemetry_batches",
            "telemetry_hourly",
            "node_inventory",
            "probe_results",
            "cert_sources",
            "commands",
            "alerts",
            "alert_state",
        ] {
            sqlx::query(&format!("DELETE FROM {tbl} WHERE node_id = ?"))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let r = sqlx::query("DELETE FROM nodes WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if r.rows_affected() == 0 {
            // Node doesn't exist: commit empty transaction, return false so caller handles as 404.
            tx.commit().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod delete_tests {
    //! Cascading and probe unbinding behavior when deleting a node.
    //!
    //! Key invariants:
    //!   1. Node row is actually deleted;
    //!   2. All tables referencing `node_id` are cleaned up;
    //!   3. The node id is removed from probes' `node_ids_json` array, other nodes' bindings unaffected;
    //!   4. `audit_log` is preserved -- it's an audit requirement, shouldn't be wiped with the node;
    //!   5. Deleting a non-existent node returns false (and doesn't error).

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

    fn make_record(id: &str, hostname: &str) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            hostname: hostname.to_string(),
            labels_json: "[]".into(),
            client_cert_pem: String::new(),
            enrolled_at_unix_nano: 0,
            last_seen_unix_nano: Some(0),
            host_info_json: "{}".into(),
            public_key: String::new(),
            alias: String::new(),
            tags_json: "[]".into(),
        }
    }

    async fn seed_node_references(pool: &sqlx::SqlitePool) {
        // One row per related table referencing node 'n1'.
        sqlx::query(
            "INSERT INTO telemetry_batches (node_id, ts_unix_nano, interval_seconds, payload_protobuf, received_at_unix_nano)
             VALUES ('n1', 0, 10, x'', 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO telemetry_hourly (node_id, ts_hour_unix_nano, metric, avg, min, max, first, last, samples)
             VALUES ('n1', 0, 'host.cpu.usage', 0, 0, 0, 0, 0, 1)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO node_inventory (node_id, ts_unix_nano, containers_json, processes_json)
             VALUES ('n1', 0, '[]', '[]')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO probe_results (probe_id, node_id, ts_unix_nano, state, latency_ms)
             VALUES ('p1', 'n1', 0, 'ok', 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO cert_sources (id, node_id, path, enabled, notify_enabled, notify_days_before, created_at_unix_nano, updated_at_unix_nano)
             VALUES ('cs1', 'n1', '/etc/ssl', 1, 1, 30, 0, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commands (id, node_id, action, params_json, payload_protobuf, issued_at_unix_nano, ttl_seconds, state)
             VALUES ('c1', 'n1', 'restart', '{}', x'', 0, 60, 'delivered')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO alerts (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                                  message, started_at_unix_nano, source, source_ref)
             VALUES (0, 'node offline', 'n1', 'host-a', 'critical', 'host.online', 'eq', 0, 0,
                     'offline', 0, 'node_offline', 'n1')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO alert_state (rule_id, node_id, breaching_since_unix_nano, firing, open_alert_id, last_value)
             VALUES (0, 'n1', 0, 1, 1, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        // Also insert one in audit_log -- must remain after node deletion
        sqlx::query(
            "INSERT INTO audit_log (at_unix_nano, actor, node_id, command_id, action, params_json, outcome)
             VALUES (0, 'admin', 'n1', 'c1', 'restart', '{}', 'issued')",
        )
        .execute(pool)
        .await
        .unwrap();
        // Probe binds two nodes: n1 (to be deleted) + n2 (to keep)
        sqlx::query(
            "INSERT INTO probes (id, service_id, name, kind, target_json, expect_json, interval_seconds, timeout_ms, failure_threshold, node_ids_json, location, enabled, created_at_unix_nano, updated_at_unix_nano)
             VALUES ('pr1', 'svc1', 'http', 'http', '{}', '{}', 60, 5000, 3, '[\"n1\",\"n2\"]', '', 1, 0, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_cascades_to_all_referencing_tables() {
        let pool = fresh_pool().await;
        let repo = NodeRepo::new(pool.clone());

        // Enroll + insert one row in each related table
        repo.insert(&make_record("n1", "host-a")).await.unwrap();
        seed_node_references(&pool).await;

        // Confirm data was actually written
        let audit_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE node_id = 'n1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_before, 1);

        // Execute deletion
        let ok = repo.delete("n1").await.unwrap();
        assert!(ok, "delete should return true (node exists)");

        // Node itself is gone
        assert!(
            repo.find_by_id(&zhiwei_common::NodeId::from_string("n1"))
                .await
                .unwrap()
                .is_none(),
            "find_by_id should not find deleted node"
        );

        // All cascade tables are cleared
        for tbl in [
            "telemetry_batches",
            "telemetry_hourly",
            "node_inventory",
            "probe_results",
            "cert_sources",
            "commands",
            "alerts",
            "alert_state",
        ] {
            let n: i64 =
                sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {tbl} WHERE node_id = 'n1'"))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(n, 0, "{tbl} should have all n1 rows cleared");
        }

        // audit_log preserved
        let audit_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE node_id = 'n1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            audit_after, 1,
            "audit_log must be preserved -- it's an audit requirement"
        );

        // n1 is removed from probe's node_ids_json, n2 remains
        let raw: String = sqlx::query_scalar("SELECT node_ids_json FROM probes WHERE id = 'pr1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let ids: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            ids,
            vec!["n2".to_string()],
            "n1 should be removed from probe binding, only n2 remains"
        );
    }

    #[tokio::test]
    async fn delete_unknown_node_returns_false() {
        let pool = fresh_pool().await;
        let repo = NodeRepo::new(pool);
        let ok = repo.delete("does-not-exist").await.unwrap();
        assert!(
            !ok,
            "deleting non-existent node should return false, not error"
        );
    }
}
