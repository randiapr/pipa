// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Concrete [`IcebergWriter`] adapter: Iceberg REST catalog access, target-table
//! auto-provisioning (introspecting the source Postgres table's schema via
//! `information_schema` on first sight), and the actual `fast_append` commit.
//!
//! Reimplements (rather than imports) the catalog-access pattern `pipa-backend`'s own
//! `crates/backend/src/iceberg/catalog.rs` uses — see the root `CLAUDE.md`'s note on why
//! `pipa-ingestion` keeps its own independent copy of the `iceberg`/`iceberg-catalog-rest`
//! dependency rather than sharing one through `pipa-backend`. Request signing for RustFS's
//! catalog is the one shared piece: it comes from `pipa-catalog-auth`, as the catalog's
//! `AuthManager`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema as ArrowSchema, SchemaRef, TimeUnit};
use async_trait::async_trait;
use iceberg::io::{
    S3_ACCESS_KEY_ID, S3_ENDPOINT, S3_PATH_STYLE_ACCESS, S3_REGION, S3_SECRET_ACCESS_KEY,
};
use iceberg::spec::{DataFile, DataFileFormat, Schema as IcebergSchema, Snapshot};
use iceberg::table::Table;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
pub(crate) use iceberg::writer::IcebergWriter as IcebergFileWriter;
use iceberg::writer::IcebergWriterBuilder;
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use iceberg::{
    Catalog, CatalogBuilder, ErrorKind, NamespaceIdent, TableCreation, TableIdent,
    arrow::{arrow_schema_to_schema_auto_assign_ids, schema_to_arrow_schema},
};
use iceberg_catalog_rest::{
    REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE, RestCatalogBuilder,
};
use iceberg_storage_opendal::OpenDalStorageFactory;
use parquet::file::properties::WriterProperties;
use pipa_catalog_auth::{
    DEFAULT_SERVICE, SigV4AuthManager, enable_s3_tables_with_retry, rustfs_catalog_uri,
    sigv4::Signer,
};
use sqlx::PgConnection;
use tokio::sync::Mutex;

use crate::capture::domain::ChangeEvent;
use crate::datasource::{DataSource, TableRef};
use crate::storage::ObjectStoreConfig;
use crate::write::domain::{
    BackfillState, COMMIT_TIMESTAMP_COLUMN, Checkpoint, IcebergWriter, OP_COLUMN, POSITION_COLUMN,
    SOURCE_ID_COLUMN, TargetTable, WriteError, events_to_record_batch, namespace_for_source,
    target_table_name,
};

/// Snapshot summary property carrying the checkpoint — the commit position of the last
/// transaction landed by that commit (see [`Checkpoint::Commit`]).
pub(crate) const COMMIT_POSITION_PROPERTY: &str = "pipa.cdc.commit_position";
/// The checkpoint property snapshots written by pipa-ingestion 0.4 and earlier carry instead: a
/// change position, read back as [`Checkpoint::LegacyChange`].
pub(crate) const LEGACY_POSITION_PROPERTY: &str = "pipa.cdc.position";
/// Table property naming the source table's row key — its replica identity columns (primary
/// key by default), as a JSON array of column names, `[]` for a table without one. Lets readers
/// such as `pipa-backend` collapse the changelog to current rows from Iceberg alone.
const KEY_COLUMNS_PROPERTY: &str = "pipa.cdc.key_columns";
/// Snapshot summary property carrying the source id a commit came from, for debuggability.
pub(crate) const COMMIT_SOURCE_ID_PROPERTY: &str = "pipa.cdc.source_id";
/// Table property recording how far the table's snapshot backfill has got, as a
/// [`BackfillState`] in JSON. A table property rather than a snapshot summary one so that
/// snapshot expiry can't lose it; it is set in the same commit as the rows it accounts for.
const BACKFILL_PROPERTY: &str = "pipa.cdc.backfill";
/// Snapshot summary property marking a commit of snapshot backfill rows (`chunk`).
const BACKFILL_SNAPSHOT_PROPERTY: &str = "pipa.cdc.backfill";

/// Connection settings for the Iceberg REST catalog CDC target tables commit to.
///
/// A second, independent copy of `pipa-backend`'s own `IcebergCatalogConfig` (same
/// `ICEBERG_CATALOG_*` env vars, so both services point at the same catalog by default without
/// sharing code — see the module doc).
#[derive(Debug, Clone)]
pub struct IcebergCatalogConfig {
    pub name: String,
    pub uri: String,
    pub warehouse: String,
    /// Signs catalog requests (RustFS's catalog only accepts SigV4-signed ones); `None` sends
    /// them unsigned.
    pub sigv4: Option<Signer>,
}

impl IcebergCatalogConfig {
    /// Reads connection settings from `ICEBERG_CATALOG_*` environment variables, defaulting
    /// `uri` to RustFS's "S3 Tables" catalog, which RustFS embeds directly into the object store
    /// itself. That catalog only accepts SigV4-signed requests, so the default signs them with
    /// `store`'s credentials (and first enables S3 Tables on `store`'s bucket). Set
    /// `ICEBERG_CATALOG_URI` to use some other catalog instead; its requests are sent unsigned.
    pub async fn from_env(store: &ObjectStoreConfig) -> anyhow::Result<Self> {
        let (uri, sigv4) = match std::env::var("ICEBERG_CATALOG_URI") {
            Ok(uri) if !uri.is_empty() => (uri, None),
            _ => {
                let signer = Signer::new(
                    store.access_key_id.clone(),
                    store.secret_access_key.clone(),
                    store.region.clone(),
                    DEFAULT_SERVICE.to_string(),
                );
                enable_s3_tables_with_retry(&store.endpoint, &store.bucket, &signer).await?;
                (rustfs_catalog_uri(&store.endpoint), Some(signer))
            }
        };
        Ok(Self {
            name: std::env::var("ICEBERG_CATALOG_NAME").unwrap_or_else(|_| "pipa".to_string()),
            uri,
            warehouse: std::env::var("ICEBERG_CATALOG_WAREHOUSE")
                .unwrap_or_else(|_| "pipa".to_string()),
            sigv4,
        })
    }

    /// Builds a REST [`Catalog`] client for this catalog. `store` supplies the RustFS/S3
    /// credentials the client uses for direct `FileIO` access to table data/metadata files.
    pub async fn build_catalog(
        &self,
        store: &ObjectStoreConfig,
    ) -> anyhow::Result<Arc<dyn Catalog>> {
        let props = HashMap::from([
            (REST_CATALOG_PROP_URI.to_string(), self.uri.clone()),
            (
                REST_CATALOG_PROP_WAREHOUSE.to_string(),
                self.warehouse.clone(),
            ),
            (S3_ENDPOINT.to_string(), store.endpoint.clone()),
            (S3_REGION.to_string(), store.region.clone()),
            (S3_ACCESS_KEY_ID.to_string(), store.access_key_id.clone()),
            (
                S3_SECRET_ACCESS_KEY.to_string(),
                store.secret_access_key.clone(),
            ),
            (S3_PATH_STYLE_ACCESS.to_string(), "true".to_string()),
        ]);

        // iceberg ships no S3 FileIO of its own: the REST catalog needs an explicit storage
        // factory to read/write table data and metadata files.
        let mut builder = RestCatalogBuilder::default().with_storage_factory(Arc::new(
            OpenDalStorageFactory::S3 {
                customized_credential_load: None,
            },
        ));
        // A fresh manager per catalog: `iceberg-catalog-rest` doesn't allow sharing one.
        if let Some(signer) = &self.sigv4 {
            builder = builder.with_auth_manager(SigV4AuthManager::new(signer.clone()));
        }
        let catalog = builder.load(self.name.clone(), props).await?;
        Ok(Arc::new(catalog))
    }
}

/// [`IcebergWriter`] adapter: lands changelog rows as append-only Parquet data files, committed
/// via `fast_append` with the batch's high-watermark position tagged onto the snapshot.
pub struct IcebergChangelogWriter {
    catalog: Arc<dyn Catalog>,
    /// Cached per-table Arrow schema, keyed by target table, so a table only gets
    /// introspected/loaded once per process lifetime rather than once per batch.
    schemas: Mutex<HashMap<TargetTable, SchemaRef>>,
}

impl IcebergChangelogWriter {
    pub fn new(catalog: Arc<dyn Catalog>) -> Self {
        Self {
            catalog,
            schemas: Mutex::new(HashMap::new()),
        }
    }

    fn table_ident(target: &TargetTable) -> TableIdent {
        TableIdent::new(
            NamespaceIdent::new(target.namespace.clone()),
            target.table.clone(),
        )
    }

    /// Ensures `target` exists in the catalog — creating it, introspecting `source`'s Postgres
    /// table `pg_schema.pg_table` on first sight — and returns the loaded/created [`Table`] plus
    /// its Arrow schema.
    async fn ensure_table(
        &self,
        source: &DataSource,
        target: &TargetTable,
        pg_schema: &str,
        pg_table: &str,
    ) -> Result<(Table, SchemaRef), WriteError> {
        let ident = Self::table_ident(target);

        if let Some(schema) = self.schemas.lock().await.get(target).cloned() {
            return Ok((self.load_table(&ident).await?, schema));
        }

        let table = self
            .open_or_create(source, &ident, pg_schema, pg_table)
            .await?;
        let arrow_schema = Arc::new(
            schema_to_arrow_schema(table.metadata().current_schema())
                .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?,
        );
        self.schemas
            .lock()
            .await
            .insert(target.clone(), arrow_schema.clone());
        Ok((table, arrow_schema))
    }

    /// Loads `ident`, creating its namespace and the table itself (from `source`'s
    /// `pg_schema.pg_table`) when they don't exist yet.
    async fn open_or_create(
        &self,
        source: &DataSource,
        ident: &TableIdent,
        pg_schema: &str,
        pg_table: &str,
    ) -> Result<Table, WriteError> {
        self.ensure_namespace(ident.namespace()).await?;
        if self
            .catalog
            .table_exists(ident)
            .await
            .map_err(catalog_error)?
        {
            return self.load_table(ident).await;
        }
        let creation = table_creation(source, ident.name(), pg_schema, pg_table).await?;
        self.create_or_load(ident, creation).await
    }

    /// Creates `namespace` unless it exists; another instance creating it first is fine.
    async fn ensure_namespace(&self, namespace: &NamespaceIdent) -> Result<(), WriteError> {
        if self
            .catalog
            .namespace_exists(namespace)
            .await
            .map_err(catalog_error)?
        {
            return Ok(());
        }
        match self
            .catalog
            .create_namespace(namespace, HashMap::new())
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NamespaceAlreadyExists => Ok(()),
            Err(err) => Err(catalog_error(err)),
        }
    }

    /// Creates `ident` as `creation` describes, or loads it when another writer created it first.
    async fn create_or_load(
        &self,
        ident: &TableIdent,
        creation: TableCreation,
    ) -> Result<Table, WriteError> {
        match self.catalog.create_table(ident.namespace(), creation).await {
            Ok(table) => Ok(table),
            Err(err) if err.kind() == ErrorKind::TableAlreadyExists => self.load_table(ident).await,
            Err(err) => Err(catalog_error(err)),
        }
    }

    async fn load_table(&self, ident: &TableIdent) -> Result<Table, WriteError> {
        self.catalog.load_table(ident).await.map_err(catalog_error)
    }

    /// The tables of `source`'s namespace that don't record their row key yet (none when the
    /// namespace doesn't exist).
    async fn tables_without_key_columns(
        &self,
        source: &DataSource,
    ) -> Result<Vec<Table>, WriteError> {
        let namespace = NamespaceIdent::new(namespace_for_source(source));
        if !self
            .catalog
            .namespace_exists(&namespace)
            .await
            .map_err(catalog_error)?
        {
            return Ok(Vec::new());
        }
        let mut missing = Vec::new();
        for ident in self
            .catalog
            .list_tables(&namespace)
            .await
            .map_err(catalog_error)?
        {
            let table = self.load_table(&ident).await?;
            if !table
                .metadata()
                .properties()
                .contains_key(KEY_COLUMNS_PROPERTY)
            {
                missing.push(table);
            }
        }
        Ok(missing)
    }

    /// Records on each of `tables` the row key `keys` has for it, keyed by target table name.
    /// A target whose source table is gone has no key to record, and is left alone.
    async fn record_keys(
        &self,
        tables: Vec<Table>,
        keys: &HashMap<String, Vec<String>>,
    ) -> Result<(), WriteError> {
        for table in tables {
            let name = table.identifier().name().to_string();
            let Some(columns) = keys.get(&name) else {
                continue;
            };
            let tx = Transaction::new(&table);
            let tx = tx
                .update_table_properties()
                .set(
                    KEY_COLUMNS_PROPERTY.to_string(),
                    key_columns_property(columns),
                )
                .apply(tx)
                .map_err(catalog_error)?;
            tx.commit(self.catalog.as_ref())
                .await
                .map_err(catalog_error)?;
            tracing::info!(table = %name, ?columns, "recorded key columns");
        }
        Ok(())
    }
}

fn catalog_error(err: iceberg::Error) -> WriteError {
    WriteError::Catalog(err.to_string())
}

/// How to create the target table `name` of `source`'s `pg_schema.pg_table`: its columns
/// introspected from the source, its row key recorded as a table property.
async fn table_creation(
    source: &DataSource,
    name: &str,
    pg_schema: &str,
    pg_table: &str,
) -> Result<TableCreation, WriteError> {
    let iceberg_schema = introspect_iceberg_schema(source, pg_schema, pg_table).await?;
    let key_columns = source_key_columns(source)
        .await?
        .remove(&(pg_schema.to_string(), pg_table.to_string()))
        .unwrap_or_default();
    Ok(TableCreation::builder()
        .name(name.to_string())
        .schema(iceberg_schema)
        .properties(HashMap::from([(
            KEY_COLUMNS_PROPERTY.to_string(),
            key_columns_property(&key_columns),
        )]))
        .build())
}

#[async_trait]
impl IcebergWriter for IcebergChangelogWriter {
    async fn commit_batch(
        &self,
        source: &DataSource,
        target: &TargetTable,
        events: &[ChangeEvent],
        high_watermark_position: &str,
    ) -> Result<(), WriteError> {
        let Some(first) = events.first() else {
            return Ok(());
        };

        let (table, arrow_schema) = self
            .ensure_table(source, target, &first.schema, &first.table)
            .await?;
        let data_files =
            write_data_files(&table, &arrow_schema, source, events, "pipa-cdc").await?;

        let properties = HashMap::from([
            (
                COMMIT_POSITION_PROPERTY.to_string(),
                high_watermark_position.to_string(),
            ),
            (COMMIT_SOURCE_ID_PROPERTY.to_string(), source.id.to_string()),
        ]);

        let tx = Transaction::new(&table);
        let action = tx
            .fast_append()
            .add_data_files(data_files)
            .set_snapshot_properties(properties);
        let tx = action
            .apply(tx)
            .map_err(|err| WriteError::Commit(err.to_string()))?;
        tx.commit(self.catalog.as_ref())
            .await
            .map_err(|err| WriteError::Commit(err.to_string()))?;

        Ok(())
    }

    async fn last_committed_position(
        &self,
        target: &TargetTable,
    ) -> Result<Option<Checkpoint>, WriteError> {
        let ident = Self::table_ident(target);

        if !self
            .catalog
            .table_exists(&ident)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
        {
            return Ok(None);
        }

        let table = self
            .catalog
            .load_table(&ident)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?;

        Ok(latest_checkpoint(&table))
    }

    async fn backfill_state(
        &self,
        target: &TargetTable,
    ) -> Result<Option<BackfillState>, WriteError> {
        let ident = Self::table_ident(target);
        if !self
            .catalog
            .table_exists(&ident)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
        {
            return Ok(None);
        }
        let table = self
            .catalog
            .load_table(&ident)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?;
        Ok(backfill_state_of(&table))
    }

    async fn commit_backfill(
        &self,
        source: &DataSource,
        target: &TargetTable,
        table_ref: &TableRef,
        rows: &[ChangeEvent],
        next: &BackfillState,
    ) -> Result<(), WriteError> {
        let (table, arrow_schema) = self
            .ensure_table(source, target, &table_ref.schema, &table_ref.name)
            .await?;

        let mut tx = Transaction::new(&table);
        if !rows.is_empty() {
            let data_files =
                write_data_files(&table, &arrow_schema, source, rows, "pipa-backfill").await?;
            let properties = backfill_snapshot_properties(source, latest_checkpoint(&table));
            tx = tx
                .fast_append()
                .add_data_files(data_files)
                .set_snapshot_properties(properties)
                .apply(tx)
                .map_err(|err| WriteError::Commit(err.to_string()))?;
        }
        let state = serde_json::to_string(next).expect("a backfill state always serializes");
        tx = tx
            .update_table_properties()
            .set(BACKFILL_PROPERTY.to_string(), state)
            .apply(tx)
            .map_err(|err| WriteError::Commit(err.to_string()))?;
        tx.commit(self.catalog.as_ref())
            .await
            .map_err(|err| WriteError::Commit(err.to_string()))?;
        Ok(())
    }

    async fn existing_tables(&self, source: &DataSource) -> Result<HashSet<String>, WriteError> {
        let namespace = NamespaceIdent::new(namespace_for_source(source));
        if !self
            .catalog
            .namespace_exists(&namespace)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
        {
            return Ok(HashSet::new());
        }
        Ok(self
            .catalog
            .list_tables(&namespace)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
            .into_iter()
            .map(|ident| ident.name().to_string())
            .collect())
    }

    async fn record_key_columns(&self, source: &DataSource) -> Result<(), WriteError> {
        let missing = self.tables_without_key_columns(source).await?;
        if missing.is_empty() {
            return Ok(());
        }
        // Keyed by the target table name each source table maps to, so matching is exact rather
        // than splitting `{schema}__{table}` back apart.
        let keys: HashMap<String, Vec<String>> = source_key_columns(source)
            .await?
            .into_iter()
            .map(|((schema, table), columns)| (target_table_name(&schema, &table), columns))
            .collect();
        self.record_keys(missing, &keys).await
    }
}

/// The summary properties of a backfill commit from `source`: what marks it as one, and the
/// table's `checkpoint` carried forward unchanged, under the key it was found under.
fn backfill_snapshot_properties(
    source: &DataSource,
    checkpoint: Option<Checkpoint>,
) -> HashMap<String, String> {
    let mut properties = HashMap::from([
        (COMMIT_SOURCE_ID_PROPERTY.to_string(), source.id.to_string()),
        (BACKFILL_SNAPSHOT_PROPERTY.to_string(), "chunk".to_string()),
    ]);
    match checkpoint {
        Some(Checkpoint::Commit(position)) => {
            properties.insert(COMMIT_POSITION_PROPERTY.to_string(), position);
        }
        Some(Checkpoint::LegacyChange(position)) => {
            properties.insert(LEGACY_POSITION_PROPERTY.to_string(), position);
        }
        None => {}
    }
    properties
}

/// Writes `events` as Parquet data files of `table` (whose Arrow schema is `arrow_schema`), not
/// yet committed.
async fn write_data_files(
    table: &Table,
    arrow_schema: &SchemaRef,
    source: &DataSource,
    events: &[ChangeEvent],
    prefix: &str,
) -> Result<Vec<DataFile>, WriteError> {
    let batch = events_to_record_batch(arrow_schema, &source.id.to_string(), events)?;
    let mut writer = new_data_file_writer(table, prefix)
        .await
        .map_err(|err| WriteError::Commit(err.to_string()))?;
    writer
        .write(batch)
        .await
        .map_err(|err| WriteError::Commit(err.to_string()))?;
    writer
        .close()
        .await
        .map_err(|err| WriteError::Commit(err.to_string()))
}

/// The backfill state `table` records, if any. One that can't be read counts as never
/// started: re-reading rows already backfilled only adds snapshot rows of the same keys, at the
/// same lowest position, which the current-rows view collapses.
fn backfill_state_of(table: &Table) -> Option<BackfillState> {
    let value = table.metadata().properties().get(BACKFILL_PROPERTY)?;
    serde_json::from_str(value)
        .inspect_err(|err| {
            tracing::warn!(table = %table.identifier(), %value, error = %err, "unreadable backfill state, starting over");
        })
        .ok()
}

/// A fresh writer of Parquet data files for `table` in its current schema, rolling over at the
/// default target file size. File names are `{prefix}-{uuid}-NNNNN.parquet`: the generator's
/// counter restarts at 0 for every writer, so without the per-writer uuid the second commit's
/// files would collide with the first's.
pub(crate) async fn new_data_file_writer(
    table: &Table,
    prefix: &str,
) -> iceberg::Result<Box<dyn IcebergFileWriter>> {
    let location_generator = DefaultLocationGenerator::new(table.metadata())?;
    let file_name_generator = DefaultFileNameGenerator::new(
        format!("{prefix}-{}", uuid::Uuid::now_v7().simple()),
        None,
        DataFileFormat::Parquet,
    );
    let parquet_writer_builder = ParquetWriterBuilder::new(
        WriterProperties::default(),
        table.metadata().current_schema().clone(),
    );
    let rolling_writer_builder = RollingFileWriterBuilder::new_with_default_file_size(
        parquet_writer_builder,
        table.file_io().clone(),
        location_generator,
        file_name_generator,
    );
    let writer = DataFileWriterBuilder::new(rolling_writer_builder)
        .build(None)
        .await?;
    Ok(Box::new(writer))
}

/// The checkpoint `snapshot`'s summary carries, if any.
pub(crate) fn checkpoint_of(snapshot: &Snapshot) -> Option<Checkpoint> {
    let properties = &snapshot.summary().additional_properties;
    properties
        .get(COMMIT_POSITION_PROPERTY)
        .map(|position| Checkpoint::Commit(position.clone()))
        .or_else(|| {
            properties
                .get(LEGACY_POSITION_PROPERTY)
                .map(|position| Checkpoint::LegacyChange(position.clone()))
        })
}

/// The checkpoint of `table`'s latest commit by this crate: the current snapshot's, or — when
/// that snapshot was made by something that doesn't carry the checkpoint forward (another
/// engine's compaction, say) — the nearest ancestor's that has one. Every snapshot pipa makes
/// carries it, so the walk normally stops at the first. An ancestor already expired ends it.
pub(crate) fn latest_checkpoint(table: &Table) -> Option<Checkpoint> {
    let metadata = table.metadata();
    let mut snapshot = metadata.current_snapshot();
    while let Some(current) = snapshot {
        if let Some(checkpoint) = checkpoint_of(current) {
            return Some(checkpoint);
        }
        snapshot = current
            .parent_snapshot_id()
            .and_then(|parent| metadata.snapshot_by_id(parent));
    }
    None
}

/// `source`'s tables' row keys ([`crate::postgres::key_columns`]).
async fn source_key_columns(
    source: &DataSource,
) -> Result<HashMap<(String, String), Vec<String>>, WriteError> {
    let mut conn = connect_source(source).await?;
    crate::postgres::key_columns(&mut conn)
        .await
        .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))
}

/// [`KEY_COLUMNS_PROPERTY`]'s value for `columns`.
fn key_columns_property(columns: &[String]) -> String {
    serde_json::to_string(columns).expect("a list of strings always serializes")
}

async fn connect_source(source: &DataSource) -> Result<PgConnection, WriteError> {
    crate::postgres::connect(source)
        .await
        .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))
}

/// Introspects `pg_schema.pg_table`'s columns via `information_schema` and builds the target
/// Iceberg table's schema: the four fixed changelog columns
/// ([`OP_COLUMN`]/[`SOURCE_ID_COLUMN`]/[`POSITION_COLUMN`]/[`COMMIT_TIMESTAMP_COLUMN`]) followed
/// by the source table's own columns, typed via [`pg_type_to_arrow`].
///
/// Opens a short-lived connection (mirroring
/// `capture::infrastructure::postgres::PostgresWalSource::ensure_replication_objects`'s own
/// pattern) rather than threading typed column info through the WAL decode path — this keeps
/// `capture/domain.rs`'s `ColumnValue` deliberately text-only, per its own doc comment.
async fn introspect_iceberg_schema(
    source: &DataSource,
    pg_schema: &str,
    pg_table: &str,
) -> Result<IcebergSchema, WriteError> {
    let mut conn = connect_source(source).await?;
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT column_name, data_type FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
    )
    .bind(pg_schema)
    .bind(pg_table)
    .fetch_all(&mut conn)
    .await
    .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?;
    changelog_schema(pg_schema, pg_table, columns)
}

/// The target table's schema for `pg_schema.pg_table`, whose `(name, information_schema
/// data_type)` columns are `columns`.
fn changelog_schema(
    pg_schema: &str,
    pg_table: &str,
    columns: Vec<(String, String)>,
) -> Result<IcebergSchema, WriteError> {
    if columns.is_empty() {
        return Err(WriteError::SchemaIntrospection(format!(
            "no columns found for {pg_schema}.{pg_table} via information_schema (table dropped, or captured before it was ever visible?)"
        )));
    }

    let mut fields = vec![
        Field::new(OP_COLUMN, DataType::Utf8, false),
        Field::new(SOURCE_ID_COLUMN, DataType::Utf8, false),
        Field::new(POSITION_COLUMN, DataType::Utf8, false),
        Field::new(COMMIT_TIMESTAMP_COLUMN, DataType::Int64, false),
    ];
    fields.extend(
        columns
            .into_iter()
            .map(|(name, pg_type)| Field::new(name, pg_type_to_arrow(&pg_type), true)),
    );

    let arrow_schema = ArrowSchema::new(fields);
    arrow_schema_to_schema_auto_assign_ids(&arrow_schema)
        .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))
}

/// Maps a Postgres `information_schema.columns.data_type` name to the Arrow type its values
/// get parsed into. Deliberately narrow: only types with an unambiguous, lossless text→typed
/// mapping get one; everything else (`numeric`, `uuid`, `json[b]`, enums, arrays, `bytea`, …)
/// lands as `Utf8`, preserving Postgres's own text-output-function representation exactly
/// rather than risking precision loss (this is most consequential for `numeric`: without the
/// column's declared scale, converting to a fixed-point Arrow `Decimal128` risks getting the
/// scale wrong, whereas the text form is always exact).
fn pg_type_to_arrow(pg_type: &str) -> DataType {
    match pg_type {
        "smallint" => DataType::Int16,
        "integer" => DataType::Int32,
        "bigint" => DataType::Int64,
        "boolean" => DataType::Boolean,
        "real" => DataType::Float32,
        "double precision" => DataType::Float64,
        "date" => DataType::Date32,
        "timestamp without time zone" => DataType::Timestamp(TimeUnit::Microsecond, None),
        "timestamp with time zone" => {
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")))
        }
        _ => DataType::Utf8,
    }
}

#[cfg(test)]
mod tests {
    use iceberg::io::LocalFsStorageFactory;
    use iceberg::memory::{MEMORY_CATALOG_WAREHOUSE, MemoryCatalog, MemoryCatalogBuilder};
    use uuid::Uuid;

    use super::*;
    use crate::datasource::{ConnectionConfig, DataSourceId, DbEngine};

    fn source() -> DataSource {
        DataSource {
            id: DataSourceId(Uuid::now_v7()),
            name: "test".to_string(),
            engine: DbEngine::Postgres,
            connection: ConnectionConfig {
                host: "unused".to_string(),
                port: 5432,
                username: "u".to_string(),
                password: "p".to_string(),
                database: "d".to_string(),
            },
            ingested_tables: Some(Vec::new()),
        }
    }

    async fn writer() -> (IcebergChangelogWriter, Arc<MemoryCatalog>) {
        let dir = std::env::temp_dir().join(format!("pipa-writer-{}", Uuid::now_v7().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let catalog = Arc::new(
            MemoryCatalogBuilder::default()
                .with_storage_factory(Arc::new(LocalFsStorageFactory))
                .load(
                    "test",
                    HashMap::from([(
                        MEMORY_CATALOG_WAREHOUSE.to_string(),
                        dir.to_str().unwrap().to_string(),
                    )]),
                )
                .await
                .unwrap(),
        );
        (IcebergChangelogWriter::new(catalog.clone()), catalog)
    }

    fn orders_creation(name: &str, properties: HashMap<String, String>) -> TableCreation {
        TableCreation::builder()
            .name(name.to_string())
            .schema(
                changelog_schema(
                    "public",
                    "orders",
                    vec![("id".to_string(), "integer".to_string())],
                )
                .unwrap(),
            )
            .properties(properties)
            .build()
    }

    #[test]
    fn postgres_types_map_to_arrow_types_and_everything_else_stays_text() {
        for (pg_type, expected) in [
            ("smallint", DataType::Int16),
            ("integer", DataType::Int32),
            ("bigint", DataType::Int64),
            ("boolean", DataType::Boolean),
            ("real", DataType::Float32),
            ("double precision", DataType::Float64),
            ("date", DataType::Date32),
            (
                "timestamp without time zone",
                DataType::Timestamp(TimeUnit::Microsecond, None),
            ),
            (
                "timestamp with time zone",
                DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            ),
            ("numeric", DataType::Utf8),
            ("uuid", DataType::Utf8),
            ("jsonb", DataType::Utf8),
        ] {
            assert_eq!(pg_type_to_arrow(pg_type), expected, "{pg_type}");
        }
    }

    #[test]
    fn the_changelog_schema_puts_the_changelog_columns_before_the_source_columns() {
        let schema = changelog_schema(
            "public",
            "orders",
            vec![
                ("id".to_string(), "bigint".to_string()),
                ("total".to_string(), "numeric".to_string()),
            ],
        )
        .unwrap();
        let arrow = schema_to_arrow_schema(&schema).unwrap();
        let fields: Vec<(&str, &DataType, bool)> = arrow
            .fields()
            .iter()
            .map(|field| {
                (
                    field.name().as_str(),
                    field.data_type(),
                    field.is_nullable(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            vec![
                (OP_COLUMN, &DataType::Utf8, false),
                (SOURCE_ID_COLUMN, &DataType::Utf8, false),
                (POSITION_COLUMN, &DataType::Utf8, false),
                (COMMIT_TIMESTAMP_COLUMN, &DataType::Int64, false),
                ("id", &DataType::Int64, true),
                ("total", &DataType::Utf8, true),
            ]
        );
    }

    #[test]
    fn a_table_without_visible_columns_has_no_schema() {
        assert!(matches!(
            changelog_schema("public", "gone", Vec::new()),
            Err(WriteError::SchemaIntrospection(_))
        ));
    }

    #[test]
    fn a_backfill_commit_carries_the_checkpoint_forward_under_its_own_key() {
        let source = source();
        let commit =
            backfill_snapshot_properties(&source, Some(Checkpoint::Commit("0/30".to_string())));
        assert_eq!(commit[COMMIT_POSITION_PROPERTY], "0/30");
        assert!(!commit.contains_key(LEGACY_POSITION_PROPERTY));
        assert_eq!(commit[BACKFILL_SNAPSHOT_PROPERTY], "chunk");
        assert_eq!(commit[COMMIT_SOURCE_ID_PROPERTY], source.id.to_string());

        let legacy = backfill_snapshot_properties(
            &source,
            Some(Checkpoint::LegacyChange("0/20".to_string())),
        );
        assert_eq!(legacy[LEGACY_POSITION_PROPERTY], "0/20");
        assert!(!legacy.contains_key(COMMIT_POSITION_PROPERTY));

        let none = backfill_snapshot_properties(&source, None);
        assert!(!none.contains_key(COMMIT_POSITION_PROPERTY));
        assert!(!none.contains_key(LEGACY_POSITION_PROPERTY));
    }

    #[tokio::test]
    async fn a_missing_namespace_is_created_and_an_existing_one_kept() {
        let (writer, catalog) = writer().await;
        let namespace = NamespaceIdent::new("cdc_test".to_string());
        writer.ensure_namespace(&namespace).await.unwrap();
        assert!(catalog.namespace_exists(&namespace).await.unwrap());
        writer.ensure_namespace(&namespace).await.unwrap();
    }

    #[tokio::test]
    async fn a_table_created_by_someone_else_first_is_loaded() {
        let (writer, catalog) = writer().await;
        let ident = TableIdent::new(
            NamespaceIdent::new("cdc_test".to_string()),
            "public__orders".to_string(),
        );
        writer.ensure_namespace(ident.namespace()).await.unwrap();

        let created = writer
            .create_or_load(&ident, orders_creation(ident.name(), HashMap::new()))
            .await
            .unwrap();
        let again = writer
            .create_or_load(&ident, orders_creation(ident.name(), HashMap::new()))
            .await
            .unwrap();
        assert_eq!(again.metadata().uuid(), created.metadata().uuid());
        assert!(catalog.table_exists(&ident).await.unwrap());

        // A table that exists is opened without reaching the source.
        let opened = writer
            .open_or_create(&source(), &ident, "public", "orders")
            .await
            .unwrap();
        assert_eq!(opened.metadata().uuid(), created.metadata().uuid());
    }

    #[tokio::test]
    async fn key_columns_are_recorded_on_the_tables_that_lack_them() {
        let (writer, _) = writer().await;
        let source = source();
        assert!(
            writer
                .tables_without_key_columns(&source)
                .await
                .unwrap()
                .is_empty(),
            "no namespace yet"
        );

        let namespace = NamespaceIdent::new(namespace_for_source(&source));
        writer.ensure_namespace(&namespace).await.unwrap();
        for (name, properties) in [
            ("public__orders", HashMap::new()),
            ("public__gone", HashMap::new()),
            (
                "public__keyed",
                HashMap::from([(KEY_COLUMNS_PROPERTY.to_string(), r#"["id"]"#.to_string())]),
            ),
        ] {
            writer
                .create_or_load(
                    &TableIdent::new(namespace.clone(), name.to_string()),
                    orders_creation(name, properties),
                )
                .await
                .unwrap();
        }

        let mut missing: Vec<String> = writer
            .tables_without_key_columns(&source)
            .await
            .unwrap()
            .iter()
            .map(|table| table.identifier().name().to_string())
            .collect();
        missing.sort();
        assert_eq!(missing, ["public__gone", "public__orders"]);

        // `public__gone`'s source table no longer exists: nothing to record for it.
        let tables = writer.tables_without_key_columns(&source).await.unwrap();
        let keys = HashMap::from([(
            "public__orders".to_string(),
            vec!["id".to_string(), "line".to_string()],
        )]);
        writer.record_keys(tables, &keys).await.unwrap();

        let still_missing: Vec<String> = writer
            .tables_without_key_columns(&source)
            .await
            .unwrap()
            .iter()
            .map(|table| table.identifier().name().to_string())
            .collect();
        assert_eq!(still_missing, ["public__gone"]);
        let orders = writer
            .load_table(&TableIdent::new(namespace, "public__orders".to_string()))
            .await
            .unwrap();
        assert_eq!(
            orders.metadata().properties()[KEY_COLUMNS_PROPERTY],
            r#"["id","line"]"#
        );
    }
}
