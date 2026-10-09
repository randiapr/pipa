//! Domain layer: the [`IcebergWriter`] port and the `ChangeEvent` → typed `RecordBatch`
//! mapping.
//!
//! `capture/domain.rs`'s [`crate::capture::domain::ColumnValue`] doc comment says typed
//! conversion against the target table's schema "happens downstream, not here" — this module
//! is that downstream. It stays Arrow-typed (rather than fully engine-agnostic) because Arrow
//! is the row shape every Iceberg writer needs; it does not otherwise know anything about
//! catalogs, Postgres, or object storage — those are [`crate::write::infrastructure`]'s job.

use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, SchemaRef, TimeUnit};
use async_trait::async_trait;

use crate::capture::domain::{ChangeEvent, ColumnValue, Operation};
use crate::datasource::DataSource;

/// Fixed metadata columns every changelog table carries, ahead of the source table's own
/// business columns — in this order, matching what
/// [`crate::write::infrastructure::IcebergChangelogWriter`] introspects/creates target tables
/// with.
pub const OP_COLUMN: &str = "_op";
pub const SOURCE_ID_COLUMN: &str = "_source_id";
pub const POSITION_COLUMN: &str = "_position";
pub const COMMIT_TIMESTAMP_COLUMN: &str = "_commit_timestamp_us";

/// Identifies the Iceberg table a set of change events land in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetTable {
    pub namespace: String,
    pub table: String,
}

impl TargetTable {
    /// Deterministic mapping from a captured event's source table to its target Iceberg
    /// table, mirroring the `pipa_cdc_{source_id}` convention
    /// `capture::infrastructure::postgres` already uses for the replication slot/publication:
    /// namespace `cdc_{source_id}`, table `{schema}__{table}` (sanitized to `[a-z0-9_]`, to
    /// keep it a valid Iceberg identifier regardless of what Postgres allows).
    pub fn for_event(source: &DataSource, event: &ChangeEvent) -> Self {
        Self {
            namespace: namespace_for_source(source),
            table: sanitize_ident(&format!("{}__{}", event.schema, event.table)),
        }
    }
}

/// The Iceberg namespace every target table for `source` lives under — shared by
/// [`TargetTable::for_event`] and [`crate::write::infrastructure::IcebergChangelogWriter`]'s
/// namespace-listing (`existing_targets`), so both agree on where a source's tables live
/// without either owning the convention twice.
pub fn namespace_for_source(source: &DataSource) -> String {
    format!("cdc_{}", source.id.0.simple())
}

fn sanitize_ident(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Errors surfaced while landing captured changes into Iceberg.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("failed to access Iceberg catalog: {0}")]
    Catalog(String),
    #[error("failed to introspect source table schema: {0}")]
    SchemaIntrospection(String),
    #[error("failed to map a captured change into an Arrow row: {0}")]
    Mapping(String),
    #[error("failed to write/commit a data file: {0}")]
    Commit(String),
}

/// Port: durably lands a batch of same-table [`ChangeEvent`]s into their target Iceberg table.
///
/// Implemented by [`crate::write::infrastructure::IcebergChangelogWriter`]. Callers (see
/// [`crate::capture::application::CaptureOrchestrator`]) are responsible for only advancing a
/// source's WAL checkpoint after [`IcebergWriter::commit_batch`] returns `Ok` — this port
/// makes no promises about a batch's durability until that point.
#[async_trait]
pub trait IcebergWriter: Send + Sync {
    /// Commits `events` — already confirmed by the caller to be non-empty, all from the same
    /// `(schema, table)` and made of whole transactions — to `target`, tagging the resulting
    /// Iceberg snapshot with `high_watermark_position` (the highest
    /// [`ChangeEvent::commit_position`] in the batch). That property is what
    /// [`IcebergWriter::last_committed_position`] reads back, so the checkpoint commits
    /// atomically with the data rather than through a side channel.
    async fn commit_batch(
        &self,
        source: &DataSource,
        target: &TargetTable,
        events: &[ChangeEvent],
        high_watermark_position: &str,
    ) -> Result<(), WriteError>;

    /// The checkpoint of the last batch durably committed to `target`, or `None` if the table
    /// doesn't exist yet / has never been committed to.
    async fn last_committed_position(
        &self,
        target: &TargetTable,
    ) -> Result<Option<Checkpoint>, WriteError>;

    /// Records the row key (the source table's replica identity columns) on every existing
    /// target table of `source` that doesn't carry it yet. New tables get it when they are
    /// created; this backfills tables created before keys were recorded, so readers can
    /// collapse their changelog to current rows from Iceberg alone.
    async fn record_key_columns(&self, source: &DataSource) -> Result<(), WriteError>;
}

/// The checkpoint a target table's latest snapshot carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checkpoint {
    /// Commit position of the last transaction landed in the table.
    Commit(String),
    /// Change position of the last event landed, from snapshots written before checkpoints
    /// tracked commit positions (pipa-ingestion 0.4 and earlier). Only comparable with
    /// [`ChangeEvent::position`]; replaced by a [`Checkpoint::Commit`] on the table's next
    /// commit.
    LegacyChange(String),
}

/// The changelog row's operation tag and the column values it should be built from:
/// `Insert`/`Update` use their post-image, `Delete` uses its pre-image (it has no
/// post-image).
fn operation_tag_and_values(operation: &Operation) -> (&'static str, &[ColumnValue]) {
    match operation {
        Operation::Insert { after } => ("insert", after),
        Operation::Update { after, .. } => ("update", after),
        Operation::Delete { before } => ("delete", before),
    }
}

/// Maps a batch of same-table change events into a [`RecordBatch`] matching `schema`, which
/// must be [`OP_COLUMN`], [`SOURCE_ID_COLUMN`], [`POSITION_COLUMN`],
/// [`COMMIT_TIMESTAMP_COLUMN`] followed by the source table's business columns, in that order
/// — the shape [`crate::write::infrastructure::IcebergChangelogWriter`] introspects/creates
/// target tables with.
pub fn events_to_record_batch(
    schema: &SchemaRef,
    source_id: &str,
    events: &[ChangeEvent],
) -> Result<RecordBatch, WriteError> {
    let columns = schema
        .fields()
        .iter()
        .map(|field| build_column(field.name(), field.data_type(), source_id, events))
        .collect::<Result<Vec<_>, _>>()?;

    RecordBatch::try_new(schema.clone(), columns)
        .map_err(|err| WriteError::Mapping(format!("failed to assemble record batch: {err}")))
}

fn build_column(
    name: &str,
    data_type: &DataType,
    source_id: &str,
    events: &[ChangeEvent],
) -> Result<ArrayRef, WriteError> {
    match name {
        OP_COLUMN => Ok(Arc::new(StringArray::from(
            events
                .iter()
                .map(|event| Some(operation_tag_and_values(&event.operation).0))
                .collect::<Vec<_>>(),
        )) as ArrayRef),
        SOURCE_ID_COLUMN => Ok(Arc::new(StringArray::from(
            events.iter().map(|_| Some(source_id)).collect::<Vec<_>>(),
        )) as ArrayRef),
        POSITION_COLUMN => Ok(Arc::new(StringArray::from(
            events
                .iter()
                .map(|event| Some(event.position.as_str()))
                .collect::<Vec<_>>(),
        )) as ArrayRef),
        COMMIT_TIMESTAMP_COLUMN => Ok(Arc::new(Int64Array::from(
            events
                .iter()
                .map(|event| Some(event.commit_timestamp_unix_micros))
                .collect::<Vec<_>>(),
        )) as ArrayRef),
        business_column => {
            let texts: Vec<Option<&str>> = events
                .iter()
                .map(|event| {
                    let (_, values) = operation_tag_and_values(&event.operation);
                    values
                        .iter()
                        .find(|column| column.name == business_column)
                        .and_then(|column| column.value.as_deref())
                })
                .collect();
            build_typed_column(data_type, &texts)
        }
    }
}

fn build_typed_column(
    data_type: &DataType,
    texts: &[Option<&str>],
) -> Result<ArrayRef, WriteError> {
    Ok(match data_type {
        DataType::Utf8 => Arc::new(StringArray::from(texts.to_vec())),
        DataType::Boolean => {
            let values = texts
                .iter()
                .map(|value| value.map(parse_bool).transpose())
                .collect::<Result<Vec<_>, _>>()?;
            Arc::new(BooleanArray::from(values))
        }
        DataType::Int16 => Arc::new(Int16Array::from(parse_numbers(texts)?)),
        DataType::Int32 => Arc::new(Int32Array::from(parse_numbers(texts)?)),
        DataType::Int64 => Arc::new(Int64Array::from(parse_numbers(texts)?)),
        DataType::Float32 => Arc::new(Float32Array::from(parse_numbers(texts)?)),
        DataType::Float64 => Arc::new(Float64Array::from(parse_numbers(texts)?)),
        DataType::Date32 => {
            let values = texts
                .iter()
                .map(|value| value.map(parse_date32).transpose())
                .collect::<Result<Vec<_>, _>>()?;
            Arc::new(Date32Array::from(values))
        }
        DataType::Timestamp(TimeUnit::Microsecond, tz) => {
            let has_tz = tz.is_some();
            let values = texts
                .iter()
                .map(|value| {
                    value
                        .map(|text| parse_timestamp_micros(text, has_tz))
                        .transpose()
                })
                .collect::<Result<Vec<_>, _>>()?;
            let array = TimestampMicrosecondArray::from(values);
            match tz {
                Some(tz) => Arc::new(array.with_timezone(tz.clone())),
                None => Arc::new(array),
            }
        }
        other => {
            return Err(WriteError::Mapping(format!(
                "unsupported target Arrow type {other:?} (schema was built by this crate, so this indicates a bug in pg_type_to_arrow)"
            )));
        }
    })
}

fn parse_numbers<T>(texts: &[Option<&str>]) -> Result<Vec<Option<T>>, WriteError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    texts
        .iter()
        .map(|value| {
            value
                .map(|text| {
                    text.parse::<T>().map_err(|err| {
                        WriteError::Mapping(format!("invalid numeric text value {text:?}: {err}"))
                    })
                })
                .transpose()
        })
        .collect()
}

fn parse_bool(text: &str) -> Result<bool, WriteError> {
    match text {
        "t" | "true" | "TRUE" | "1" => Ok(true),
        "f" | "false" | "FALSE" | "0" => Ok(false),
        other => Err(WriteError::Mapping(format!(
            "invalid boolean text value {other:?}"
        ))),
    }
}

/// Parses a Postgres `date` text value (ISO `DateStyle` output, the default: `YYYY-MM-DD`)
/// into days since the Unix epoch.
fn parse_date32(text: &str) -> Result<i32, WriteError> {
    let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|err| WriteError::Mapping(format!("invalid date text value {text:?}: {err}")))?;
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch date");
    Ok((date - epoch).num_days() as i32)
}

/// Parses a Postgres `timestamp [with time zone]` text value (ISO `DateStyle` output, the
/// default) into microseconds since the Unix epoch (UTC).
fn parse_timestamp_micros(text: &str, has_tz: bool) -> Result<i64, WriteError> {
    if has_tz {
        for format in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%d %H:%M:%S%#z"] {
            if let Ok(dt) = chrono::DateTime::parse_from_str(text, format) {
                return Ok(dt.timestamp_micros());
            }
        }
        Err(WriteError::Mapping(format!(
            "invalid timestamptz text value {text:?} (expected Postgres ISO DateStyle output)"
        )))
    } else {
        let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S"))
            .map_err(|err| {
                WriteError::Mapping(format!("invalid timestamp text value {text:?}: {err}"))
            })?;
        Ok(naive.and_utc().timestamp_micros())
    }
}
