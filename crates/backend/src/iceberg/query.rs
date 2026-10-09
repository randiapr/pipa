//! Ad-hoc SQL query execution against Iceberg tables, via DataFusion, through the REST catalog.

use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;

use crate::datasource::DataSourceId;
use crate::storage::ObjectStoreConfig;
use datafusion::arrow::json::writer::ArrayWriter;
use datafusion::catalog::{CatalogProvider, SchemaProvider};
use datafusion::execution::context::SQLOptions;
use datafusion::functions::string::expr_fn::{concat, split_part};
use datafusion::functions::unicode::expr_fn::lpad;
use datafusion::functions_window::expr_fn::row_number;
use datafusion::logical_expr::{Expr, ExprFunctionExt};
use datafusion::prelude::{SessionContext, ident, lit};
use datafusion::sql::TableReference;
use iceberg::{Catalog, NamespaceIdent, TableIdent};
use iceberg_datafusion::IcebergCatalogProvider;
use thiserror::Error;

use super::catalog::IcebergCatalogConfig;

/// Errors that can occur while running an ad-hoc SQL query.
#[derive(Debug, Error)]
pub enum QueryError {
    #[error("failed to connect to the Iceberg catalog: {0}")]
    Catalog(#[source] anyhow::Error),
    #[error("query failed: {0}")]
    Execution(#[from] datafusion::error::DataFusionError),
    #[error("failed to encode query results: {0}")]
    Encoding(#[from] datafusion::arrow::error::ArrowError),
}

/// The Iceberg namespace `pipa-ingestion` writes a data source's tables into.
///
/// This duplicates `namespace_for_source` in `crates/ingestion/src/write/domain.rs` on purpose
/// (`pipa-backend` never depends on `pipa-ingestion`): the two must produce the same string, or
/// project-scoped queries would silently see nothing.
pub fn namespace_for_source(id: DataSourceId) -> String {
    format!("cdc_{}", id.0.simple())
}

/// A catalog that only exposes some of another catalog's namespaces, so a query can't name
/// tables outside them.
#[derive(Debug)]
struct ScopedCatalog {
    inner: Arc<dyn CatalogProvider>,
    allowed: HashSet<String>,
}

impl CatalogProvider for ScopedCatalog {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema_names(&self) -> Vec<String> {
        self.inner
            .schema_names()
            .into_iter()
            .filter(|name| self.allowed.contains(name))
            .collect()
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        if self.allowed.contains(name) {
            self.inner.schema(name)
        } else {
            None
        }
    }
}

/// Runs SQL queries, via DataFusion, against the Iceberg tables a REST catalog exposes.
#[derive(Debug, Clone)]
pub struct QueryService {
    catalog: IcebergCatalogConfig,
    store: ObjectStoreConfig,
}

impl QueryService {
    pub fn new(catalog: IcebergCatalogConfig, store: ObjectStoreConfig) -> Self {
        Self { catalog, store }
    }

    /// Executes `sql` and returns the result rows JSON-encoded as an array of objects, one per
    /// row. Tables are addressed as `<catalog>.<namespace>.<table>`, e.g.
    /// `SELECT * FROM pipa.public.orders`.
    ///
    ///
    /// `allowed_namespaces` limits which Iceberg namespaces the query can see (`None` sees them
    /// all). Only read-only queries are accepted: DDL (e.g. `CREATE EXTERNAL TABLE`), DML and
    /// session statements are rejected, since they could reach around that limit.
    ///
    /// Builds a fresh DataFusion session and catalog provider on every call rather than caching
    /// one, since `IcebergCatalogProvider` snapshots the catalog's namespaces/tables at
    /// construction time and would otherwise miss tables registered after the first query.
    pub async fn query(
        &self,
        sql: &str,
        allowed_namespaces: Option<HashSet<String>>,
    ) -> Result<Vec<u8>, QueryError> {
        let ctx = self.session(allowed_namespaces).await?;
        run_read_only(&ctx, sql).await
    }

    /// Lists the tables of `namespaces`, as `(namespace, table)` pairs sorted by name.
    pub async fn list_tables(
        &self,
        namespaces: HashSet<String>,
    ) -> Result<Vec<(String, String)>, QueryError> {
        let ctx = self.session(Some(namespaces)).await?;
        let catalog = ctx
            .catalog(&self.catalog.name)
            .expect("session() registers the catalog under this name");
        Ok(tables_of(catalog.as_ref()))
    }

    /// Reads a page of `namespace.table`'s current rows, JSON-encoded like
    /// [`query`](Self::query), `offset` rows in: the table as the source has it now — one row
    /// per key (the table's [`KEY_COLUMNS_PROPERTY`]) at its latest change, deleted rows dropped,
    /// without the changelog columns, sorted by key. A table without a recorded key (or one that
    /// isn't a changelog, like a metadata table) is read as stored.
    ///
    /// Takes no SQL: the read is assembled with the DataFrame API, and the session only sees
    /// `namespace`, so it can't reach another data source's tables. `limit` defaults to
    /// [`DEFAULT_TABLE_ROWS`] and is capped at [`MAX_TABLE_ROWS`].
    pub async fn read_table(
        &self,
        namespace: &str,
        table: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<u8>, QueryError> {
        let catalog = self
            .catalog
            .build_catalog(&self.store)
            .await
            .map_err(QueryError::Catalog)?;
        let key_columns = key_columns(catalog.as_ref(), namespace, table).await;
        let ctx = self
            .session_with(catalog, Some(HashSet::from([namespace.to_string()])))
            .await?;
        let table = TableReference::full(self.catalog.name.as_str(), namespace, table);
        match key_columns {
            Some(keys) => read_current_rows(&ctx, table, &keys, limit, offset).await,
            None => read_rows(&ctx, table, limit, offset).await,
        }
    }

    /// A fresh DataFusion session with the Iceberg catalog registered, optionally narrowed to
    /// `allowed_namespaces`.
    async fn session(
        &self,
        allowed_namespaces: Option<HashSet<String>>,
    ) -> Result<SessionContext, QueryError> {
        let catalog = self
            .catalog
            .build_catalog(&self.store)
            .await
            .map_err(QueryError::Catalog)?;
        self.session_with(catalog, allowed_namespaces).await
    }

    /// [`Self::session`] over an already built `catalog`.
    async fn session_with(
        &self,
        catalog: Arc<dyn Catalog>,
        allowed_namespaces: Option<HashSet<String>>,
    ) -> Result<SessionContext, QueryError> {
        let provider = IcebergCatalogProvider::try_new(catalog)
            .await
            .map_err(|err| QueryError::Catalog(err.into()))?;

        let provider: Arc<dyn CatalogProvider> = match allowed_namespaces {
            Some(allowed) => Arc::new(ScopedCatalog {
                inner: Arc::new(provider),
                allowed,
            }),
            None => Arc::new(provider),
        };

        let ctx = SessionContext::new();
        ctx.register_catalog(self.catalog.name.clone(), provider);
        Ok(ctx)
    }
}

/// Rows a table read returns when the caller doesn't say.
pub const DEFAULT_TABLE_ROWS: usize = 100;
/// The most rows a single table read may return.
pub const MAX_TABLE_ROWS: usize = 1000;

/// Suffixes of the metadata tables iceberg-datafusion exposes next to every Iceberg table
/// (`orders$snapshots`, `orders$manifests`).
const METADATA_TABLE_SUFFIXES: [&str; 2] = ["$snapshots", "$manifests"];

/// Whether `table` is one of iceberg-datafusion's metadata tables rather than a data table. Only
/// the exact suffixes count, since `$` is legal in a Postgres table name.
pub fn is_metadata_table(table: &str) -> bool {
    METADATA_TABLE_SUFFIXES
        .iter()
        .any(|suffix| table.ends_with(suffix))
}

/// Table property `pipa-ingestion` records each target table's row key in (its source table's
/// replica identity columns, as a JSON array). Duplicates `KEY_COLUMNS_PROPERTY` in
/// `crates/ingestion/src/write/infrastructure.rs` on purpose (no shared crate); keep them equal.
const KEY_COLUMNS_PROPERTY: &str = "pipa.cdc.key_columns";

/// Columns `pipa-ingestion` adds to every changelog row (`crates/ingestion/src/write/domain.rs`).
const OP_COLUMN: &str = "_op";
const POSITION_COLUMN: &str = "_position";
const CHANGELOG_COLUMNS: [&str; 4] = [
    OP_COLUMN,
    "_source_id",
    POSITION_COLUMN,
    "_commit_timestamp_us",
];
/// The window column [`read_current_rows`] ranks a key's changes in; dropped before returning.
const LATEST_COLUMN: &str = "__pipa_latest";

/// `namespace.table`'s recorded row key, if any. `None` when the property is missing or
/// unreadable, or the table can't be loaded (the read then reports that itself).
async fn key_columns(catalog: &dyn Catalog, namespace: &str, table: &str) -> Option<Vec<String>> {
    let ident = TableIdent::new(
        NamespaceIdent::new(namespace.to_string()),
        table.to_string(),
    );
    let table = catalog.load_table(&ident).await.ok()?;
    let value = table.metadata().properties().get(KEY_COLUMNS_PROPERTY)?;
    serde_json::from_str(value).ok()
}

/// A sortable form of a Postgres LSN (`X/Y`, hex without leading zeros): both halves padded to
/// eight digits, so string order is LSN order.
fn sortable_position() -> Expr {
    let half = |n: i64| {
        lpad(vec![
            split_part(ident(POSITION_COLUMN), lit("/"), lit(n)),
            lit(8i64),
            lit("0"),
        ])
    };
    concat(vec![half(1), half(2)])
}

/// Reads the current rows of a changelog `table` keyed by `keys` (see [`QueryService::read_table`]).
/// A row's changes are ranked by `_position`, the order they happened in. With no key
/// (`keys` empty — the source table had no replica identity, so it only ever got inserts),
/// every non-deleted change is a row of its own.
async fn read_current_rows(
    ctx: &SessionContext,
    table: TableReference,
    keys: &[String],
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<u8>, QueryError> {
    let limit = limit.unwrap_or(DEFAULT_TABLE_ROWS).min(MAX_TABLE_ROWS);
    let mut df = ctx.table(table).await?;
    if !keys.is_empty() {
        let latest = row_number()
            .partition_by(keys.iter().map(ident).collect())
            .order_by(vec![sortable_position().sort(false, true)])
            .build()?
            .alias(LATEST_COLUMN);
        df = df
            .window(vec![latest])?
            .filter(ident(LATEST_COLUMN).eq(lit(1u64)))?;
    }
    let order = if keys.is_empty() {
        vec![sortable_position().sort(true, true)]
    } else {
        keys.iter().map(|key| ident(key).sort(true, true)).collect()
    };
    let mut hidden = CHANGELOG_COLUMNS.to_vec();
    hidden.push(LATEST_COLUMN);
    let batches = df
        .filter(ident(OP_COLUMN).not_eq(lit("delete")))?
        .sort(order)?
        .drop_columns(&hidden)?
        .limit(offset.unwrap_or(0), Some(limit))?
        .collect()
        .await?;
    encode(&batches)
}

/// Every `(schema, table)` of `catalog`, sorted.
fn tables_of(catalog: &dyn CatalogProvider) -> Vec<(String, String)> {
    let mut tables: Vec<(String, String)> = catalog
        .schema_names()
        .into_iter()
        .filter_map(|name| catalog.schema(&name).map(|schema| (name, schema)))
        .flat_map(|(name, schema)| {
            schema
                .table_names()
                .into_iter()
                .map(move |table| (name.clone(), table))
        })
        .collect();
    tables.sort();
    tables
}

/// Reads `offset..offset + limit` rows of `table` on `ctx` and JSON-encodes them.
async fn read_rows(
    ctx: &SessionContext,
    table: TableReference,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<u8>, QueryError> {
    let limit = limit.unwrap_or(DEFAULT_TABLE_ROWS).min(MAX_TABLE_ROWS);
    let batches = ctx
        .table(table)
        .await?
        .limit(offset.unwrap_or(0), Some(limit))?
        .collect()
        .await?;
    encode(&batches)
}

/// JSON-encodes result batches as an array of objects, one per row.
fn encode(batches: &[datafusion::arrow::array::RecordBatch]) -> Result<Vec<u8>, QueryError> {
    let mut writer = ArrayWriter::new(Vec::new());
    writer.write_batches(&batches.iter().collect::<Vec<_>>())?;
    writer.finish()?;
    Ok(writer.into_inner())
}

/// Executes `sql` on `ctx` and JSON-encodes the rows, rejecting anything but a read-only query.
async fn run_read_only(ctx: &SessionContext, sql: &str) -> Result<Vec<u8>, QueryError> {
    let options = SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false);
    let batches = ctx.sql_with_options(sql, options).await?.collect().await?;
    encode(&batches)
}

#[cfg(test)]
mod tests {
    use datafusion::catalog::{MemoryCatalogProvider, MemorySchemaProvider};

    use super::*;

    /// `(op, position, id, tier)` of one changelog row.
    type ChangelogRow = (&'static str, &'static str, i32, &'static str);

    /// A changelog table `t` (`_op`, `_source_id`, `_position`, `_commit_timestamp_us`, `id`,
    /// `tier`) holding `rows`.
    fn changelog(rows: &[ChangelogRow]) -> SessionContext {
        use datafusion::arrow::array::{Int32Array, Int64Array, StringArray};
        use datafusion::arrow::datatypes::{DataType, Field, Schema};
        use datafusion::arrow::record_batch::RecordBatch;

        let strings = |pick: fn(&ChangelogRow) -> &'static str| {
            Arc::new(StringArray::from(rows.iter().map(pick).collect::<Vec<_>>()))
        };
        let schema = Arc::new(Schema::new(vec![
            Field::new("_op", DataType::Utf8, false),
            Field::new("_source_id", DataType::Utf8, false),
            Field::new("_position", DataType::Utf8, false),
            Field::new("_commit_timestamp_us", DataType::Int64, false),
            Field::new("id", DataType::Int32, false),
            Field::new("tier", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                strings(|row| row.0),
                strings(|_| "source"),
                strings(|row| row.1),
                Arc::new(Int64Array::from(vec![0; rows.len()])),
                Arc::new(Int32Array::from(
                    rows.iter().map(|row| row.2).collect::<Vec<_>>(),
                )),
                strings(|row| row.3),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        ctx.register_batch("t", batch).unwrap();
        ctx
    }

    async fn current_rows(ctx: &SessionContext, keys: &[&str]) -> serde_json::Value {
        let keys: Vec<String> = keys.iter().map(|key| key.to_string()).collect();
        let body = read_current_rows(ctx, TableReference::bare("t"), &keys, None, None)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn current_rows_keep_each_keys_latest_change() {
        let ctx = changelog(&[
            ("update", "0/1CC23E0", 1, "silver"),
            ("insert", "0/1C8A348", 1, "bronze"),
            ("insert", "0/1C8A528", 2, "bronze"),
            // `0/1000000` is after `0/9FFFFF`, though it sorts before it as plain text.
            ("insert", "0/9FFFFF", 3, "bronze"),
            ("update", "0/1000000", 3, "gold"),
            ("insert", "0/1C8A600", 4, "bronze"),
            ("delete", "0/1D00000", 4, "bronze"),
        ]);

        assert_eq!(
            current_rows(&ctx, &["id"]).await,
            serde_json::json!([
                { "id": 1, "tier": "silver" },
                { "id": 2, "tier": "bronze" },
                { "id": 3, "tier": "gold" },
            ])
        );
    }

    #[tokio::test]
    async fn without_a_key_every_change_but_deletes_is_a_row() {
        let ctx = changelog(&[
            ("insert", "0/20", 2, "b"),
            ("insert", "0/10", 1, "a"),
            ("delete", "0/30", 1, "a"),
        ]);

        assert_eq!(
            current_rows(&ctx, &[]).await,
            serde_json::json!([
                { "id": 1, "tier": "a" },
                { "id": 2, "tier": "b" },
            ])
        );
    }

    #[test]
    fn recognizes_only_iceberg_metadata_tables() {
        assert!(is_metadata_table("public__orders$snapshots"));
        assert!(is_metadata_table("public__orders$manifests"));
        assert!(!is_metadata_table("public__orders"));
        assert!(!is_metadata_table("public__price$list"));
    }

    fn catalog_with(namespaces: &[&str]) -> Arc<dyn CatalogProvider> {
        let catalog = MemoryCatalogProvider::new();
        for namespace in namespaces {
            catalog
                .register_schema(namespace, Arc::new(MemorySchemaProvider::new()))
                .unwrap();
        }
        Arc::new(catalog)
    }

    #[test]
    fn scoped_catalog_hides_other_namespaces() {
        let scoped = ScopedCatalog {
            inner: catalog_with(&["cdc_a", "cdc_b"]),
            allowed: HashSet::from(["cdc_a".to_string()]),
        };
        assert_eq!(scoped.schema_names(), vec!["cdc_a".to_string()]);
        assert!(scoped.schema("cdc_a").is_some());
        assert!(scoped.schema("cdc_b").is_none());
    }

    #[test]
    fn namespace_matches_the_ingestion_convention() {
        let id =
            DataSourceId(uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap());
        assert_eq!(
            namespace_for_source(id),
            "cdc_11111111222233334444555555555555"
        );
    }

    /// A context with `orders(id, region, amount)` holding five rows in a plain memory table.
    fn orders_ctx() -> SessionContext {
        use datafusion::arrow::array::{Int64Array, RecordBatch, StringArray};
        use datafusion::arrow::datatypes::{DataType, Field, Schema};
        use datafusion::datasource::MemTable;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("region", DataType::Utf8, false),
            Field::new("amount", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3, 4, 5])),
                Arc::new(StringArray::from(vec!["EU", "US", "EU", "EU", "US"])),
                Arc::new(Int64Array::from(vec![10, 20, 30, 40, 50])),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        ctx.register_table(
            "orders",
            Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
        )
        .unwrap();
        ctx
    }

    async fn rows(limit: Option<usize>, offset: Option<usize>) -> Result<String, QueryError> {
        read_rows(&orders_ctx(), TableReference::bare("orders"), limit, offset)
            .await
            .map(|body| String::from_utf8(body).unwrap())
    }

    #[tokio::test]
    async fn reading_a_table_returns_its_rows_and_columns() {
        let body = rows(Some(1), None).await.unwrap();
        assert_eq!(body, r#"[{"id":1,"region":"EU","amount":10}]"#);
    }

    #[tokio::test]
    async fn limit_and_offset_page_through_rows_and_limit_is_capped() {
        let body = rows(Some(2), Some(1)).await.unwrap();
        assert!(body.starts_with(r#"[{"id":2,"#) && body.contains(r#""id":3"#));
        assert_eq!(body.matches("\"id\"").count(), 2);

        // An absurd limit is clamped rather than rejected; the table only has five rows anyway.
        let body = rows(Some(usize::MAX), None).await.unwrap();
        assert_eq!(body.matches("\"id\"").count(), 5);

        // Past the end there is simply nothing.
        assert_eq!(rows(None, Some(5)).await.unwrap(), "[]");
    }

    #[tokio::test]
    async fn an_unknown_table_is_rejected() {
        let result = read_rows(&orders_ctx(), TableReference::bare("nope"), None, None).await;
        assert!(matches!(result, Err(QueryError::Execution(_))));
    }

    #[test]
    fn tables_are_listed_per_namespace_and_sorted() {
        use datafusion::arrow::datatypes::Schema;
        use datafusion::datasource::MemTable;

        let catalog = MemoryCatalogProvider::new();
        for (namespace, tables) in [
            ("cdc_b", vec!["t2", "t1"]),
            ("cdc_a", vec!["t3"]),
            ("cdc_c", vec![]),
        ] {
            let schema = Arc::new(MemorySchemaProvider::new());
            for table in tables {
                let empty = MemTable::try_new(Arc::new(Schema::empty()), vec![vec![]]).unwrap();
                schema
                    .register_table(table.to_string(), Arc::new(empty))
                    .unwrap();
            }
            catalog.register_schema(namespace, schema).unwrap();
        }
        assert_eq!(
            tables_of(&catalog),
            [
                ("cdc_a".to_string(), "t3".to_string()),
                ("cdc_b".to_string(), "t1".to_string()),
                ("cdc_b".to_string(), "t2".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn read_only_queries_run() {
        let ctx = SessionContext::new();
        let body = run_read_only(&ctx, "SELECT 1 AS one").await.unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), r#"[{"one":1}]"#);
    }

    #[tokio::test]
    async fn ddl_dml_and_session_statements_are_rejected() {
        let ctx = SessionContext::new();
        for sql in [
            "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION '/etc/passwd'",
            "CREATE TABLE t AS SELECT 1",
            "SET datafusion.execution.batch_size = 1",
        ] {
            assert!(
                matches!(
                    run_read_only(&ctx, sql).await,
                    Err(QueryError::Execution(_))
                ),
                "{sql} should be rejected"
            );
        }
    }
}
