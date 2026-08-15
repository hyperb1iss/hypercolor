//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;

use axum::extract::{Extension, State};
use axum::response::Response;
use tracing::{info, warn};

use hypercolor_core::input::{
    MacosCapabilityOwner, ProtectedSourceActionOwner, ResolvedProtectedSourceAction, SourceKind,
};
#[cfg(target_os = "macos")]
use hypercolor_core::input::{MacosSelectionState, SourcePlatformStatus, SourceStatusHandle};
use hypercolor_types::api::capture::{
    CaptureAuthorizationResponse, CaptureMonitor, CapturePickerResponse, ProtectedSourceGrantOwner,
};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::security::RequestAuthContext;

pub(crate) fn protected_control_rejection(auth_context: RequestAuthContext) -> Option<Response> {
    (!auth_context.can_protected_control())
        .then(|| ApiError::forbidden("Protected capture access requires a control credential"))
}

const fn grant_owner(owner: MacosCapabilityOwner) -> ProtectedSourceGrantOwner {
    match owner {
        MacosCapabilityOwner::AppSidecar => ProtectedSourceGrantOwner::AppSidecar,
        MacosCapabilityOwner::App => ProtectedSourceGrantOwner::App,
        MacosCapabilityOwner::LaunchdService => ProtectedSourceGrantOwner::LaunchdService,
        MacosCapabilityOwner::HomebrewService => ProtectedSourceGrantOwner::HomebrewService,
        MacosCapabilityOwner::Broker => ProtectedSourceGrantOwner::Broker,
        MacosCapabilityOwner::Standalone => ProtectedSourceGrantOwner::Standalone,
    }
}

const fn protected_action_owner(owner: ProtectedSourceActionOwner) -> ProtectedSourceGrantOwner {
    match owner {
        ProtectedSourceActionOwner::Macos(owner) => grant_owner(owner),
        ProtectedSourceActionOwner::PlatformBackend => ProtectedSourceGrantOwner::PlatformBackend,
    }
}

fn requires_app_ui_details(active_owner: MacosCapabilityOwner) -> serde_json::Value {
    serde_json::json!({
        "active_owner": grant_owner(active_owner),
        "remedy": { "kind": "requires_app_ui" },
    })
}

fn requires_app_ui(action: &str, active_owner: MacosCapabilityOwner) -> Response {
    ApiError::validation_with_details(
        format!("{action} must run in Hypercolor.app for the active process topology"),
        requires_app_ui_details(active_owner),
    )
}

#[cfg(target_os = "macos")]
fn macos_selection(status: &SourceStatusHandle) -> Option<(u64, MacosSelectionState)> {
    let status = status.snapshot();
    let SourcePlatformStatus::MacosScreen(platform) = status.platform.as_deref()? else {
        return None;
    };
    Some((platform.selection_revision, platform.selection.clone()))
}

#[cfg(target_os = "macos")]
fn persisted_macos_selection(selection: &MacosSelectionState) -> Option<String> {
    match selection {
        MacosSelectionState::None => None,
        MacosSelectionState::Display { source_id } => Some(source_id.to_string()),
        MacosSelectionState::SessionScoped { .. } => Some("session_scoped".to_owned()),
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum MacosPickerPersistenceDecision {
    Wait,
    Persist(String),
    Cancel,
}

#[cfg(target_os = "macos")]
fn macos_picker_persistence_decision(
    baseline_revision: u64,
    selection_revision: u64,
    selection: &MacosSelectionState,
) -> MacosPickerPersistenceDecision {
    if selection_revision <= baseline_revision {
        return MacosPickerPersistenceDecision::Wait;
    }
    persisted_macos_selection(selection).map_or(
        MacosPickerPersistenceDecision::Cancel,
        MacosPickerPersistenceDecision::Persist,
    )
}

#[cfg(target_os = "macos")]
async fn persist_next_macos_selection(
    status: SourceStatusHandle,
    baseline_revision: u64,
    configured_source: String,
    persistence: crate::startup::services::CaptureConfigPersistenceGate,
) {
    let mut subscription = status.subscribe();
    loop {
        let Some((revision, selection)) = macos_selection(&status) else {
            return;
        };
        match macos_picker_persistence_decision(baseline_revision, revision, &selection) {
            MacosPickerPersistenceDecision::Wait => {}
            MacosPickerPersistenceDecision::Persist(resolved) => {
                persistence.publish_macos_selection(configured_source, resolved);
                return;
            }
            MacosPickerPersistenceDecision::Cancel => return,
        }
        if subscription.changed().await.is_none() {
            return;
        }
    }
}

#[cfg(target_os = "macos")]
fn install_macos_picker_persistence_task(
    current: &mut Option<(u64, tokio::task::JoinHandle<()>)>,
    request_epoch: u64,
    spawn: impl FnOnce() -> tokio::task::JoinHandle<()>,
) {
    if current
        .as_ref()
        .is_some_and(|(current_epoch, _)| *current_epoch >= request_epoch)
    {
        return;
    }
    let task = spawn();
    if let Some((_, previous)) = current.replace((request_epoch, task)) {
        previous.abort();
    }
}

/// `POST /api/v1/input/authorize` — Request macOS Input Monitoring.
#[utoipa::path(
    post,
    path = "/api/v1/input/authorize",
    responses(
        (
            status = 200,
            description = "Input Monitoring authorization result",
            body = crate::api::envelope::ApiResponse<CaptureAuthorizationResponse>
        ),
        (
            status = 403,
            description = "Control credential required",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "capture"
)]
pub(crate) async fn authorize_input_monitoring(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if let Some(response) = protected_control_rejection(auth_context) {
        return response;
    }
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
            ApiResponse::ok(CaptureAuthorizationResponse {
                authorized,
                grant_owner: protected_action_owner(grant_owner),
            })
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
#[utoipa::path(
    post,
    path = "/api/v1/capture/authorize",
    responses(
        (
            status = 200,
            description = "Screen Recording authorization result",
            body = crate::api::envelope::ApiResponse<CaptureAuthorizationResponse>
        ),
        (
            status = 403,
            description = "Control credential required",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "capture"
)]
pub(crate) async fn authorize_screen_recording(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if let Some(response) = protected_control_rejection(auth_context) {
        return response;
    }
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
            ApiResponse::ok(CaptureAuthorizationResponse {
                authorized,
                grant_owner: protected_action_owner(grant_owner),
            })
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
/// The accepted choice is persisted according to the platform source grammar.
#[utoipa::path(
    post,
    path = "/api/v1/capture/source/pick",
    responses(
        (
            status = 200,
            description = "Capture source picker dispatched",
            body = crate::api::envelope::ApiResponse<CapturePickerResponse>
        ),
        (
            status = 403,
            description = "Control credential required",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "capture"
)]
pub(crate) async fn pick_capture_source(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if let Some(response) = protected_control_rejection(auth_context) {
        return response;
    }
    let Some(manager) = state.config_manager.as_ref() else {
        return ApiError::internal("Config manager unavailable in this runtime");
    };

    let expected = manager.get();
    if !expected.capture.enabled {
        return ApiError::validation(
            "Screen capture is disabled; enable capture.enabled before picking a source",
        );
    }

    let (action, screen_status) = {
        let input_manager = state.input_manager.lock().await;
        if !input_manager.has_screen_source() {
            return ApiError::validation(
                "No screen capture source is registered; restart the daemon or re-enable capture",
            );
        }
        let status = input_manager
            .source_status_registry()
            .snapshot()
            .handles()
            .iter()
            .find(|status| status.snapshot().kind == SourceKind::Screen)
            .cloned();
        (input_manager.resolved_screen_source_picker_action(), status)
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
    #[cfg(target_os = "macos")]
    let Some(macos_status) = screen_status else {
        return ApiError::internal("macOS screen source status is unavailable");
    };
    #[cfg(target_os = "macos")]
    let Some((baseline_revision, _)) = macos_selection(&macos_status) else {
        return ApiError::internal("macOS screen source status is unavailable");
    };
    #[cfg(target_os = "macos")]
    let macos_persistence =
        match crate::startup::services::CaptureConfigPersistenceGate::for_macos_picker(
            Arc::clone(manager),
            &expected,
            macos_status.clone(),
        ) {
            Ok(persistence) => persistence,
            Err(error) => {
                return ApiError::conflict(format!(
                    "Capture configuration changed before picker dispatch: {error}"
                ));
            }
        };
    #[cfg(target_os = "macos")]
    let configured_source = expected.capture.source.clone();
    #[cfg(target_os = "macos")]
    let request_epoch = state
        .capture_picker_request_epoch
        .fetch_add(1, Ordering::Relaxed)
        .checked_add(1)
        .expect("macOS picker request epoch exhausted");
    #[cfg(not(target_os = "macos"))]
    let _ = screen_status;
    let picker_result = tokio::task::spawn_blocking(move || action.execute())
        .await
        .map_err(|error| anyhow::anyhow!("source picker task failed: {error}"))
        .and_then(|result| result);
    if let Err(error) = picker_result {
        #[cfg(target_os = "macos")]
        macos_persistence.revoke();
        warn!(%error, "Failed to re-open screen source picker");
        return ApiError::internal(format!("Failed to re-open source picker: {error}"));
    }

    #[cfg(target_os = "macos")]
    {
        let mut current = state
            .capture_picker_persistence_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        install_macos_picker_persistence_task(&mut current, request_epoch, || {
            tokio::spawn(persist_next_macos_selection(
                macos_status,
                baseline_revision,
                configured_source,
                macos_persistence,
            ))
        });
    }

    info!("Screen capture source picker requested");
    ApiResponse::ok(CapturePickerResponse {
        picking: true,
        grant_owner: protected_action_owner(grant_owner),
    })
}

/// `GET /api/v1/capture/monitors` — Display outputs capture can address.
///
/// Empty on platforms where the backend picks its own source (the XDG
/// portal on Linux); the UI uses emptiness to decide between a monitor
/// dropdown and the portal picker button.
#[utoipa::path(
    get,
    path = "/api/v1/capture/monitors",
    responses(
        (
            status = 200,
            description = "Addressable capture displays",
            body = crate::api::envelope::ApiResponse<Vec<CaptureMonitor>>
        ),
        (
            status = 403,
            description = "Control credential required",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "capture"
)]
pub(crate) async fn list_capture_monitors(
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if let Some(response) = protected_control_rejection(auth_context) {
        return response;
    }
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
    #[cfg(target_os = "macos")]
    use std::cell::Cell;
    #[cfg(target_os = "macos")]
    use std::sync::Arc;

    #[cfg(target_os = "macos")]
    use hypercolor_core::input::MacosSelectionState;
    use hypercolor_core::input::{MacosCapabilityOwner, ProtectedSourceActionOwner};
    use hypercolor_types::api::capture::ProtectedSourceGrantOwner;

    #[cfg(target_os = "macos")]
    use super::{
        MacosPickerPersistenceDecision, install_macos_picker_persistence_task,
        macos_picker_persistence_decision,
    };
    use super::{grant_owner, protected_action_owner, requires_app_ui_details};

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
            .map(grant_owner),
            [
                ProtectedSourceGrantOwner::AppSidecar,
                ProtectedSourceGrantOwner::App,
                ProtectedSourceGrantOwner::LaunchdService,
                ProtectedSourceGrantOwner::HomebrewService,
                ProtectedSourceGrantOwner::Broker,
                ProtectedSourceGrantOwner::Standalone,
            ]
        );
        assert_eq!(
            protected_action_owner(ProtectedSourceActionOwner::PlatformBackend),
            ProtectedSourceGrantOwner::PlatformBackend
        );
        assert_eq!(
            requires_app_ui_details(MacosCapabilityOwner::LaunchdService),
            serde_json::json!({
                "active_owner": "launchd_service",
                "remedy": { "kind": "requires_app_ui" },
            })
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn picker_persistence_requires_a_strictly_newer_accepted_selection() {
        let display = MacosSelectionState::Display {
            source_id: Arc::from("display:7a3f4954-3d72-47a6-a914-16ef68d02122"),
        };
        let session = MacosSelectionState::SessionScoped {
            content_style: Arc::from("application"),
        };

        assert_eq!(
            macos_picker_persistence_decision(7, 7, &display),
            MacosPickerPersistenceDecision::Wait
        );
        assert_eq!(
            macos_picker_persistence_decision(7, 8, &display),
            MacosPickerPersistenceDecision::Persist(
                "display:7a3f4954-3d72-47a6-a914-16ef68d02122".to_owned()
            )
        );
        assert_eq!(
            macos_picker_persistence_decision(7, 8, &session),
            MacosPickerPersistenceDecision::Persist("session_scoped".to_owned())
        );
        assert_eq!(
            macos_picker_persistence_decision(7, 8, &MacosSelectionState::None),
            MacosPickerPersistenceDecision::Cancel
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn picker_observer_installation_preserves_newest_request_order() {
        let mut current = None;
        let spawn_count = Cell::new(0);
        install_macos_picker_persistence_task(&mut current, 2, || {
            spawn_count.set(spawn_count.get() + 1);
            tokio::spawn(std::future::pending::<()>())
        });
        install_macos_picker_persistence_task(&mut current, 1, || {
            spawn_count.set(spawn_count.get() + 1);
            tokio::spawn(std::future::pending::<()>())
        });

        assert_eq!(current.as_ref().map(|(epoch, _)| *epoch), Some(2));
        assert_eq!(spawn_count.get(), 1);
        current
            .take()
            .expect("newer observer should remain")
            .1
            .abort();
    }
}
