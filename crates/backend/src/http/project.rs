//! `/projects` routes: register/list/get/update/delete.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use pipa_storage::project::{
    NewProject, Project, ProjectError, ProjectId, ProjectService, ProjectUpdate,
};
use uuid::Uuid;

use super::error::error_response;

type SharedProjectService = Arc<ProjectService>;

pub fn routes() -> Router<SharedProjectService> {
    Router::new()
        .route("/projects", get(list_projects).post(register_project))
        .route(
            "/projects/{id}",
            get(get_project).put(update_project).delete(delete_project),
        )
}

async fn list_projects(
    State(service): State<SharedProjectService>,
) -> Result<Json<Vec<Project>>, ApiError> {
    Ok(Json(service.list().await?))
}

async fn register_project(
    State(service): State<SharedProjectService>,
    Json(new_project): Json<NewProject>,
) -> Result<(StatusCode, Json<Project>), ApiError> {
    let project = service.register(new_project).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

async fn get_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<Json<Project>, ApiError> {
    Ok(Json(service.get(ProjectId(id)).await?))
}

async fn update_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
    Json(update): Json<ProjectUpdate>,
) -> Result<Json<Project>, ApiError> {
    Ok(Json(service.update(ProjectId(id), update).await?))
}

async fn delete_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service.remove(ProjectId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

struct ApiError(ProjectError);

impl From<ProjectError> for ApiError {
    fn from(err: ProjectError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            ProjectError::NotFound(_) => StatusCode::NOT_FOUND,
            ProjectError::InvalidField(_) => StatusCode::BAD_REQUEST,
            ProjectError::DuplicateName(_) => StatusCode::CONFLICT,
            ProjectError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.0)
    }
}
