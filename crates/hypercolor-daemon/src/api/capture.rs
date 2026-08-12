//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use tracing::{info, warn};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

/// `POST /api/v1/input/authorize` — Request macOS Input Monitoring.
pub async fn authorize_input_monitoring(State(state): State<Arc<AppState>>) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return ApiError::internal("Config manager unavailable in this runtime");
    };
    let config = manager.get();
    if !config.input.enabled || !config.input.keyboard {
        return ApiError::validation(
            "Keyboard input is disabled; enable input.enabled and input.keyboard before authorizing",
        );
    }
    let action = {
        let input_manager = state.input_manager.lock().await;
        input_manager.input_authorization_action()
    };
    let Some(action) = action else {
        return ApiError::validation("No Input Monitoring authorization action is available");
    };
    match tokio::task::spawn_blocking(move || action()).await {
        Ok(Ok(authorized)) => {
            info!(authorized, "Input Monitoring authorization requested");
            ApiResponse::ok(serde_json::json!({ "authorized": authorized }))
        }
        Ok(Err(error)) => {
            warn!(%error, "Input Monitoring authorization failed");
            ApiError::internal(format!("Failed to authorize Input Monitoring: {error}"))
        }
        Err(error) => ApiError::internal(format!(
            "Input Monitoring authorization task failed: {error}"
        )),
    }
}

/// `POST /api/v1/capture/authorize` — Request macOS Screen Recording.
pub async fn authorize_screen_recording(State(state): State<Arc<AppState>>) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return ApiError::internal("Config manager unavailable in this runtime");
    };
    if !manager.get().capture.enabled {
        return ApiError::validation(
            "Screen capture is disabled; enable capture.enabled before authorizing",
        );
    }
    let action = {
        let input_manager = state.input_manager.lock().await;
        input_manager.screen_authorization_action()
    };
    let Some(action) = action else {
        return ApiError::validation("No Screen Recording authorization action is available");
    };
    match tokio::task::spawn_blocking(move || action()).await {
        Ok(Ok(authorized)) => {
            info!(authorized, "Screen Recording authorization requested");
            ApiResponse::ok(serde_json::json!({ "authorized": authorized }))
        }
        Ok(Err(error)) => {
            warn!(%error, "Screen Recording authorization failed");
            ApiError::internal(format!("Failed to authorize Screen Recording: {error}"))
        }
        Err(error) => ApiError::internal(format!(
            "Screen Recording authorization task failed: {error}"
        )),
    }
}

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

    let picker_result = {
        let mut input_manager = state.input_manager.lock().await;
        if !input_manager.has_screen_source() {
            return ApiError::validation(
                "No screen capture source is registered; restart the daemon or re-enable capture",
            );
        }
        if let Some(action) = input_manager.screen_source_picker_action() {
            drop(input_manager);
            tokio::task::spawn_blocking(move || action())
                .await
                .map_err(|error| anyhow::anyhow!("source picker task failed: {error}"))
                .and_then(|result| result)
        } else {
            input_manager.reselect_screen_source()
        }
    };
    if let Err(error) = picker_result {
        warn!(%error, "Failed to re-open screen source picker");
        return ApiError::internal(format!("Failed to re-open source picker: {error}"));
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
