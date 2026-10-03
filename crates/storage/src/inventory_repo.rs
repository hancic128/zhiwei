use sqlx::SqlitePool;
use zhiwei_common::NodeId;

/// Container and process snapshots: only the latest one is kept per node (UPSERT).
/// This kind of data is "current state" rather than time-series, not suitable for `telemetry_batches`.
#[derive(Clone)]
pub struct InventoryRepo {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
pub struct InventoryRow {
    pub ts_unix_nano: i64,
    pub containers_json: String,
    pub processes_json: String,
    pub certificates_json: String,
}

impl InventoryRepo {
    #[must_use] pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert the latest container/process/certificate snapshot for a node.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn upsert(
        &self,
        node_id: &NodeId,
        ts_unix_nano: i64,
        containers_json: &str,
        processes_json: &str,
        certificates_json: &str,
    ) -> anyhow::Result<()> {
        // cron_jobs_json column kept but no longer written: scheduled task feature is deprecated,
        // nodes no longer report; old data kept only for audit. cron_jobs_json is not in
        // ON CONFLICT SET, old data not overwritten.
        sqlx::query(
            r"
            INSERT INTO node_inventory (node_id, ts_unix_nano, containers_json, processes_json, certificates_json, cron_jobs_json)
            VALUES (?, ?, ?, ?, ?, '[]')
            ON CONFLICT(node_id) DO UPDATE SET
                ts_unix_nano = excluded.ts_unix_nano,
                containers_json = excluded.containers_json,
                processes_json = excluded.processes_json,
                certificates_json = excluded.certificates_json
            ",
        )
        .bind(node_id.as_str())
        .bind(ts_unix_nano)
        .bind(containers_json)
        .bind(processes_json)
        .bind(certificates_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up the current inventory snapshot for a node.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn find(&self, node_id: &NodeId) -> anyhow::Result<Option<InventoryRow>> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT ts_unix_nano, containers_json, processes_json, certificates_json FROM node_inventory WHERE node_id = ?",
        )
        .bind(node_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(ts_unix_nano, containers_json, processes_json, certificates_json)| InventoryRow {
                ts_unix_nano,
                containers_json,
                processes_json,
                certificates_json,
            },
        ))
    }
}
