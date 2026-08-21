//! The output resource — `GET`/`PATCH /api/v1/output` (Spec 78 §4).
//!
//! Both handlers are adapters over [`crate::domain::output`]: power and
//! brightness live in one service, and this module only converts wire
//! input and wraps the outcome.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::output::OutputPatchRequest;

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain;

/// `GET /api/v1/output` — Read global output power and brightness.
pub async fn get_output(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(domain::output::get_output(&state.output))
}

/// `PATCH /api/v1/output` — Set power, brightness, or both.
pub async fn patch_output(
    State(state): State<Arc<AppState>>,
    Json(request): Json<OutputPatchRequest>,
) -> Response {
    match domain::output::patch_output(&state.output, request).await {
        Ok(output) => envelope::ok(output),
        Err(error) => error.into_response(),
    }
}
