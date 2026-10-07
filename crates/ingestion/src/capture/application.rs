//! Orchestration layer driving a [`CdcSource`] and an [`IcebergWriter`] together: computes a
//! safe resume position on startup, batches captured events per target table, and only
//! advances the source's own WAL checkpoint (via [`ChangeStream::confirm`]) after a batch has
//! durably committed to Iceberg.
//!
//! This is the piece `capture/mod.rs` used to describe as future work before an Iceberg write
//! path existed. See the root `CLAUDE.md`'s `pipa-ingestion::capture` architecture section for
//! the exactly-once design this implements: the checkpoint is a per-table high-watermark
//! position stored *in* the Iceberg snapshot each commit produces (see
//! [`crate::write::domain::IcebergWriter`]), not a separate side channel — so a crash can
//! never leave the checkpoint and the data it describes out of sync with each other.
//!
//! Scoped to Postgres: position comparison here leans on `pgwire_replication::Lsn`'s `u64`
//! total order, which a future MySQL GTID-set-based source would not have.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pgwire_replication::Lsn;
use tokio::time::Instant;

use crate::capture::domain::{CaptureError, CdcSource, ChangeEvent, ChangeStream};
use crate::datasource::DataSource;
use crate::write::domain::{IcebergWriter, TargetTable};

/// Batching knobs. The right commit cadence is workload-dependent — frequent commits keep the
/// crash-replay window small but risk Iceberg's small-file problem (more manifests/data files
/// per row landed); infrequent commits are the opposite tradeoff. Defaults sit in the same
/// 1–10s / hundreds-to-low-thousands-of-rows range most CDC connectors use for this reason.
#[derive(Debug, Clone, Copy)]
pub struct BatchConfig {
    pub max_events: usize,
    pub max_flush_interval: Duration,
}

impl BatchConfig {
    /// Reads `INGESTION_BATCH_MAX_EVENTS`/`INGESTION_BATCH_FLUSH_INTERVAL_MS`, defaulting to
    /// 1000 events / 5s per target table.
    pub fn from_env() -> Self {
        Self {
            max_events: std::env::var("INGESTION_BATCH_MAX_EVENTS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1000),
            max_flush_interval: Duration::from_millis(
                std::env::var("INGESTION_BATCH_FLUSH_INTERVAL_MS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(5_000),
            ),
        }
    }
}

/// Per-table batching/dedup state. `watermark = None` means "no confirmed baseline for this
/// table yet" — covering both a genuinely fresh table (dedup should let everything through)
/// and a transient readback failure (dedup and the global confirm floor both fail safe by
/// treating it the same as fresh: never skip, never let the global checkpoint advance past
/// it).
struct TableState {
    target: TargetTable,
    watermark: Option<u64>,
    pending: Vec<ChangeEvent>,
    last_flush: Instant,
}

/// Drives capture + write for one data source. Constructed once per Postgres/MySQL adapter and
/// writer pair; `run` is spawned per registered source, mirroring how `main.rs` already spawns
/// one task per source for `CdcSource` alone.
pub struct CaptureOrchestrator {
    cdc_source: Arc<dyn CdcSource>,
    writer: Arc<dyn IcebergWriter>,
    batch: BatchConfig,
}

impl CaptureOrchestrator {
    pub fn new(
        cdc_source: Arc<dyn CdcSource>,
        writer: Arc<dyn IcebergWriter>,
        batch: BatchConfig,
    ) -> Self {
        Self {
            cdc_source,
            writer,
            batch,
        }
    }

    /// Drives `source` until its stream ends or errors. Does not retry/reconnect — mirrors the
    /// original `spawn_capture`'s behavior of logging and returning; a supervising restart
    /// policy is future work, orthogonal to the exactly-once mechanism itself (a restart is
    /// just another crash-and-resume, which this design already handles safely).
    pub async fn run(&self, source: DataSource) {
        if let Err(err) = self.run_once(&source).await {
            tracing::error!(id = %source.id, error = %err, "capture orchestrator stopped");
        }
    }

    async fn run_once(&self, source: &DataSource) -> Result<(), CaptureError> {
        let resume_from = self.resume_position(source).await?;
        tracing::info!(id = %source.id, resume_from = ?resume_from, "starting capture");

        let mut stream = self
            .cdc_source
            .stream_changes(source, resume_from.as_deref())
            .await?;

        let mut tables: HashMap<TargetTable, TableState> = HashMap::new();
        let mut last_confirmed: Option<u64> = None;
        let mut ticker = tokio::time::interval(self.batch.max_flush_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                event = stream.events.recv() => {
                    match event {
                        Some(Ok(event)) => {
                            self.handle_event(source, &mut tables, event).await;
                            self.confirm_floor(&stream, &tables, &mut last_confirmed).await;
                        }
                        Some(Err(err)) => {
                            tracing::error!(id = %source.id, error = %err, "change capture error");
                        }
                        None => {
                            tracing::warn!(id = %source.id, "change capture stream ended");
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    self.flush_due(source, &mut tables).await;
                    self.confirm_floor(&stream, &tables, &mut last_confirmed).await;
                }
            }
        }

        Ok(())
    }

    /// Computes the safe resume position across every target table this source has ever
    /// written to: the `min` of their last committed positions, or `None` (fall back to the
    /// source adapter's own default) if any existing target has never been durably committed
    /// to — see [`TableState`]'s doc comment for why that's the safe choice.
    async fn resume_position(&self, source: &DataSource) -> Result<Option<String>, CaptureError> {
        let targets = self
            .writer
            .existing_targets(source)
            .await
            .map_err(|err| CaptureError::Checkpoint(err.to_string()))?;

        let mut floor: Option<u64> = None;
        for target in &targets {
            let position = self
                .writer
                .last_committed_position(target)
                .await
                .map_err(|err| CaptureError::Checkpoint(err.to_string()))?;

            let Some(position) = position else {
                return Ok(None);
            };

            let parsed = parse_position(&position)?;
            floor = Some(floor.map_or(parsed, |current| current.min(parsed)));
        }

        Ok(floor.map(|lsn| Lsn::from(lsn).to_string()))
    }

    async fn seed_watermark(&self, target: &TargetTable) -> Option<u64> {
        match self.writer.last_committed_position(target).await {
            Ok(Some(position)) => match parse_position(&position) {
                Ok(parsed) => Some(parsed),
                Err(err) => {
                    tracing::error!(?target, error = %err, "unparseable committed position readback, treating table as unconfirmed");
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                tracing::error!(?target, error = %err, "failed to read back last committed position, treating table as unconfirmed");
                None
            }
        }
    }

    async fn handle_event(
        &self,
        source: &DataSource,
        tables: &mut HashMap<TargetTable, TableState>,
        event: ChangeEvent,
    ) {
        let target = TargetTable::for_event(source, &event);

        let position = match parse_position(&event.position) {
            Ok(position) => position,
            Err(err) => {
                tracing::error!(id = %source.id, error = %err, "dropping event with an unparseable position");
                return;
            }
        };

        if !tables.contains_key(&target) {
            let watermark = self.seed_watermark(&target).await;
            tables.insert(
                target.clone(),
                TableState {
                    target: target.clone(),
                    watermark,
                    pending: Vec::new(),
                    last_flush: Instant::now(),
                },
            );
        }

        let state = tables.get_mut(&target).expect("just ensured present");

        if let Some(watermark) = state.watermark
            && position <= watermark
        {
            // Already durably committed in a previous run — a redelivery following a crash
            // between decode and commit, or after a shard reassignment. Drop it silently.
            return;
        }

        state.pending.push(event);

        if state.pending.len() >= self.batch.max_events {
            self.flush_table(source, state).await;
        }
    }

    async fn flush_due(&self, source: &DataSource, tables: &mut HashMap<TargetTable, TableState>) {
        let now = Instant::now();
        for state in tables.values_mut() {
            if !state.pending.is_empty()
                && now.duration_since(state.last_flush) >= self.batch.max_flush_interval
            {
                self.flush_table(source, state).await;
            }
        }
    }

    /// Commits `state`'s pending batch. On success, advances `state.watermark` to the batch's
    /// high-watermark position (its last event — pending events are appended in the WAL's own
    /// monotonic order, so the last one is always the highest). On failure, leaves `pending`
    /// and `watermark` untouched so the batch is retried on the next flush trigger.
    async fn flush_table(&self, source: &DataSource, state: &mut TableState) {
        if state.pending.is_empty() {
            state.last_flush = Instant::now();
            return;
        }

        let high_watermark_position = state
            .pending
            .last()
            .expect("checked non-empty above")
            .position
            .clone();

        match self
            .writer
            .commit_batch(
                source,
                &state.target,
                &state.pending,
                &high_watermark_position,
            )
            .await
        {
            Ok(()) => {
                match parse_position(&high_watermark_position) {
                    Ok(parsed) => state.watermark = Some(parsed),
                    Err(err) => tracing::error!(
                        id = %source.id,
                        target = ?state.target,
                        error = %err,
                        "committed batch but couldn't parse its own high-watermark position back",
                    ),
                }
                tracing::info!(
                    id = %source.id,
                    target = ?state.target,
                    events = state.pending.len(),
                    "committed batch to Iceberg",
                );
                state.pending.clear();
                state.last_flush = Instant::now();
            }
            Err(err) => {
                tracing::error!(
                    id = %source.id,
                    target = ?state.target,
                    error = %err,
                    "failed to commit batch, leaving it buffered for retry",
                );
            }
        }
    }

    /// Sends the `min` watermark across every table touched this run as the new confirm
    /// position — unless at least one table has no confirmed baseline yet (`watermark ==
    /// None`), in which case nothing is sent: advancing the WAL past a table with events only
    /// in memory would let Postgres discard WAL the process hasn't durably landed anywhere. A
    /// table that hasn't been touched at all this run doesn't block the floor — the source
    /// stream delivers events in strictly increasing position order, so any table's first
    /// event from here on can only have a position at or after whatever's already confirmed.
    ///
    /// Only sends when the floor has actually advanced since the last send (`last_confirmed`)
    /// — recomputing and resending the same value on every event would be harmless (the
    /// adapter's own checkpoint update is documented as monotonic and cheap) but wasteful.
    async fn confirm_floor(
        &self,
        stream: &ChangeStream,
        tables: &HashMap<TargetTable, TableState>,
        last_confirmed: &mut Option<u64>,
    ) {
        if tables.is_empty() {
            return;
        }

        let mut floor: Option<u64> = None;
        for state in tables.values() {
            match state.watermark {
                Some(watermark) => {
                    floor = Some(floor.map_or(watermark, |current| current.min(watermark)))
                }
                None => return,
            }
        }

        let Some(floor) = floor else {
            return;
        };
        if *last_confirmed == Some(floor) {
            return;
        }

        if stream
            .confirm
            .send(Lsn::from(floor).to_string())
            .await
            .is_ok()
        {
            *last_confirmed = Some(floor);
        }
    }
}

fn parse_position(position: &str) -> Result<u64, CaptureError> {
    position.parse::<Lsn>().map(u64::from).map_err(|err| {
        CaptureError::Checkpoint(format!("unparseable position {position:?}: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::*;
    use crate::capture::domain::Operation;
    use crate::datasource::{ConnectionConfig, DataSourceId, DbEngine};
    use crate::write::domain::WriteError;

    fn test_source() -> DataSource {
        DataSource {
            id: DataSourceId(Uuid::now_v7()),
            name: "test".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: "127.0.0.1".to_string(),
                port: 5432,
                username: "postgres".to_string(),
                password: "postgres".to_string(),
                database: "testdb".to_string(),
            },
        }
    }

    fn test_event(position: u64) -> ChangeEvent {
        ChangeEvent {
            schema: "public".to_string(),
            table: "orders".to_string(),
            operation: Operation::Insert { after: vec![] },
            position: Lsn::from(position).to_string(),
            commit_timestamp_unix_micros: 0,
        }
    }

    /// A [`CdcSource`] that replays a fixed list of events once, records the `resume_from` it
    /// was given, and forwards whatever the orchestrator sends on `confirm` into a shared
    /// `Vec` the test can assert against.
    struct FakeCdcSource {
        events: Mutex<Option<Vec<ChangeEvent>>>,
        resume_from: Mutex<Option<String>>,
        confirms: Arc<Mutex<Vec<String>>>,
        /// The confirm-draining task spawned by `stream_changes`. `join` lets a test wait for
        /// it to fully drain `confirm_rx` (it only exits once the orchestrator's `ChangeStream`
        /// — and with it, `confirm_tx` — is dropped) before asserting on `confirms`; otherwise
        /// asserting right after `CaptureOrchestrator::run` returns races the drain, since nothing
        /// requires a spawned task to have been polled yet by then.
        task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    }

    impl FakeCdcSource {
        fn new(events: Vec<ChangeEvent>, confirms: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                events: Mutex::new(Some(events)),
                resume_from: Mutex::new(None),
                confirms,
                task: Mutex::new(None),
            }
        }

        async fn join(&self) {
            let handle = self.task.lock().unwrap().take();
            if let Some(handle) = handle {
                handle
                    .await
                    .expect("confirm-draining task should not panic");
            }
        }
    }

    #[async_trait]
    impl CdcSource for FakeCdcSource {
        async fn stream_changes(
            &self,
            _source: &DataSource,
            resume_from: Option<&str>,
        ) -> Result<ChangeStream, CaptureError> {
            *self.resume_from.lock().unwrap() = resume_from.map(|value| value.to_string());
            let events = self
                .events
                .lock()
                .unwrap()
                .take()
                .expect("stream_changes called once");

            let (tx, rx) = mpsc::channel(64);
            let (confirm_tx, mut confirm_rx) = mpsc::channel::<String>(64);
            let confirms = Arc::clone(&self.confirms);

            let handle = tokio::spawn(async move {
                for event in events {
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                // All events sent: drop `tx` so the orchestrator's `events.recv()` observes
                // end-of-stream and returns — but keep draining `confirm_rx` first so a final,
                // event-triggered flush's confirm isn't lost as the task tears down.
                drop(tx);
                while let Some(position) = confirm_rx.recv().await {
                    confirms.lock().unwrap().push(position);
                }
            });
            *self.task.lock().unwrap() = Some(handle);

            Ok(ChangeStream {
                events: rx,
                confirm: confirm_tx,
            })
        }
    }

    /// `(committed events, high-watermark position)` per call to `commit_batch`.
    type RecordedCommits = HashMap<TargetTable, Vec<(Vec<ChangeEvent>, String)>>;

    /// An [`IcebergWriter`] that commits into an in-memory map instead of a real Iceberg
    /// catalog, so the orchestrator's batching/dedup/checkpoint logic can be tested without
    /// Postgres or RustFS.
    #[derive(Default)]
    struct FakeIcebergWriter {
        commits: Mutex<RecordedCommits>,
        watermarks: Mutex<HashMap<TargetTable, String>>,
    }

    #[async_trait]
    impl IcebergWriter for FakeIcebergWriter {
        async fn commit_batch(
            &self,
            _source: &DataSource,
            target: &TargetTable,
            events: &[ChangeEvent],
            high_watermark_position: &str,
        ) -> Result<(), WriteError> {
            self.commits
                .lock()
                .unwrap()
                .entry(target.clone())
                .or_default()
                .push((events.to_vec(), high_watermark_position.to_string()));
            self.watermarks
                .lock()
                .unwrap()
                .insert(target.clone(), high_watermark_position.to_string());
            Ok(())
        }

        async fn last_committed_position(
            &self,
            target: &TargetTable,
        ) -> Result<Option<String>, WriteError> {
            Ok(self.watermarks.lock().unwrap().get(target).cloned())
        }

        async fn existing_targets(
            &self,
            _source: &DataSource,
        ) -> Result<Vec<TargetTable>, WriteError> {
            Ok(self.watermarks.lock().unwrap().keys().cloned().collect())
        }
    }

    /// The core exactly-once property: a table's already-committed events must never be
    /// recommitted, even when the capture adapter redelivers them (simulating Postgres
    /// replaying from the last confirmed LSN after a crash/restart) — proven across two
    /// separate `CaptureOrchestrator` runs sharing one `FakeIcebergWriter` (standing in for
    /// durable Iceberg state) but fresh `FakeCdcSource`s each time (standing in for a fresh
    /// WAL replay).
    #[tokio::test]
    async fn redelivered_events_below_watermark_are_not_recommitted() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let batch = BatchConfig {
            max_events: 2,
            max_flush_interval: Duration::from_secs(3600),
        };

        let events: Vec<ChangeEvent> = (1..=4).map(test_event).collect();
        let target = TargetTable::for_event(&source, &events[0]);

        // Run 1: only the first two events have arrived so far.
        let confirms_1 = Arc::new(Mutex::new(Vec::new()));
        let cdc_source_1 = Arc::new(FakeCdcSource::new(
            events[0..2].to_vec(),
            Arc::clone(&confirms_1),
        ));
        let orchestrator_1 = CaptureOrchestrator::new(cdc_source_1.clone(), writer.clone(), batch);
        orchestrator_1.run(source.clone()).await;
        cdc_source_1.join().await;

        assert_eq!(*cdc_source_1.resume_from.lock().unwrap(), None);
        let commits_after_run_1 = writer
            .commits
            .lock()
            .unwrap()
            .get(&target)
            .cloned()
            .unwrap();
        assert_eq!(commits_after_run_1.len(), 1);
        assert_eq!(commits_after_run_1[0].0, events[0..2]);
        assert_eq!(commits_after_run_1[0].1, events[1].position);
        assert_eq!(
            *confirms_1.lock().unwrap(),
            vec![events[1].position.clone()]
        );

        // Run 2 ("restart"): the fake adapter redelivers *all four* events, including the two
        // already committed in run 1 — worse than a real Postgres slot would ever do (it would
        // only redeliver from the confirmed LSN onward), which makes this the stress case for
        // the orchestrator's own per-table dedup rather than relying on the adapter to help.
        let confirms_2 = Arc::new(Mutex::new(Vec::new()));
        let cdc_source_2 = Arc::new(FakeCdcSource::new(events.clone(), Arc::clone(&confirms_2)));
        let orchestrator_2 = CaptureOrchestrator::new(cdc_source_2.clone(), writer.clone(), batch);
        orchestrator_2.run(source.clone()).await;
        cdc_source_2.join().await;

        // The orchestrator computed its resume position from the writer's durable state.
        assert_eq!(
            *cdc_source_2.resume_from.lock().unwrap(),
            Some(events[1].position.clone())
        );

        // Only the two genuinely new events (positions 3 and 4) were committed this run — the
        // redelivered duplicates of positions 1 and 2 were dropped, not recommitted.
        let commits_after_run_2 = writer
            .commits
            .lock()
            .unwrap()
            .get(&target)
            .cloned()
            .unwrap();
        assert_eq!(commits_after_run_2.len(), 2);
        assert_eq!(commits_after_run_2[1].0, events[2..4]);
        assert_eq!(commits_after_run_2[1].1, events[3].position);
        // The first confirm restates the resume floor on this run's fresh connection (harmless
        // — monotonic/idempotent per the adapter's own contract); the second reflects the new
        // high watermark after committing positions 3–4.
        assert_eq!(
            *confirms_2.lock().unwrap(),
            vec![events[1].position.clone(), events[3].position.clone()]
        );
    }

    /// Two tables, interleaved so both have events sitting in an unflushed buffer before
    /// either one flushes: the confirm floor must stay withheld while `customers` still has
    /// pending, unflushed data, even after `orders` has already committed — advancing past
    /// `orders`' position would let Postgres discard WAL covering `customers`' still-buffered
    /// event 2, which exists only in memory at that point. Once `customers` also flushes, the
    /// floor is free to advance to the lower of the two watermarks.
    #[tokio::test]
    async fn confirm_is_withheld_while_a_touched_table_has_unflushed_data() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let batch = BatchConfig {
            max_events: 2,
            max_flush_interval: Duration::from_secs(3600),
        };

        let mut customers_event_2 = test_event(2);
        customers_event_2.table = "customers".to_string();
        let mut customers_event_4 = test_event(4);
        customers_event_4.table = "customers".to_string();

        // Order: orders(1), customers(2), orders(3) [orders now at 2 pending -> flushes],
        // customers(4) [customers now at 2 pending -> flushes].
        let events = vec![
            test_event(1),
            customers_event_2.clone(),
            test_event(3),
            customers_event_4.clone(),
        ];
        let orders_target = TargetTable::for_event(&source, &events[0]);
        let customers_target = TargetTable::for_event(&source, &customers_event_2);

        let confirms = Arc::new(Mutex::new(Vec::new()));
        let cdc_source = Arc::new(FakeCdcSource::new(events, Arc::clone(&confirms)));
        let orchestrator = CaptureOrchestrator::new(cdc_source.clone(), writer.clone(), batch);
        orchestrator.run(source.clone()).await;
        cdc_source.join().await;

        // Both tables committed once each...
        assert_eq!(
            writer
                .commits
                .lock()
                .unwrap()
                .get(&orders_target)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            writer
                .commits
                .lock()
                .unwrap()
                .get(&customers_target)
                .unwrap()
                .len(),
            1
        );
        // ...but only one confirm was ever sent, after `customers` finally flushed too — not
        // right after `orders` flushed, while `customers`' event 2 was still only in memory.
        assert_eq!(*confirms.lock().unwrap(), vec![Lsn::from(3).to_string()]);
    }
}
