use sqlx::SqlitePool;
use zhiwei_common::NodeId;

/// 容器与进程快照：每个节点只保留最新一份（UPSERT）。
/// 这类数据是「当前状态」而非时序，不适合写进 telemetry_batches。
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
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        node_id: &NodeId,
        ts_unix_nano: i64,
        containers_json: &str,
        processes_json: &str,
        certificates_json: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO node_inventory (node_id, ts_unix_nano, containers_json, processes_json, certificates_json)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(node_id) DO UPDATE SET
                ts_unix_nano = excluded.ts_unix_nano,
                containers_json = excluded.containers_json,
                processes_json = excluded.processes_json,
                certificates_json = excluded.certificates_json
            "#,
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
