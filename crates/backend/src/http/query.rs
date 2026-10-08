//! `/query` route: ad-hoc SQL over Iceberg tables via `crate::iceberg`'s `QueryService`, limited
//! to the tables of one project. Free SQL is for developers and admins; view-only users
//! browse tables instead (see `table.rs`).

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use pipa_api::{QueryRequest, Rows, RowsResponse, path};
use uuid::Uuid;

use crate::datasource::domain::DataSource;
use crate::datasource::{DataSourceError, DataSourceService};
use crate::iceberg::{QueryError, QueryService, namespace_for_source};
use crate::project::ProjectId;

use super::auth::{AuthError, AuthUser};
use super::error::{BaseResponse, ResponseCode, error_response};

/// What `/query` needs: the query engine, plus the data sources to work out which tables belong
/// to a project.
pub struct QueryApi {
    pub queries: QueryService,
    pub datasources: Arc<DataSourceService>,
}

type SharedQueryApi = Arc<QueryApi>;

pub fn routes() -> Router<SharedQueryApi> {
    Router::new().route(path::QUERY, post(run_query))
}

/// Runs `sql` via DataFusion against the Iceberg tables of `project_id`, returning the result
/// rows under the `rows` field of the shared envelope. `QueryService` hands back its result
/// already JSON-encoded as bytes (from the Arrow result batches), so those are parsed back into
/// a `serde_json::Value` here to nest under `rows`.
///
/// Needs a developer or admin. A developer must name a project they may access. An admin may
/// omit `project_id` to query every table.
async fn run_query(
    auth: AuthUser,
    State(api): State<SharedQueryApi>,
    Json(request): Json<QueryRequest>,
) -> Result<Json<RowsResponse>, ApiError> {
    auth.require_developer()?;
    let project = request
        .project_id
        .as_deref()
        .map(|id| {
            Uuid::parse_str(id)
                .map(ProjectId)
                .map_err(|_| ApiError::InvalidProject)
        })
        .transpose()?;
    if project.is_some() || !auth.is_admin() {
        // For a non-admin this also rejects a missing `project_id`.
        auth.require_project(project)?;
    }

    let namespaces = match project {
        Some(project) => Some(project_namespaces(&api.datasources, project).await?),
        None => None,
    };

    let body = api.queries.query(&request.sql, namespaces).await?;
    let rows: serde_json::Value =
        serde_json::from_slice(&body).expect("QueryService always encodes a valid JSON array");
    Ok(Json(BaseResponse::new(ResponseCode::Ok, Rows { rows })))
}

/// The data sources registered in `project`.
pub(super) async fn project_sources(
    datasources: &DataSourceService,
    project: ProjectId,
) -> Result<Vec<DataSource>, DataSourceError> {
    Ok(datasources
        .list()
        .await?
        .into_iter()
        .filter(|source| source.project_id == Some(project))
        .collect())
}

/// The Iceberg namespaces of every data source in `project`.
pub(super) async fn project_namespaces(
    datasources: &DataSourceService,
    project: ProjectId,
) -> Result<HashSet<String>, DataSourceError> {
    Ok(project_sources(datasources, project)
        .await?
        .into_iter()
        .map(|source| namespace_for_source(source.id))
        .collect())
}

enum ApiError {
    Query(QueryError),
    DataSource(DataSourceError),
    Auth(AuthError),
    InvalidProject,
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
            Self::InvalidProject => error_response(
                StatusCode::BAD_REQUEST,
                ResponseCode::BadRequest,
                "project_id must be a valid UUID",
            ),
            Self::DataSource(err) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
                err,
            ),
            Self::Query(err) => query_error_response(err),
        }
    }
}

/// How a failed query is reported: an unreachable catalog is the upstream's fault, a rejected
/// query the caller's, anything else ours.
pub(super) fn query_error_response(err: QueryError) -> Response {
    let (status, code) = match &err {
        QueryError::Catalog(_) => (StatusCode::BAD_GATEWAY, ResponseCode::UpstreamError),
        QueryError::Execution(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
        QueryError::Encoding(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            ResponseCode::InternalError,
        ),
    };
    error_response(status, code, err)
}
