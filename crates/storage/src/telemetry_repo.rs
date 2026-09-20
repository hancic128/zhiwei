use chrono::Utc;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct TelemetryRepo {
    pool: SqlitePool,
}

impl TelemetryRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(
        &self,
        node_id: &str,
        ts_unix_nano: i64,
        interval_seconds: i32,
        payload: &[u8],
    ) -> anyhow::Result<i64> {
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let result = sqlx::query(
            r#"
            INSERT INTO telemetry_batches (node_id, ts_unix_nano, interval_seconds, payload_protobuf, received_at_unix_nano)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(node_id)
        .bind(ts_unix_nano)
        .bind(interval_seconds)
        .bind(payload)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn count_by_node(&self, node_id: &str) -> anyhow::Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_batches WHERE node_id = ?")
                .bind(node_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    pub async fn count_all(&self) -> anyhow::Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_batches")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// Most recent batches for a node, newest first.
    pub async fn recent(
        &self,
        node_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, i32, Vec<u8>)>> {
        let rows: Vec<(i64, i32, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT ts_unix_nano, interval_seconds, payload_protobuf
            FROM telemetry_batches WHERE node_id = ?
            ORDER BY id DESC LIMIT ?
            "#,
        )
        .bind(node_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 时间窗内的批次（纳秒、闭区间），按时间升序。
    ///
    /// 长窗口（7 天 / 30 天）按 10s 采样会有几万个点，直接全取既慢又画不动，
    /// 所以在 SQL 里做等步长抽稀：窗口内不足 `limit` 条就全取，否则按
    /// `ceil(total / limit)` 的步长取每个桶的第一条，点数≈limit 且覆盖整个窗口。
    pub async fn range(
        &self,
        node_id: &str,
        from_ns: i64,
        to_ns: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, i32, Vec<u8>)>> {
        let limit = limit.clamp(2, 2000);
        let rows: Vec<(i64, i32, Vec<u8>)> = sqlx::query_as(
            r#"
            WITH win AS (
                SELECT ts_unix_nano, interval_seconds, payload_protobuf,
                       ROW_NUMBER() OVER (ORDER BY ts_unix_nano) AS rn,
                       COUNT(*) OVER () AS total
                FROM telemetry_batches
                WHERE node_id = ? AND ts_unix_nano >= ? AND ts_unix_nano <= ?
            )
            SELECT ts_unix_nano, interval_seconds, payload_protobuf
            FROM win
            WHERE total <= ? OR rn % ((total + ? - 1) / ?) = 1
            ORDER BY ts_unix_nano
            "#,
        )
        .bind(node_id)
        .bind(from_ns)
        .bind(to_ns)
        .bind(limit)
        .bind(limit)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 每个节点的最新一帧（节点列表一次取齐，避免逐节点 N 次查询）。
    pub async fn latest_per_node(&self) -> anyhow::Result<Vec<(String, i64, Vec<u8>)>> {
        let rows: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT node_id, ts_unix_nano, payload_protobuf
            FROM telemetry_batches
            WHERE id IN (SELECT MAX(id) FROM telemetry_batches GROUP BY node_id)
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ---------- 小时聚合（降采样，见 migrations.rs 的 Migration 011）----------

    /// 时间窗内的所有节点的原始批次——留存任务一次取齐，避免逐节点查询。
    pub async fn batches_in_window(
        &self,
        from_ns: i64,
        to_ns: i64,
    ) -> anyhow::Result<Vec<(String, i64, Vec<u8>)>> {
        let rows: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT node_id, ts_unix_nano, payload_protobuf
            FROM telemetry_batches
            WHERE ts_unix_nano >= ? AND ts_unix_nano < ?
            ORDER BY node_id, ts_unix_nano
            "#,
        )
        .bind(from_ns)
        .bind(to_ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 写入 / 覆盖一个 (节点, 小时, 指标) 的聚合值。
    /// 用 upsert 而不是 insert：重跑同一小时是幂等的。
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_hourly(
        &self,
        node_id: &str,
        ts_hour_unix_nano: i64,
        metric: &str,
        avg: f64,
        min: f64,
        max: f64,
        first: f64,
        last: f64,
        samples: i64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO telemetry_hourly
                (node_id, ts_hour_unix_nano, metric, avg, min, max, first, last, samples)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(node_id, ts_hour_unix_nano, metric) DO UPDATE SET
                avg = excluded.avg, min = excluded.min, max = excluded.max,
                first = excluded.first, last = excluded.last, samples = excluded.samples
            "#,
        )
        .bind(node_id)
        .bind(ts_hour_unix_nano)
        .bind(metric)
        .bind(avg)
        .bind(min)
        .bind(max)
        .bind(first)
        .bind(last)
        .bind(samples)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 已聚合到哪个小时（水位线）——留存任务只往前推，不回头重算。
    pub async fn max_hourly_ts(&self) -> anyhow::Result<Option<i64>> {
        let v: Option<i64> =
            sqlx::query_scalar("SELECT MAX(ts_hour_unix_nano) FROM telemetry_hourly")
                .fetch_one(&self.pool)
                .await?;
        Ok(v)
    }

    /// 长窗口查询：从小时聚合表按步长抽稀取值。
    /// 返回 `(ts_nano, avg, min, max, first, last, samples)`。
    pub async fn hourly_range(
        &self,
        node_id: &str,
        metric: &str,
        from_ns: i64,
        to_ns: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, f64, f64, f64, f64, f64, i64)>> {
        let limit = limit.clamp(2, 2000);
        let rows: Vec<(i64, f64, f64, f64, f64, f64, i64)> = sqlx::query_as(
            r#"
            WITH win AS (
                SELECT ts_hour_unix_nano, avg, min, max, first, last, samples,
                       ROW_NUMBER() OVER (ORDER BY ts_hour_unix_nano) AS rn,
                       COUNT(*) OVER () AS total
                FROM telemetry_hourly
                WHERE node_id = ? AND metric = ?
                  AND ts_hour_unix_nano >= ? AND ts_hour_unix_nano <= ?
            )
            SELECT ts_hour_unix_nano, avg, min, max, first, last, samples
            FROM win
            WHERE total <= ? OR rn % ((total + ? - 1) / ?) = 1
            ORDER BY ts_hour_unix_nano
            "#,
        )
        .bind(node_id)
        .bind(metric)
        .bind(from_ns)
        .bind(to_ns)
        .bind(limit)
        .bind(limit)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// 滚掉过期的原始数据，返回删除行数。
    pub async fn delete_raw_before(&self, cutoff_ns: i64) -> anyhow::Result<u64> {
        let r = sqlx::query("DELETE FROM telemetry_batches WHERE ts_unix_nano < ?")
            .bind(cutoff_ns)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 滚掉过期的小时聚合，返回删除行数。
    pub async fn delete_hourly_before(&self, cutoff_ns: i64) -> anyhow::Result<u64> {
        let r = sqlx::query("DELETE FROM telemetry_hourly WHERE ts_hour_unix_nano < ?")
            .bind(cutoff_ns)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }
}
