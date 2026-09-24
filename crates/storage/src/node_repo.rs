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
    /// 主机基本信息（JSON，来自 telemetry 的 HostInfo）
    pub host_info_json: String,
    /// 节点 Ed25519 公钥（base64）。请求签名用它验签，取代原 mTLS 客户端证书。
    pub public_key: String,
    /// 管理员给的简短别称（≤10 字符）；空串表示未设置
    pub alias: String,
    /// 管理员给的标签（JSON 数组，≤10 个）
    pub tags_json: String,
}

/// SQLite 行的裸形态：与 SELECT 的列顺序一一对应。
type NodeRow = (
    String,      // id
    String,      // hostname
    String,      // labels_json
    String,      // client_cert_pem（历史列，已不再读写）
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
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, record: &NodeRecord) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO nodes (id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, public_key, alias, tags_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
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

    pub async fn find_by_id(&self, id: &NodeId) -> anyhow::Result<Option<NodeRecord>> {
        let row: Option<NodeRow> =
            sqlx::query_as(
                r#"
            SELECT id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, host_info_json, public_key, alias, tags_json
            FROM nodes WHERE id = ?
            "#,
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

    pub async fn touch_last_seen(&self, id: &NodeId) -> anyhow::Result<()> {
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query("UPDATE nodes SET last_seen_unix_nano = ? WHERE id = ?")
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 允许节点自报新名字（例如首次上报时系统主机名是 bogon，后来改用
    /// LocalHostName 或 --node-name）。仅在名字非空且确实不同时更新。
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

    /// 用最新一次上报的 HostInfo 覆盖节点基本信息。
    pub async fn update_host_info(&self, id: &NodeId, host_info_json: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE nodes SET host_info_json = ? WHERE id = ?")
            .bind(host_info_json)
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 覆盖管理员维护的别名与标签（节点上报不会碰这两列）。
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

    pub async fn list_all(&self) -> anyhow::Result<Vec<NodeRecord>> {
        let rows: Vec<NodeRow> =
            sqlx::query_as(
                r#"
            SELECT id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, host_info_json, public_key, alias, tags_json
            FROM nodes ORDER BY enrolled_at_unix_nano DESC
            "#,
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

    /// 节点删除：把这一行连带所有引用它的数据一起清掉。
    ///
    /// 设计取舍：
    ///   - **直接 SQL 而不是事务包装**：SQLite 是单文件锁，没有真正的并发事务边界，
    ///     加 BEGIN/COMMIT 反而容易让人误以为有回滚能力。失败就直接冒泡给调用方。
    ///   - **同步删除强引用**：telemetry_batches / node_inventory 有 FK，必须先删。
    ///     telemetry_hourly / probe_results / cert_sources / alerts / alert_state 没 FK，
    ///     但删节点时也得跟着清——节点没了，历史指标和「自动关停告警」就毫无意义。
    ///   - **probes.node_ids_json 是字符串数组**：要把这个节点 id 从所有探针里摘掉，
    ///     不能直接 `DELETE`，否则剩下的探针会一直把它当绑定节点。这里用 SQL 表达式
    ///     在数据库里改 JSON 数组，单条 UPDATE 就能干掉所有引用。
    ///   - **commands 一起删**：发出去的命令都是「针对这台节点」的，节点没了留着无意义。
    ///     唯一需要保的是 `audit_log`，但 audit_log 表没有 node_id 字段。
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;

        // 把节点 id 从所有探针的 node_ids_json 里摘掉。节点删除是低频动作，
        // 应用层做比写嵌套 json_remove 表达式直观。
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

        // telemetry_batches / node_inventory 是 FK 引用，必须先删。
        // audit_log 保留：操作审计是事后追责用的，节点没了该看还得看。
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
            // 节点不存在：提交空事务，返回 false 让调用方按 404 处理。
            tx.commit().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod delete_tests {
    //! 删除节点的级联与探针解绑行为。
    //!
    //! 关键不变量：
    //!   1. 节点行真的被删了；
    //!   2. 所有引用 node_id 的表都跟着清干净了；
    //!   3. 探针的 node_ids_json 数组里，这个节点 id 被摘掉了，其它节点的绑定不受影响；
    //!   4. audit_log 保留——它是审计需求，不该跟着节点被洗白；
    //!   5. 删一个不存在的节点返回 false（且不报错）。

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

    #[tokio::test]
    async fn delete_cascades_to_all_referencing_tables() {
        let pool = fresh_pool().await;
        let repo = NodeRepo::new(pool.clone());

        // 入网 + 在每张相关表里塞一条
        repo.insert(&make_record("n1", "host-a")).await.unwrap();
        sqlx::query(
            "INSERT INTO telemetry_batches (node_id, ts_unix_nano, interval_seconds, payload_protobuf, received_at_unix_nano)
             VALUES ('n1', 0, 10, x'', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO telemetry_hourly (node_id, ts_hour_unix_nano, metric, avg, min, max, first, last, samples)
             VALUES ('n1', 0, 'host.cpu.usage', 0, 0, 0, 0, 0, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO node_inventory (node_id, ts_unix_nano, containers_json, processes_json)
             VALUES ('n1', 0, '[]', '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO probe_results (probe_id, node_id, ts_unix_nano, state, latency_ms) VALUES ('p1', 'n1', 0, 'ok', 0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO cert_sources (id, node_id, path, enabled, notify_enabled, notify_days_before, created_at_unix_nano, updated_at_unix_nano)
             VALUES ('cs1', 'n1', '/etc/ssl', 1, 1, 30, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commands (id, node_id, action, params_json, payload_protobuf, issued_at_unix_nano, ttl_seconds, state)
             VALUES ('c1', 'n1', 'restart', '{}', x'', 0, 60, 'delivered')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO alerts (rule_id, rule_name, node_id, hostname, severity, metric, op, threshold, value,
                                  message, started_at_unix_nano, source, source_ref)
             VALUES (0, 'node offline', 'n1', 'host-a', 'critical', 'host.online', 'eq', 0, 0,
                     'offline', 0, 'node_offline', 'n1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO alert_state (rule_id, node_id, breaching_since_unix_nano, firing, open_alert_id, last_value)
             VALUES (0, 'n1', 0, 1, 1, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // audit_log 也存一行——删除节点后这条必须还在
        sqlx::query(
            "INSERT INTO audit_log (at_unix_nano, actor, node_id, command_id, action, params_json, outcome)
             VALUES (0, 'admin', 'n1', 'c1', 'restart', '{}', 'issued')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 探针绑定两台节点：n1（待删） + n2（要保）
        sqlx::query(
            "INSERT INTO probes (id, service_id, name, kind, target_json, expect_json, interval_seconds, timeout_ms, failure_threshold, node_ids_json, location, enabled, created_at_unix_nano, updated_at_unix_nano)
             VALUES ('pr1', 'svc1', 'http', 'http', '{}', '{}', 60, 5000, 3, '[\"n1\",\"n2\"]', '', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 确认数据真的写进去了
        let audit_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE node_id = 'n1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_before, 1);

        // 执行删除
        let ok = repo.delete("n1").await.unwrap();
        assert!(ok, "delete 应该返回 true（节点存在）");

        // 节点本身没了
        assert!(
            repo.find_by_id(&zhiwei_common::NodeId::from_string("n1"))
                .await
                .unwrap()
                .is_none(),
            "find_by_id 应该找不到被删的节点"
        );

        // 级联表全部清空
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
            assert_eq!(n, 0, "{tbl} 应该清空 n1 的全部行");
        }

        // audit_log 保留
        let audit_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE node_id = 'n1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_after, 1, "audit_log 必须保留——它是审计需求");

        // 探针的 node_ids_json 里 n1 被摘掉了，n2 还在
        let raw: String = sqlx::query_scalar("SELECT node_ids_json FROM probes WHERE id = 'pr1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let ids: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            ids,
            vec!["n2".to_string()],
            "探针绑定里 n1 应被摘掉，只剩 n2"
        );
    }

    #[tokio::test]
    async fn delete_unknown_node_returns_false() {
        let pool = fresh_pool().await;
        let repo = NodeRepo::new(pool);
        let ok = repo.delete("does-not-exist").await.unwrap();
        assert!(!ok, "删除不存在的节点应该返回 false 而不是报错");
    }
}
