//! Orchestration layer driving a [`CdcSource`] and an [`IcebergWriter`] together: batches
//! captured events per target table, commits them, and only advances the source's own WAL
//! checkpoint (via [`ChangeStream::confirm`]) over data that has durably landed in Iceberg.
//!
//! See the root `CLAUDE.md`'s `pipa-ingestion::capture` architecture section for the
//! exactly-once design this implements. Everything here is ordered by *commit* position
//! ([`ChangeEvent::commit_position`]), the order transactions are delivered in — never by a
//! change's own position, which goes backwards whenever transactions overlap:
//! - **Whole transactions only.** A batch is flushed only between transactions, so a commit to
//!   Iceberg never holds part of one.
//! - **Dedup.** Each table's checkpoint is the commit position of the last transaction it
//!   landed, stored *in* the Iceberg snapshot of that commit (see
//!   [`crate::write::domain::IcebergWriter`]), so a crash can never leave the checkpoint and
//!   its data out of sync. A redelivered transaction at or below it is skipped.
//! - **Confirm.** The slot is confirmed up to just before the oldest transaction still
//!   buffered, or — with nothing buffered — as far as the stream has been delivered. An idle
//!   table never holds it back.
//! - **Chosen tables only.** Changes to a table the source doesn't ingest
//!   ([`TableSelection`]) are dropped as they arrive, so they never hold back the confirm
//!   either. The selection can change while a session runs; a table chosen later is captured
//!   from then on, with no backfill of what changed before.
//!
//! Scoped to Postgres: positions compare as `pgwire_replication::Lsn`'s `u64` total order,
//! which a future MySQL GTID-set-based source would not have.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pgwire_replication::Lsn;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::capture::domain::{CaptureError, CdcSource, ChangeEvent, ChangeStream, StreamItem};
use crate::datasource::{DataSource, TableSelection};
use crate::write::domain::{Checkpoint, IcebergWriter, TargetTable};

/// First delay before reconnecting a capture session that ended; doubles on every consecutive
/// failure up to [`RECONNECT_BACKOFF_MAX`].
pub const RECONNECT_BACKOFF_MIN: Duration = Duration::from_secs(1);
/// Longest delay between reconnect attempts. A session that stayed up at least this long
/// resets the delay back to [`RECONNECT_BACKOFF_MIN`].
pub const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(60);

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

/// A table's committed checkpoint, parsed. `None` (in [`TableState::watermark`]) means nothing
/// was ever committed to the table, or its checkpoint couldn't be read: nothing is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watermark {
    /// Commit position of the last transaction landed.
    Commit(u64),
    /// Change position of the last event landed by a pre-commit-position version
    /// ([`Checkpoint::LegacyChange`]): events are compared by their own change position until
    /// the table's next commit.
    LegacyChange(u64),
}

impl Watermark {
    /// Whether `event` (whose commit position is `commit`) is already in the table.
    fn covers(self, event: &ChangeEvent, commit: u64) -> bool {
        match self {
            Watermark::Commit(watermark) => commit <= watermark,
            Watermark::LegacyChange(watermark) => {
                parse_position(&event.position).is_ok_and(|position| position <= watermark)
            }
        }
    }
}

/// Per-table batching/dedup state.
struct TableState {
    target: TargetTable,
    watermark: Option<Watermark>,
    /// Events waiting to be committed, each with its parsed commit position, in order.
    pending: Vec<(u64, ChangeEvent)>,
    last_flush: Instant,
}

/// How far one capture session has got: what the stream has delivered and what has been
/// confirmed back to the source.
#[derive(Default)]
struct SessionProgress {
    /// Inside a transaction: a change arrived and its transaction's `Progress` hasn't yet.
    in_transaction: bool,
    /// The highest `Progress` position seen: every transaction committed before it has been
    /// delivered in full.
    delivered: Option<u64>,
    /// The last position sent on `confirm`.
    confirmed: Option<u64>,
}

/// Drives capture + write for one data source. Constructed once per Postgres/MySQL adapter and
/// writer pair; `run` is spawned per registered source.
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

    /// Drives `source` for as long as the process runs. Whenever a capture session ends — the
    /// source dropped the replication connection (e.g. Postgres restarted), the stream failed,
    /// or setup failed (source or catalog unreachable) — it reconnects after an exponential
    /// backoff ([`RECONNECT_BACKOFF_MIN`] doubling up to [`RECONNECT_BACKOFF_MAX`], reset after a
    /// session that stayed up at least [`RECONNECT_BACKOFF_MAX`]).
    ///
    /// A reconnect is just another crash-and-resume, which the exactly-once design already
    /// handles: each session recomputes its resume position from Iceberg, events that were only
    /// buffered in memory are redelivered from the slot (nothing past them was ever confirmed),
    /// and redeliveries at or below a table's committed watermark are skipped.
    ///
    /// `selection` holds the tables to capture, kept current by the caller as the source's
    /// registration changes; it is read on every change, so an update applies straight away.
    pub async fn run(&self, source: DataSource, selection: watch::Receiver<TableSelection>) {
        let mut backoff = RECONNECT_BACKOFF_MIN;
        loop {
            let started = Instant::now();
            match self.run_once(&source, &selection).await {
                Ok(()) => tracing::warn!(id = %source.id, "change capture stream ended"),
                Err(err) => tracing::error!(id = %source.id, error = %err, "change capture failed"),
            }
            if started.elapsed() >= RECONNECT_BACKOFF_MAX {
                backoff = RECONNECT_BACKOFF_MIN;
            }
            tracing::info!(id = %source.id, retry_in = ?backoff, "reconnecting change capture");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
        }
    }

    /// One capture session: stream from the source's own checkpoint until the stream ends
    /// (`Ok`) or can't be opened (`Err`). Pending, uncommitted events are dropped with the
    /// session; nothing past them was confirmed, so the next session gets them again.
    ///
    /// The session starts from the source's checkpoint (`resume_from = None`) rather than from
    /// Iceberg's: the slot is only ever confirmed over durable data, while starting at the
    /// lowest table checkpoint in Iceberg could pass a transaction that was still buffered for a
    /// table that doesn't exist in Iceberg yet. Redelivery from the slot is what the per-table
    /// dedup is for.
    async fn run_once(
        &self,
        source: &DataSource,
        selection: &watch::Receiver<TableSelection>,
    ) -> Result<(), CaptureError> {
        tracing::info!(id = %source.id, "starting capture");
        // Not needed for capture itself, so a failure only costs readers the current-rows view.
        if let Err(err) = self.writer.record_key_columns(source).await {
            tracing::warn!(id = %source.id, error = %err, "could not record target tables' key columns");
        }
        let mut stream = self.cdc_source.stream_changes(source, None).await?;

        let mut tables: HashMap<TargetTable, TableState> = HashMap::new();
        let mut progress = SessionProgress::default();
        let mut ticker = tokio::time::interval(self.batch.max_flush_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                item = stream.events.recv() => {
                    match item {
                        Some(Ok(StreamItem::Change(event))) => {
                            progress.in_transaction = true;
                            let wanted = selection.borrow().includes(&event.schema, &event.table);
                            if wanted {
                                self.handle_event(source, &mut tables, event).await;
                            }
                        }
                        Some(Ok(StreamItem::Progress { position })) => {
                            progress.in_transaction = false;
                            match parse_position(&position) {
                                Ok(position) => {
                                    progress.delivered = Some(
                                        progress.delivered.map_or(position, |seen| seen.max(position)),
                                    );
                                }
                                Err(err) => {
                                    tracing::error!(id = %source.id, error = %err, "ignoring an unparseable progress position");
                                }
                            }
                            self.flush_full(source, &mut tables).await;
                            self.confirm(&stream, &tables, &mut progress).await;
                        }
                        Some(Err(err)) => {
                            tracing::error!(id = %source.id, error = %err, "change capture error");
                        }
                        None => break,
                    }
                }
                _ = ticker.tick() => {
                    if !progress.in_transaction {
                        self.flush_due(source, &mut tables).await;
                    }
                    self.confirm(&stream, &tables, &mut progress).await;
                }
            }
        }

        Ok(())
    }

    async fn seed_watermark(&self, target: &TargetTable) -> Option<Watermark> {
        let parsed = match self.writer.last_committed_position(target).await {
            Ok(Some(Checkpoint::Commit(position))) => {
                parse_position(&position).map(Watermark::Commit)
            }
            Ok(Some(Checkpoint::LegacyChange(position))) => {
                parse_position(&position).map(Watermark::LegacyChange)
            }
            Ok(None) => return None,
            Err(err) => {
                tracing::error!(?target, error = %err, "failed to read back the table's checkpoint, skipping nothing");
                return None;
            }
        };
        parsed
            .inspect_err(|err| {
                tracing::error!(?target, error = %err, "unparseable table checkpoint, skipping nothing");
            })
            .ok()
    }

    async fn handle_event(
        &self,
        source: &DataSource,
        tables: &mut HashMap<TargetTable, TableState>,
        event: ChangeEvent,
    ) {
        let target = TargetTable::for_event(source, &event);

        let commit = match parse_position(&event.commit_position) {
            Ok(commit) => commit,
            Err(err) => {
                tracing::error!(id = %source.id, error = %err, "dropping event with an unparseable commit position");
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

        if state
            .watermark
            .is_some_and(|watermark| watermark.covers(&event, commit))
        {
            // Already durably committed — a redelivery after a crash between commit and
            // confirm, a reconnect, or a shard reassignment.
            return;
        }

        state.pending.push((commit, event));
    }

    /// Flushes every table holding at least a full batch. Called between transactions only.
    async fn flush_full(&self, source: &DataSource, tables: &mut HashMap<TargetTable, TableState>) {
        for state in tables.values_mut() {
            if state.pending.len() >= self.batch.max_events {
                self.flush_table(source, state).await;
            }
        }
    }

    /// Flushes every table whose oldest pending event has waited a full flush interval. Called
    /// between transactions only.
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

    async fn flush_table(&self, source: &DataSource, state: &mut TableState) {
        let Some(&(high_watermark, _)) = state.pending.last() else {
            return;
        };
        let events: Vec<ChangeEvent> = state
            .pending
            .iter()
            .map(|(_, event)| event.clone())
            .collect();

        match self
            .writer
            .commit_batch(
                source,
                &state.target,
                &events,
                &Lsn::from(high_watermark).to_string(),
            )
            .await
        {
            Ok(()) => {
                state.watermark = Some(Watermark::Commit(high_watermark));
                tracing::info!(
                    id = %source.id,
                    target = ?state.target,
                    events = events.len(),
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

    /// Confirms the furthest position whose data is all durable, if it moved:
    /// - with events buffered, just before the oldest buffered transaction's commit — every
    ///   transaction committed before it was delivered earlier and is either in Iceberg or was
    ///   skipped as already there;
    /// - with nothing buffered, the furthest `Progress` position.
    ///
    /// Tables with nothing buffered don't factor in, however old their last commit: an idle
    /// table must not keep the source from releasing WAL.
    async fn confirm(
        &self,
        stream: &ChangeStream,
        tables: &HashMap<TargetTable, TableState>,
        progress: &mut SessionProgress,
    ) {
        let oldest_buffered = tables
            .values()
            .filter_map(|state| state.pending.first().map(|&(commit, _)| commit))
            .min();
        let floor = match oldest_buffered {
            Some(commit) => commit.saturating_sub(1),
            None => match progress.delivered {
                Some(delivered) => delivered,
                None => return,
            },
        };
        if progress
            .confirmed
            .is_some_and(|confirmed| floor <= confirmed)
        {
            return;
        }

        if stream
            .confirm
            .send(Lsn::from(floor).to_string())
            .await
            .is_ok()
        {
            progress.confirmed = Some(floor);
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
            ingested_tables: Some(Vec::new()),
        }
    }

    fn lsn(position: u64) -> String {
        Lsn::from(position).to_string()
    }

    /// A change to `table` at change position `position`, in the transaction committed at
    /// `commit`.
    fn change(table: &str, position: u64, commit: u64) -> ChangeEvent {
        ChangeEvent {
            schema: "public".to_string(),
            table: table.to_string(),
            operation: Operation::Insert { after: vec![] },
            position: lsn(position),
            commit_position: lsn(commit),
            commit_timestamp_unix_micros: 0,
        }
    }

    fn item(event: &ChangeEvent) -> StreamItem {
        StreamItem::Change(event.clone())
    }

    fn progress(position: u64) -> StreamItem {
        StreamItem::Progress {
            position: lsn(position),
        }
    }

    /// Every table the tests change.
    fn test_tables() -> TableSelection {
        TableSelection::of([("public", "orders"), ("public", "customers")])
    }

    fn batch(max_events: usize) -> BatchConfig {
        BatchConfig {
            max_events,
            max_flush_interval: Duration::from_secs(3600),
        }
    }

    /// A [`CdcSource`] that plays a fixed list of items once, records the `resume_from` it was
    /// given, and collects whatever the orchestrator sends on `confirm`.
    struct FakeCdcSource {
        items: Mutex<Option<Vec<StreamItem>>>,
        resume_from: Mutex<Option<Option<String>>>,
        confirms: Arc<Mutex<Vec<String>>>,
        /// The task playing the items and draining `confirm`. It only exits once the
        /// orchestrator drops its `ChangeStream`, so `join` lets a test wait until every
        /// confirm has been collected.
        task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    }

    impl FakeCdcSource {
        fn new(items: Vec<StreamItem>) -> Arc<Self> {
            Arc::new(Self {
                items: Mutex::new(Some(items)),
                resume_from: Mutex::new(None),
                confirms: Arc::new(Mutex::new(Vec::new())),
                task: Mutex::new(None),
            })
        }

        async fn join(&self) {
            let handle = self.task.lock().unwrap().take();
            if let Some(handle) = handle {
                handle
                    .await
                    .expect("the fake source's task should not panic");
            }
        }

        fn confirms(&self) -> Vec<String> {
            self.confirms.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CdcSource for FakeCdcSource {
        async fn stream_changes(
            &self,
            _source: &DataSource,
            resume_from: Option<&str>,
        ) -> Result<ChangeStream, CaptureError> {
            *self.resume_from.lock().unwrap() = Some(resume_from.map(str::to_string));
            let items = self
                .items
                .lock()
                .unwrap()
                .take()
                .expect("stream_changes called once");

            let (tx, rx) = mpsc::channel(64);
            let (confirm_tx, mut confirm_rx) = mpsc::channel::<String>(64);
            let confirms = Arc::clone(&self.confirms);
            let handle = tokio::spawn(async move {
                for item in items {
                    if tx.send(Ok(item)).await.is_err() {
                        return;
                    }
                }
                // End the stream, but keep collecting confirms until the orchestrator is gone.
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

    /// An [`IcebergWriter`] committing into memory, standing in for durable Iceberg state.
    #[derive(Default)]
    struct FakeIcebergWriter {
        commits: Mutex<RecordedCommits>,
        checkpoints: Mutex<HashMap<TargetTable, Checkpoint>>,
    }

    impl FakeIcebergWriter {
        fn commits(&self, target: &TargetTable) -> Vec<(Vec<ChangeEvent>, String)> {
            self.commits
                .lock()
                .unwrap()
                .get(target)
                .cloned()
                .unwrap_or_default()
        }
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
            self.checkpoints.lock().unwrap().insert(
                target.clone(),
                Checkpoint::Commit(high_watermark_position.to_string()),
            );
            Ok(())
        }

        async fn last_committed_position(
            &self,
            target: &TargetTable,
        ) -> Result<Option<Checkpoint>, WriteError> {
            Ok(self.checkpoints.lock().unwrap().get(target).cloned())
        }

        async fn record_key_columns(&self, _source: &DataSource) -> Result<(), WriteError> {
            Ok(())
        }

        async fn existing_tables(
            &self,
            _source: &DataSource,
        ) -> Result<std::collections::HashSet<String>, WriteError> {
            Ok(self
                .checkpoints
                .lock()
                .unwrap()
                .keys()
                .map(|target| target.table.clone())
                .collect())
        }
    }

    async fn run(
        source: &DataSource,
        writer: &Arc<FakeIcebergWriter>,
        config: BatchConfig,
        items: Vec<StreamItem>,
    ) -> Arc<FakeCdcSource> {
        run_selected(source, writer, config, test_tables(), items).await
    }

    async fn run_selected(
        source: &DataSource,
        writer: &Arc<FakeIcebergWriter>,
        config: BatchConfig,
        selection: TableSelection,
        items: Vec<StreamItem>,
    ) -> Arc<FakeCdcSource> {
        let cdc = FakeCdcSource::new(items);
        let (_selection_tx, selection) = watch::channel(selection);
        CaptureOrchestrator::new(cdc.clone(), writer.clone(), config)
            .run_once(source, &selection)
            .await
            .unwrap();
        cdc.join().await;
        cdc
    }

    /// The core exactly-once property, across a "crash": a transaction committed in one session
    /// is skipped when redelivered in the next, and one cut off mid-way is committed whole.
    #[tokio::test]
    async fn redelivered_transactions_are_not_recommitted() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let (a, b) = (change("orders", 1, 10), change("orders", 2, 10));
        let (c, d) = (change("orders", 3, 20), change("orders", 4, 20));
        let target = TargetTable::for_event(&source, &a);

        // Session 1 stops part-way through transaction 20.
        let cdc = run(
            &source,
            &writer,
            batch(1),
            vec![item(&a), item(&b), progress(11), item(&c)],
        )
        .await;
        assert_eq!(*cdc.resume_from.lock().unwrap(), Some(None));
        assert_eq!(
            writer.commits(&target),
            vec![(vec![a.clone(), b.clone()], lsn(10))]
        );
        assert_eq!(cdc.confirms(), vec![lsn(11)]);

        // Session 2 gets everything again — more than a slot would redeliver.
        let cdc = run(
            &source,
            &writer,
            batch(1),
            vec![
                item(&a),
                item(&b),
                progress(11),
                item(&c),
                item(&d),
                progress(21),
            ],
        )
        .await;
        let commits = writer.commits(&target);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[1], (vec![c, d], lsn(20)));
        assert_eq!(cdc.confirms(), vec![lsn(11), lsn(21)]);
    }

    /// Transactions overlap: one that started first (lower change position) commits after
    /// another that has already landed. Dedup compares commit positions, so it is kept.
    #[tokio::test]
    async fn a_transaction_committing_after_an_overlapping_one_is_kept() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let committed_first = change("customers", 110, 200);
        let started_first = change("customers", 100, 300);
        let target = TargetTable::for_event(&source, &started_first);

        run(
            &source,
            &writer,
            batch(1),
            vec![
                item(&committed_first),
                progress(201),
                item(&started_first),
                progress(301),
            ],
        )
        .await;

        assert_eq!(
            writer.commits(&target),
            vec![
                (vec![committed_first], lsn(200)),
                (vec![started_first], lsn(300)),
            ]
        );
    }

    /// A transaction bigger than a batch is still committed in one piece, at its end.
    #[tokio::test]
    async fn a_transaction_is_never_split_across_commits() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let events: Vec<ChangeEvent> = (1..=3).map(|n| change("orders", n, 10)).collect();
        let target = TargetTable::for_event(&source, &events[0]);

        let mut items: Vec<StreamItem> = events.iter().map(item).collect();
        items.push(progress(11));
        run(&source, &writer, batch(2), items).await;

        assert_eq!(writer.commits(&target), vec![(events, lsn(10))]);
    }

    /// A [`CdcSource`] whose stream the test feeds by hand.
    struct ManualCdcSource {
        stream: Mutex<Option<ChangeStream>>,
    }

    #[async_trait]
    impl CdcSource for ManualCdcSource {
        async fn stream_changes(
            &self,
            _source: &DataSource,
            _resume_from: Option<&str>,
        ) -> Result<ChangeStream, CaptureError> {
            Ok(self.stream.lock().unwrap().take().expect("one session"))
        }
    }

    /// The flush timer doesn't fire part-way through a transaction.
    #[tokio::test(start_paused = true)]
    async fn the_flush_timer_waits_for_the_transaction_to_end() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let (tx, rx) = mpsc::channel(8);
        let (confirm_tx, _confirm_rx) = mpsc::channel(8);
        let cdc = Arc::new(ManualCdcSource {
            stream: Mutex::new(Some(ChangeStream {
                events: rx,
                confirm: confirm_tx,
            })),
        });
        let interval = Duration::from_secs(5);
        let orchestrator = CaptureOrchestrator::new(
            cdc,
            writer.clone(),
            BatchConfig {
                max_events: 100,
                max_flush_interval: interval,
            },
        );
        let session_source = source.clone();
        let (_selection_tx, selection) = watch::channel(test_tables());
        let session =
            tokio::spawn(async move { orchestrator.run_once(&session_source, &selection).await });

        let (a, b) = (change("orders", 1, 10), change("orders", 2, 10));
        let target = TargetTable::for_event(&source, &a);
        tx.send(Ok(item(&a))).await.unwrap();
        tokio::time::sleep(interval * 3).await;
        assert!(
            writer.commits(&target).is_empty(),
            "flushed mid-transaction"
        );

        tx.send(Ok(item(&b))).await.unwrap();
        tx.send(Ok(progress(11))).await.unwrap();
        tokio::time::sleep(interval * 2).await;
        assert_eq!(writer.commits(&target), vec![(vec![a, b], lsn(10))]);

        drop(tx);
        session.await.unwrap().unwrap();
    }

    /// A table that stopped changing doesn't hold the confirm at its last commit.
    #[tokio::test]
    async fn an_idle_table_does_not_hold_back_the_confirm() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let cdc = run(
            &source,
            &writer,
            batch(1),
            vec![
                item(&change("orders", 1, 10)),
                progress(11),
                item(&change("customers", 2, 20)),
                progress(21),
                item(&change("customers", 3, 30)),
                progress(31),
            ],
        )
        .await;

        assert_eq!(cdc.confirms(), vec![lsn(11), lsn(21), lsn(31)]);
    }

    /// While a transaction is buffered, the confirm stays below its commit position, so the
    /// source keeps the WAL it needs to redeliver it.
    #[tokio::test]
    async fn the_confirm_stays_below_the_oldest_buffered_transaction() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let cdc = run(
            &source,
            &writer,
            batch(2),
            vec![
                // orders flushes (2 events) -> nothing buffered -> confirm 11.
                item(&change("orders", 1, 10)),
                item(&change("orders", 2, 10)),
                progress(11),
                // customers buffers transaction 20 -> confirm 19.
                item(&change("customers", 3, 20)),
                progress(21),
                // orders buffers transaction 30; 20 is still the oldest -> no new confirm.
                item(&change("orders", 4, 30)),
                progress(31),
                // customers flushes; orders' transaction 30 is now the oldest -> confirm 29.
                item(&change("customers", 5, 40)),
                progress(41),
            ],
        )
        .await;

        assert_eq!(cdc.confirms(), vec![lsn(11), lsn(19), lsn(29)]);
    }

    /// Progress alone (a source whose captured tables don't change) still advances the
    /// confirm, so the source can release WAL.
    #[tokio::test]
    async fn progress_alone_advances_the_confirm() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let cdc = run(
            &source,
            &writer,
            batch(1),
            vec![progress(100), progress(200)],
        )
        .await;

        assert_eq!(cdc.confirms(), vec![lsn(100), lsn(200)]);
    }

    /// A table last committed by a version that checkpointed change positions is deduplicated
    /// by change position until its next commit, which stores a commit position.
    #[tokio::test]
    async fn a_legacy_change_position_checkpoint_is_still_honored() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let landed = change("orders", 90, 150);
        let new = change("orders", 120, 150);
        let target = TargetTable::for_event(&source, &landed);
        writer
            .checkpoints
            .lock()
            .unwrap()
            .insert(target.clone(), Checkpoint::LegacyChange(lsn(100)));

        run(
            &source,
            &writer,
            batch(1),
            vec![item(&landed), item(&new), progress(151)],
        )
        .await;

        assert_eq!(writer.commits(&target), vec![(vec![new], lsn(150))]);
        assert_eq!(
            writer.checkpoints.lock().unwrap().get(&target),
            Some(&Checkpoint::Commit(lsn(150)))
        );
    }

    /// Changes to tables the source doesn't ingest are never committed, and don't hold back the
    /// confirm while the chosen tables' transactions land.
    #[tokio::test]
    async fn only_chosen_tables_are_committed() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let order = change("orders", 1, 10);
        let customer = change("customers", 2, 20);
        let selection = TableSelection::of([("public", "orders")]);

        let cdc = run_selected(
            &source,
            &writer,
            batch(1),
            selection,
            vec![item(&order), progress(11), item(&customer), progress(21)],
        )
        .await;
        assert_eq!(
            writer.commits(&TargetTable::for_event(&source, &order)),
            vec![(vec![order], lsn(10))]
        );
        assert!(
            writer
                .commits(&TargetTable::for_event(&source, &customer))
                .is_empty()
        );
        assert_eq!(cdc.confirms(), vec![lsn(11), lsn(21)]);
    }

    /// A [`CdcSource`] whose sessions go wrong in a scripted order: the first `stream_changes`
    /// call fails to connect, the second returns a stream that ends straight away (the source
    /// dropping the connection), and every later one stays open. Records when each call happened
    /// and the `resume_from` it was given.
    #[derive(Default)]
    struct FlakyCdcSource {
        calls: Mutex<Vec<(Instant, Option<String>)>>,
        open_streams: Mutex<Vec<mpsc::Sender<Result<StreamItem, CaptureError>>>>,
        reconnected: tokio::sync::Notify,
    }

    #[async_trait]
    impl CdcSource for FlakyCdcSource {
        async fn stream_changes(
            &self,
            _source: &DataSource,
            resume_from: Option<&str>,
        ) -> Result<ChangeStream, CaptureError> {
            let call = {
                let mut calls = self.calls.lock().unwrap();
                calls.push((Instant::now(), resume_from.map(str::to_string)));
                calls.len()
            };
            if call == 1 {
                return Err(CaptureError::Connect("connection refused".to_string()));
            }

            let (tx, rx) = mpsc::channel(1);
            let (confirm_tx, _confirm_rx) = mpsc::channel(1);
            if call == 2 {
                drop(tx);
            } else {
                self.open_streams.lock().unwrap().push(tx);
                self.reconnected.notify_one();
            }
            Ok(ChangeStream {
                events: rx,
                confirm: confirm_tx,
            })
        }
    }

    /// A failed connect and a dropped stream both lead to a reconnect, after a backoff that
    /// doubles, and every session starts from the source's own checkpoint.
    #[tokio::test(start_paused = true)]
    async fn reconnects_with_backoff_after_a_session_ends() {
        let source = test_source();
        let writer = Arc::new(FakeIcebergWriter::default());
        let cdc = Arc::new(FlakyCdcSource::default());
        let orchestrator = CaptureOrchestrator::new(cdc.clone(), writer, batch(10));

        let (_selection_tx, selection) = watch::channel(test_tables());
        let task = tokio::spawn(async move { orchestrator.run(source, selection).await });
        cdc.reconnected.notified().await;
        task.abort();

        let calls = cdc.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1].0 - calls[0].0, RECONNECT_BACKOFF_MIN);
        assert_eq!(calls[2].0 - calls[1].0, RECONNECT_BACKOFF_MIN * 2);
        assert!(calls.iter().all(|(_, resume_from)| resume_from.is_none()));
    }
}
