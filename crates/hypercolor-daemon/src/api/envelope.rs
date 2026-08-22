//! The standard API success envelope.
//!
//! Every successful response flows through [`ApiResponse`] so the JSON
//! shape is consistent across endpoints. Request IDs use UUID v7 for
//! time-ordered traceability. Errors are not built here: they are
//! [`DomainError`](crate::domain::DomainError) values that render
//! themselves, which is the daemon's only error rendering.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub use hypercolor_types::api::envelope::ApiResponse;

// ── Success Envelope ─────────────────────────────────────────────────────

fn respond<T: Serialize>(status: StatusCode, data: T) -> Response {
    let body = ApiResponse {
        data,
        meta: crate::domain::response_meta(),
    };
    (status, axum::Json(body)).into_response()
}

/// Wrap data in a 200 OK envelope.
pub fn ok<T: Serialize>(data: T) -> Response {
    respond(StatusCode::OK, data)
}

/// Wrap data in a 201 Created envelope.
pub fn created<T: Serialize>(data: T) -> Response {
    respond(StatusCode::CREATED, data)
}

/// Wrap data in a 202 Accepted envelope.
pub fn accepted<T: Serialize>(data: T) -> Response {
    respond(StatusCode::ACCEPTED, data)
}
