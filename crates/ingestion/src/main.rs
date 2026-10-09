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
//!
//! The registered sources are read again every `INGESTION_SOURCE_REFRESH_INTERVAL_SECS`
//! (default 30): a newly registered source starts capturing, a removed one stops, and a change
//! to the tables a source ingests (chosen in the dashboard's data source explorer) is handed to
//! its running capture, which applies it from the next change on.

mod capture;
mod datasource;
mod storage;
mod write;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use object_store::ObjectStore;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use datasource::{DataSource, DataSourceId, DbEngine, TableSelection};
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

    let refresh_interval = Duration::from_secs(
        std::env::var("INGESTION_SOURCE_REFRESH_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(30)
            .max(1),
    );

    let store_config = ObjectStoreConfig::from_env();
    let store = store_config.build_store()?;
    let sources = assigned_sources(store.as_ref(), shard_index, shard_count).await?;

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

    let mut captures = Captures::new(orchestrator, Arc::clone(&writer));
    captures.reconcile(sources).await;

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut refresh = tokio::time::interval(refresh_interval);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    refresh.tick().await; // The first tick is immediate; the sources were just read.
    loop {
        tokio::select! {
            result = &mut shutdown => {
                result?;
                break;
            }
            _ = refresh.tick() => {
                match assigned_sources(store.as_ref(), shard_index, shard_count).await {
                    Ok(sources) => captures.reconcile(sources).await,
                    Err(err) => {
                        tracing::warn!(error = %err, "could not re-read the registered data sources, keeping the current ones");
                    }
                }
            }
        }
    }
    tracing::info!("pipa-ingestion shutting down");

    Ok(())
}

/// The registered data sources this shard owns.
async fn assigned_sources(
    store: &dyn ObjectStore,
    shard_index: u32,
    shard_count: u32,
) -> anyhow::Result<Vec<DataSource>> {
    Ok(datasource::list_registered(store)
        .await?
        .into_iter()
        .filter(|source| shard_of(source, shard_count) == shard_index)
        .collect())
}

/// A source's running capture task, and the handle that keeps its table selection current.
struct Capture {
    selection: watch::Sender<TableSelection>,
    task: JoinHandle<()>,
}

/// The capture tasks of this shard, kept in line with the registered sources.
struct Captures {
    orchestrator: Arc<CaptureOrchestrator>,
    /// Lists the target tables a source without a saved choice already has.
    writer: Arc<dyn IcebergWriter>,
    running: HashMap<DataSourceId, Capture>,
    /// Sources that can't be captured (MySQL, for now), so they are only warned about once.
    skipped: HashSet<DataSourceId>,
}

impl Captures {
    fn new(orchestrator: Arc<CaptureOrchestrator>, writer: Arc<dyn IcebergWriter>) -> Self {
        Self {
            orchestrator,
            writer,
            running: HashMap::new(),
            skipped: HashSet::new(),
        }
    }

    /// Starts capturing sources not seen before, hands a changed table selection to running
    /// ones, and stops those no longer registered. A source's capture keeps running even with
    /// no table chosen: stopping it would leave its replication slot unconsumed, holding WAL.
    async fn reconcile(&mut self, sources: Vec<DataSource>) {
        let registered: HashSet<DataSourceId> = sources.iter().map(|source| source.id).collect();

        for source in sources {
            let running = self.running.contains_key(&source.id);
            if !running && source.engine == DbEngine::MySql {
                if self.skipped.insert(source.id) {
                    tracing::warn!(id = %source.id, "MySQL change capture is not implemented yet, skipping");
                }
                continue;
            }
            // Can't tell yet what a source without a saved choice captures: leave a running
            // capture as it is, and don't start one — its slot keeps the changes meanwhile,
            // where starting with nothing selected would skip them.
            let Some(selection) = self.selection_of(&source).await else {
                continue;
            };

            if let Some(capture) = self.running.get(&source.id) {
                let changed = capture.selection.send_if_modified(|current| {
                    let changed = *current != selection;
                    if changed {
                        *current = selection;
                    }
                    changed
                });
                if changed {
                    tracing::info!(id = %source.id, tables = %capture.selection.borrow().describe(), "ingested tables changed");
                }
                continue;
            }

            tracing::info!(
                id = %source.id,
                engine = ?source.engine,
                tables = %selection.describe(),
                "registered OLTP data source: {}",
                source.name
            );
            let (selection_tx, selection_rx) = watch::channel(selection);
            let orchestrator = Arc::clone(&self.orchestrator);
            let id = source.id;
            let task = tokio::spawn(async move {
                orchestrator.run(source, selection_rx).await;
            });
            self.running.insert(
                id,
                Capture {
                    selection: selection_tx,
                    task,
                },
            );
        }

        self.running.retain(|id, capture| {
            let keep = registered.contains(id);
            if !keep {
                tracing::info!(%id, "data source no longer registered, stopping its capture");
                capture.task.abort();
            }
            keep
        });
        self.skipped.retain(|id| registered.contains(id));
    }

    /// What `source` captures: its saved choice, or — without one — the tables it already has
    /// in pipa. `None` when those can't be listed just now.
    async fn selection_of(&self, source: &DataSource) -> Option<TableSelection> {
        if let Some(chosen) = source.chosen_tables() {
            return Some(chosen);
        }
        match self.writer.existing_tables(source).await {
            Ok(targets) => Some(TableSelection::Existing(targets)),
            Err(err) => {
                tracing::warn!(id = %source.id, error = %err, "could not list the tables a source without a saved choice has, retrying at the next refresh");
                None
            }
        }
    }
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
