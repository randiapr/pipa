// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Application layer: use cases orchestrating the saved query domain via its repository port.

use std::sync::Arc;

use crate::project::ProjectId;
use crate::saved_query::domain::{
    NewSavedQuery, SavedQuery, SavedQueryError, SavedQueryId, SavedQueryRepository,
    SavedQueryUpdate,
};

/// Orchestrates saving, listing, updating and removing saved queries.
///
/// Depends only on the [`SavedQueryRepository`] port. Whether a caller may touch a project's
/// queries is the HTTP layer's call (it authorizes before asking), as for the other contexts.
pub struct SavedQueryService {
    repository: Arc<dyn SavedQueryRepository>,
}

impl SavedQueryService {
    pub fn new(repository: Arc<dyn SavedQueryRepository>) -> Self {
        Self { repository }
    }

    pub async fn register(&self, new: NewSavedQuery) -> Result<SavedQuery, SavedQueryError> {
        let saved_query = SavedQuery::register(new)?;
        self.ensure_name_available(saved_query.project_id, &saved_query.name, None)
            .await?;
        self.repository.save(&saved_query).await?;
        Ok(saved_query)
    }

    /// The queries of `project`, by name.
    pub async fn list(&self, project: ProjectId) -> Result<Vec<SavedQuery>, SavedQueryError> {
        let mut queries: Vec<_> = self
            .repository
            .list()
            .await?
            .into_iter()
            .filter(|query| query.project_id == project)
            .collect();
        queries.sort_by_key(|query| query.name.to_lowercase());
        Ok(queries)
    }

    pub async fn get(&self, id: SavedQueryId) -> Result<SavedQuery, SavedQueryError> {
        self.repository
            .find_by_id(id)
            .await?
            .ok_or(SavedQueryError::NotFound(id))
    }

    pub async fn update(
        &self,
        id: SavedQueryId,
        update: SavedQueryUpdate,
    ) -> Result<SavedQuery, SavedQueryError> {
        let mut saved_query = self.get(id).await?;
        saved_query.apply_update(update)?;
        self.ensure_name_available(saved_query.project_id, &saved_query.name, Some(id))
            .await?;
        self.repository.save(&saved_query).await?;
        Ok(saved_query)
    }

    pub async fn remove(&self, id: SavedQueryId) -> Result<(), SavedQueryError> {
        self.get(id).await?;
        self.repository.delete(id).await
    }

    /// Rejects a name already held by another query of the project (case-insensitive), so two
    /// entries of the list can never be confused. `exclude` is the query being updated, which
    /// may keep its own name.
    async fn ensure_name_available(
        &self,
        project: ProjectId,
        name: &str,
        exclude: Option<SavedQueryId>,
    ) -> Result<(), SavedQueryError> {
        let taken = self
            .list(project)
            .await?
            .into_iter()
            .any(|query| Some(query.id) != exclude && query.name.eq_ignore_ascii_case(name));
        if taken {
            return Err(SavedQueryError::DuplicateName(name.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;

    use super::*;
    use crate::saved_query::infrastructure::ObjectStoreSavedQueryRepository;

    fn service() -> SavedQueryService {
        SavedQueryService::new(Arc::new(ObjectStoreSavedQueryRepository::new(Arc::new(
            InMemory::new(),
        ))))
    }

    fn new(project: ProjectId, name: &str) -> NewSavedQuery {
        NewSavedQuery {
            project_id: project,
            name: name.to_string(),
            sql: "SELECT 1".to_string(),
        }
    }

    fn update(name: &str, sql: &str) -> SavedQueryUpdate {
        SavedQueryUpdate {
            name: name.to_string(),
            sql: sql.to_string(),
        }
    }

    #[tokio::test]
    async fn lists_only_the_projects_queries_sorted_by_name() {
        let service = service();
        let (alpha, beta) = (ProjectId::new(), ProjectId::new());
        service.register(new(alpha, "b report")).await.unwrap();
        service.register(new(alpha, "A report")).await.unwrap();
        service.register(new(beta, "other")).await.unwrap();

        let names: Vec<_> = service
            .list(alpha)
            .await
            .unwrap()
            .into_iter()
            .map(|query| query.name)
            .collect();
        assert_eq!(names, ["A report", "b report"]);
    }

    #[tokio::test]
    async fn a_name_is_unique_within_a_project_only() {
        let service = service();
        let (alpha, beta) = (ProjectId::new(), ProjectId::new());
        service.register(new(alpha, "Daily")).await.unwrap();

        let err = service.register(new(alpha, " daily ")).await.unwrap_err();
        assert!(matches!(err, SavedQueryError::DuplicateName(_)));
        service.register(new(beta, "Daily")).await.unwrap();
    }

    #[tokio::test]
    async fn an_update_may_keep_its_own_name_but_not_take_another() {
        let service = service();
        let project = ProjectId::new();
        let daily = service.register(new(project, "Daily")).await.unwrap();
        service.register(new(project, "Weekly")).await.unwrap();

        let kept = service
            .update(daily.id, update("daily", "SELECT 2"))
            .await
            .unwrap();
        assert_eq!(
            (kept.name.as_str(), kept.sql.as_str()),
            ("daily", "SELECT 2")
        );
        assert!(kept.updated_at_unix >= daily.updated_at_unix);

        let err = service
            .update(daily.id, update("WEEKLY", "SELECT 3"))
            .await
            .unwrap_err();
        assert!(matches!(err, SavedQueryError::DuplicateName(_)));
        assert_eq!(service.get(daily.id).await.unwrap().sql, "SELECT 2");
    }

    #[tokio::test]
    async fn a_removed_query_is_gone() {
        let service = service();
        let saved = service
            .register(new(ProjectId::new(), "Daily"))
            .await
            .unwrap();
        service.remove(saved.id).await.unwrap();
        assert!(matches!(
            service.get(saved.id).await,
            Err(SavedQueryError::NotFound(_))
        ));
        assert!(matches!(
            service.remove(saved.id).await,
            Err(SavedQueryError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn updating_an_unknown_query_is_not_found() {
        let err = service()
            .update(SavedQueryId::new(), update("a", "SELECT 1"))
            .await
            .unwrap_err();
        assert!(matches!(err, SavedQueryError::NotFound(_)));
    }
}
