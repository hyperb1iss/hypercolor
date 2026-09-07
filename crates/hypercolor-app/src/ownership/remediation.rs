use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosOwnerExecutionError, MacosOwnerExecutor as _, MacosOwnerRemedy,
    MacosOwnerStore,
};
use tauri::{AppHandle, Runtime};

use super::executor::AppOwnerExecutor;
use super::model::MacosDaemonOwnerRemedyOutcome;
use crate::supervisor::{MacosDaemonOwnerOfflineStatus, SupervisorState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingOfflineRemedy {
    pub(super) status: MacosDaemonOwnerOfflineStatus,
    pub(super) owner: MacosDaemonOwner,
    pub(super) after_epoch: u64,
}

pub(super) fn execute_offline_remedy_inner<R: Runtime>(
    app: &AppHandle<R>,
    state: SupervisorState,
    remedy: MacosOwnerRemedy,
) -> Result<(PendingOfflineRemedy, url::Url), anyhow::Error> {
    use hypercolor_core::config::paths::data_dir;

    let store = MacosOwnerStore::new(data_dir());
    let daemon_url: url::Url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| crate::DEFAULT_DAEMON_URL.to_owned())
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid HYPERCOLOR_URL: {error}"))?;
    let after_epoch = store
        .load_owner_record()?
        .ok_or_else(|| anyhow::anyhow!("macOS daemon owner record is unavailable"))?
        .owner_epoch;
    let mut executor =
        AppOwnerExecutor::new(app.clone(), state.clone(), daemon_url.clone(), store)?;
    let pending =
        execute_offline_remedy_with(&state, remedy, after_epoch, |owner| executor.start(owner))?;
    Ok((pending, daemon_url))
}

pub(super) fn execute_offline_remedy_with(
    state: &SupervisorState,
    remedy: MacosOwnerRemedy,
    after_epoch: u64,
    start_owner: impl FnOnce(MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError>,
) -> Result<PendingOfflineRemedy, MacosOwnerExecutionError> {
    let status = state.macos_owner_offline().ok_or_else(|| {
        MacosOwnerExecutionError::new("no selected macOS daemon owner is currently offline")
    })?;
    if status.remedy != remedy {
        return Err(MacosOwnerExecutionError::new(
            "offline-owner remedy is stale or does not match the selected topology",
        ));
    }
    let owner = match remedy {
        MacosOwnerRemedy::StartLaunchdService => MacosDaemonOwner::DirectLaunchd,
        MacosOwnerRemedy::StartHomebrewService => MacosDaemonOwner::Homebrew,
        MacosOwnerRemedy::StartAppSidecar
        | MacosOwnerRemedy::RestartStandalone { .. }
        | MacosOwnerRemedy::StopStandaloneOwner { .. } => {
            return Err(MacosOwnerExecutionError::new(
                "offline-owner status only permits an external service start",
            ));
        }
    };
    if owner != status.selected_owner {
        return Err(MacosOwnerExecutionError::new(
            "offline-owner remedy does not match the selected owner",
        ));
    }
    start_owner(owner)?;
    Ok(PendingOfflineRemedy {
        status,
        owner,
        after_epoch,
    })
}

pub(super) fn complete_offline_remedy_with(
    state: &SupervisorState,
    pending: PendingOfflineRemedy,
    authoritative_owner_converged: bool,
) -> Result<MacosDaemonOwnerRemedyOutcome, MacosOwnerExecutionError> {
    if !authoritative_owner_converged {
        return Err(MacosOwnerExecutionError::new(
            "selected macOS daemon owner did not publish a newer healthy epoch within ten seconds",
        ));
    }
    if !state.clear_macos_owner_offline_if(pending.status) {
        return Err(MacosOwnerExecutionError::new(
            "offline-owner status changed while the selected owner was starting",
        ));
    }
    Ok(MacosDaemonOwnerRemedyOutcome::Started {
        owner: pending.owner,
    })
}
