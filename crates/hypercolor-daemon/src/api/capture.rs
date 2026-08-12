//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use tracing::{info, warn};

use hypercolor_core::input::{
    MacosCapabilityOwner, ProtectedSourceActionOwner, ResolvedProtectedSourceAction,
};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

const fn grant_owner_name(owner: MacosCapabilityOwner) -> &'static str {
    match owner {
        MacosCapabilityOwner::AppSidecar => "app_sidecar",
        MacosCapabilityOwner::App => "app",
        MacosCapabilityOwner::LaunchdService => "launchd_service",
        MacosCapabilityOwner::HomebrewService => "homebrew_service",
        MacosCapabilityOwner::Broker => "broker",
        MacosCapabilityOwner::Standalone => "standalone",
    }
}

const fn protected_action_owner_name(owner: ProtectedSourceActionOwner) -> &'static str {
    match owner {
        ProtectedSourceActionOwner::Macos(owner) => grant_owner_name(owner),
        ProtectedSourceActionOwner::PlatformBackend => "platform_backend",
    }
}

fn requires_app_ui_details(active_owner: MacosCapabilityOwner) -> serde_json::Value {
    serde_json::json!({
        "active_owner": grant_owner_name(active_owner),
        "remedy": { "kind": "requires_app_ui" },
    })
}

fn requires_app_ui(action: &str, active_owner: MacosCapabilityOwner) -> Response {
    ApiError::validation_with_details(
        format!("{action} must run in Hypercolor.app for the active process topology"),
        requires_app_ui_details(active_owner),
    )
}

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
        input_manager.resolved_input_authorization_action()
    };
    let Some(action) = action else {
        return ApiError::validation("No Input Monitoring authorization action is available");
    };
    let (action, grant_owner) = match action {
        ResolvedProtectedSourceAction::Local { action, owner } => (action, owner),
        ResolvedProtectedSourceAction::RequiresAppUi { active_owner } => {
            return requires_app_ui("Input Monitoring authorization", active_owner);
        }
    };
    match tokio::task::spawn_blocking(move || action.execute()).await {
        Ok(Ok(authorized)) => {
            info!(authorized, "Input Monitoring authorization requested");
            ApiResponse::ok(serde_json::json!({
                "authorized": authorized,
                "grant_owner": protected_action_owner_name(grant_owner),
            }))
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
        input_manager.resolved_screen_authorization_action()
    };
    let Some(action) = action else {
        return ApiError::validation("No Screen Recording authorization action is available");
    };
    let (action, grant_owner) = match action {
        ResolvedProtectedSourceAction::Local { action, owner } => (action, owner),
        ResolvedProtectedSourceAction::RequiresAppUi { active_owner } => {
            return requires_app_ui("Screen Recording authorization", active_owner);
        }
    };
    match tokio::task::spawn_blocking(move || action.execute()).await {
        Ok(Ok(authorized)) => {
            info!(authorized, "Screen Recording authorization requested");
            ApiResponse::ok(serde_json::json!({
                "authorized": authorized,
                "grant_owner": protected_action_owner_name(grant_owner),
            }))
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

    let action = {
        let input_manager = state.input_manager.lock().await;
        if !input_manager.has_screen_source() {
            return ApiError::validation(
                "No screen capture source is registered; restart the daemon or re-enable capture",
            );
        }
        input_manager.resolved_screen_source_picker_action()
    };
    let Some(action) = action else {
        return ApiError::validation("No detached screen source picker action is available");
    };
    let (action, grant_owner) = match action {
        ResolvedProtectedSourceAction::Local { action, owner } => (action, owner),
        ResolvedProtectedSourceAction::RequiresAppUi { active_owner } => {
            return requires_app_ui("Screen source picker", active_owner);
        }
    };
    let picker_result = tokio::task::spawn_blocking(move || action.execute())
        .await
        .map_err(|error| anyhow::anyhow!("source picker task failed: {error}"))
        .and_then(|result| result);
    if let Err(error) = picker_result {
        warn!(%error, "Failed to re-open screen source picker");
        return ApiError::internal(format!("Failed to re-open source picker: {error}"));
    }

    info!("Screen capture source picker requested");
    ApiResponse::ok(serde_json::json!({
        "picking": true,
        "grant_owner": protected_action_owner_name(grant_owner),
    }))
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

#[cfg(test)]
mod tests {
    use hypercolor_core::input::{MacosCapabilityOwner, ProtectedSourceActionOwner};

    use super::{grant_owner_name, protected_action_owner_name, requires_app_ui_details};

    #[test]
    fn protected_grant_owner_names_are_stable_and_process_specific() {
        assert_eq!(
            [
                MacosCapabilityOwner::AppSidecar,
                MacosCapabilityOwner::App,
                MacosCapabilityOwner::LaunchdService,
                MacosCapabilityOwner::HomebrewService,
                MacosCapabilityOwner::Broker,
                MacosCapabilityOwner::Standalone,
            ]
            .map(grant_owner_name),
            [
                "app_sidecar",
                "app",
                "launchd_service",
                "homebrew_service",
                "broker",
                "standalone",
            ]
        );
        assert_eq!(
            protected_action_owner_name(ProtectedSourceActionOwner::PlatformBackend),
            "platform_backend"
        );
        assert_eq!(
            requires_app_ui_details(MacosCapabilityOwner::LaunchdService),
            serde_json::json!({
                "active_owner": "launchd_service",
                "remedy": { "kind": "requires_app_ui" },
            })
        );
    }
}
