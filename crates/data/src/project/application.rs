//! Application layer: use cases orchestrating the project domain via its repository port.

use std::sync::Arc;

use crate::project::domain::{
    NewProject, Project, ProjectError, ProjectId, ProjectRepository, ProjectUpdate,
};

/// Orchestrates registering, listing, updating, and removing projects.
///
/// Depends only on the [`ProjectRepository`] port, so it stays agnostic to how projects are
/// persisted — callers inject a concrete adapter from [`crate::project::infrastructure`].
pub struct ProjectService {
    repository: Arc<dyn ProjectRepository>,
}

impl ProjectService {
    pub fn new(repository: Arc<dyn ProjectRepository>) -> Self {
        Self { repository }
    }

    pub async fn register(&self, new: NewProject) -> Result<Project, ProjectError> {
        let project = Project::register(new)?;
        self.repository.save(&project).await?;
        Ok(project)
    }

    pub async fn list(&self) -> Result<Vec<Project>, ProjectError> {
        self.repository.list().await
    }

    pub async fn get(&self, id: ProjectId) -> Result<Project, ProjectError> {
        self.repository
            .find_by_id(id)
            .await?
            .ok_or(ProjectError::NotFound(id))
    }

    pub async fn update(
        &self,
        id: ProjectId,
        update: ProjectUpdate,
    ) -> Result<Project, ProjectError> {
        let mut project = self.get(id).await?;
        project.apply_update(update)?;
        self.repository.save(&project).await?;
        Ok(project)
    }

    pub async fn remove(&self, id: ProjectId) -> Result<(), ProjectError> {
        self.repository.delete(id).await
    }
}
