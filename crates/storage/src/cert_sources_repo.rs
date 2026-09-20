//! 证书扫描来源：控制台配置的「节点 + 路径」。
//!
//! 节点侧不存配置——每次采集快照前拉一次 `/v1/cert-config`，扫完把结果随
//! inventory 上报，证书条目带上 `source_id` 指回这里。因此「改了配置立刻
//! 生效」只需要让节点重采一次快照。
//!
//! `node_id` 为空串表示**所有节点**（同一路径在很多机器上都有，一条配置搞定）；
//! 非空则只作用于那一台。用空串而不是 NULL：唯一索引 `(node_id, path)` 照常生效，
//! 查询也不用额外处理 NULL 语义。

use sqlx::SqlitePool;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CertSource {
    pub id: String,
    pub node_id: String,
    pub path: String,
    pub enabled: bool,
    pub notify_enabled: bool,
    pub notify_days_before: i64,
    pub created_at_unix_nano: i64,
    pub updated_at_unix_nano: i64,
}

impl CertSource {
    /// 是否作用于所有节点（`node_id` 为空串）
    pub fn is_all_nodes(&self) -> bool {
        self.node_id.is_empty()
    }
}

/// 修改来源：只改给出的字段（PATCH 语义）
#[derive(Debug, Clone, Default)]
pub struct CertSourcePatch {
    /// 空串 = 改成「所有节点」
    pub node_id: Option<String>,
    pub path: Option<String>,
    pub enabled: Option<bool>,
    pub notify_enabled: Option<bool>,
    pub notify_days_before: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct SourceRow {
    id: String,
    node_id: String,
    path: String,
    enabled: i64,
    notify_enabled: i64,
    notify_days_before: i64,
    created_at_unix_nano: i64,
    updated_at_unix_nano: i64,
}

impl From<SourceRow> for CertSource {
    fn from(r: SourceRow) -> Self {
        Self {
            id: r.id,
            node_id: r.node_id,
            path: r.path,
            enabled: r.enabled != 0,
            notify_enabled: r.notify_enabled != 0,
            notify_days_before: r.notify_days_before,
            created_at_unix_nano: r.created_at_unix_nano,
            updated_at_unix_nano: r.updated_at_unix_nano,
        }
    }
}

const COLS: &str = "id, node_id, path, enabled, notify_enabled, notify_days_before,
                    created_at_unix_nano, updated_at_unix_nano";

#[derive(Clone)]
pub struct CertSourcesRepo {
    pool: SqlitePool,
}

impl CertSourcesRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list_all(&self) -> anyhow::Result<Vec<CertSource>> {
        let rows: Vec<SourceRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM cert_sources ORDER BY created_at_unix_nano"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(CertSource::from).collect())
    }

    /// 某节点要扫描的来源。`enabled_only` 为真时只返回启用的（节点侧用）。
    /// 包含 `node_id = ''` 的「所有节点」来源。
    pub async fn list_for_node(
        &self,
        node_id: &str,
        enabled_only: bool,
    ) -> anyhow::Result<Vec<CertSource>> {
        let sql = format!(
            "SELECT {COLS} FROM cert_sources
             WHERE (node_id = ? OR node_id = ''){}
             ORDER BY created_at_unix_nano",
            if enabled_only { " AND enabled = 1" } else { "" }
        );
        let rows: Vec<SourceRow> = sqlx::query_as(&sql)
            .bind(node_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(CertSource::from).collect())
    }

    pub async fn find(&self, id: &str) -> anyhow::Result<Option<CertSource>> {
        let row: Option<SourceRow> =
            sqlx::query_as(&format!("SELECT {COLS} FROM cert_sources WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(CertSource::from))
    }

    pub async fn create(
        &self,
        node_id: &str,
        path: &str,
        notify_enabled: bool,
        notify_days_before: i64,
        now: i64,
    ) -> anyhow::Result<CertSource> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO cert_sources
             (id, node_id, path, enabled, notify_enabled, notify_days_before,
              created_at_unix_nano, updated_at_unix_nano)
             VALUES (?, ?, ?, 1, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(node_id)
        .bind(path)
        .bind(notify_enabled as i64)
        .bind(notify_days_before)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.find(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("cert_source 创建后读不到"))
    }

    pub async fn update(
        &self,
        id: &str,
        patch: &CertSourcePatch,
        now: i64,
    ) -> anyhow::Result<CertSource> {
        let mut cur = self
            .find(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("证书来源不存在"))?;
        if let Some(v) = &patch.node_id {
            cur.node_id = v.clone();
        }
        if let Some(v) = &patch.path {
            cur.path = v.clone();
        }
        if let Some(v) = patch.enabled {
            cur.enabled = v;
        }
        if let Some(v) = patch.notify_enabled {
            cur.notify_enabled = v;
        }
        if let Some(v) = patch.notify_days_before {
            cur.notify_days_before = v;
        }
        sqlx::query(
            "UPDATE cert_sources SET node_id = ?, path = ?, enabled = ?, notify_enabled = ?,
                    notify_days_before = ?, updated_at_unix_nano = ? WHERE id = ?",
        )
        .bind(&cur.node_id)
        .bind(&cur.path)
        .bind(cur.enabled as i64)
        .bind(cur.notify_enabled as i64)
        .bind(cur.notify_days_before)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(cur)
    }

    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let r = sqlx::query("DELETE FROM cert_sources WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}
