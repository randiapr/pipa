//! `/tables` request and response bodies: browsing the Iceberg tables of a project's data
//! sources without writing SQL. Open to every role with access to the project, including the
//! view-only `user` role. Rows come back in the same [`Rows`](crate::query::Rows) shape as
//! `POST /query`.

use serde::{Deserialize, Serialize};

use crate::envelope::BaseResponse;

/// One Iceberg table of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableView {
    pub source_id: String,
    pub source_name: String,
    /// The table's name inside the source's namespace, e.g. `public__orders`.
    pub name: String,
}

/// Payload of `GET /tables`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tables {
    pub tables: Vec<TableView>,
}

pub type TablesResponse = BaseResponse<Tables>;

/// `POST /tables/rows` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadTableRequest {
    pub project_id: String,
    pub source_id: String,
    pub table: String,
    /// Rows to return. Defaults to 100 and is capped by the backend.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Rows to skip, for paging.
    #[serde(default)]
    pub offset: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_fields_are_optional() {
        let request: ReadTableRequest =
            serde_json::from_str(r#"{"project_id":"p","source_id":"s","table":"public__orders"}"#)
                .unwrap();
        assert!(request.limit.is_none() && request.offset.is_none());
    }
}
