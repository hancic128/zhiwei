//! Repository for node agent version tracking and upgrade history.

use sqlx::SqlitePool;

/// Current version record for a node.
#[derive(Debug, Clone)]
pub struct NodeVersion {
    pub node_id: String,
    pub version: String,
    pub upgraded_at_unix_nano: i64,
}

/// Upgrade history record.
#[derive(Debug, Clone)]
pub struct UpgradeHistoryEntry {
    pub id: i64,
    pub node_id: String,
    pub from_version: Option<String>,
    pub to_version: String,
    pub status: UpgradeStatus,
    pub error: Option<String>,
    pub created_at_unix_nano: i64,
    pub finished_at_unix_nano: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeStatus {
    Success,
    Failed,
    RollbackSuccess,
    RollbackFailed,
}

impl std::fmt::Display for UpgradeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
            Self::RollbackSuccess => write!(f, "rollback_success"),
            Self::RollbackFailed => write!(f, "rollback_failed"),
        }
    }
}

impl TryFrom<&str> for UpgradeStatus {
    type Error = anyhow::Error;

    fn try_from(s: &str) -> anyhow::Result<Self> {
        match s {
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            "rollback_success" => Ok(Self::RollbackSuccess),
            "rollback_failed" => Ok(Self::RollbackFailed),
            _ => anyhow::bail!("unknown upgrade status: {s}"),
        }
    }
}

/// Get the current version of a node.
///
/// # Errors
/// Returns error if database query fails.
#[allow(clippy::missing_errors_doc)]
pub async fn get_node_version(
    pool: &SqlitePool,
    node_id: &str,
) -> anyhow::Result<Option<NodeVersion>> {
    let row = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT node_id, version, upgraded_at_unix_nano FROM node_versions WHERE node_id = ?",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;

    Ok(
        row.map(|(node_id, version, upgraded_at_unix_nano)| NodeVersion {
            node_id,
            version,
            upgraded_at_unix_nano,
        }),
    )
}

/// Set or update the current version of a node.
///
/// # Errors
/// Returns error if database operation fails.
pub async fn set_node_version(
    pool: &SqlitePool,
    node_id: &str,
    version: &str,
    upgraded_at_unix_nano: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO node_versions (node_id, version, upgraded_at_unix_nano)
         VALUES (?, ?, ?)
         ON CONFLICT(node_id) DO UPDATE SET version = excluded.version, upgraded_at_unix_nano = excluded.upgraded_at_unix_nano",
    )
    .bind(node_id)
    .bind(version)
    .bind(upgraded_at_unix_nano)
    .execute(pool)
    .await?;

    Ok(())
}

/// Record an upgrade start in history.
///
/// # Errors
/// Returns error if database operation fails.
/// # Panics
/// Panics if system time is before UNIX epoch (should not happen).
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
#[allow(clippy::cast_possible_truncation)]
pub async fn record_upgrade_start(
    pool: &SqlitePool,
    node_id: &str,
    from_version: Option<&str>,
    to_version: &str,
) -> anyhow::Result<i64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    let result = sqlx::query(
        "INSERT INTO upgrade_history (node_id, from_version, to_version, status, created_at_unix_nano)
         VALUES (?, ?, ?, 'success', ?)",
    )
    .bind(node_id)
    .bind(from_version)
    .bind(to_version)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Record upgrade completion (success or failure).
///
/// # Errors
/// Returns error if database operation fails.
/// # Panics
/// Panics if system time is before UNIX epoch (should not happen).
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
#[allow(clippy::cast_possible_truncation)]
pub async fn record_upgrade_finish(
    pool: &SqlitePool,
    node_id: &str,
    to_version: &str,
    status: UpgradeStatus,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;

    // Find the most recent upgrade entry for this node+version and update it
    sqlx::query(
        "UPDATE upgrade_history
         SET status = ?, error = ?, finished_at_unix_nano = ?
         WHERE id = (
             SELECT id FROM upgrade_history
             WHERE node_id = ? AND to_version = ?
             ORDER BY created_at_unix_nano DESC
             LIMIT 1
         )",
    )
    .bind(status.to_string())
    .bind(error)
    .bind(now)
    .bind(node_id)
    .bind(to_version)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get upgrade history for a node.
///
/// # Errors
/// Returns error if database query fails.
pub async fn get_upgrade_history(
    pool: &SqlitePool,
    node_id: &str,
    limit: i64,
) -> anyhow::Result<Vec<UpgradeHistoryEntry>> {
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, String, String, Option<String>, i64, Option<i64>)>(
        "SELECT id, node_id, from_version, to_version, status, error, created_at_unix_nano, finished_at_unix_nano
         FROM upgrade_history
         WHERE node_id = ?
         ORDER BY created_at_unix_nano DESC
         LIMIT ?",
    )
    .bind(node_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(
            |(id, node_id, from_version, to_version, status, error, created_at, finished_at)| {
                Ok(UpgradeHistoryEntry {
                    id,
                    node_id,
                    from_version,
                    to_version,
                    status: status.as_str().try_into()?,
                    error,
                    created_at_unix_nano: created_at,
                    finished_at_unix_nano: finished_at,
                })
            },
        )
        .collect()
}

/// Get the latest version info across all nodes.
///
/// # Errors
/// Returns error if database query fails.
pub async fn get_all_node_versions(pool: &SqlitePool) -> anyhow::Result<Vec<NodeVersion>> {
    let rows = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT node_id, version, upgraded_at_unix_nano FROM node_versions",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(node_id, version, upgraded_at_unix_nano)| NodeVersion {
            node_id,
            version,
            upgraded_at_unix_nano,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect("sqlite::memory:")
            .await
            .unwrap();

        // Run migrations
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS node_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id TEXT NOT NULL,
                version TEXT NOT NULL,
                upgraded_at_unix_nano INTEGER NOT NULL,
                UNIQUE(node_id)
            );
            CREATE TABLE IF NOT EXISTS upgrade_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id TEXT NOT NULL,
                from_version TEXT,
                to_version TEXT NOT NULL,
                status TEXT NOT NULL,
                error TEXT,
                created_at_unix_nano INTEGER NOT NULL,
                finished_at_unix_nano INTEGER
            );
            ",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    #[allow(clippy::cast_possible_truncation)]
    async fn set_and_get_version() {
        let pool = test_pool().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;

        set_node_version(&pool, "node-1", "v1.0.0", now)
            .await
            .unwrap();

        let version = get_node_version(&pool, "node-1").await.unwrap();
        assert!(version.is_some());
        let v = version.unwrap();
        assert_eq!(v.node_id, "node-1");
        assert_eq!(v.version, "v1.0.0");

        // Update version
        set_node_version(&pool, "node-1", "v1.1.0", now + 1000)
            .await
            .unwrap();
        let version = get_node_version(&pool, "node-1").await.unwrap();
        assert_eq!(version.unwrap().version, "v1.1.0");
    }

    #[tokio::test]
    async fn upgrade_history() {
        let pool = test_pool().await;

        record_upgrade_start(&pool, "node-1", Some("v1.0.0"), "v1.1.0")
            .await
            .unwrap();

        record_upgrade_finish(&pool, "node-1", "v1.1.0", UpgradeStatus::Success, None)
            .await
            .unwrap();

        let history = get_upgrade_history(&pool, "node-1", 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].to_version, "v1.1.0");
        assert_eq!(history[0].status, UpgradeStatus::Success);
    }
}
