//! Object-store-backed repository for the `DataSource` aggregate.
//!
//! Persists each data source as a JSON object under the `datasources/` prefix of the
//! shared RustFS/S3-compatible object store — the same layout `pipa-ingestion` reads with its
//! own standalone duplicate of this logic, without needing a separate metadata database.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path as ObjectPath};

use crate::datasource::domain::{DataSource, DataSourceError, DataSourceId, DataSourceRepository};

const PREFIX: &str = "datasources";

/// Persists `DataSource` aggregates as JSON objects in the configured object store.
pub struct ObjectStoreDataSourceRepository {
    store: Arc<dyn ObjectStore>,
}

impl ObjectStoreDataSourceRepository {
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    fn path_for(id: DataSourceId) -> ObjectPath {
        ObjectPath::from(format!("{PREFIX}/{id}.json"))
    }
}

#[async_trait]
impl DataSourceRepository for ObjectStoreDataSourceRepository {
    async fn save(&self, source: &DataSource) -> Result<(), DataSourceError> {
        let bytes =
            serde_json::to_vec(source).map_err(|err| DataSourceError::Storage(err.to_string()))?;
        self.store
            .put(&Self::path_for(source.id), PutPayload::from(bytes))
            .await
            .map_err(|err| DataSourceError::Storage(err.to_string()))?;
        Ok(())
    }

    async fn find_by_id(&self, id: DataSourceId) -> Result<Option<DataSource>, DataSourceError> {
        match self.store.get(&Self::path_for(id)).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|err| DataSourceError::Storage(err.to_string()))?;
                let source = serde_json::from_slice(&bytes)
                    .map_err(|err| DataSourceError::Storage(err.to_string()))?;
                Ok(Some(source))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(DataSourceError::Storage(err.to_string())),
        }
    }

    async fn list(&self) -> Result<Vec<DataSource>, DataSourceError> {
        let mut listing = self.store.list(Some(&ObjectPath::from(PREFIX)));
        let mut sources = Vec::new();

        while let Some(meta) = listing.next().await {
            let meta = meta.map_err(|err| DataSourceError::Storage(err.to_string()))?;
            let bytes = self
                .store
                .get(&meta.location)
                .await
                .map_err(|err| DataSourceError::Storage(err.to_string()))?
                .bytes()
                .await
                .map_err(|err| DataSourceError::Storage(err.to_string()))?;
            let source = serde_json::from_slice(&bytes)
                .map_err(|err| DataSourceError::Storage(err.to_string()))?;
            sources.push(source);
        }

        Ok(sources)
    }

    async fn delete(&self, id: DataSourceId) -> Result<(), DataSourceError> {
        self.store
            .delete(&Self::path_for(id))
            .await
            .map_err(|err| DataSourceError::Storage(err.to_string()))?;
        Ok(())
    }
}
