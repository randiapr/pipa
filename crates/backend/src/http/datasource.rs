//! `/datasources` routes: register/list/get/delete OLTP data sources, test connectivity, explore
//! a source's tables and choose the ones to ingest (`/datasources/{id}/tables`). Every
//! route needs a developer or admin (a data source carries its connection password, so view-only
//! users never see one) and is limited to data sources in projects the caller may access;
//! project-less data sources are admin-only.

use std::sync::Arc;

use crate::datasource::{DataSourceError, DataSourceId, DataSourceService};
use crate::project::ProjectId;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pipa_api::{
    ConnectionTest, ConnectionTestResponse, DataSourceData, DataSourceResponse, DataSources,
    DataSourcesResponse, IngestedTablesUpdate, NewDataSource, SourceTables, SourceTablesResponse,
    path,
};
use serde::Deserialize;
use uuid::Uuid;

use super::auth::{AuthError, AuthUser};
use super::convert::source_table_view;
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
        .route(
            path::DATASOURCE_TABLES,
            get(list_source_tables).put(choose_ingested_tables),
        )
}

/// Query string of `GET /datasources`.
#[derive(Debug, Deserialize)]
struct ListParams {
    /// Only return data sources of this project.
    project_id: Option<Uuid>,
}

async fn list_datasources(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Query(params): Query<ListParams>,
) -> Result<Json<DataSourcesResponse>, ApiError> {
    auth.require_developer()?;
    let wanted = params.project_id.map(ProjectId);
    if let Some(project) = wanted {
        auth.require_project(Some(project))?;
    }
    let datasources = service
        .list()
        .await?
        .into_iter()
        .filter(|source| wanted.is_none_or(|project| source.project_id == Some(project)))
        .filter(|source| auth.require_project(source.project_id).is_ok())
        .map(Into::into)
        .collect();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        DataSources { datasources },
    )))
}

async fn register_datasource(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Json(new_source): Json<NewDataSource>,
) -> Result<(StatusCode, Json<DataSourceResponse>), ApiError> {
    auth.require_developer()?;
    let new_source: crate::datasource::domain::NewDataSource = new_source.try_into()?;
    auth.require_project(new_source.project_id)?;
    let datasource = service.register(new_source).await?.into();
    Ok((
        StatusCode::CREATED,
        Json(BaseResponse::new(
            ResponseCode::Created,
            DataSourceData { datasource },
        )),
    ))
}

async fn get_datasource(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<DataSourceResponse>, ApiError> {
    auth.require_developer()?;
    let datasource = service.get(DataSourceId(id)).await?;
    auth.require_project(datasource.project_id)?;
    let datasource = datasource.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        DataSourceData { datasource },
    )))
}

async fn delete_datasource(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    auth.require_developer()?;
    let datasource = service.get(DataSourceId(id)).await?;
    auth.require_project(datasource.project_id)?;
    service.remove(DataSourceId(id)).await?;
    Ok(Json(BaseResponse::new(ResponseCode::Deleted, Empty {})))
}

async fn test_datasource(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectionTestResponse>, ApiError> {
    auth.require_developer()?;
    let datasource = service.get(DataSourceId(id)).await?;
    auth.require_project(datasource.project_id)?;
    let connection_test = service.test_connection(DataSourceId(id)).await?.into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        ConnectionTest { connection_test },
    )))
}

/// The tables of the source's database, read live from it, each flagged with whether it is
/// ingested. 502 when the database can't be read.
async fn list_source_tables(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceTablesResponse>, ApiError> {
    auth.require_developer()?;
    let datasource = service.get(DataSourceId(id)).await?;
    auth.require_project(datasource.project_id)?;
    let tables = service
        .list_tables(&datasource)
        .await?
        .into_iter()
        .map(|table| source_table_view(&datasource, table))
        .collect();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        SourceTables { tables },
    )))
}

/// Replaces the tables the source ingests; `pipa-ingestion` picks the change up on its next
/// refresh of the registered sources.
async fn choose_ingested_tables(
    auth: AuthUser,
    State(service): State<SharedDataSourceService>,
    Path(id): Path<Uuid>,
    Json(update): Json<IngestedTablesUpdate>,
) -> Result<Json<DataSourceResponse>, ApiError> {
    auth.require_developer()?;
    let datasource = service.get(DataSourceId(id)).await?;
    auth.require_project(datasource.project_id)?;
    let tables = update.tables.into_iter().map(Into::into).collect();
    let datasource = service
        .choose_ingested_tables(DataSourceId(id), tables)
        .await?
        .into();
    Ok(Json(BaseResponse::new(
        ResponseCode::Ok,
        DataSourceData { datasource },
    )))
}

enum ApiError {
    DataSource(DataSourceError),
    Auth(AuthError),
}

impl From<DataSourceError> for ApiError {
    fn from(err: DataSourceError) -> Self {
        Self::DataSource(err)
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
            Self::DataSource(err) => err,
            Self::Auth(err) => return err.into_response(),
        };
        let (status, code) = match &err {
            DataSourceError::NotFound(_) => (StatusCode::NOT_FOUND, ResponseCode::NotFound),
            DataSourceError::InvalidField(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            DataSourceError::SourceUnavailable(_) => {
                (StatusCode::BAD_GATEWAY, ResponseCode::UpstreamError)
            }
            DataSourceError::Storage(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, err)
    }
}
