//! A response from `pipa-backend`, with assertions on the shared envelope
//! (`response_code`/`response_message`) and on the HTTP status.

use pipa_api::ResponseCode;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    /// The JSON body; a body that isn't JSON (axum's own rejections are plain text) is a string.
    pub body: Value,
}

impl Reply {
    pub async fn from_response(response: reqwest::Response) -> Self {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Self { status, body }
    }

    /// Asserts the HTTP status and the envelope's application code, and returns `self`.
    #[track_caller]
    pub fn expect(self, status: u16, code: ResponseCode) -> Self {
        assert!(
            self.status == status && self.body["response_code"] == code.code(),
            "expected HTTP {status} with response_code {} ({}), got HTTP {} with {}",
            code.code(),
            code.message(),
            self.status,
            self.body
        );
        assert_eq!(
            self.body["response_message"],
            code.message(),
            "response_message"
        );
        self
    }

    #[track_caller]
    pub fn ok(self) -> Self {
        self.expect(200, ResponseCode::Ok)
    }

    #[track_caller]
    pub fn created(self) -> Self {
        self.expect(201, ResponseCode::Created)
    }

    #[track_caller]
    pub fn deleted(self) -> Self {
        self.expect(200, ResponseCode::Deleted)
    }

    #[track_caller]
    pub fn bad_request(self) -> Self {
        self.expect(400, ResponseCode::BadRequest)
    }

    #[track_caller]
    pub fn unauthorized(self) -> Self {
        self.expect(401, ResponseCode::Unauthorized)
    }

    #[track_caller]
    pub fn forbidden(self) -> Self {
        self.expect(403, ResponseCode::Forbidden)
    }

    #[track_caller]
    pub fn not_found(self) -> Self {
        self.expect(404, ResponseCode::NotFound)
    }

    #[track_caller]
    pub fn conflict(self) -> Self {
        self.expect(409, ResponseCode::Conflict)
    }

    #[track_caller]
    pub fn bad_gateway(self) -> Self {
        self.expect(502, ResponseCode::UpstreamError)
    }

    /// The body as `T`, one of the `pipa-api` payload types (which are flattened next to the
    /// envelope fields).
    #[track_caller]
    pub fn parse<T: DeserializeOwned>(&self) -> T {
        serde_json::from_value(self.body.clone()).unwrap_or_else(|err| {
            panic!(
                "body does not parse as {}: {err}; body: {}",
                std::any::type_name::<T>(),
                self.body
            )
        })
    }

    /// The `error` message of a failure response.
    pub fn error(&self) -> &str {
        self.body["error"].as_str().unwrap_or_default()
    }

    /// The `rows` of a query or table-read response.
    #[track_caller]
    pub fn rows(&self) -> &Vec<Value> {
        self.body["rows"]
            .as_array()
            .unwrap_or_else(|| panic!("no rows in {}", self.body))
    }
}
