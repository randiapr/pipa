//! Ingestion engine: connects to OLTP databases and streams captured changes into Iceberg.
//!
//! Standalone service: it takes no dependency on any other crate in this workspace. Data
//! sources are registered through the `pipa-ui` dashboard, which submits them via `pipa-backend`;
//! `pipa-ingestion` only ever reads what that writes, via the shared RustFS/S3 object store's
//! `datasources/` JSON layout (`datasource.rs` duplicates just enough of the shape to
//! deserialize it), never through a shared Rust crate. That keeps its release and deploy cycle
//! fully independent of `pipa-storage`/`pipa-backend`'s.
//!
//! Designed to run as multiple independent instances: `INGESTION_SHARD_INDEX`/
//! `INGESTION_SHARD_COUNT` (default `0`/`1`, i.e. a single instance handling everything) split
//! the registered sources deterministically by id, so N instances can each own a disjoint
//! subset without coordinating — necessary because a Postgres replication slot can only be
//! consumed by one client at a time, so two instances must never both attach to the same
//! source. This is static sharding, not consistent hashing: changing `INGESTION_SHARD_COUNT`
//! reassigns most sources to a different shard, briefly interrupting their capture as the old
//! shard's slot is released and the new shard's instance reattaches.

mod capture;
mod datasource;
mod storage;

use std::sync::Arc;

use datasource::{DataSource, DbEngine};
use storage::ObjectStoreConfig;

use crate::capture::{CdcSource, PostgresWalSource};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let shard_index: u32 = std::env::var("INGESTION_SHARD_INDEX")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let shard_count: u32 = std::env::var("INGESTION_SHARD_COUNT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1)
        .max(1);
    if shard_index >= shard_count {
        anyhow::bail!(
            "INGESTION_SHARD_INDEX ({shard_index}) must be less than INGESTION_SHARD_COUNT ({shard_count})"
        );
    }

    tracing::info!(shard_index, shard_count, "pipa-ingestion starting");

    let store = ObjectStoreConfig::from_env().build_store()?;
    let sources: Vec<DataSource> = datasource::list_registered(store.as_ref())
        .await?
        .into_iter()
        .filter(|source| shard_of(source, shard_count) == shard_index)
        .collect();

    if sources.is_empty() {
        tracing::info!("no OLTP data sources assigned to this shard");
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
    tracing::info!("pipa-ingestion shutting down");

    Ok(())
}

/// Deterministically assigns `source` to one of `shard_count` shards from its id — stable
/// across restarts and independent of registration order.
fn shard_of(source: &DataSource, shard_count: u32) -> u32 {
    (source.id.0.as_u128() % u128::from(shard_count)) as u32
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
