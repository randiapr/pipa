//! Application layer: use cases orchestrating the data source domain via its ports.

use std::sync::Arc;

use crate::datasource::domain::{
    ConnectionTestOutcome, ConnectionTester, DataSource, DataSourceError, DataSourceId,
    DataSourceRepository, NewDataSource, SchemaExplorer, SourceTable, TableRef,
};

/// Orchestrates registering, listing, testing and exploring OLTP data sources, and choosing the
/// tables to ingest.
///
/// Depends only on the [`DataSourceRepository`], [`ConnectionTester`] and [`SchemaExplorer`]
/// ports, so it stays
/// agnostic to how sources are persisted or how connections are verified — callers inject
/// concrete adapters from [`crate::datasource::infrastructure`].
pub struct DataSourceService {
    repository: Arc<dyn DataSourceRepository>,
    tester: Arc<dyn ConnectionTester>,
    explorer: Arc<dyn SchemaExplorer>,
}

impl DataSourceService {
    pub fn new(
        repository: Arc<dyn DataSourceRepository>,
        tester: Arc<dyn ConnectionTester>,
        explorer: Arc<dyn SchemaExplorer>,
    ) -> Self {
        Self {
            repository,
            tester,
            explorer,
        }
    }

    pub async fn register(&self, new: NewDataSource) -> Result<DataSource, DataSourceError> {
        let source = DataSource::register(new)?;
        self.repository.save(&source).await?;
        Ok(source)
    }

    pub async fn list(&self) -> Result<Vec<DataSource>, DataSourceError> {
        self.repository.list().await
    }

    pub async fn get(&self, id: DataSourceId) -> Result<DataSource, DataSourceError> {
        self.repository
            .find_by_id(id)
            .await?
            .ok_or(DataSourceError::NotFound(id))
    }

    pub async fn remove(&self, id: DataSourceId) -> Result<(), DataSourceError> {
        self.repository.delete(id).await
    }

    pub async fn test_connection(
        &self,
        id: DataSourceId,
    ) -> Result<ConnectionTestOutcome, DataSourceError> {
        let source = self.get(id).await?;
        Ok(self.tester.test(&source).await)
    }

    /// The tables of `source`'s database, read live from it.
    pub async fn list_tables(
        &self,
        source: &DataSource,
    ) -> Result<Vec<SourceTable>, DataSourceError> {
        self.explorer.list_tables(source).await
    }

    /// Replaces the tables data source `id` ingests.
    pub async fn choose_ingested_tables(
        &self,
        id: DataSourceId,
        tables: Vec<TableRef>,
    ) -> Result<DataSource, DataSourceError> {
        let mut source = self.get(id).await?;
        source.choose_ingested_tables(tables)?;
        self.repository.save(&source).await?;
        Ok(source)
    }
}
