//! Shared HTTP error mapping for the facade's route modules. The response envelope itself
//! (`BaseResponse`, `ResponseCode`, ...) is part of the `pipa-api` contract; this module only
//! adapts it to Axum.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use pipa_api::ErrorBody;
pub(super) use pipa_api::{BaseResponse, Empty, MessageResponse, ResponseCode};

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
