use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerCoordinatorOutcome, MacosOwnerRemedy};
use tauri::{AppHandle, State};

use super::model::{
    MacosCaptureOwner, MacosCaptureOwnerRestartOutcome, MacosDaemonOwnerRemedyOutcome,
};
use crate::supervisor::{MacosDaemonOwnerOfflineStatus, SupervisorState};

/// Select one local macOS daemon topology through the durable coordinator.
#[tauri::command]
pub async fn choose_daemon_owner(
    app: AppHandle,
    state: State<'_, SupervisorState>,
    requested_owner: MacosDaemonOwner,
) -> Result<MacosOwnerCoordinatorOutcome, String> {
    #[cfg(target_os = "macos")]
    {
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            super::planning::choose_daemon_owner_inner(&app, state, requested_owner)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, requested_owner);
        Err("macOS daemon owner selection is unavailable on this platform".to_owned())
    }
}

/// Return app-local external-owner state even when the daemon is offline.
#[tauri::command]
pub fn macos_daemon_owner_offline_status(
    state: State<'_, SupervisorState>,
) -> Option<MacosDaemonOwnerOfflineStatus> {
    state.macos_owner_offline()
}

/// Execute the exact start remedy published by the current offline-owner status.
#[tauri::command]
pub async fn execute_macos_daemon_owner_offline_remedy(
    app: AppHandle,
    state: State<'_, SupervisorState>,
    remedy: MacosOwnerRemedy,
) -> Result<MacosDaemonOwnerRemedyOutcome, String> {
    #[cfg(target_os = "macos")]
    {
        let state = state.inner().clone();
        let start_state = state.clone();
        let (pending, daemon_url) = tauri::async_runtime::spawn_blocking(move || {
            super::remediation::execute_offline_remedy_inner(&app, start_state, remedy)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        let converged = crate::supervisor::wait_for_authoritative_macos_owner(
            &reqwest::Client::new(),
            &daemon_url,
            pending.owner,
            Some(pending.after_epoch),
            hypercolor_macos_owner::MACOS_MANAGED_HANDOVER_TIMEOUT,
        )
        .await;
        super::remediation::complete_offline_remedy_with(&state, pending, converged)
            .map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, remedy);
        Err("macOS daemon owner remedies are unavailable on this platform".to_owned())
    }
}

/// Restart the exact authoritative owner named by a protected-source status.
#[tauri::command]
pub async fn restart_macos_capture_owner(
    app: AppHandle,
    state: State<'_, SupervisorState>,
    active_owner: MacosCaptureOwner,
    owner_epoch: u64,
) -> Result<MacosCaptureOwnerRestartOutcome, String> {
    #[cfg(target_os = "macos")]
    {
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            super::planning::restart_capture_owner_inner(&app, state, active_owner, owner_epoch)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, active_owner, owner_epoch);
        Err("macOS capture-owner restart is unavailable on this platform".to_owned())
    }
}
