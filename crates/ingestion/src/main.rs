// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

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
//! Designed to run as multiple independent instances: the `coordination` module decides which
//! instance captures which source, because a Postgres replication slot can only be consumed by
//! one client at a time, so two instances must never both attach to the same source. Without
//! `REDIS_URL` that is static sharding by id (`INGESTION_SHARD_INDEX`/`INGESTION_SHARD_COUNT`);
//! with it, instances discover each other through Redis, sources are shared out by rendezvous
//! hashing and a dead instance's sources move to the others, so instances can be added and
//! removed freely. Either way, handing a source to another instance is safe for delivery
//! guarantees because checkpoint state lives durably in Iceberg itself (see
//! `write::domain::IcebergWriter`), not in-process — it behaves exactly like a cold restart.
//!
//! The registered sources are read again every `INGESTION_SOURCE_REFRESH_INTERVAL_SECS`
//! (default 30): a newly registered source starts capturing, a removed one stops, and a change
//! to the tables a source ingests (chosen in the dashboard's data source explorer) is handed to
//! its running capture, which applies it from the next change on.
//!
//! Every `INGESTION_MAINTENANCE_INTERVAL_SECS` (default 300), the target tables of the sources an
//! instance captures are compacted and their old snapshots expired (`maintenance`), so they stay
//! fast to read however long ingestion runs.

mod capture;
mod coordination;
mod datasource;
mod maintenance;
mod postgres;
mod storage;
mod write;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use datasource::{DataSource, DataSourceId, DbEngine, TableSelection};
use storage::ObjectStoreConfig;

use crate::capture::{
    BackfillConfig, BatchConfig, CaptureOrchestrator, CdcSource, PostgresSnapshotSource,
    PostgresWalSource, SnapshotSource,
};
use crate::coordination::Coordinator;
use crate::maintenance::{
    IcebergTableMaintainer, MaintenanceConfig, MaintenanceScheduler, OrphanSweeper,
    RestSnapshotCommitter,
};
use crate::write::{IcebergCatalogConfig, IcebergChangelogWriter, IcebergWriter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    tracing::info!("pipa-ingestion starting");
    let coordination = coordination::from_env()?;
    let coordinator = coordination.coordinator;

    let refresh_interval = Duration::from_secs(
        std::env::var("INGESTION_SOURCE_REFRESH_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(30)
            .max(1),
    );

    let store_config = ObjectStoreConfig::from_env();
    let store = store_config.build_store()?;

    let catalog_config = IcebergCatalogConfig::from_env(&store_config).await?;
    let catalog = catalog_config.build_catalog(&store_config).await?;
    let writer: Arc<dyn IcebergWriter> =
        Arc::new(IcebergChangelogWriter::new(Arc::clone(&catalog)));
    let postgres_source: Arc<dyn CdcSource> = Arc::new(PostgresWalSource::new());
    let mut orchestrator = CaptureOrchestrator::new(
        Arc::clone(&postgres_source),
        Arc::clone(&writer),
        BatchConfig::from_env(),
    );
    match BackfillConfig::from_env() {
        Some(config) => {
            tracing::info!(chunk_rows = config.chunk_rows, "snapshot backfill enabled");
            let snapshots: Arc<dyn SnapshotSource> = Arc::new(PostgresSnapshotSource::new());
            orchestrator = orchestrator.with_backfill(snapshots, config);
        }
        None => tracing::info!("snapshot backfill disabled"),
    }
    let orchestrator = Arc::new(orchestrator);

    let (owned_tx, owned_rx) = watch::channel(Vec::new());
    let maintenance_config = MaintenanceConfig::from_env();
    let maintenance = match maintenance_config.interval {
        Some(interval) => {
            let orphans = maintenance_config.orphan_min_age.map(|min_age| {
                OrphanSweeper::new(
                    Arc::clone(&store),
                    format!("s3://{}/", store_config.bucket),
                    min_age,
                )
            });
            let maintainer = Arc::new(IcebergTableMaintainer::new(
                Arc::clone(&catalog),
                Arc::new(RestSnapshotCommitter::new(&catalog_config)),
                maintenance_config.compaction,
                maintenance_config.expiry,
                orphans,
            ));
            tracing::info!(
                interval_secs = interval.as_secs(),
                "table maintenance enabled"
            );
            Some(tokio::spawn(
                MaintenanceScheduler::new(maintainer, interval).run(owned_rx),
            ))
        }
        None => {
            tracing::info!("table maintenance disabled");
            None
        }
    };

    let mut captures = Captures::new(orchestrator, Arc::clone(&writer));
    let mut assignment = Assignment::new(coordinator, owned_tx);
    assignment.registered = datasource::list_registered(store.as_ref()).await?;
    assignment.coordinate(&mut captures, true).await;

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut refresh = tokio::time::interval(refresh_interval);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    refresh.tick().await; // The first tick is immediate; the sources were just read.
    let mut coordinate = tokio::time::interval(coordination.interval);
    coordinate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    coordinate.tick().await;
    loop {
        tokio::select! {
            result = &mut shutdown => {
                result?;
                break;
            }
            _ = refresh.tick() => {
                match datasource::list_registered(store.as_ref()).await {
                    Ok(registered) => assignment.registered = registered,
                    Err(err) => {
                        tracing::warn!(error = %err, "could not re-read the registered data sources, keeping the current ones");
                    }
                }
                assignment.coordinate(&mut captures, true).await;
            }
            _ = coordinate.tick() => {
                assignment.coordinate(&mut captures, false).await;
            }
        }
    }
    tracing::info!("pipa-ingestion shutting down");
    // Before the sources are handed back, so their new owners don't race a compaction here.
    if let Some(maintenance) = maintenance {
        maintenance.abort();
    }
    assignment.leave(&mut captures).await;

    Ok(())
}

/// Logs to stdout: JSON, one object per line, with `LOG_FORMAT=json` (what `just local::up` sets,
/// for the files under `logs/`), otherwise human-readable text — coloured only on a terminal,
/// so redirecting it to a file doesn't fill it with escape codes.
fn init_tracing() {
    use std::io::IsTerminal;

    if std::env::var("LOG_FORMAT").is_ok_and(|format| format.eq_ignore_ascii_case("json")) {
        tracing_subscriber::fmt().json().flatten_event(true).init();
    } else {
        tracing_subscriber::fmt()
            .with_ansi(std::io::stdout().is_terminal())
            .init();
    }
}

/// What the coordinator says this instance captures out of the registered sources, kept in line
/// with the running captures.
struct Assignment {
    coordinator: Arc<dyn Coordinator>,
    registered: Vec<DataSource>,
    owned: HashSet<DataSourceId>,
    /// The sources captured here, for table maintenance.
    owned_sources: watch::Sender<Vec<DataSource>>,
}

impl Assignment {
    fn new(
        coordinator: Arc<dyn Coordinator>,
        owned_sources: watch::Sender<Vec<DataSource>>,
    ) -> Self {
        Self {
            coordinator,
            registered: Vec::new(),
            owned: HashSet::new(),
            owned_sources,
        }
    }

    /// Asks the coordinator which sources are ours and applies the answer. The running captures
    /// are reconciled when the assignment changed, or when `force`d (the registered sources were
    /// just re-read, which can change a source's table selection without changing who owns it).
    ///
    /// When the coordinator can't answer, the current assignment stands: an outage of the
    /// coordination service never stops a running capture, which would only let its slot hold
    /// WAL back.
    async fn coordinate(&mut self, captures: &mut Captures, force: bool) {
        let ids: Vec<DataSourceId> = self.registered.iter().map(|source| source.id).collect();
        let owned = match self.coordinator.assign(&ids).await {
            Ok(owned) => owned,
            Err(err) => {
                tracing::warn!(error = %err, "could not coordinate, keeping the current assignment");
                if !force {
                    return;
                }
                self.owned.clone()
            }
        };
        let changed = owned != self.owned;
        if !changed && !force {
            return;
        }

        let mine: Vec<DataSource> = self
            .registered
            .iter()
            .filter(|source| owned.contains(&source.id))
            .cloned()
            .collect();
        captures.reconcile(mine.clone()).await;
        self.owned_sources.send_replace(mine);

        // Only now that those captures are stopped can another instance take the slot.
        let dropped: Vec<DataSourceId> = self.owned.difference(&owned).copied().collect();
        if !dropped.is_empty() {
            tracing::info!(
                sources = dropped.len(),
                "no longer capturing sources that moved to another instance or were removed"
            );
            self.coordinator.release(&dropped).await;
        }
        self.owned = owned;
    }

    /// Stops every capture and hands the sources back.
    async fn leave(&mut self, captures: &mut Captures) {
        captures.reconcile(Vec::new()).await;
        self.owned_sources.send_replace(Vec::new());
        let held: Vec<DataSourceId> = self.owned.drain().collect();
        self.coordinator.leave(&held).await;
    }
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
                tracing::info!(%id, "data source no longer registered or assigned to this instance, stopping its capture");
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
