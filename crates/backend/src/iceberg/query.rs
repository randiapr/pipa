//! Ad-hoc SQL query execution against Iceberg tables, via DataFusion, through the REST catalog.

use std::sync::Arc;

use datafusion::arrow::json::writer::ArrayWriter;
use datafusion::prelude::SessionContext;
use iceberg_datafusion::IcebergCatalogProvider;
use pipa_storage::ObjectStoreConfig;
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
    /// Builds a fresh DataFusion session and catalog provider on every call rather than caching
    /// one, since `IcebergCatalogProvider` snapshots the catalog's namespaces/tables at
    /// construction time and would otherwise miss tables registered after the first query.
    pub async fn query(&self, sql: &str) -> Result<Vec<u8>, QueryError> {
        let catalog = self
            .catalog
            .build_catalog(&self.store)
            .await
            .map_err(QueryError::Catalog)?;
        let provider = IcebergCatalogProvider::try_new(catalog)
            .await
            .map_err(|err| QueryError::Catalog(err.into()))?;

        let ctx = SessionContext::new();
        ctx.register_catalog(self.catalog.name.clone(), Arc::new(provider));

        let batches = ctx.sql(sql).await?.collect().await?;

        let mut writer = ArrayWriter::new(Vec::new());
        writer.write_batches(&batches.iter().collect::<Vec<_>>())?;
        writer.finish()?;
        Ok(writer.into_inner())
    }
}
