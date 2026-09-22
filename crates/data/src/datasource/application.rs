//! Application layer: use cases orchestrating the data source domain via its ports.

use std::sync::Arc;

use crate::datasource::domain::{
    ConnectionTestOutcome, ConnectionTester, DataSource, DataSourceError, DataSourceId,
    DataSourceRepository, NewDataSource,
};

/// Orchestrates registering, listing, and testing OLTP data sources.
///
/// Depends only on the [`DataSourceRepository`] and [`ConnectionTester`] ports, so it stays
/// agnostic to how sources are persisted or how connections are verified — callers inject
/// concrete adapters from [`crate::datasource::infrastructure`].
pub struct DataSourceService {
    repository: Arc<dyn DataSourceRepository>,
    tester: Arc<dyn ConnectionTester>,
}

impl DataSourceService {
    pub fn new(
        repository: Arc<dyn DataSourceRepository>,
        tester: Arc<dyn ConnectionTester>,
    ) -> Self {
        Self { repository, tester }
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
}
