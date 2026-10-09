//! Concrete [`IcebergWriter`] adapter: Iceberg REST catalog access, target-table
//! auto-provisioning (introspecting the source Postgres table's schema via
//! `information_schema` on first sight), and the actual `fast_append` commit.
//!
//! Reimplements (rather than imports) the catalog-access pattern `pipa-backend`'s own
//! `crates/backend/src/iceberg/catalog.rs` uses — see the root `CLAUDE.md`'s note on why
//! `pipa-ingestion` keeps its own independent copy of the `iceberg`/`iceberg-catalog-rest`
//! dependency rather than sharing one through `pipa-backend`. Request signing for RustFS's
//! catalog is the one shared piece: it comes from `pipa-catalog-proxy`, embedded in-process.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema as ArrowSchema, SchemaRef, TimeUnit};
use async_trait::async_trait;
use iceberg::io::{
    S3_ACCESS_KEY_ID, S3_ENDPOINT, S3_PATH_STYLE_ACCESS, S3_REGION, S3_SECRET_ACCESS_KEY,
};
use iceberg::spec::{DataFileFormat, Schema as IcebergSchema};
use iceberg::table::Table;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use iceberg::writer::{IcebergWriter as IcebergFileWriter, IcebergWriterBuilder};
use iceberg::{
    Catalog, CatalogBuilder, ErrorKind, NamespaceIdent, TableCreation, TableIdent,
    arrow::{arrow_schema_to_schema_auto_assign_ids, schema_to_arrow_schema},
};
use iceberg_catalog_rest::{
    REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE, RestCatalogBuilder,
};
use iceberg_storage_opendal::OpenDalStorageFactory;
use parquet::file::properties::WriterProperties;
use pipa_catalog_proxy::{CatalogProxy, DEFAULT_SERVICE, sigv4::Signer};
use sqlx::{Connection, PgConnection, Row, postgres::PgConnectOptions};
use tokio::sync::Mutex;

use crate::capture::domain::ChangeEvent;
use crate::datasource::DataSource;
use crate::storage::ObjectStoreConfig;
use crate::write::domain::{
    COMMIT_TIMESTAMP_COLUMN, Checkpoint, IcebergWriter, OP_COLUMN, POSITION_COLUMN,
    SOURCE_ID_COLUMN, TargetTable, WriteError, events_to_record_batch, namespace_for_source,
    target_table_name,
};

/// Snapshot summary property carrying the checkpoint — the commit position of the last
/// transaction landed by that commit (see [`Checkpoint::Commit`]).
const COMMIT_POSITION_PROPERTY: &str = "pipa.cdc.commit_position";
/// The checkpoint property snapshots written by pipa-ingestion 0.4 and earlier carry instead: a
/// change position, read back as [`Checkpoint::LegacyChange`].
const LEGACY_POSITION_PROPERTY: &str = "pipa.cdc.position";
/// Table property naming the source table's row key — its replica identity columns (primary
/// key by default), as a JSON array of column names, `[]` for a table without one. Lets readers
/// such as `pipa-backend` collapse the changelog to current rows from Iceberg alone.
const KEY_COLUMNS_PROPERTY: &str = "pipa.cdc.key_columns";
/// Snapshot summary property carrying the source id a commit came from, for debuggability.
const COMMIT_SOURCE_ID_PROPERTY: &str = "pipa.cdc.source_id";

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
}

impl IcebergCatalogConfig {
    /// Reads connection settings from `ICEBERG_CATALOG_*` environment variables, defaulting
    /// `uri` to RustFS's "S3 Tables" catalog, which RustFS embeds directly into the object store
    /// itself. That catalog only accepts SigV4-signed requests, which `iceberg-catalog-rest`
    /// can't make, so the default starts a `pipa-catalog-proxy` signer inside this process (which
    /// also enables S3 Tables on `store`'s bucket) and points `uri` at it.
    pub async fn from_env(store: &ObjectStoreConfig) -> anyhow::Result<Self> {
        let uri = match std::env::var("ICEBERG_CATALOG_URI") {
            Ok(uri) if !uri.is_empty() => uri,
            _ => embedded_catalog_uri(store).await?,
        };
        Ok(Self {
            name: std::env::var("ICEBERG_CATALOG_NAME").unwrap_or_else(|_| "pipa".to_string()),
            uri,
            warehouse: std::env::var("ICEBERG_CATALOG_WAREHOUSE")
                .unwrap_or_else(|_| "pipa".to_string()),
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

        // iceberg 0.10 ships no S3 FileIO of its own: the REST catalog needs an explicit storage
        // factory to read/write table data and metadata files.
        let catalog = RestCatalogBuilder::default()
            .with_storage_factory(Arc::new(OpenDalStorageFactory::S3 {
                customized_credential_load: None,
            }))
            .load(self.name.clone(), props)
            .await?;
        Ok(Arc::new(catalog))
    }
}

/// Starts the in-process signer in front of RustFS's catalog at `store`'s endpoint, with `store`'s
/// credentials, and returns its catalog URI.
async fn embedded_catalog_uri(store: &ObjectStoreConfig) -> anyhow::Result<String> {
    let signer = Signer::new(
        store.access_key_id.clone(),
        store.secret_access_key.clone(),
        store.region.clone(),
        DEFAULT_SERVICE.to_string(),
    );
    CatalogProxy::new(&store.endpoint, signer)?
        .serve_embedded(&store.bucket)
        .await
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
    /// table for `events[0]`'s `(schema, table)` on first sight — and returns the loaded/created
    /// [`Table`] plus its Arrow schema.
    async fn ensure_table(
        &self,
        source: &DataSource,
        target: &TargetTable,
        events: &[ChangeEvent],
    ) -> Result<(Table, SchemaRef), WriteError> {
        let ident = Self::table_ident(target);

        if let Some(schema) = self.schemas.lock().await.get(target).cloned() {
            let table = self
                .catalog
                .load_table(&ident)
                .await
                .map_err(|err| WriteError::Catalog(err.to_string()))?;
            return Ok((table, schema));
        }

        if !self
            .catalog
            .namespace_exists(ident.namespace())
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
        {
            match self
                .catalog
                .create_namespace(ident.namespace(), HashMap::new())
                .await
            {
                Ok(_) => {}
                Err(err) if err.kind() == ErrorKind::NamespaceAlreadyExists => {}
                Err(err) => return Err(WriteError::Catalog(err.to_string())),
            }
        }

        let table_exists = self
            .catalog
            .table_exists(&ident)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?;

        let table = if table_exists {
            self.catalog
                .load_table(&ident)
                .await
                .map_err(|err| WriteError::Catalog(err.to_string()))?
        } else {
            let first = events.first().ok_or_else(|| {
                WriteError::Mapping("cannot create a target table from an empty batch".to_string())
            })?;
            let iceberg_schema =
                introspect_iceberg_schema(source, &first.schema, &first.table).await?;
            let key_columns = source_key_columns(source)
                .await?
                .remove(&(first.schema.clone(), first.table.clone()))
                .unwrap_or_default();
            let creation = TableCreation::builder()
                .name(target.table.clone())
                .schema(iceberg_schema)
                .properties(HashMap::from([(
                    KEY_COLUMNS_PROPERTY.to_string(),
                    key_columns_property(&key_columns),
                )]))
                .build();

            match self.catalog.create_table(ident.namespace(), creation).await {
                Ok(table) => table,
                Err(err) if err.kind() == ErrorKind::TableAlreadyExists => self
                    .catalog
                    .load_table(&ident)
                    .await
                    .map_err(|err| WriteError::Catalog(err.to_string()))?,
                Err(err) => return Err(WriteError::Catalog(err.to_string())),
            }
        };

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
        if events.is_empty() {
            return Ok(());
        }

        let (table, arrow_schema) = self.ensure_table(source, target, events).await?;
        let batch = events_to_record_batch(&arrow_schema, &source.id.to_string(), events)?;

        let location_generator = DefaultLocationGenerator::new(table.metadata())
            .map_err(|err| WriteError::Commit(err.to_string()))?;
        // The generator's counter restarts at 0 for every batch, so a fixed prefix would name each
        // batch's file `pipa-cdc-00000.parquet` and the second commit would collide with the first.
        let file_name_generator = DefaultFileNameGenerator::new(
            format!("pipa-cdc-{}", uuid::Uuid::now_v7().simple()),
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
        let data_file_writer_builder = DataFileWriterBuilder::new(rolling_writer_builder);

        let mut writer = data_file_writer_builder
            .build(None)
            .await
            .map_err(|err| WriteError::Commit(err.to_string()))?;
        writer
            .write(batch)
            .await
            .map_err(|err| WriteError::Commit(err.to_string()))?;
        let data_files = writer
            .close()
            .await
            .map_err(|err| WriteError::Commit(err.to_string()))?;

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

        Ok(table.metadata().current_snapshot().and_then(|snapshot| {
            let properties = &snapshot.summary().additional_properties;
            properties
                .get(COMMIT_POSITION_PROPERTY)
                .map(|position| Checkpoint::Commit(position.clone()))
                .or_else(|| {
                    properties
                        .get(LEGACY_POSITION_PROPERTY)
                        .map(|position| Checkpoint::LegacyChange(position.clone()))
                })
        }))
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
        let namespace = NamespaceIdent::new(namespace_for_source(source));
        if !self
            .catalog
            .namespace_exists(&namespace)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
        {
            return Ok(());
        }
        let idents = self
            .catalog
            .list_tables(&namespace)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?;

        let mut missing = Vec::new();
        for ident in idents {
            let table = self
                .catalog
                .load_table(&ident)
                .await
                .map_err(|err| WriteError::Catalog(err.to_string()))?;
            if !table
                .metadata()
                .properties()
                .contains_key(KEY_COLUMNS_PROPERTY)
            {
                missing.push(table);
            }
        }
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
        for table in missing {
            let name = table.identifier().name().to_string();
            // A target whose source table is gone has no key to record.
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
                .map_err(|err| WriteError::Catalog(err.to_string()))?;
            tx.commit(self.catalog.as_ref())
                .await
                .map_err(|err| WriteError::Catalog(err.to_string()))?;
            tracing::info!(table = %name, ?columns, "recorded key columns");
        }
        Ok(())
    }
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
/// Every table of `source`'s database with its row key: the replica identity's columns, in
/// index order — the primary key by default, the chosen index for `REPLICA IDENTITY USING
/// INDEX`. Tables without one (no primary key, or `REPLICA IDENTITY FULL`/`NOTHING`) map to
/// nothing.
async fn source_key_columns(
    source: &DataSource,
) -> Result<HashMap<(String, String), Vec<String>>, WriteError> {
    let mut conn = connect_source(source).await?;
    let rows = sqlx::query(
        "SELECT n.nspname AS schema_name, c.relname AS table_name, a.attname AS column_name \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_index i ON i.indrelid = c.oid \
           AND ((c.relreplident = 'd' AND i.indisprimary) \
             OR (c.relreplident = 'i' AND i.indisreplident)) \
         CROSS JOIN LATERAL unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
         JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum \
         WHERE c.relkind IN ('r', 'p') \
           AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
         ORDER BY n.nspname, c.relname, k.ord",
    )
    .fetch_all(&mut conn)
    .await
    .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?;

    let mut keys: HashMap<(String, String), Vec<String>> = HashMap::new();
    for row in rows {
        let get = |name: &str| -> Result<String, WriteError> {
            row.try_get(name)
                .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))
        };
        keys.entry((get("schema_name")?, get("table_name")?))
            .or_default()
            .push(get("column_name")?);
    }
    Ok(keys)
}

/// [`KEY_COLUMNS_PROPERTY`]'s value for `columns`.
fn key_columns_property(columns: &[String]) -> String {
    serde_json::to_string(columns).expect("a list of strings always serializes")
}

async fn connect_source(source: &DataSource) -> Result<PgConnection, WriteError> {
    let options = PgConnectOptions::new()
        .host(&source.connection.host)
        .port(source.connection.port)
        .username(&source.connection.username)
        .password(&source.connection.password)
        .database(&source.connection.database);
    PgConnection::connect_with(&options)
        .await
        .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))
}

async fn introspect_iceberg_schema(
    source: &DataSource,
    pg_schema: &str,
    pg_table: &str,
) -> Result<IcebergSchema, WriteError> {
    let mut conn = connect_source(source).await?;

    let rows = sqlx::query(
        "SELECT column_name, data_type FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
    )
    .bind(pg_schema)
    .bind(pg_table)
    .fetch_all(&mut conn)
    .await
    .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?;

    if rows.is_empty() {
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

    for row in rows {
        let name: String = row
            .try_get("column_name")
            .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?;
        let pg_type: String = row
            .try_get("data_type")
            .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?;
        fields.push(Field::new(name, pg_type_to_arrow(&pg_type), true));
    }

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
