//! Interface adapter: translates between Axum HTTP requests/responses and the
//! `pipa-data` data source application service. Keeps the domain/application layers in
//! `pipa-data` free of any HTTP concerns.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use pipa_data::datasource::{
    ConnectionTestOutcome, DataSource, DataSourceError, DataSourceId, DataSourceService,
    NewDataSource,
};
use serde::Serialize;
use uuid::Uuid;

pub type SharedDataSourceService = Arc<DataSourceService>;

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
