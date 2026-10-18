//! Snapshot backfill: lands the rows a chosen table already held before its changes were
//! captured, so its current rows in Iceberg are the whole table rather than only the rows that
//! changed since.
//!
//! **Snapshot rows sort below every captured change.** Each existing row is written as a
//! changelog row with `_op = "read"` and `_position` [`SNAPSHOT_POSITION`] (`0/0`, the lowest
//! LSN). A key's current row is its change with the highest `_position`, so a snapshot row is
//! current only while no change to that row has been captured — and then it *is* the row's
//! state, because capture keeps every change to a table from the moment the table is chosen,
//! which is before any of its rows are read. Rows changed while they are being read, by
//! transactions still open during a read, or deleted afterwards, are all decided by the
//! captured change. So the reads can be plain, lock-free pages, each its own point in time,
//! with no snapshot exported from the replication slot and no watermarks in the stream.
//!
//! **Progress is kept with the data.** Each page is committed together with the backfill's
//! [`BackfillState`] (see [`IcebergWriter::commit_backfill`]), so a backfill that stops — a
//! session ending, a restart, a handoff to another instance — resumes after the last page that
//! landed. A page committed twice only adds snapshot rows of the same keys at the same
//! position, which the current rows collapse.
//!
//! **Known limits.** A table without a row key is skipped: its snapshot rows couldn't be told
//! apart from the changes captured over them. A table is backfilled once: one chosen again
//! after a while unchosen isn't re-read, so rows that changed in between stay as they were.
//!
//! Commits go through [`crate::capture::CaptureOrchestrator`]'s session task, the one that
//! commits the table's captured changes too, so the two never race on the table's checkpoint.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::capture::domain::{CaptureError, ChangeEvent, Operation, SnapshotRead, SnapshotSource};
use crate::datasource::{DataSource, TableRef, TableSelection};
use crate::write::domain::{BackfillState, IcebergWriter, TargetTable, WriteError};

/// The `_position` (and commit position) of every snapshot row: the lowest Postgres LSN, below
/// every captured change.
pub const SNAPSHOT_POSITION: &str = "0/0";

/// How long a backfill that failed (source or catalog unreachable) waits before trying again,
/// unless the source's table selection changes first.
const RETRY_DELAY: Duration = Duration::from_secs(30);

/// Backfill knobs.
#[derive(Debug, Clone, Copy)]
pub struct BackfillConfig {
    /// Rows read and committed per page.
    pub chunk_rows: usize,
}

impl BackfillConfig {
    /// Reads `INGESTION_BACKFILL` (`off`/`false`/`0` disables backfilling, which is otherwise
    /// on) and `INGESTION_BACKFILL_CHUNK_ROWS` (default 5000). `None` when disabled.
    pub fn from_env() -> Option<Self> {
        let disabled = std::env::var("INGESTION_BACKFILL").is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "off" | "false" | "0" | "no"
            )
        });
        if disabled {
            return None;
        }
        Some(Self {
            chunk_rows: std::env::var("INGESTION_BACKFILL_CHUNK_ROWS")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|&rows: &usize| rows > 0)
                .unwrap_or(5000),
        })
    }
}

/// One step of a table's backfill, for the session task to commit: a page of snapshot rows
/// (possibly none) and the state the backfill is in once they have landed.
#[derive(Debug)]
pub(crate) struct BackfillStep {
    pub table: TableRef,
    pub target: TargetTable,
    pub rows: Vec<ChangeEvent>,
    pub next: BackfillState,
}

#[derive(Debug, thiserror::Error)]
enum BackfillError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// Reads the existing rows of a source's chosen tables, one table and one page at a time, and
/// hands them to the session task as [`BackfillStep`]s. Runs for one capture session.
pub(crate) struct Backfiller {
    pub snapshots: Arc<dyn SnapshotSource>,
    pub writer: Arc<dyn IcebergWriter>,
    pub config: BackfillConfig,
    pub source: DataSource,
    pub selection: watch::Receiver<TableSelection>,
}

impl Backfiller {
    /// Backfills every chosen table that needs it, then again whenever the selection changes,
    /// until `steps` closes (the session ended).
    pub async fn run(mut self, steps: mpsc::Sender<BackfillStep>) {
        // Tables whose backfill this session finished (handed over its last step), and where
        // unfinished ones got to. Kept in memory because the steps last handed over may not
        // have been committed yet, so the state the table records can be behind.
        let mut finished: HashSet<TargetTable> = HashSet::new();
        let mut cursors: HashMap<TargetTable, Vec<String>> = HashMap::new();
        loop {
            self.selection.borrow_and_update();
            let wait = match self.pass(&steps, &mut finished, &mut cursors).await {
                Ok(()) => None,
                Err(err) => {
                    tracing::warn!(id = %self.source.id, error = %err, retry_in = ?RETRY_DELAY, "snapshot backfill failed");
                    Some(RETRY_DELAY)
                }
            };
            if steps.is_closed() {
                return;
            }
            let changed = async {
                match wait {
                    Some(delay) => {
                        tokio::select! {
                            changed = self.selection.changed() => changed.is_ok(),
                            _ = tokio::time::sleep(delay) => true,
                        }
                    }
                    None => self.selection.changed().await.is_ok(),
                }
            };
            if !changed.await {
                return;
            }
        }
    }

    /// One pass over the chosen tables.
    async fn pass(
        &self,
        steps: &mpsc::Sender<BackfillStep>,
        finished: &mut HashSet<TargetTable>,
        cursors: &mut HashMap<TargetTable, Vec<String>>,
    ) -> Result<(), BackfillError> {
        let tables = self.snapshots.list_tables(&self.source).await?;
        for table in tables {
            if !self.chosen(&table) {
                continue;
            }
            let target = TargetTable::for_source_table(&self.source, &table.schema, &table.name);
            if finished.contains(&target) {
                continue;
            }
            let mut after = match cursors.get(&target) {
                Some(cursor) => Some(cursor.clone()),
                None => match self.writer.backfill_state(&target).await? {
                    Some(BackfillState::Done | BackfillState::Skipped) => {
                        finished.insert(target);
                        continue;
                    }
                    Some(BackfillState::Running { after }) => Some(after),
                    None => None,
                },
            };
            tracing::info!(
                id = %self.source.id,
                table = %format!("{}.{}", table.schema, table.name),
                resumed = after.is_some(),
                "backfilling existing rows",
            );

            loop {
                // Unchosen part-way: stop; the state recorded so far is kept for later.
                if !self.chosen(&table) {
                    break;
                }
                let read = self
                    .snapshots
                    .read_chunk(
                        &self.source,
                        &table,
                        after.as_deref(),
                        self.config.chunk_rows,
                    )
                    .await?;
                let (rows, next) = match read {
                    SnapshotRead::Keyless => (Vec::new(), BackfillState::Skipped),
                    SnapshotRead::Chunk { rows, last_key } => {
                        // A full page may be followed by more; a short one ends the table.
                        let next = match last_key {
                            Some(key) if rows.len() >= self.config.chunk_rows => {
                                BackfillState::Running { after: key }
                            }
                            _ => BackfillState::Done,
                        };
                        (snapshot_events(&table, rows), next)
                    }
                };
                after = match &next {
                    BackfillState::Running { after } => {
                        cursors.insert(target.clone(), after.clone());
                        Some(after.clone())
                    }
                    BackfillState::Done | BackfillState::Skipped => None,
                };
                let step = BackfillStep {
                    table: table.clone(),
                    target: target.clone(),
                    rows,
                    next,
                };
                if steps.send(step).await.is_err() {
                    return Ok(());
                }
                if after.is_none() {
                    cursors.remove(&target);
                    finished.insert(target);
                    break;
                }
            }
        }
        Ok(())
    }

    fn chosen(&self, table: &TableRef) -> bool {
        self.selection.borrow().includes(&table.schema, &table.name)
    }
}

/// `rows` of `table` as snapshot rows: [`Operation::Read`] at [`SNAPSHOT_POSITION`], stamped
/// with the time they were read.
fn snapshot_events(
    table: &TableRef,
    rows: Vec<Vec<crate::capture::domain::ColumnValue>>,
) -> Vec<ChangeEvent> {
    let read_at = chrono::Utc::now().timestamp_micros();
    rows.into_iter()
        .map(|after| ChangeEvent {
            schema: table.schema.clone(),
            table: table.name.clone(),
            operation: Operation::Read { after },
            position: SNAPSHOT_POSITION.to_string(),
            commit_position: SNAPSHOT_POSITION.to_string(),
            commit_timestamp_unix_micros: read_at,
        })
        .collect()
}
