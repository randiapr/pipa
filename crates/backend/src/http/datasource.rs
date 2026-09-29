//! `/datasources` routes: register/list/get/delete OLTP data sources, test connectivity.

use std::sync::Arc;

use crate::datasource::{DataSourceError, DataSourceId, DataSourceService};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pipa_api::{
    ConnectionTest, ConnectionTestResponse, DataSourceData, DataSourceResponse, DataSources,
    DataSourcesResponse, NewDataSource, path,
};
use uuid::Uuid;

use super::error::{BaseResponse, Empty, MessageResponse, ResponseCode, error_response};

type SharedDataSourceService = Arc<DataSourceService>;

pub fn routes() -> Router<SharedDataSourceService> {
    Router::new()
        .route(
            path::DATASOURCES,
            get(list_datasources).post(register_datasource),
        )
        .route(
            path::DATASOURCE,
            get(get_datasource).delete(delete_datasource),
        )
        .route(path::DATASOURCE_TEST, post(test_datasource))
}

async fn list_datasources(
    State(service): State<SharedDataSourceService>,
) -> Result<Json<DataSourcesResponse>, ApiError> {
    let datasources = service.list().await?.into_iter().map(Into::into).collect();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        DataSources { datasources },
    )))
}

async fn register_datasource(
    State(service): State<SharedDataSourceService>,
    Json(new_source): Json<NewDataSource>,
) -> Result<(StatusCode, Json<DataSourceResponse>), ApiError> {
    let datasource = service.register(new_source.try_into()?).await?.into();
    Ok((
        StatusCode::CREATED,
        Json(BaseResponse::new(
            ResponseCode::Created,
            DataSourceData { datasource },
        )),
    ))
}

async fn get_datasource(
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataSourceResponse>, ApiError> {
    let datasource = service.get(DataSourceId(id)).await?.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        DataSourceData { datasource },
    )))
}

async fn delete_datasource(
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    service.remove(DataSourceId(id)).await?;
    Ok(Json(BaseResponse::new(ResponseCode::Deleted, Empty {})))
}

async fn test_datasource(
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectionTestResponse>, ApiError> {
    let connection_test = service.test_connection(DataSourceId(id)).await?.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        ConnectionTest { connection_test },
    )))
}

struct ApiError(DataSourceError);

impl From<DataSourceError> for ApiError {
    fn from(err: DataSourceError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            DataSourceError::NotFound(_) => (StatusCode::NOT_FOUND, ResponseCode::NotFound),
            DataSourceError::InvalidField(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            DataSourceError::Storage(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, self.0)
    }
}
