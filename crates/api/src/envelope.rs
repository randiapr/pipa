//! The envelope every response body is shaped as: `response_code`/`response_message` from one
//! global registry ([`ResponseCode`]), flattened alongside either the resource payload `T`
//! (success) or an `error` string ([`ErrorBody`], failure). `response_code` is an
//! application-level code independent of the HTTP status actually sent on the wire.

use serde::{Deserialize, Serialize};

/// Global registry of application-level response codes, shared across every resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseCode {
    Ok,
    Created,
    Deleted,
    BadRequest,
    NotFound,
    Conflict,
    InternalError,
    UpstreamError,
    Unauthorized,
    Forbidden,
}

impl ResponseCode {
    pub fn code(self) -> u32 {
        match self {
            Self::Ok => 1000,
            Self::Created => 1001,
            Self::Deleted => 1002,
            Self::BadRequest => 2000,
            Self::NotFound => 2001,
            Self::Conflict => 2002,
            Self::InternalError => 2003,
            Self::UpstreamError => 2004,
            Self::Unauthorized => 2005,
            Self::Forbidden => 2006,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Created => "Created",
            Self::Deleted => "Deleted",
            Self::BadRequest => "Bad Request",
            Self::NotFound => "Not Found",
            Self::Conflict => "Conflict",
            Self::InternalError => "Internal Server Error",
            Self::UpstreamError => "Upstream Error",
            Self::Unauthorized => "Unauthorized",
            Self::Forbidden => "Forbidden",
        }
    }
}

/// `response_code`/`response_message` from [`ResponseCode`], plus whatever `T` contributes
/// (flattened into the same JSON object) — a named resource field for success bodies, or
/// `error` for failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseResponse<T> {
    pub response_code: u32,
    pub response_message: String,
    #[serde(flatten)]
    pub body: T,
}

impl<T> BaseResponse<T> {
    pub fn new(code: ResponseCode, body: T) -> Self {
        Self {
            response_code: code.code(),
            response_message: code.message().to_string(),
            body,
        }
    }
}

/// Payload of every failure response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

/// Empty flattened payload for success responses with nothing to report beyond the code/message
/// pair (e.g. DELETE).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Empty {}

pub type MessageResponse = BaseResponse<Empty>;
pub type ErrorResponse = BaseResponse<ErrorBody>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_body_next_to_code_and_message() {
        let json = serde_json::to_value(BaseResponse::new(
            ResponseCode::NotFound,
            ErrorBody {
                error: "gone".to_string(),
            },
        ))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "response_code": 2001,
                "response_message": "Not Found",
                "error": "gone",
            })
        );
    }

    #[test]
    fn auth_failures_have_their_own_codes() {
        assert_eq!(ResponseCode::Unauthorized.code(), 2005);
        assert_eq!(ResponseCode::Forbidden.code(), 2006);
        assert_eq!(ResponseCode::Forbidden.message(), "Forbidden");
    }

    #[test]
    fn round_trips_through_json() {
        let json =
            serde_json::to_string(&BaseResponse::new(ResponseCode::Deleted, Empty {})).unwrap();
        let parsed: MessageResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.response_code, ResponseCode::Deleted.code());
    }
}
