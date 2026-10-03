use chrono::Utc;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct TelemetryRepo {
    pool: SqlitePool,
}

impl TelemetryRepo {
    #[must_use] pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a raw telemetry batch. Returns the row id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert fails.
    pub async fn insert(
        &self,
        node_id: &str,
        ts_unix_nano: i64,
        interval_seconds: i32,
        payload: &[u8],
    ) -> anyhow::Result<i64> {
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let result = sqlx::query(
            r"
            INSERT INTO telemetry_batches (node_id, ts_unix_nano, interval_seconds, payload_protobuf, received_at_unix_nano)
            VALUES (?, ?, ?, ?, ?)
            ",
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

    /// Count of raw telemetry batches for a single node.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn count_by_node(&self, node_id: &str) -> anyhow::Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_batches WHERE node_id = ?")
                .bind(node_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// Total count of raw telemetry batches across all nodes.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn count_all(&self) -> anyhow::Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_batches")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// Most recent batches for a node, newest first.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn recent(
        &self,
        node_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, i32, Vec<u8>)>> {
        let rows: Vec<(i64, i32, Vec<u8>)> = sqlx::query_as(
            r"
            SELECT ts_unix_nano, interval_seconds, payload_protobuf
            FROM telemetry_batches WHERE node_id = ?
            ORDER BY id DESC LIMIT ?
            ",
        )
        .bind(node_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Batches within a time window (nanoseconds, inclusive), ordered by time ascending.
    ///
    /// Long windows (7 days / 30 days) at 10s sampling have tens of thousands of points;
    /// fetching all is slow and hard to visualize. So we downsample in SQL: if fewer than
    /// `limit` rows in the window, fetch all; otherwise take one row per bucket at
    /// `ceil(total / limit)` stride, giving ~limit points covering the whole window.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn range(
        &self,
        node_id: &str,
        from_ns: i64,
        to_ns: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, i32, Vec<u8>)>> {
        let limit = limit.clamp(2, 2000);
        let rows: Vec<(i64, i32, Vec<u8>)> = sqlx::query_as(
            r"
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
            ",
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

    /// Latest frame per node (get all nodes at once, avoid N queries per node).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn latest_per_node(&self) -> anyhow::Result<Vec<(String, i64, Vec<u8>)>> {
        let rows: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(
            r"
            SELECT node_id, ts_unix_nano, payload_protobuf
            FROM telemetry_batches
            WHERE id IN (SELECT MAX(id) FROM telemetry_batches GROUP BY node_id)
            ",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ---------- Hourly aggregation (downsample, see Migration 011 in migrations.rs) ----------

    /// Raw batches for all nodes within a time window -- retention task fetches all at once,
    /// avoids querying per node.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn batches_in_window(
        &self,
        from_ns: i64,
        to_ns: i64,
    ) -> anyhow::Result<Vec<(String, i64, Vec<u8>)>> {
        let rows: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(
            r"
            SELECT node_id, ts_unix_nano, payload_protobuf
            FROM telemetry_batches
            WHERE ts_unix_nano >= ? AND ts_unix_nano < ?
            ORDER BY node_id, ts_unix_nano
            ",
        )
        .bind(from_ns)
        .bind(to_ns)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Insert/overwrite an aggregated value for (node, hour, metric).
    /// Uses upsert rather than insert: re-running the same hour is idempotent.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
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
            r"
            INSERT INTO telemetry_hourly
                (node_id, ts_hour_unix_nano, metric, avg, min, max, first, last, samples)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(node_id, ts_hour_unix_nano, metric) DO UPDATE SET
                avg = excluded.avg, min = excluded.min, max = excluded.max,
                first = excluded.first, last = excluded.last, samples = excluded.samples
            ",
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

    /// Which hour has been aggregated up to (watermark) -- retention task only moves forward,
    /// does not recalculate the past.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn max_hourly_ts(&self) -> anyhow::Result<Option<i64>> {
        let v: Option<i64> =
            sqlx::query_scalar("SELECT MAX(ts_hour_unix_nano) FROM telemetry_hourly")
                .fetch_one(&self.pool)
                .await?;
        Ok(v)
    }

    /// Long window query: downsample from hourly aggregation table by stride.
    /// Returns `(ts_nano, avg, min, max, first, last, samples)`.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
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
            r"
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
            ",
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

    /// Delete expired raw data, returns number of rows deleted.
    ///
    /// Uses `id` subquery + limit, deletes in small batches (see `delete_in_batches`):
    /// retention data can be millions of rows; one `DELETE ... WHERE ts_unix_nano < ?`
    /// holds a write lock until the full scan is done, during which telemetry reports
    /// and command dispatch all queue up.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if any batch delete fails.
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

    /// Delete expired hourly aggregates, returns number of rows deleted.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if any batch delete fails.
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

/// Maximum rows per delete statement.
///
/// Not arbitrary: `SQLite`'s write lock is database-level; deleting 5000 rows on a typical VPS
/// takes milliseconds -- short enough that other write transactions can still queue in time.
/// Any larger and a single report gets dragged into a "slow statement".
pub const DELETE_BATCH_ROWS: i64 = 5_000;

/// Maximum batches per retention run. When a node is offline for a long time (watermark
/// lags by months), one run won't finish everything -- don't hold the write lock for minutes
/// trying to "clean everything at once".
const MAX_DELETE_BATCHES: usize = 200;

/// Delete in batches: one statement per batch, one commit, release write lock after each.
///
/// Symptom observed (2026-09-23): clicking container operations had no response, monitor logs
/// showed `INSERT INTO telemetry_batches ... elapsed=2.88s` -- that full-table DELETE held
/// the write lock for nearly 3 seconds, INSERT for commands just waited. Index (migration 014)
/// solves "full table scan", batching solves "deleting too much at once".
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
        // `n` is the rows actually deleted this batch; `batch_rows` is the
        // bound. Convert via `try_from` to honour the `cast_possible_wrap`
        // lint: the caller already clamps `batch_rows` to `DELETE_BATCH_ROWS`
        // (5_000), so the conversion cannot fail in practice, but the lint
        // requires the explicit handling.
        let n_i64 = i64::try_from(n).unwrap_or(i64::MAX);
        if n_i64 < batch_rows {
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

    /// In-memory repo + run migrations to get a usable repo.
    /// Insert a node row first: telemetry has foreign keys, sqlx enables `PRAGMA foreign_keys` by default.
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
        // When batch only deletes 2 rows, 3 expired rows need two batches to fully delete --
        // deleting only one batch leaves residue, and retention is 8640 rows/day * node count,
        // which definitely exceeds one batch.
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

    /// Deletes must use the time index.
    ///
    /// The index created by 001 is (node_id, ts_unix_nano), first column is not time --
    /// `WHERE ts < ?` can't use it, SQLite does a full table scan; one DELETE holds write lock
    /// the whole time, symptom is `INSERT INTO telemetry_batches ... elapsed=2.88s` +
    /// container operations "clicked but no response". The idx_telemetry_ts added by
    /// migration 014 is for this query.
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
                "{sql} not using index {index}: {plan:?}"
            );
            assert!(
                !plan.iter().any(|d| d.starts_with("SCAN")),
                "{sql} still doing full table scan: {plan:?}"
            );
        }
    }
}
