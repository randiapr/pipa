//! Domain layer: the `Project` aggregate and the `ProjectRepository` port the application
//! layer depends on. A project groups related data sources.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identity of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

impl ProjectId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A project (aggregate root) grouping related data sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub description: Option<String>,
    pub created_at_unix: u64,
}

/// Fields needed to register a new project, before an identity is assigned.
#[derive(Debug, Clone, Deserialize)]
pub struct NewProject {
    pub name: String,
    pub description: Option<String>,
}

/// Fields that can be changed on an existing project.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectUpdate {
    pub name: String,
    pub description: Option<String>,
}

impl Project {
    /// Validates and constructs a new `Project` aggregate, assigning it a fresh identity.
    pub fn register(new: NewProject) -> Result<Self, ProjectError> {
        if new.name.trim().is_empty() {
            return Err(ProjectError::InvalidField(
                "name must not be empty".to_string(),
            ));
        }

        Ok(Self {
            id: ProjectId::new(),
            name: new.name,
            description: new.description,
            created_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
        })
    }

    /// Validates and applies an update to this project.
    pub fn apply_update(&mut self, update: ProjectUpdate) -> Result<(), ProjectError> {
        if update.name.trim().is_empty() {
            return Err(ProjectError::InvalidField(
                "name must not be empty".to_string(),
            ));
        }
        self.name = update.name;
        self.description = update.description;
        Ok(())
    }
}

/// Errors surfaced by the project domain and its use cases.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("invalid project: {0}")]
    InvalidField(String),
    #[error("project {0} was not found")]
    NotFound(ProjectId),
    #[error("project storage error: {0}")]
    Storage(String),
}

/// Port: persistence for `Project` aggregates, implemented by an infrastructure adapter.
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    async fn save(&self, project: &Project) -> Result<(), ProjectError>;
    async fn find_by_id(&self, id: ProjectId) -> Result<Option<Project>, ProjectError>;
    async fn list(&self) -> Result<Vec<Project>, ProjectError>;
    async fn delete(&self, id: ProjectId) -> Result<(), ProjectError>;
}
