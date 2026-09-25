//! `/query` route: ad-hoc SQL over Iceberg tables via `crate::iceberg`'s `QueryService`.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;

use crate::iceberg::{QueryError, QueryService};

use super::error::error_response;

type SharedQueryService = Arc<QueryService>;

pub fn routes() -> Router<SharedQueryService> {
    Router::new().route("/query", post(run_query))
}

#[derive(Deserialize)]
struct QueryRequest {
    sql: String,
}

/// Runs `sql` via DataFusion against the Iceberg tables the REST catalog exposes, returning the
/// result rows as a raw JSON array — not wrapped in `Json<T>`, since the body is already the
/// JSON bytes `QueryService` encoded from the Arrow result batches.
async fn run_query(
    State(service): State<SharedQueryService>,
    Json(request): Json<QueryRequest>,
) -> Result<Response, ApiError> {
    let body = service.query(&request.sql).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}

struct ApiError(QueryError);

impl From<QueryError> for ApiError {
    fn from(err: QueryError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            QueryError::Catalog(_) => StatusCode::BAD_GATEWAY,
            QueryError::Execution(_) => StatusCode::BAD_REQUEST,
            QueryError::Encoding(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error_response(status, self.0)
    }
}
