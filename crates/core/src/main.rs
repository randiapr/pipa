//! CDC engine: connects to OLTP databases and streams changes into Iceberg via `pipa-data`.
//!
//! Data sources are registered through the `pipa-ui` dashboard, which submits them via the
//! `pipa-rest` API. Both write to and read from the same `pipa-data` object-store-backed
//! repository, so `pipa-core` picks up newly registered sources without a direct dependency
//! on `pipa-rest`.

mod capture;

use std::sync::Arc;

use pipa_data::{
    datasource::{infrastructure::ObjectStoreDataSourceRepository, DataSource, DataSourceRepository, DbEngine},
    ObjectStoreConfig,
};

use crate::capture::{CdcSource, PostgresWalSource};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("pipa-core starting");

    let store = ObjectStoreConfig::from_env().build_store()?;
    let repository = ObjectStoreDataSourceRepository::new(store);

    let sources = repository.list().await?;
    if sources.is_empty() {
        tracing::info!("no OLTP data sources registered yet");
    }

    let postgres_source: Arc<dyn CdcSource> = Arc::new(PostgresWalSource::new());

    for source in sources {
        tracing::info!(
            id = %source.id,
            engine = ?source.engine,
            "registered OLTP data source: {}",
            source.name
        );

        match source.engine {
            DbEngine::Postgres => spawn_capture(Arc::clone(&postgres_source), source),
            DbEngine::MySql => {
                tracing::warn!(id = %source.id, "MySQL change capture is not implemented yet, skipping");
            }
        }
    }

    tokio::signal::ctrl_c().await?;
    tracing::info!("pipa-core shutting down");

    Ok(())
}

/// Streams changes from `source` and logs each one. Placeholder for the eventual Iceberg
/// writer — the point today is proving capture works end to end, not landing rows yet.
fn spawn_capture(cdc_source: Arc<dyn CdcSource>, source: DataSource) {
    tokio::spawn(async move {
        let mut changes = match cdc_source.stream_changes(&source).await {
            Ok(changes) => changes,
            Err(err) => {
                tracing::error!(id = %source.id, error = %err, "failed to start change capture");
                return;
            }
        };

        while let Some(change) = changes.recv().await {
            match change {
                Ok(event) => tracing::info!(
                    id = %source.id,
                    schema = %event.schema,
                    table = %event.table,
                    position = %event.position,
                    "captured change: {:?}",
                    event.operation
                ),
                Err(err) => tracing::error!(id = %source.id, error = %err, "change capture error"),
            }
        }

        tracing::warn!(id = %source.id, "change capture stream ended");
    });
}
