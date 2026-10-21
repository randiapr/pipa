// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Object-store-backed repository for the `SavedQuery` aggregate.
//!
//! Persists each saved query as a JSON object under the `saved_queries/` prefix of the shared
//! RustFS/S3-compatible object store, mirroring how projects and data sources are persisted.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path as ObjectPath};

use crate::saved_query::domain::{SavedQuery, SavedQueryError, SavedQueryId, SavedQueryRepository};

const PREFIX: &str = "saved_queries";

/// Persists `SavedQuery` aggregates as JSON objects in the configured object store.
pub struct ObjectStoreSavedQueryRepository {
    store: Arc<dyn ObjectStore>,
}

impl ObjectStoreSavedQueryRepository {
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    fn path_for(id: SavedQueryId) -> ObjectPath {
        ObjectPath::from(format!("{PREFIX}/{id}.json"))
    }
}

fn storage(err: impl ToString) -> SavedQueryError {
    SavedQueryError::Storage(err.to_string())
}

#[async_trait]
impl SavedQueryRepository for ObjectStoreSavedQueryRepository {
    async fn save(&self, saved_query: &SavedQuery) -> Result<(), SavedQueryError> {
        let bytes = serde_json::to_vec(saved_query).map_err(storage)?;
        self.store
            .put(&Self::path_for(saved_query.id), PutPayload::from(bytes))
            .await
            .map_err(storage)?;
        Ok(())
    }

    async fn find_by_id(&self, id: SavedQueryId) -> Result<Option<SavedQuery>, SavedQueryError> {
        match self.store.get(&Self::path_for(id)).await {
            Ok(result) => {
                let bytes = result.bytes().await.map_err(storage)?;
                Ok(Some(serde_json::from_slice(&bytes).map_err(storage)?))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(storage(err)),
        }
    }

    async fn list(&self) -> Result<Vec<SavedQuery>, SavedQueryError> {
        let mut listing = self.store.list(Some(&ObjectPath::from(PREFIX)));
        let mut queries = Vec::new();

        while let Some(meta) = listing.next().await {
            let meta = meta.map_err(storage)?;
            // An object listed a moment ago may have been deleted since: that is not an error,
            // the entry is simply gone.
            let bytes = match self.store.get(&meta.location).await {
                Ok(result) => result.bytes().await.map_err(storage)?,
                Err(object_store::Error::NotFound { .. }) => continue,
                Err(err) => return Err(storage(err)),
            };
            queries.push(serde_json::from_slice(&bytes).map_err(storage)?);
        }

        Ok(queries)
    }

    async fn delete(&self, id: SavedQueryId) -> Result<(), SavedQueryError> {
        self.store
            .delete(&Self::path_for(id))
            .await
            .map_err(storage)?;
        Ok(())
    }
}
