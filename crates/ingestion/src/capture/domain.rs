//! Domain layer: the engine-agnostic change event shape and the `CdcSource` port that
//! infrastructure adapters (WAL/binlog readers) implement.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::datasource::DataSource;

/// A single column's value as decoded off the wire, text-encoded.
///
/// Kept as text (rather than typed) at this layer because the two source engines encode
/// values completely differently on the wire; converting to Iceberg's typed columns happens
/// downstream, against the target table's schema, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnValue {
    pub name: String,
    /// `None` represents SQL NULL, as distinct from an empty string.
    pub value: Option<String>,
}

/// The row-level change an insert/update/delete produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Insert {
        after: Vec<ColumnValue>,
    },
    Update {
        /// Only present when the source's replica identity captures the pre-image
        /// (e.g. Postgres `REPLICA IDENTITY FULL`); absent otherwise.
        before: Option<Vec<ColumnValue>>,
        after: Vec<ColumnValue>,
    },
    Delete {
        before: Vec<ColumnValue>,
    },
}

/// A single captured change, positioned in the source's change stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEvent {
    pub schema: String,
    pub table: String,
    pub operation: Operation,
    /// Where this change sits in the source's log (a Postgres change LSN, a MySQL binlog offset,
    /// …), as text so callers can persist/compare it without depending on the source engine's
    /// own position type. Orders the changes of one row; written as the `_position` column.
    pub position: String,
    /// Position of the commit of the transaction this change belongs to (a Postgres commit
    /// LSN), as text. Transactions are delivered in commit order, so this — not `position` —
    /// is what checkpoints, dedup and confirms compare: a transaction that started earlier can
    /// commit later, carrying lower change positions than one already delivered.
    pub commit_position: String,
    pub commit_timestamp_unix_micros: i64,
}

/// One item of a [`ChangeStream`], in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamItem {
    /// A captured row change. The changes of one transaction arrive back to back and share a
    /// `commit_position`.
    Change(ChangeEvent),
    /// Every transaction committed before `position` has been delivered in full, and none is
    /// part-way through. Sent after each transaction and on source heartbeats between
    /// transactions, so progress can be confirmed even while no captured table changes.
    Progress { position: String },
}

/// Errors surfaced while capturing changes from a data source.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("failed to connect to data source: {0}")]
    Connect(String),
    #[error("failed to prepare replication objects (slot/publication): {0}")]
    Setup(String),
    #[error("failed to decode change stream: {0}")]
    Decode(String),
    #[error("data source read error: {0}")]
    Stream(String),
    #[error("failed to read back or confirm a source's Iceberg checkpoint: {0}")]
    Checkpoint(String),
}

/// A running change stream: the events themselves, plus a back-channel the caller uses to
/// report how far it has durably persisted.
///
/// Decoupling "advance the source's own checkpoint" (e.g. a Postgres replication slot's
/// `confirmed_flush_lsn`) from decoding is what makes exactly-once (in practice,
/// effectively-once) delivery possible: an adapter must never treat an event as consumed until
/// the caller sends its position — or a later one — on [`ChangeStream::confirm`]. See
/// [`crate::capture::application::CaptureOrchestrator`], which owns computing a safe confirm
/// floor across every table a batch of events fans out to.
pub struct ChangeStream {
    pub events: mpsc::Receiver<Result<StreamItem, CaptureError>>,
    /// Send the highest position durably persisted so far; the adapter advances the source's
    /// checkpoint only in response, never on decode.
    pub confirm: mpsc::Sender<String>,
}

/// Port: streams row-level changes out of an OLTP data source's change log (WAL, binlog, …).
///
/// Implemented per engine in [`crate::capture::infrastructure`]. Runs its own background
/// task and hands the caller a channel rather than a `Stream`, so the port stays simple to
/// implement without pinning/boxing — the same shape a Debezium-style connector uses.
#[async_trait]
pub trait CdcSource: Send + Sync {
    /// `resume_from`, when `Some`, asks the source to start at that position; `None` starts
    /// from the source's own checkpoint (a Postgres replication slot's confirmed position).
    /// [`crate::capture::application::CaptureOrchestrator`] always passes `None`: the slot only
    /// ever advances on confirms, which never pass data that isn't durable yet, whereas
    /// starting later than the slot can skip a transaction still buffered when the previous
    /// session stopped.
    async fn stream_changes(
        &self,
        source: &DataSource,
        resume_from: Option<&str>,
    ) -> Result<ChangeStream, CaptureError>;
}
