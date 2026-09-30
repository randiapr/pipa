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
        run_read_only(&ctx, sql).await
    }
}

/// Executes `sql` on `ctx` and JSON-encodes the rows, rejecting anything but a read-only query.
async fn run_read_only(ctx: &SessionContext, sql: &str) -> Result<Vec<u8>, QueryError> {
    let options = SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false);
    let batches = ctx.sql_with_options(sql, options).await?.collect().await?;

    let mut writer = ArrayWriter::new(Vec::new());
    writer.write_batches(&batches.iter().collect::<Vec<_>>())?;
    writer.finish()?;
    Ok(writer.into_inner())
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
