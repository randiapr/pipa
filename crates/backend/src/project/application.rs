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
        self.ensure_name_available(&new.name, None).await?;
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
        self.ensure_name_available(&update.name, Some(id)).await?;
        project.apply_update(update)?;
        self.repository.save(&project).await?;
        Ok(project)
    }

    pub async fn remove(&self, id: ProjectId) -> Result<(), ProjectError> {
        self.get(id).await?;
        self.repository.delete(id).await
    }

    /// Rejects a name already held by another project (case-insensitive, trimmed), so two
    /// projects can never be confused for one another in the dashboard. `exclude` is the
    /// project being updated, which is allowed to keep its own name.
    async fn ensure_name_available(
        &self,
        name: &str,
        exclude: Option<ProjectId>,
    ) -> Result<(), ProjectError> {
        let name = name.trim();
        let taken = self.repository.list().await?.into_iter().any(|project| {
            Some(project.id) != exclude && project.name.trim().eq_ignore_ascii_case(name)
        });
        if taken {
            return Err(ProjectError::DuplicateName(name.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;

    use super::*;
    use crate::project::infrastructure::ObjectStoreProjectRepository;

    fn service() -> ProjectService {
        ProjectService::new(Arc::new(ObjectStoreProjectRepository::new(Arc::new(
            InMemory::new(),
        ))))
    }

    fn new_project(name: &str) -> NewProject {
        NewProject {
            name: name.to_string(),
            description: None,
        }
    }

    #[tokio::test]
    async fn removing_an_unknown_project_is_not_found() {
        let err = service().remove(ProjectId::new()).await.unwrap_err();
        assert!(matches!(err, ProjectError::NotFound(_)));
    }

    #[tokio::test]
    async fn a_removed_project_is_gone() {
        let service = service();
        let project = service.register(new_project("alpha")).await.unwrap();
        service.remove(project.id).await.unwrap();
        assert!(matches!(
            service.get(project.id).await,
            Err(ProjectError::NotFound(_))
        ));
        assert!(matches!(
            service.remove(project.id).await,
            Err(ProjectError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn updating_an_unknown_project_is_not_found_even_with_a_taken_name() {
        let service = service();
        service.register(new_project("taken")).await.unwrap();
        let err = service
            .update(
                ProjectId::new(),
                ProjectUpdate {
                    name: "Taken".to_string(),
                    description: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProjectError::NotFound(_)));
    }
}
