//! Certificate scan sources: node + path configured in the console.
//!
//! The node side does not store configuration - it pulls `/v1/cert-config` once before each snapshot,
//! then reports results along with inventory. Certificate entries carry `source_id` to reference here.
//! So "config changes take effect immediately" only requires the node to re-collect a snapshot.
//!
//! Empty `node_id` means **all nodes** (same path exists on many machines, one config handles it);
//! non-empty means only that specific machine. Using empty string instead of NULL: the unique index
//! `(node_id, path)` still works normally, and no extra NULL semantics handling needed for queries.

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
    /// Whether this applies to all nodes (`node_id` is empty string)
    #[must_use] pub fn is_all_nodes(&self) -> bool {
        self.node_id.is_empty()
    }
}

/// Patch for a source: only changes the provided fields (PATCH semantics)
#[derive(Debug, Clone, Default)]
pub struct CertSourcePatch {
    /// Empty string = change to "all nodes"
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
    #[must_use] pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// List all certificate scan sources.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn list_all(&self) -> anyhow::Result<Vec<CertSource>> {
        let rows: Vec<SourceRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM cert_sources ORDER BY created_at_unix_nano"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(CertSource::from).collect())
    }

    /// Sources that a specific node should scan. When `enabled_only` is true, only returns enabled ones (used by node side).
    /// Includes sources with `node_id = ''` (all nodes).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
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

    /// Look up a source by id.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn find(&self, id: &str) -> anyhow::Result<Option<CertSource>> {
        let row: Option<SourceRow> =
            sqlx::query_as(&format!("SELECT {COLS} FROM cert_sources WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(CertSource::from))
    }

    /// Insert a new certificate scan source and return the resulting row.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the insert or follow-up lookup fails, or
    /// `anyhow::Error` if the row is missing after a successful insert.
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
        .bind(i64::from(notify_enabled))
        .bind(notify_days_before)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.find(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("cert_source not found after creation"))
    }

    /// Apply a patch (PATCH semantics) to an existing source.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the lookup or update fails, or `anyhow::Error`
    /// if no source matches the given id.
    pub async fn update(
        &self,
        id: &str,
        patch: &CertSourcePatch,
        now: i64,
    ) -> anyhow::Result<CertSource> {
        let mut cur = self
            .find(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("certificate source not found"))?;
        if let Some(v) = &patch.node_id {
            cur.node_id.clone_from(v);
        }
        if let Some(v) = &patch.path {
            cur.path.clone_from(v);
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
        .bind(i64::from(cur.enabled))
        .bind(i64::from(cur.notify_enabled))
        .bind(cur.notify_days_before)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(cur)
    }

    /// Delete a source by id. Returns whether a row was actually removed.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the delete fails.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let r = sqlx::query("DELETE FROM cert_sources WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}
