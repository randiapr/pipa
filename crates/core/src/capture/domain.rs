//! Domain layer: the engine-agnostic change event shape and the `CdcSource` port that
//! infrastructure adapters (WAL/binlog readers) implement.

use async_trait::async_trait;
use pipa_data::datasource::DataSource;
use tokio::sync::mpsc;

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
    /// Opaque, source-specific stream position (a Postgres LSN, a MySQL binlog offset, …)
    /// serialized as text so callers can persist/compare it without depending on the
    /// source engine's own position type.
    pub position: String,
    pub commit_timestamp_unix_micros: i64,
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
}

/// Port: streams row-level changes out of an OLTP data source's change log (WAL, binlog, …).
///
/// Implemented per engine in [`crate::capture::infrastructure`]. Runs its own background
/// task and hands the caller a channel rather than a `Stream`, so the port stays simple to
/// implement without pinning/boxing — the same shape a Debezium-style connector uses.
#[async_trait]
pub trait CdcSource: Send + Sync {
    async fn stream_changes(
        &self,
        source: &DataSource,
    ) -> Result<mpsc::Receiver<Result<ChangeEvent, CaptureError>>, CaptureError>;
}
