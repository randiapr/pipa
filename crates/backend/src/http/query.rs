//! `/query` route: ad-hoc SQL over Iceberg tables via `crate::iceberg`'s `QueryService`.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use pipa_api::{QueryRequest, Rows, RowsResponse, path};

use crate::iceberg::{QueryError, QueryService};

use super::error::{BaseResponse, ResponseCode, error_response};

type SharedQueryService = Arc<QueryService>;

pub fn routes() -> Router<SharedQueryService> {
    Router::new().route(path::QUERY, post(run_query))
}

/// Runs `sql` via DataFusion against the Iceberg tables the REST catalog exposes, returning the
/// result rows under the `rows` field of the shared envelope. `QueryService` hands back its
/// result already JSON-encoded as bytes (from the Arrow result batches), so those are parsed
/// back into a `serde_json::Value` here to nest under `rows`.
async fn run_query(
    State(service): State<SharedQueryService>,
    Json(request): Json<QueryRequest>,
) -> Result<Json<RowsResponse>, ApiError> {
    let body = service.query(&request.sql).await?;
    let rows: serde_json::Value =
        serde_json::from_slice(&body).expect("QueryService always encodes a valid JSON array");
    Ok(Json(BaseResponse::new(ResponseCode::Ok, Rows { rows })))
}

struct ApiError(QueryError);

impl From<QueryError> for ApiError {
    fn from(err: QueryError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            QueryError::Catalog(_) => (StatusCode::BAD_GATEWAY, ResponseCode::UpstreamError),
            QueryError::Execution(_) => (StatusCode::BAD_REQUEST, ResponseCode::BadRequest),
            QueryError::Encoding(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ResponseCode::InternalError,
            ),
        };
        error_response(status, code, self.0)
    }
}
