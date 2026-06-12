//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use tracing::{info, warn};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

/// `POST /api/v1/capture/source/pick` — Re-open the portal source picker.
///
/// Drops the persisted restore token so the desktop portal prompts for a
/// fresh source selection. The new choice is persisted automatically once
/// the user confirms the picker.
pub async fn pick_capture_source(State(state): State<Arc<AppState>>) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return ApiError::internal("Config manager unavailable in this runtime");
    };

    if !manager.get().capture.enabled {
        return ApiError::validation(
            "Screen capture is disabled; enable capture.enabled before picking a source",
        );
    }

    let mut input_manager = state.input_manager.lock().await;
    if !input_manager.has_screen_source() {
        return ApiError::validation(
            "No screen capture source is registered; restart the daemon or re-enable capture",
        );
    }

    if let Err(error) = input_manager.reselect_screen_source() {
        warn!(%error, "Failed to re-open screen source picker");
        return ApiError::internal(format!("Failed to re-open source picker: {error}"));
    }

    info!("Screen capture source picker requested");
    ApiResponse::ok(serde_json::json!({ "picking": true }))
}
