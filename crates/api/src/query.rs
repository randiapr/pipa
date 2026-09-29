//! `/query` request and response bodies.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// `POST /query` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub sql: String,
}

/// Payload of `POST /query`: the result rows as a JSON array of objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rows {
    pub rows: serde_json::Value,
}

pub type RowsResponse = BaseResponse<Rows>;
