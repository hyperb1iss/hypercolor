//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::domain::DomainError;

/// `POST /api/v1/capture/source/pick` — Re-open the portal source picker.
///
/// Drops the persisted restore token so the desktop portal prompts for a
/// fresh source selection. The new choice is persisted automatically once
/// the user confirms the picker.
pub async fn pick_capture_source(State(state): State<Arc<AppState>>) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return DomainError::Internal(anyhow::anyhow!(
            "Config manager unavailable in this runtime"
        ))
        .into_response();
    };

    if !manager.get().capture.enabled {
        return DomainError::validation(
            "Screen capture is disabled; enable capture.enabled before picking a source",
        )
        .into_response();
    }

    let mut input_manager = state.input_manager.lock().await;
    if !input_manager.has_screen_source() {
        return DomainError::validation(
            "No screen capture source is registered; restart the daemon or re-enable capture",
        )
        .into_response();
    }

    if let Err(error) = input_manager.reselect_screen_source() {
        warn!(%error, "Failed to re-open screen source picker");
        return DomainError::Internal(anyhow::anyhow!("Failed to re-open source picker: {error}"))
            .into_response();
    }

    info!("Screen capture source picker requested");
    ApiResponse::ok(serde_json::json!({ "picking": true }))
}

/// One display output the capture backend can address, for monitor pickers.
#[derive(Debug, serde::Serialize)]
pub struct CaptureMonitor {
    /// Zero-based capture index.
    pub index: usize,
    /// Stable source id persisted in capture configuration.
    pub id: String,
    /// OS device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Desktop width in pixels.
    pub width: u32,
    /// Desktop height in pixels.
    pub height: u32,
    /// Whether this output anchors the virtual desktop origin.
    pub primary: bool,
    /// Ready-to-store `capture.source` value selecting this output.
    pub value: String,
}

/// `GET /api/v1/capture/monitors` — Display outputs capture can address.
///
/// Empty on platforms where the backend picks its own source (the XDG
/// portal on Linux); the UI uses emptiness to decide between a monitor
/// dropdown and the portal picker button.
pub async fn list_capture_monitors() -> Response {
    let monitors: Vec<CaptureMonitor> = hypercolor_core::input::screen::available_monitors()
        .into_iter()
        .map(|monitor| CaptureMonitor {
            value: format!("monitor:{}", monitor.id),
            index: monitor.index,
            id: monitor.id,
            name: monitor.name,
            width: monitor.width,
            height: monitor.height,
            primary: monitor.primary,
        })
        .collect();

    ApiResponse::ok(monitors)
}
