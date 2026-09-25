//! `/datasources` routes: register/list/get/delete OLTP data sources, test connectivity.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pipa_storage::datasource::{
    ConnectionTestOutcome, DataSource, DataSourceError, DataSourceId, DataSourceService,
    NewDataSource,
};
use uuid::Uuid;

use super::error::error_response;

type SharedDataSourceService = Arc<DataSourceService>;

pub fn routes() -> Router<SharedDataSourceService> {
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            DataSourceError::NotFound(_) => StatusCode::NOT_FOUND,
            DataSourceError::InvalidField(_) => StatusCode::BAD_REQUEST,
            DataSourceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.0)
    }
}
