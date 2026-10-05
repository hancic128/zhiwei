//! SQLite-backed storage for zhiwei monitor.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]
// `multiple_crate_versions` flags transitive deps (e.g. ed25519-dalek pulls
// `rand_core` 0.10 while `rand` 0.8 pulls 0.6; sqlx pulls `thiserror` 2 while
// we pin 1). Not actionable from project code — pinned by upstream crates.
#![allow(clippy::multiple_crate_versions)]

pub mod ai_tokens_repo;
pub mod alerts_repo;
pub mod cert_sources_repo;
pub mod commands_repo;
pub mod inventory_repo;
pub mod migrations;
pub mod node_repo;
pub mod node_versions_repo;
pub mod probes_repo;
pub mod settings_repo;
pub mod telemetry_repo;

pub use ai_tokens_repo::AiTokensRepo;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

type SqlxPool = SqlitePool;

pub use alerts_repo::AlertsRepo;
pub use cert_sources_repo::CertSourcesRepo;
pub use commands_repo::CommandsRepo;
pub use inventory_repo::InventoryRepo;
pub use node_repo::NodeRepo;
pub use node_versions_repo::{
    get_node_version, get_upgrade_history, record_upgrade_finish, record_upgrade_start,
    set_node_version, NodeVersion, UpgradeHistoryEntry, UpgradeStatus,
};
pub use probes_repo::ProbesRepo;
pub use settings_repo::SettingsRepo;
pub use telemetry_repo::TelemetryRepo;

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    /// Open a `SQLite` database at `path`, configure `WAL` mode, and run migrations.
    ///
    /// # Errors
    ///
    /// Returns `anyhow::Error` if the connection fails, the pool cannot be
    /// created, or migrations fail.
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let url = format!("sqlite://{}", path.as_ref().display());
        let opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;

        migrations::run(&pool).await?;

        Ok(Self { pool })
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub fn nodes(&self) -> NodeRepo {
        NodeRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn telemetry(&self) -> TelemetryRepo {
        TelemetryRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn inventory(&self) -> InventoryRepo {
        InventoryRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn alerts(&self) -> AlertsRepo {
        AlertsRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn commands(&self) -> CommandsRepo {
        CommandsRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn probes(&self) -> ProbesRepo {
        ProbesRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn cert_sources(&self) -> CertSourcesRepo {
        CertSourcesRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn ai_tokens(&self) -> AiTokensRepo {
        AiTokensRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn settings(&self) -> SettingsRepo {
        SettingsRepo::new(self.pool.clone())
    }

    #[must_use]
    pub fn node_versions(&self) -> NodeVersionsRepo {
        NodeVersionsRepo::new(self.pool.clone())
    }
}

/// Repository for node agent version tracking and upgrade history.
#[derive(Clone)]
pub struct NodeVersionsRepo {
    pool: SqlxPool,
}

#[allow(clippy::cast_possible_truncation)]
impl NodeVersionsRepo {
    #[must_use]
    pub const fn new(pool: SqlxPool) -> Self {
        Self { pool }
    }

    /// Get the current version of a node.
    ///
    /// # Errors
    /// Returns error if database query fails.
    #[allow(clippy::missing_errors_doc)]
    pub async fn get(&self, node_id: &str) -> anyhow::Result<Option<NodeVersion>> {
        node_versions_repo::get_node_version(&self.pool, node_id).await
    }

    /// Set or update the current version of a node.
    ///
    /// # Errors
    /// Returns error if database operation fails.
    /// # Panics
    /// Panics if system time is before UNIX epoch (should not happen).
    #[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
    pub async fn set(&self, node_id: &str, version: &str) -> anyhow::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        node_versions_repo::set_node_version(&self.pool, node_id, version, now).await
    }

    /// Record an upgrade start in history.
    ///
    /// # Errors
    /// Returns error if database operation fails.
    pub async fn record_upgrade_start(
        &self,
        node_id: &str,
        from_version: Option<&str>,
        to_version: &str,
    ) -> anyhow::Result<i64> {
        node_versions_repo::record_upgrade_start(&self.pool, node_id, from_version, to_version)
            .await
    }

    /// Record upgrade completion (success or failure).
    ///
    /// # Errors
    /// Returns error if database operation fails.
    pub async fn record_upgrade_finish(
        &self,
        node_id: &str,
        to_version: &str,
        status: UpgradeStatus,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        node_versions_repo::record_upgrade_finish(&self.pool, node_id, to_version, status, error)
            .await
    }

    /// Get upgrade history for a node.
    ///
    /// # Errors
    /// Returns error if database query fails.
    pub async fn history(
        &self,
        node_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<UpgradeHistoryEntry>> {
        node_versions_repo::get_upgrade_history(&self.pool, node_id, limit).await
    }

    /// Get the latest version info across all nodes.
    ///
    /// # Errors
    /// Returns error if database query fails.
    pub async fn get_all(&self) -> anyhow::Result<Vec<NodeVersion>> {
        node_versions_repo::get_all_node_versions(&self.pool).await
    }
}
