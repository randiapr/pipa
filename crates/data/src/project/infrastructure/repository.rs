//! Object-store-backed repository for the `Project` aggregate.
//!
//! Persists each project as a JSON object under the `projects/` prefix of the shared
//! RustFS/S3-compatible object store, mirroring how data sources are persisted.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path as ObjectPath};

use crate::project::domain::{Project, ProjectError, ProjectId, ProjectRepository};

const PREFIX: &str = "projects";

/// Persists `Project` aggregates as JSON objects in the configured object store.
pub struct ObjectStoreProjectRepository {
    store: Arc<dyn ObjectStore>,
}

impl ObjectStoreProjectRepository {
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    fn path_for(id: ProjectId) -> ObjectPath {
        ObjectPath::from(format!("{PREFIX}/{id}.json"))
    }
}

#[async_trait]
impl ProjectRepository for ObjectStoreProjectRepository {
    async fn save(&self, project: &Project) -> Result<(), ProjectError> {
        let bytes =
            serde_json::to_vec(project).map_err(|err| ProjectError::Storage(err.to_string()))?;
        self.store
            .put(&Self::path_for(project.id), PutPayload::from(bytes))
            .await
            .map_err(|err| ProjectError::Storage(err.to_string()))?;
        Ok(())
    }

    async fn find_by_id(&self, id: ProjectId) -> Result<Option<Project>, ProjectError> {
        match self.store.get(&Self::path_for(id)).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|err| ProjectError::Storage(err.to_string()))?;
                let project = serde_json::from_slice(&bytes)
                    .map_err(|err| ProjectError::Storage(err.to_string()))?;
                Ok(Some(project))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(ProjectError::Storage(err.to_string())),
        }
    }

    async fn list(&self) -> Result<Vec<Project>, ProjectError> {
        let mut listing = self.store.list(Some(&ObjectPath::from(PREFIX)));
        let mut projects = Vec::new();

        while let Some(meta) = listing.next().await {
            let meta = meta.map_err(|err| ProjectError::Storage(err.to_string()))?;
            let bytes = self
                .store
                .get(&meta.location)
                .await
                .map_err(|err| ProjectError::Storage(err.to_string()))?
                .bytes()
                .await
                .map_err(|err| ProjectError::Storage(err.to_string()))?;
            let project = serde_json::from_slice(&bytes)
                .map_err(|err| ProjectError::Storage(err.to_string()))?;
            projects.push(project);
        }

        Ok(projects)
    }

    async fn delete(&self, id: ProjectId) -> Result<(), ProjectError> {
        self.store
            .delete(&Self::path_for(id))
            .await
            .map_err(|err| ProjectError::Storage(err.to_string()))?;
        Ok(())
    }
}
