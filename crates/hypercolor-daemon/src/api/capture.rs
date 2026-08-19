//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;

use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

#[cfg(target_os = "macos")]
use hypercolor_core::input::SourceStatusHandle;
use hypercolor_core::input::{CapabilityActionIdentity, ResolvedProtectedSourceAction, SourceKind};
#[cfg(target_os = "macos")]
use hypercolor_macos_capture::{MacosCaptureSelection, screen_selection_snapshot};
use hypercolor_types::api::capture::{
    CaptureAuthorizationResponse, CaptureMonitor, CapturePickerResponse, ProtectedSourceGrantOwner,
};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::api::security::RequestAuthContext;
use crate::domain::DomainError;

fn domain_validation(message: impl Into<String>) -> Response {
    DomainError::validation(message).into_response()
}

fn domain_validation_details(message: impl Into<String>, details: serde_json::Value) -> Response {
    DomainError::validation_details(message, details).into_response()
}

fn domain_internal(message: impl Into<String>) -> Response {
    DomainError::Internal(anyhow::anyhow!(message.into())).into_response()
}

#[cfg(target_os = "macos")]
fn domain_conflict(message: impl Into<String>) -> Response {
    DomainError::conflict(message).into_response()
}

pub(crate) fn protected_control_rejection(auth_context: RequestAuthContext) -> Option<Response> {
    (!auth_context.can_protected_control()).then(|| {
        DomainError::forbidden("Protected capture access requires a control credential")
            .into_response()
    })
}

fn grant_owner(identity: &CapabilityActionIdentity) -> ProtectedSourceGrantOwner {
    ProtectedSourceGrantOwner::new(identity.owner())
}

fn requires_ui_details(identity: &CapabilityActionIdentity) -> serde_json::Value {
    serde_json::json!({
        "active_owner": grant_owner(identity),
        "remedy": { "kind": "requires_ui" },
    })
}

fn requires_ui(action: &str, identity: &CapabilityActionIdentity) -> Response {
    domain_validation_details(
        format!("{action} must run in a UI-capable Hypercolor process"),
        requires_ui_details(identity),
    )
}

#[cfg(target_os = "macos")]
fn macos_selection(status: &SourceStatusHandle) -> Option<(u64, MacosCaptureSelection)> {
    let status = status.snapshot();
    let snapshot = screen_selection_snapshot(status.diagnostics.as_deref()?).ok()??;
    Some((snapshot.revision, snapshot.selection))
}

#[cfg(target_os = "macos")]
fn persisted_macos_selection(selection: &MacosCaptureSelection) -> Option<String> {
    match selection {
        MacosCaptureSelection::None => None,
        MacosCaptureSelection::Display { source_id } => Some(source_id.to_string()),
        MacosCaptureSelection::SessionScoped { .. } => Some("session_scoped".to_owned()),
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
    selection: &MacosCaptureSelection,
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
            body = hypercolor_types::api::envelope::ApiErrorBody
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
        return DomainError::Internal(anyhow::anyhow!(
            "Config manager unavailable in this runtime"
        ))
        .into_response();
    };
    let config = manager.get();
    if !config.input.enabled || !config.input.keyboard {
        return domain_validation(
            "Keyboard input is disabled; enable input.enabled and input.keyboard before authorizing",
        );
    }
    let action = state.input_manager.resolved_input_authorization_action();
    let Some(action) = action else {
        return DomainError::validation("No Input Monitoring authorization action is available")
            .into_response();
    };
    let (action, identity) = match action {
        ResolvedProtectedSourceAction::Local { action, identity } => (action, identity),
        ResolvedProtectedSourceAction::RequiresUi { identity } => {
            return requires_ui("Input Monitoring authorization", &identity);
        }
    };
    match tokio::task::spawn_blocking(move || action.execute()).await {
        Ok(Ok(authorized)) => {
            info!(authorized, "Input Monitoring authorization requested");
            ApiResponse::ok(CaptureAuthorizationResponse {
                authorized,
                grant_owner: grant_owner(&identity),
            })
        }
        Ok(Err(error)) => {
            warn!(%error, "Input Monitoring authorization failed");
            domain_internal(format!("Failed to authorize Input Monitoring: {error}"))
        }
        Err(error) => domain_internal(format!(
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
            body = hypercolor_types::api::envelope::ApiErrorBody
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
        return DomainError::Internal(anyhow::anyhow!(
            "Config manager unavailable in this runtime"
        ))
        .into_response();
    };
    if !manager.get().capture.enabled {
        return domain_validation(
            "Screen capture is disabled; enable capture.enabled before authorizing",
        );
    }
    let action = state.input_manager.resolved_screen_authorization_action();
    let Some(action) = action else {
        return DomainError::validation("No Screen Recording authorization action is available")
            .into_response();
    };
    let (action, identity) = match action {
        ResolvedProtectedSourceAction::Local { action, identity } => (action, identity),
        ResolvedProtectedSourceAction::RequiresUi { identity } => {
            return requires_ui("Screen Recording authorization", &identity);
        }
    };
    match tokio::task::spawn_blocking(move || action.execute()).await {
        Ok(Ok(authorized)) => {
            info!(authorized, "Screen Recording authorization requested");
            ApiResponse::ok(CaptureAuthorizationResponse {
                authorized,
                grant_owner: grant_owner(&identity),
            })
        }
        Ok(Err(error)) => {
            warn!(%error, "Screen Recording authorization failed");
            domain_internal(format!("Failed to authorize Screen Recording: {error}"))
        }
        Err(error) => domain_internal(format!(
            "Screen Recording authorization task failed: {error}"
        )),
    }
}

/// `PUT /api/v1/capture/source` — Re-open the portal source picker.
///
/// The accepted choice is persisted according to the platform source grammar.
#[utoipa::path(
    put,
    path = "/api/v1/capture/source",
    responses(
        (
            status = 200,
            description = "Capture source picker dispatched",
            body = crate::api::envelope::ApiResponse<CapturePickerResponse>
        ),
        (
            status = 403,
            description = "Control credential required",
            body = hypercolor_types::api::envelope::ApiErrorBody
        )
    ),
    tag = "capture"
)]
pub(crate) async fn set_capture_source(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if let Some(response) = protected_control_rejection(auth_context) {
        return response;
    }
    let Some(manager) = state.config_manager.as_ref() else {
        return DomainError::Internal(anyhow::anyhow!(
            "Config manager unavailable in this runtime"
        ))
        .into_response();
    };

    let expected = manager.get();
    if !expected.capture.enabled {
        return DomainError::validation(
            "Screen capture is disabled; enable capture.enabled before picking a source",
        )
        .into_response();
    }

    let (action, screen_status) = {
        let input_manager = &state.input_manager;
        if !input_manager.has_screen_source() {
            return domain_validation(
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
        return DomainError::validation("No detached screen source picker action is available")
            .into_response();
    };
    let (action, identity) = match action {
        ResolvedProtectedSourceAction::Local { action, identity } => (action, identity),
        ResolvedProtectedSourceAction::RequiresUi { identity } => {
            return requires_ui("Screen source picker", &identity);
        }
    };
    #[cfg(target_os = "macos")]
    let Some(macos_status) = screen_status else {
        return DomainError::Internal(anyhow::anyhow!("macOS screen source status is unavailable"))
            .into_response();
    };
    #[cfg(target_os = "macos")]
    let Some((baseline_revision, _)) = macos_selection(&macos_status) else {
        return DomainError::Internal(anyhow::anyhow!("macOS screen source status is unavailable"))
            .into_response();
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
                return domain_conflict(format!(
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
        return DomainError::Internal(anyhow::anyhow!("Failed to re-open source picker: {error}"))
            .into_response();
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
        grant_owner: grant_owner(&identity),
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
            body = hypercolor_types::api::envelope::ApiErrorBody
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

    use hypercolor_core::input::{CapabilityActionDisposition, CapabilityActionIdentity};
    #[cfg(target_os = "macos")]
    use hypercolor_macos_capture::{MacosCaptureContentStyle, MacosCaptureSelection};
    use hypercolor_types::api::capture::ProtectedSourceGrantOwner;

    #[cfg(target_os = "macos")]
    use super::{
        MacosPickerPersistenceDecision, install_macos_picker_persistence_task,
        macos_picker_persistence_decision,
    };
    use super::{grant_owner, requires_ui_details};

    #[test]
    fn protected_grant_owner_preserves_backend_identity() {
        let identity = CapabilityActionIdentity::new(
            "future_ui_host",
            CapabilityActionDisposition::RequiresUi,
        );
        assert_eq!(
            grant_owner(&identity),
            ProtectedSourceGrantOwner::new("future_ui_host")
        );
        assert_eq!(
            requires_ui_details(&identity),
            serde_json::json!({
                "active_owner": "future_ui_host",
                "remedy": { "kind": "requires_ui" },
            })
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn picker_persistence_requires_a_strictly_newer_accepted_selection() {
        let display = MacosCaptureSelection::Display {
            source_id: Arc::from("display:7a3f4954-3d72-47a6-a914-16ef68d02122"),
        };
        let session = MacosCaptureSelection::SessionScoped {
            content_style: MacosCaptureContentStyle::Application,
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
            macos_picker_persistence_decision(7, 8, &MacosCaptureSelection::None),
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
