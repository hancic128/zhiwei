//! SQLite-backed storage for zhiwei monitor.

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

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn nodes(&self) -> NodeRepo {
        NodeRepo::new(self.pool.clone())
    }

    pub fn telemetry(&self) -> TelemetryRepo {
        TelemetryRepo::new(self.pool.clone())
    }

    pub fn inventory(&self) -> InventoryRepo {
        InventoryRepo::new(self.pool.clone())
    }

    pub fn alerts(&self) -> AlertsRepo {
        AlertsRepo::new(self.pool.clone())
    }

    pub fn commands(&self) -> CommandsRepo {
        CommandsRepo::new(self.pool.clone())
    }

    pub fn probes(&self) -> ProbesRepo {
        ProbesRepo::new(self.pool.clone())
    }

    pub fn cert_sources(&self) -> CertSourcesRepo {
        CertSourcesRepo::new(self.pool.clone())
    }

    pub fn ai_tokens(&self) -> AiTokensRepo {
        AiTokensRepo::new(self.pool.clone())
    }
}
