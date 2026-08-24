//! Screen capture endpoints — `/api/v1/capture/*`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

use hypercolor_core::input::{
    CapabilityActionIdentity, PickerPersistenceDecision, PickerSelectionSnapshot,
    ResolvedProtectedSourceAction, SourceKind, SourceStatusHandle, picker_persistence_decision,
    picker_selection_snapshot,
};
use hypercolor_types::api::capture::{
    CaptureAuthorizationResponse, CaptureMonitor, CapturePickerResponse, ProtectedSourceGrantOwner,
};

use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::domain::DomainError;

fn domain_validation(message: impl Into<String>) -> DomainError {
    DomainError::validation(message)
}

fn domain_validation_details(
    message: impl Into<String>,
    details: serde_json::Value,
) -> DomainError {
    DomainError::validation_details(message, details)
}

fn domain_internal(message: impl Into<String>) -> DomainError {
    DomainError::Internal(anyhow::anyhow!(message.into()))
}

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
    match identity.owner() {
        "app_sidecar" => ProtectedSourceGrantOwner::AppSidecar,
        "app" => ProtectedSourceGrantOwner::App,
        "launchd_service" => ProtectedSourceGrantOwner::LaunchdService,
        "homebrew_service" => ProtectedSourceGrantOwner::HomebrewService,
        "broker" => ProtectedSourceGrantOwner::Broker,
        "standalone" => ProtectedSourceGrantOwner::Standalone,
        _ => ProtectedSourceGrantOwner::PlatformBackend,
    }
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
    .into_response()
}

/// The picker selection a screen source currently reports, if its backend
/// resolves sources through a picker at all.
fn picker_selection(status: &SourceStatusHandle) -> Option<PickerSelectionSnapshot> {
    let status = status.snapshot();
    picker_selection_snapshot(status.diagnostics.as_deref()?)
}

async fn persist_next_picker_selection(
    status: SourceStatusHandle,
    baseline_revision: u64,
    configured_source: String,
    persistence: crate::startup::services::CaptureConfigPersistenceGate,
) {
    let mut subscription = status.subscribe();
    loop {
        let Some(snapshot) = picker_selection(&status) else {
            return;
        };
        match picker_persistence_decision(baseline_revision, &snapshot) {
            PickerPersistenceDecision::Wait => {}
            PickerPersistenceDecision::Persist(resolved) => {
                persistence.publish_picker_selection(configured_source, resolved);
                return;
            }
            PickerPersistenceDecision::Cancel => return,
        }
        if subscription.changed().await.is_none() {
            return;
        }
    }
}

fn install_picker_persistence_task(
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
        )
        .into_response();
    }
    let action = {
        let input_manager = state.input_manager();
        input_manager.resolved_input_authorization_action()
    };
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
            envelope::ok(CaptureAuthorizationResponse {
                authorized,
                grant_owner: grant_owner(&identity),
            })
        }
        Ok(Err(error)) => {
            warn!(%error, "Input Monitoring authorization failed");
            domain_internal(format!("Failed to authorize Input Monitoring: {error}"))
                .into_response()
        }
        Err(error) => domain_internal(format!(
            "Input Monitoring authorization task failed: {error}"
        ))
        .into_response(),
    }
}

/// `POST /api/v1/capture/authorize` — Request macOS Screen Recording.
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
        )
        .into_response();
    }
    let action = {
        let input_manager = state.input_manager();
        input_manager.resolved_screen_authorization_action()
    };
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
            envelope::ok(CaptureAuthorizationResponse {
                authorized,
                grant_owner: grant_owner(&identity),
            })
        }
        Ok(Err(error)) => {
            warn!(%error, "Screen Recording authorization failed");
            domain_internal(format!("Failed to authorize Screen Recording: {error}"))
                .into_response()
        }
        Err(error) => domain_internal(format!(
            "Screen Recording authorization task failed: {error}"
        ))
        .into_response(),
    }
}

/// `PUT /api/v1/capture/source` — Re-open the portal source picker.
///
/// The accepted choice is persisted according to the platform source grammar.
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
        let input_manager = state.input_manager();
        if !input_manager.has_screen_source() {
            return domain_validation(
                "No screen capture source is registered; restart the daemon or re-enable capture",
            )
            .into_response();
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
    // Backends that report a revisioned picker selection (ScreenCaptureKit
    // today) get a persistence observer so the accepted choice lands in
    // config. Backends that persist their own restore token report nothing
    // here and need no observer.
    let observer = match screen_status {
        Some(status) => match picker_selection(&status) {
            Some(baseline) => {
                let persistence =
                    match crate::startup::services::CaptureConfigPersistenceGate::for_picker(
                        Arc::clone(manager),
                        &expected,
                        status.clone(),
                    ) {
                        Ok(persistence) => persistence,
                        Err(error) => {
                            return domain_conflict(format!(
                                "Capture configuration changed before picker dispatch: {error}"
                            ));
                        }
                    };
                let request_epoch = state
                    .capture_picker_request_epoch
                    .fetch_add(1, Ordering::Relaxed)
                    .checked_add(1)
                    .expect("picker request epoch exhausted");
                Some((status, baseline.revision, persistence, request_epoch))
            }
            None => None,
        },
        None => None,
    };
    let configured_source = expected.capture.source.clone();
    let picker_result = tokio::task::spawn_blocking(move || action.execute())
        .await
        .map_err(|error| anyhow::anyhow!("source picker task failed: {error}"))
        .and_then(|result| result);
    if let Err(error) = picker_result {
        if let Some((_, _, persistence, _)) = observer {
            persistence.revoke();
        }
        warn!(%error, "Failed to re-open screen source picker");
        return DomainError::Internal(anyhow::anyhow!("Failed to re-open source picker: {error}"))
            .into_response();
    }

    if let Some((status, baseline_revision, persistence, request_epoch)) = observer {
        let mut current = state
            .capture_picker_persistence_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        install_picker_persistence_task(&mut current, request_epoch, || {
            tokio::spawn(persist_next_picker_selection(
                status,
                baseline_revision,
                configured_source,
                persistence,
            ))
        });
    }

    info!("Screen capture source picker requested");
    envelope::ok(CapturePickerResponse {
        picking: true,
        grant_owner: grant_owner(&identity),
    })
}

/// `GET /api/v1/capture/monitors` — Display outputs capture can address.
///
/// Empty on platforms where the backend picks its own source (the XDG
/// portal on Linux); the UI uses emptiness to decide between a monitor
/// dropdown and the portal picker button.
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

    envelope::ok(monitors)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use hypercolor_core::input::{CapabilityActionDisposition, CapabilityActionIdentity};
    use hypercolor_types::api::capture::ProtectedSourceGrantOwner;

    use super::{grant_owner, install_picker_persistence_task, requires_ui_details};

    #[test]
    fn protected_grant_owner_maps_capability_identity_to_api_contract() {
        for (owner, expected) in [
            ("app_sidecar", ProtectedSourceGrantOwner::AppSidecar),
            ("app", ProtectedSourceGrantOwner::App),
            ("launchd_service", ProtectedSourceGrantOwner::LaunchdService),
            (
                "homebrew_service",
                ProtectedSourceGrantOwner::HomebrewService,
            ),
            ("broker", ProtectedSourceGrantOwner::Broker),
            ("standalone", ProtectedSourceGrantOwner::Standalone),
            (
                "platform_backend",
                ProtectedSourceGrantOwner::PlatformBackend,
            ),
            ("future_backend", ProtectedSourceGrantOwner::PlatformBackend),
        ] {
            let identity =
                CapabilityActionIdentity::new(owner, CapabilityActionDisposition::RequiresUi);
            assert_eq!(grant_owner(&identity), expected);
        }

        let identity = CapabilityActionIdentity::new(
            "future_backend",
            CapabilityActionDisposition::RequiresUi,
        );
        assert_eq!(
            requires_ui_details(&identity),
            serde_json::json!({
                "active_owner": "platform_backend",
                "remedy": { "kind": "requires_ui" },
            })
        );
    }

    #[tokio::test]
    async fn picker_observer_installation_preserves_newest_request_order() {
        let mut current = None;
        let spawn_count = Cell::new(0);
        install_picker_persistence_task(&mut current, 2, || {
            spawn_count.set(spawn_count.get() + 1);
            tokio::spawn(std::future::pending::<()>())
        });
        install_picker_persistence_task(&mut current, 1, || {
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
