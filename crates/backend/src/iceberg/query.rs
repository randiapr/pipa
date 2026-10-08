//! Ad-hoc SQL query execution against Iceberg tables, via DataFusion, through the REST catalog.

use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;

use crate::datasource::DataSourceId;
use crate::storage::ObjectStoreConfig;
use datafusion::arrow::json::writer::ArrayWriter;
use datafusion::catalog::{CatalogProvider, SchemaProvider};
use datafusion::execution::context::SQLOptions;
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;
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

    /// Reads rows of `namespace.table`, JSON-encoded like [`query`](Self::query), `offset` rows
    /// in.
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
        let ctx = self
            .session(Some(HashSet::from([namespace.to_string()])))
            .await?;
        let table = TableReference::full(self.catalog.name.as_str(), namespace, table);
        read_rows(&ctx, table, limit, offset).await
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
