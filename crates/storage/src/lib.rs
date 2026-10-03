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
pub mod probes_repo;
pub mod telemetry_repo;

pub use ai_tokens_repo::AiTokensRepo;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

pub use alerts_repo::AlertsRepo;
pub use cert_sources_repo::CertSourcesRepo;
pub use commands_repo::CommandsRepo;
pub use inventory_repo::InventoryRepo;
pub use node_repo::NodeRepo;
pub use probes_repo::ProbesRepo;
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

    #[must_use] pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use] pub fn nodes(&self) -> NodeRepo {
        NodeRepo::new(self.pool.clone())
    }

    #[must_use] pub fn telemetry(&self) -> TelemetryRepo {
        TelemetryRepo::new(self.pool.clone())
    }

    #[must_use] pub fn inventory(&self) -> InventoryRepo {
        InventoryRepo::new(self.pool.clone())
    }

    #[must_use] pub fn alerts(&self) -> AlertsRepo {
        AlertsRepo::new(self.pool.clone())
    }

    #[must_use] pub fn commands(&self) -> CommandsRepo {
        CommandsRepo::new(self.pool.clone())
    }

    #[must_use] pub fn probes(&self) -> ProbesRepo {
        ProbesRepo::new(self.pool.clone())
    }

    #[must_use] pub fn cert_sources(&self) -> CertSourcesRepo {
        CertSourcesRepo::new(self.pool.clone())
    }

    #[must_use] pub fn ai_tokens(&self) -> AiTokensRepo {
        AiTokensRepo::new(self.pool.clone())
    }
}
