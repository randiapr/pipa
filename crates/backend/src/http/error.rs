//! Shared HTTP response envelope for the facade's route modules: every response body is a
//! `BaseResponse<T>` carrying a `response_code`/`response_message` pair from one global
//! registry, flattened alongside either the resource payload `T` (success) or an `error` string
//! (failure). `response_code` is an application-level code independent of the HTTP status
//! actually sent on the wire.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Global registry of application-level response codes, shared across every resource.
#[derive(Clone, Copy)]
pub(super) enum ResponseCode {
    Ok,
    Created,
    Deleted,
    BadRequest,
    NotFound,
    Conflict,
    InternalError,
    UpstreamError,
}

impl ResponseCode {
    fn code(self) -> u32 {
        match self {
            Self::Ok => 1000,
            Self::Created => 1001,
            Self::Deleted => 1002,
            Self::BadRequest => 2000,
            Self::NotFound => 2001,
            Self::Conflict => 2002,
            Self::InternalError => 2003,
            Self::UpstreamError => 2004,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Created => "Created",
            Self::Deleted => "Deleted",
            Self::BadRequest => "Bad Request",
            Self::NotFound => "Not Found",
            Self::Conflict => "Conflict",
            Self::InternalError => "Internal Server Error",
            Self::UpstreamError => "Upstream Error",
        }
    }
}

/// The envelope every response body is shaped as: `response_code`/`response_message` from the
/// registry above, plus whatever `T` contributes (flattened into the same JSON object) — a named
/// resource field for success bodies (see route modules), or `error` for failures.
#[derive(Serialize)]
pub(super) struct BaseResponse<T> {
    response_code: u32,
    response_message: String,
    #[serde(flatten)]
    body: T,
}

impl<T> BaseResponse<T> {
    pub(super) fn new(code: ResponseCode, body: T) -> Self {
        Self {
            response_code: code.code(),
            response_message: code.message().to_string(),
            body,
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

/// Builds a `{"response_code", "response_message", "error"}` JSON response. Each route module's
/// `IntoResponse` impl calls this after mapping its own error enum to a status code and
/// `ResponseCode`, so that mapping is the only per-module logic — the response shape itself
/// stays identical everywhere.
pub(super) fn error_response(
    status: StatusCode,
    code: ResponseCode,
    detail: impl ToString,
) -> Response {
    (
        status,
        Json(BaseResponse::new(
            code,
            ErrorBody {
                error: detail.to_string(),
            },
        )),
    )
        .into_response()
}

/// Empty flattened payload for success responses with nothing to report beyond the code/message
/// pair (e.g. DELETE).
#[derive(Serialize)]
pub(super) struct Empty {}

pub(super) type MessageResponse = BaseResponse<Empty>;
