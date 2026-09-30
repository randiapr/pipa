//! `/projects` routes: register/list/get/update/delete. Registering, updating and deleting are
//! admin-only; listing and reading are limited to the projects the caller may access.

use std::sync::Arc;

use crate::project::{ProjectError, ProjectId, ProjectService};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use pipa_api::{
    NewProject, ProjectData, ProjectResponse, ProjectUpdate, Projects, ProjectsResponse, path,
};
use uuid::Uuid;

use super::auth::{AdminOnly, AuthError, AuthUser};
use super::error::{BaseResponse, Empty, MessageResponse, ResponseCode, error_response};

type SharedProjectService = Arc<ProjectService>;

pub fn routes() -> Router<SharedProjectService> {
    Router::new()
        .route(path::PROJECTS, get(list_projects).post(register_project))
        .route(
            path::PROJECT,
            get(get_project).put(update_project).delete(delete_project),
        )
}

async fn list_projects(
    auth: AuthUser,
    State(service): State<SharedProjectService>,
) -> Result<Json<ProjectsResponse>, ApiError> {
    let projects = service
        .list()
        .await?
        .into_iter()
        .filter(|project| auth.can_access(project.id))
        .map(Into::into)
        .collect();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        Projects { projects },
    )))
}

async fn register_project(
    _admin: AdminOnly,
    State(service): State<SharedProjectService>,
    Json(new_project): Json<NewProject>,
) -> Result<(StatusCode, Json<ProjectResponse>), ApiError> {
    let project = service.register(new_project.into()).await?.into();
    Ok((
        StatusCode::CREATED,
        Json(BaseResponse::new(
            ResponseCode::Created,
            ProjectData { project },
        )),
    ))
}

async fn get_project(
    auth: AuthUser,
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectResponse>, ApiError> {
    auth.require_project(Some(ProjectId(id)))?;
    let project = service.get(ProjectId(id)).await?.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        ProjectData { project },
    )))
}

async fn update_project(
    _admin: AdminOnly,
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
    Json(update): Json<ProjectUpdate>,
) -> Result<Json<ProjectResponse>, ApiError> {
    let project = service.update(ProjectId(id), update.into()).await?.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        ProjectData { project },
    )))
}

async fn delete_project(
    _admin: AdminOnly,
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    service.remove(ProjectId(id)).await?;
    Ok(Json(BaseResponse::new(ResponseCode::Deleted, Empty {})))
}

enum ApiError {
    Project(ProjectError),
    Auth(AuthError),
}

impl From<ProjectError> for ApiError {
    fn from(err: ProjectError) -> Self {
        Self::Project(err)
    }
}

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        Self::Auth(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let err = match self {
            Self::Project(err) => err,
            Self::Auth(err) => return err.into_response(),
        };
        let (status, code) = match &err {
            ProjectError::NotFound(_) => (StatusCode::NOT_FOUND, ResponseCode::NotFound),
            ProjectError::InvalidField(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            ProjectError::DuplicateName(_) => (StatusCode::CONFLICT, ResponseCode::Conflict),
            ProjectError::Storage(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, err)
    }
}
