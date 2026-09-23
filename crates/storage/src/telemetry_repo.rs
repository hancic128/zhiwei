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
    ///
    /// 走 `id` 子查询 + 上限，分小批删（见 `delete_in_batches`）：留存量以百万行
    /// 计时，一条 `DELETE ... WHERE ts_unix_nano < ?` 会一路持有写锁到扫完为止，
    /// 期间 telemetry 上报与命令下发全部排队。
    pub async fn delete_raw_before(&self, cutoff_ns: i64) -> anyhow::Result<u64> {
        delete_in_batches(
            &self.pool,
            "DELETE FROM telemetry_batches WHERE id IN (
                 SELECT id FROM telemetry_batches WHERE ts_unix_nano < ? LIMIT ?
             )",
            cutoff_ns,
            DELETE_BATCH_ROWS,
        )
        .await
    }

    /// 滚掉过期的小时聚合，返回删除行数。
    pub async fn delete_hourly_before(&self, cutoff_ns: i64) -> anyhow::Result<u64> {
        delete_in_batches(
            &self.pool,
            "DELETE FROM telemetry_hourly WHERE rowid IN (
                 SELECT rowid FROM telemetry_hourly WHERE ts_hour_unix_nano < ? LIMIT ?
             )",
            cutoff_ns,
            DELETE_BATCH_ROWS,
        )
        .await
    }
}

/// 一条删除语句最多处理多少行。
///
/// 数字不是拍脑袋：SQLite 的写锁是**库级**的，一批 5000 行的删除在普通 VPS 上
/// 是毫秒级——足够短，别的写事务排队也来得及；再大就会把一次上报拖成「慢语句」。
pub const DELETE_BATCH_ROWS: i64 = 5_000;

/// 一轮留存最多删多少批。长期停机（水位线落后几个月）时，一轮删不完就留给下一轮，
/// 不为了「一次清干净」把写锁攥住几分钟。
const MAX_DELETE_BATCHES: usize = 200;

/// 分批删除：每批一条语句、一次提交，写完就让出写锁。
///
/// 现场症状（2026-09-23）：容器操作点了没反应，monitor 日志里
/// `INSERT INTO telemetry_batches ... elapsed=2.88s`——留存那条全表 DELETE 把
/// 写锁占了近 3 秒，命令的 INSERT 只能干等。索引（migration 014）解决「扫全表」，
/// 分批解决「一次删太多」。
async fn delete_in_batches(
    pool: &SqlitePool,
    sql: &str,
    cutoff_ns: i64,
    batch_rows: i64,
) -> anyhow::Result<u64> {
    let mut total = 0u64;
    for _ in 0..MAX_DELETE_BATCHES {
        let r = sqlx::query(sql)
            .bind(cutoff_ns)
            .bind(batch_rows)
            .execute(pool)
            .await?;
        let n = r.rows_affected();
        total += n;
        if (n as i64) < batch_rows {
            break;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::Row;

    /// 内存库 + 跑一遍迁移，拿到可用的 repo。
    /// 先补一行节点：telemetry 有外键，sqlx 默认开 `PRAGMA foreign_keys`。
    async fn repo() -> TelemetryRepo {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        crate::migrations::run(&pool).await.expect("migrations");
        sqlx::query(
            "INSERT INTO nodes (id, hostname, client_cert_pem, enrolled_at_unix_nano)
             VALUES ('n1', 'host', '', 0)",
        )
        .execute(&pool)
        .await
        .expect("node row");
        TelemetryRepo::new(pool)
    }

    async fn seed(r: &TelemetryRepo, count: i64) {
        for ts in 0..count {
            r.insert("n1", ts, 10, b"x").await.expect("insert");
        }
    }

    const DELETE_ONE_BATCH_SQL: &str = "DELETE FROM telemetry_batches WHERE id IN (
         SELECT id FROM telemetry_batches WHERE ts_unix_nano < ? LIMIT ?
     )";

    #[tokio::test]
    async fn delete_raw_before_removes_only_older_rows() {
        let r = repo().await;
        seed(&r, 10).await;
        assert_eq!(r.delete_raw_before(4).await.unwrap(), 4);
        assert_eq!(r.count_by_node("n1").await.unwrap(), 6);
    }

    #[tokio::test]
    async fn delete_loops_until_the_backlog_is_gone() {
        // 一批只删 2 行时，3 行过期的数据要分两批删完——只删一批会留下残渣，
        // 而留存量是每天 8640 行 × 节点数，必然远超一批。
        let r = repo().await;
        seed(&r, 10).await;
        let deleted = delete_in_batches(&r.pool, DELETE_ONE_BATCH_SQL, 3, 2)
            .await
            .unwrap();
        assert_eq!(deleted, 3);
        assert_eq!(r.count_by_node("n1").await.unwrap(), 7);
    }

    #[tokio::test]
    async fn delete_of_nothing_is_zero() {
        let r = repo().await;
        seed(&r, 3).await;
        assert_eq!(r.delete_raw_before(0).await.unwrap(), 0);
        assert_eq!(r.count_by_node("n1").await.unwrap(), 3);
    }

    /// 删除必须走时间索引。
    ///
    /// 001 建的索引是 (node_id, ts_unix_nano)，首列不是时间——`WHERE ts < ?` 用不上，
    /// SQLite 只能全表扫描；一条 DELETE 全程持写锁，现场表现为
    /// `INSERT INTO telemetry_batches ... elapsed=2.88s` + 容器操作「点了没反应」。
    /// migration 014 补的 idx_telemetry_ts 就是为这条语句。
    #[tokio::test]
    async fn time_based_delete_uses_the_ts_index() {
        let r = repo().await;
        for (sql, index) in [
            (
                "EXPLAIN QUERY PLAN SELECT id FROM telemetry_batches WHERE ts_unix_nano < 1",
                "idx_telemetry_ts",
            ),
            (
                "EXPLAIN QUERY PLAN SELECT rowid FROM telemetry_hourly WHERE ts_hour_unix_nano < 1",
                "idx_telemetry_hourly_ts",
            ),
        ] {
            let rows = sqlx::query(sql).fetch_all(&r.pool).await.unwrap();
            let plan: Vec<String> = rows.iter().map(|row| row.get::<String, _>(3)).collect();
            assert!(
                plan.iter().any(|d| d.contains(index)),
                "{sql} 没走索引 {index}：{plan:?}"
            );
            assert!(
                !plan.iter().any(|d| d.starts_with("SCAN")),
                "{sql} 仍在全表扫描：{plan:?}"
            );
        }
    }
}
