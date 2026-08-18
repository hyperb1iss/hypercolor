//! The output resource — `GET`/`PATCH /api/v1/output` (Spec 78 §4).
//!
//! Both handlers are adapters over [`crate::domain::output`]: power and
//! brightness live in one service, and this module only converts wire
//! input and wraps the outcome.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::output::{OutputPatchRequest, OutputResource};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::domain;

/// `GET /api/v1/output` — Read global output power and brightness.
#[utoipa::path(
    get,
    path = "/api/v1/output",
    responses(
        (status = 200, description = "Current global output state", body = crate::api::envelope::ApiResponse<OutputResource>)
    ),
    tag = "output"
)]
pub async fn get_output(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(domain::output::get_output(state.as_ref()))
}

/// `PATCH /api/v1/output` — Set power, brightness, or both.
#[utoipa::path(
    patch,
    path = "/api/v1/output",
    request_body = OutputPatchRequest,
    responses(
        (status = 200, description = "Updated global output state", body = crate::api::envelope::ApiResponse<OutputResource>),
        (status = 422, description = "Empty patch or brightness outside 0.0..=1.0")
    ),
    tag = "output"
)]
pub async fn patch_output(
    State(state): State<Arc<AppState>>,
    Json(request): Json<OutputPatchRequest>,
) -> Response {
    match domain::output::patch_output(state.as_ref(), request).await {
        Ok(output) => ApiResponse::ok(output),
        Err(error) => error.into_response(),
    }
}
