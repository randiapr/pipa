//! `/tables` routes: browse the Iceberg tables of a project's data sources, read-only and
//! without SQL. Open to every role with access to the project, which is how the view-only
//! `user` role sees data (it gets 403 on `/query` and `/datasources`).
//!
//! Both routes are scoped to the project's own data sources: a read names a table of one of
//! them, and the query session only sees that source's namespace. The Iceberg metadata tables
//! (`…$snapshots`, `…$manifests`) aren't browsable for any role: they are left out of the list
//! and a read naming one is a bad request (`POST /query` can still select them). Every role reads a table's current rows (one row per key,
//! without the changelog columns), all from Iceberg; the changelog itself is `/query`'s.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pipa_api::{ReadTableRequest, Rows, RowsResponse, TableView, Tables, TablesResponse, path};
use serde::Deserialize;
use uuid::Uuid;

use crate::datasource::{DataSourceError, DataSourceId};
use crate::iceberg::{QueryError, is_metadata_table, namespace_for_source};
use crate::project::ProjectId;

use super::auth::{AuthError, AuthUser};
use super::error::{BaseResponse, ResponseCode, error_response};
use super::query::{QueryApi, project_sources, query_error_response};

type SharedQueryApi = Arc<QueryApi>;

pub fn routes() -> Router<SharedQueryApi> {
    Router::new()
        .route(path::TABLES, get(list_tables))
        .route(path::TABLE_ROWS, post(read_table))
}

/// Query string of `GET /tables`.
#[derive(Debug, Deserialize)]
struct ListParams {
    project_id: Uuid,
}

async fn list_tables(
    auth: AuthUser,
    State(api): State<SharedQueryApi>,
    Query(params): Query<ListParams>,
) -> Result<Json<TablesResponse>, ApiError> {
    let project = ProjectId(params.project_id);
    auth.require_project(Some(project))?;

    let sources = project_sources(&api.datasources, project).await?;
    let namespaces = sources
        .iter()
        .map(|source| namespace_for_source(source.id))
        .collect();
    let found = api.queries.list_tables(namespaces).await?;

    let tables = found
        .into_iter()
        .filter(|(_, name)| !is_metadata_table(name))
        .filter_map(|(namespace, name)| {
            let source = sources
                .iter()
                .find(|source| namespace_for_source(source.id) == namespace)?;
            Some(TableView {
                source_id: source.id.to_string(),
                source_name: source.name.clone(),
                name,
            })
        })
        .collect();
    Ok(Json(BaseResponse::new(ResponseCode::Ok, Tables { tables })))
}

async fn read_table(
    auth: AuthUser,
    State(api): State<SharedQueryApi>,
    Json(request): Json<ReadTableRequest>,
) -> Result<Json<RowsResponse>, ApiError> {
    let project = parse_id(&request.project_id, "project_id").map(ProjectId)?;
    let source = parse_id(&request.source_id, "source_id").map(DataSourceId)?;
    auth.require_project(Some(project))?;
    if is_metadata_table(&request.table) {
        return Err(ApiError::MetadataTable);
    }

    if !project_sources(&api.datasources, project)
        .await?
        .iter()
        .any(|candidate| candidate.id == source)
    {
        return Err(ApiError::UnknownSource);
    }

    let body = api
        .queries
        .read_table(
            &namespace_for_source(source),
            &request.table,
            request.limit,
            request.offset,
        )
        .await?;
    let rows: serde_json::Value =
        serde_json::from_slice(&body).expect("QueryService always encodes a valid JSON array");
    Ok(Json(BaseResponse::new(ResponseCode::Ok, Rows { rows })))
}

fn parse_id(id: &str, field: &'static str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| ApiError::InvalidId(field))
}

enum ApiError {
    Query(QueryError),
    DataSource(DataSourceError),
    Auth(AuthError),
    InvalidId(&'static str),
    /// The data source isn't one of the project's.
    UnknownSource,
    /// The table is one of Iceberg's metadata tables, which aren't browsable.
    MetadataTable,
}

impl From<QueryError> for ApiError {
    fn from(err: QueryError) -> Self {
        Self::Query(err)
    }
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
        match self {
            Self::Auth(err) => err.into_response(),
            Self::Query(err) => query_error_response(err),
            Self::InvalidId(field) => error_response(
                StatusCode::BAD_REQUEST,
                ResponseCode::BadRequest,
                format!("{field} must be a valid UUID"),
            ),
            Self::UnknownSource => error_response(
                StatusCode::BAD_REQUEST,
                ResponseCode::BadRequest,
                "source_id must be a data source of the project",
            ),
            Self::MetadataTable => error_response(
                StatusCode::BAD_REQUEST,
                ResponseCode::BadRequest,
                "Iceberg metadata tables can't be browsed; query them with POST /query",
            ),
            Self::DataSource(err) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
                err,
            ),
        }
    }
}
