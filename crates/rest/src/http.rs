//! Interface adapter: translates between Axum HTTP requests/responses and the
//! `pipa-data` data source and project application services. Keeps the domain/application
//! layers in `pipa-data` free of any HTTP concerns.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pipa_data::datasource::{
    ConnectionTestOutcome, DataSource, DataSourceError, DataSourceId, DataSourceService,
    NewDataSource,
};
use pipa_data::project::{
    NewProject, Project, ProjectError, ProjectId, ProjectService, ProjectUpdate,
};
use serde::Serialize;
use uuid::Uuid;

pub type SharedDataSourceService = Arc<DataSourceService>;
pub type SharedProjectService = Arc<ProjectService>;

pub fn datasource_routes() -> Router<SharedDataSourceService> {
    Router::new()
        .route(
            "/datasources",
            get(list_datasources).post(register_datasource),
        )
        .route(
            "/datasources/{id}",
            get(get_datasource).delete(delete_datasource),
        )
        .route("/datasources/{id}/test", post(test_datasource))
}

async fn list_datasources(
    State(service): State<SharedDataSourceService>,
) -> Result<Json<Vec<DataSource>>, ApiError> {
    Ok(Json(service.list().await?))
}

async fn register_datasource(
    State(service): State<SharedDataSourceService>,
    Json(new_source): Json<NewDataSource>,
) -> Result<(StatusCode, Json<DataSource>), ApiError> {
    let source = service.register(new_source).await?;
    Ok((StatusCode::CREATED, Json(source)))
}

async fn get_datasource(
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataSource>, ApiError> {
    Ok(Json(service.get(DataSourceId(id)).await?))
}

async fn delete_datasource(
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service.remove(DataSourceId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_datasource(
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectionTestOutcome>, ApiError> {
    Ok(Json(service.test_connection(DataSourceId(id)).await?))
}

struct ApiError(DataSourceError);

impl From<DataSourceError> for ApiError {
    fn from(err: DataSourceError) -> Self {
        Self(err)
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            DataSourceError::NotFound(_) => StatusCode::NOT_FOUND,
            DataSourceError::InvalidField(_) => StatusCode::BAD_REQUEST,
            DataSourceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

pub fn project_routes() -> Router<SharedProjectService> {
    Router::new()
        .route("/projects", get(list_projects).post(register_project))
        .route(
            "/projects/{id}",
            get(get_project).put(update_project).delete(delete_project),
        )
}

async fn list_projects(
    State(service): State<SharedProjectService>,
) -> Result<Json<Vec<Project>>, ProjectApiError> {
    Ok(Json(service.list().await?))
}

async fn register_project(
    State(service): State<SharedProjectService>,
    Json(new_project): Json<NewProject>,
) -> Result<(StatusCode, Json<Project>), ProjectApiError> {
    let project = service.register(new_project).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

async fn get_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<Json<Project>, ProjectApiError> {
    Ok(Json(service.get(ProjectId(id)).await?))
}

async fn update_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
    Json(update): Json<ProjectUpdate>,
) -> Result<Json<Project>, ProjectApiError> {
    Ok(Json(service.update(ProjectId(id), update).await?))
}

async fn delete_project(
    State(service): State<SharedProjectService>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ProjectApiError> {
    service.remove(ProjectId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

struct ProjectApiError(ProjectError);

impl From<ProjectError> for ProjectApiError {
    fn from(err: ProjectError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ProjectApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            ProjectError::NotFound(_) => StatusCode::NOT_FOUND,
            ProjectError::InvalidField(_) => StatusCode::BAD_REQUEST,
            ProjectError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}
