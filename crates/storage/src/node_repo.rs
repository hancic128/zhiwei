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
            INSERT INTO nodes (id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, public_key)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record.id)
        .bind(&record.hostname)
        .bind(&record.labels_json)
        .bind(&record.client_cert_pem)
        .bind(record.enrolled_at_unix_nano)
        .bind(record.last_seen_unix_nano)
        .bind(&record.public_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: &NodeId) -> anyhow::Result<Option<NodeRecord>> {
        let row: Option<NodeRow> =
            sqlx::query_as(
                r#"
            SELECT id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, host_info_json, public_key
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
            )| NodeRecord {
                id,
                hostname,
                labels_json,
                client_cert_pem,
                enrolled_at_unix_nano,
                last_seen_unix_nano,
                host_info_json,
                public_key,
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

    pub async fn list_all(&self) -> anyhow::Result<Vec<NodeRecord>> {
        let rows: Vec<NodeRow> =
            sqlx::query_as(
                r#"
            SELECT id, hostname, labels_json, client_cert_pem, enrolled_at_unix_nano, last_seen_unix_nano, host_info_json, public_key
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
                )| NodeRecord {
                    id,
                    hostname,
                    labels_json,
                    client_cert_pem,
                    enrolled_at_unix_nano,
                    last_seen_unix_nano,
                    host_info_json,
                    public_key,
                },
            )
            .collect())
    }
}
