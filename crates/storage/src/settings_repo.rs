//! Key-value settings storage.

use sqlx::SqlitePool;

/// Settings repository for key-value configuration.
#[derive(Clone)]
pub struct SettingsRepo {
    pool: SqlitePool,
}

impl SettingsRepo {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Get a setting value by key. Returns None if not found.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(v,)| v))
    }

    /// Set a setting value. Creates if not exists, updates if exists.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get alert retention days setting.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn alert_retention_days(&self) -> anyhow::Result<i64> {
        let value = self.get("alert_retention_days").await?;
        Ok(value.and_then(|v| v.parse().ok()).unwrap_or(365))
    }

    /// Get raw data retention days setting.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn raw_retention_days(&self) -> anyhow::Result<i64> {
        let value = self.get("raw_retention_days").await?;
        Ok(value.and_then(|v| v.parse().ok()).unwrap_or(14))
    }

    /// Get hourly aggregate retention days setting.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the query fails.
    pub async fn hourly_retention_days(&self) -> anyhow::Result<i64> {
        let value = self.get("hourly_retention_days").await?;
        Ok(value.and_then(|v| v.parse().ok()).unwrap_or(730))
    }
}
