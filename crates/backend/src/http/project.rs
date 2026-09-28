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
use serde::Serialize;
use uuid::Uuid;

use super::error::{BaseResponse, Empty, MessageResponse, ResponseCode, error_response};

type SharedProjectService = Arc<ProjectService>;

#[derive(Serialize)]
struct Projects {
    projects: Vec<Project>,
}

#[derive(Serialize)]
struct ProjectData {
    project: Project,
}

type ProjectsResponse = BaseResponse<Projects>;
type ProjectResponse = BaseResponse<ProjectData>;

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
) -> Result<Json<ProjectsResponse>, ApiError> {
    let projects = service.list().await?;
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        Projects { projects },
    )))
}

async fn register_project(
    State(service): State<SharedProjectService>,
    Json(new_project): Json<NewProject>,
) -> Result<(StatusCode, Json<ProjectResponse>), ApiError> {
    let project = service.register(new_project).await?;
    Ok((
        StatusCode::CREATED,
        Json(BaseResponse::new(
            ResponseCode::Created,
            ProjectData { project },
        )),
    ))
}

async fn get_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectResponse>, ApiError> {
    let project = service.get(ProjectId(id)).await?;
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        ProjectData { project },
    )))
}

async fn update_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
    Json(update): Json<ProjectUpdate>,
) -> Result<Json<ProjectResponse>, ApiError> {
    let project = service.update(ProjectId(id), update).await?;
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        ProjectData { project },
    )))
}

async fn delete_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    service.remove(ProjectId(id)).await?;
    Ok(Json(BaseResponse::new(ResponseCode::Deleted, Empty {})))
}

struct ApiError(ProjectError);

impl From<ProjectError> for ApiError {
    fn from(err: ProjectError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            ProjectError::NotFound(_) => (StatusCode::NOT_FOUND, ResponseCode::NotFound),
            ProjectError::InvalidField(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            ProjectError::DuplicateName(_) => (StatusCode::CONFLICT, ResponseCode::Conflict),
            ProjectError::Storage(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, self.0)
    }
}
