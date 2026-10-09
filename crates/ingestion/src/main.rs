//! Ingestion engine: connects to OLTP databases and streams captured changes into Iceberg.
//!
//! Standalone service: the only workspace crate it depends on is `pipa-catalog-proxy`, which it
//! embeds to sign its requests to RustFS's Iceberg catalog — not `pipa-backend`, not even for its
//! `iceberg`/`iceberg-catalog-rest` access (`write/` keeps its own independent copy of that
//! dependency; see the root `CLAUDE.md`). Data sources
//! are registered through the `pipa-ui` dashboard, which submits them via `pipa-backend`;
//! `pipa-ingestion` only ever reads what that writes, via the shared RustFS/S3 object store's
//! `datasources/` JSON layout (`datasource.rs` duplicates just enough of the shape to
//! deserialize it), never through a shared Rust crate. That keeps its release and deploy cycle
//! fully independent of `pipa-backend`'s.
//!
//! Designed to run as multiple independent instances: `INGESTION_SHARD_INDEX`/
//! `INGESTION_SHARD_COUNT` (default `0`/`1`, i.e. a single instance handling everything) split
//! the registered sources deterministically by id, so N instances can each own a disjoint
//! subset without coordinating — necessary because a Postgres replication slot can only be
//! consumed by one client at a time, so two instances must never both attach to the same
//! source. This is static sharding, not consistent hashing: changing `INGESTION_SHARD_COUNT`
//! reassigns most sources to a different shard, briefly interrupting their capture as the old
//! shard's slot is released and the new shard's instance reattaches. That reassignment is safe
//! for delivery guarantees because checkpoint state lives durably in Iceberg itself (see
//! `write::domain::IcebergWriter`), not in-process — a source moving to a different instance
//! behaves exactly like a cold restart of the old one.

mod capture;
mod datasource;
mod storage;
mod write;

use std::sync::Arc;

use tokio::signal::unix::{SignalKind, signal};

use datasource::{DataSource, DbEngine};
use storage::ObjectStoreConfig;

use crate::capture::{BatchConfig, CaptureOrchestrator, CdcSource, PostgresWalSource};
use crate::write::{IcebergCatalogConfig, IcebergChangelogWriter, IcebergWriter};

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

    let store_config = ObjectStoreConfig::from_env();
    let store = store_config.build_store()?;
    let sources: Vec<DataSource> = datasource::list_registered(store.as_ref())
        .await?
        .into_iter()
        .filter(|source| shard_of(source, shard_count) == shard_index)
        .collect();

    if sources.is_empty() {
        tracing::info!("no OLTP data sources assigned to this shard");
    }

    let catalog = IcebergCatalogConfig::from_env(&store_config)
        .await?
        .build_catalog(&store_config)
        .await?;
    let writer: Arc<dyn IcebergWriter> = Arc::new(IcebergChangelogWriter::new(catalog));
    let postgres_source: Arc<dyn CdcSource> = Arc::new(PostgresWalSource::new());
    let orchestrator = Arc::new(CaptureOrchestrator::new(
        Arc::clone(&postgres_source),
        Arc::clone(&writer),
        BatchConfig::from_env(),
    ));

    for source in sources {
        tracing::info!(
            id = %source.id,
            engine = ?source.engine,
            "registered OLTP data source: {}",
            source.name
        );

        match source.engine {
            DbEngine::Postgres => spawn_capture(Arc::clone(&orchestrator), source),
            DbEngine::MySql => {
                tracing::warn!(id = %source.id, "MySQL change capture is not implemented yet, skipping");
            }
        }
    }

    shutdown_signal().await?;
    tracing::info!("pipa-ingestion shutting down");

    Ok(())
}

/// Waits for Ctrl-C or SIGTERM. SIGTERM is what `docker stop` sends, and as a container's PID 1
/// the process would otherwise ignore it and get killed only after the stop timeout. Nothing
/// needs flushing on the way out: buffered events were never confirmed, so the next run gets
/// them again from the slot.
async fn shutdown_signal() -> anyhow::Result<()> {
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {}
    }
    Ok(())
}

/// Deterministically assigns `source` to one of `shard_count` shards from its id — stable
/// across restarts and independent of registration order.
fn shard_of(source: &DataSource, shard_count: u32) -> u32 {
    (source.id.0.as_u128() % u128::from(shard_count)) as u32
}

/// Drives capture + write for `source` on its own task via `orchestrator`.
fn spawn_capture(orchestrator: Arc<CaptureOrchestrator>, source: DataSource) {
    tokio::spawn(async move {
        orchestrator.run(source).await;
    });
}
