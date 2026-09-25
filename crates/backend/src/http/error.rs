//! Shared HTTP error response shape for the facade's route modules.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

/// Builds a `{"error": "..."}` JSON response. Each route module's `IntoResponse` impl calls
/// this after mapping its own error enum to a status code, so that mapping is the only
/// per-module logic — the response shape itself stays identical everywhere.
pub(super) fn error_response(status: StatusCode, message: impl ToString) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.to_string(),
        }),
    )
        .into_response()
}
