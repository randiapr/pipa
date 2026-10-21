// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

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

/// [`Rows`] as the backend sends it: the result is already JSON text, so it is nested under `rows`
/// as it is instead of being parsed into a `serde_json::Value` first (which would also sort each
/// row's columns alphabetically). Serialization only; clients read the same wire shape as
/// [`Rows`].
#[derive(Debug, Serialize)]
pub struct RawRows {
    pub rows: Box<serde_json::value::RawValue>,
}

pub type RawRowsResponse = BaseResponse<RawRows>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::ResponseCode;

    #[test]
    fn raw_rows_have_the_wire_shape_of_rows_and_keep_their_column_order() {
        let text = r#"[{"zeta":1,"alpha":null},{"zeta":2,"alpha":"a"}]"#;
        let raw = BaseResponse::new(
            ResponseCode::Ok,
            RawRows {
                rows: serde_json::value::RawValue::from_string(text.to_string()).unwrap(),
            },
        );
        let wire = serde_json::to_string(&raw).unwrap();
        assert_eq!(
            wire,
            format!(r#"{{"response_code":1000,"response_message":"OK","rows":{text}}}"#)
        );

        let parsed: RowsResponse = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            parsed.body.rows,
            serde_json::from_str::<serde_json::Value>(text).unwrap()
        );
    }
}
