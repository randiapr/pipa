//! `/query` request and response bodies.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// `POST /query` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub sql: String,
    /// Project whose data the query may read. Required for `developer` accounts; an `admin` may
    /// omit it to query every table. `user` accounts cannot run free SQL at all.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Payload of `POST /query`: the result rows as a JSON array of objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rows {
    pub rows: serde_json::Value,
}

pub type RowsResponse = BaseResponse<Rows>;
