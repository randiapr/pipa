//! Concrete [`IcebergWriter`] adapter: Iceberg REST catalog access, target-table
//! auto-provisioning (introspecting the source Postgres table's schema via
//! `information_schema` on first sight), and the actual `fast_append` commit.
//!
//! Reimplements (rather than imports) the catalog-access pattern `pipa-backend`'s own
//! `crates/backend/src/iceberg/catalog.rs` uses — see the root `CLAUDE.md`'s note on why
//! `pipa-ingestion` keeps its own independent copy of the `iceberg`/`iceberg-catalog-rest`
//! dependency rather than sharing one through `pipa-backend`.

use std::collections::HashMap;
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
use parquet::file::properties::WriterProperties;
use sqlx::{Connection, PgConnection, Row, postgres::PgConnectOptions};
use tokio::sync::Mutex;

use crate::capture::domain::ChangeEvent;
use crate::datasource::DataSource;
use crate::storage::ObjectStoreConfig;
use crate::write::domain::{
    COMMIT_TIMESTAMP_COLUMN, IcebergWriter, OP_COLUMN, POSITION_COLUMN, SOURCE_ID_COLUMN,
    TargetTable, WriteError, events_to_record_batch, namespace_for_source,
};

/// Snapshot summary property carrying the checkpoint — the highest source position landed by
/// the commit that set it. Read back by [`IcebergChangelogWriter::last_committed_position`].
const COMMIT_POSITION_PROPERTY: &str = "pipa.cdc.position";
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
    /// `uri` to `store_endpoint`'s own `/iceberg` path — RustFS's "S3 Tables" feature embeds an
    /// Iceberg REST Catalog directly into the object store itself.
    pub fn from_env(store_endpoint: &str) -> Self {
        Self {
            name: std::env::var("ICEBERG_CATALOG_NAME").unwrap_or_else(|_| "pipa".to_string()),
            uri: std::env::var("ICEBERG_CATALOG_URI")
                .unwrap_or_else(|_| format!("{}/iceberg", store_endpoint.trim_end_matches('/'))),
            warehouse: std::env::var("ICEBERG_CATALOG_WAREHOUSE")
                .unwrap_or_else(|_| "pipa".to_string()),
        }
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

        let catalog = RestCatalogBuilder::default()
            .load(self.name.clone(), props)
            .await?;
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
            let creation = TableCreation::builder()
                .name(target.table.clone())
                .schema(iceberg_schema)
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
        let file_name_generator =
            DefaultFileNameGenerator::new("pipa-cdc".to_string(), None, DataFileFormat::Parquet);
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
    ) -> Result<Option<String>, WriteError> {
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

        Ok(table
            .metadata()
            .current_snapshot()
            .and_then(|snapshot| {
                snapshot
                    .summary()
                    .additional_properties
                    .get(COMMIT_POSITION_PROPERTY)
            })
            .cloned())
    }

    async fn existing_targets(&self, source: &DataSource) -> Result<Vec<TargetTable>, WriteError> {
        let namespace_name = namespace_for_source(source);
        let namespace = NamespaceIdent::new(namespace_name.clone());

        if !self
            .catalog
            .namespace_exists(&namespace)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?
        {
            return Ok(Vec::new());
        }

        let idents = self
            .catalog
            .list_tables(&namespace)
            .await
            .map_err(|err| WriteError::Catalog(err.to_string()))?;

        Ok(idents
            .into_iter()
            .map(|ident| TargetTable {
                namespace: namespace_name.clone(),
                table: ident.name,
            })
            .collect())
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
async fn introspect_iceberg_schema(
    source: &DataSource,
    pg_schema: &str,
    pg_table: &str,
) -> Result<IcebergSchema, WriteError> {
    let options = PgConnectOptions::new()
        .host(&source.connection.host)
        .port(source.connection.port)
        .username(&source.connection.username)
        .password(&source.connection.password)
        .database(&source.connection.database);

    let mut conn = PgConnection::connect_with(&options)
        .await
        .map_err(|err| WriteError::SchemaIntrospection(err.to_string()))?;

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
