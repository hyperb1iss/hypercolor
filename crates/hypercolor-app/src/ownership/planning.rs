use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosHandoverPhase, MacosHandoverTransactionId, MacosOwnerCoordinatorOutcome,
    MacosOwnerExecutor as _, MacosOwnerRemedy, MacosOwnerStore,
};
use tauri::{AppHandle, Runtime};

use super::executor::{AppOwnerExecutor, release_app_sidecar_supervisor};
use super::model::{MacosCaptureOwner, MacosCaptureOwnerRestartOutcome};
use crate::supervisor::SupervisorState;

pub(super) fn restart_capture_owner_inner<R: Runtime>(
    app: &AppHandle<R>,
    state: SupervisorState,
    active_owner: MacosCaptureOwner,
    owner_epoch: u64,
) -> Result<MacosCaptureOwnerRestartOutcome, anyhow::Error> {
    use hypercolor_core::config::paths::data_dir;

    let store = MacosOwnerStore::new(data_dir());
    let daemon_url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| crate::DEFAULT_DAEMON_URL.to_owned())
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid HYPERCOLOR_URL: {error}"))?;
    let active_owner = MacosDaemonOwner::from(active_owner);
    let mut executor =
        AppOwnerExecutor::new(app.clone(), state.clone(), daemon_url, store.clone())?;
    restart_capture_owner_with(&store, &mut executor, active_owner, owner_epoch)
}

pub(super) fn restart_capture_owner_with(
    store: &MacosOwnerStore,
    executor: &mut impl hypercolor_macos_owner::MacosOwnerExecutor,
    active_owner: MacosDaemonOwner,
    owner_epoch: u64,
) -> Result<MacosCaptureOwnerRestartOutcome, anyhow::Error> {
    let record = store
        .load_owner_record()?
        .ok_or_else(|| anyhow::anyhow!("macOS daemon owner record is unavailable"))?;
    if store
        .load_handover_journal()?
        .is_some_and(|journal| !journal.phase.is_terminal())
    {
        anyhow::bail!("macOS daemon owner handover requires recovery before capture-owner restart");
    }
    if record.active_owner != active_owner || record.owner_epoch != owner_epoch {
        anyhow::bail!(
            "macOS capture-owner status is stale: requested {active_owner:?} epoch {owner_epoch}, authoritative {:?} epoch {}",
            record.active_owner,
            record.owner_epoch
        );
    }
    if active_owner == MacosDaemonOwner::Standalone {
        return Ok(MacosCaptureOwnerRestartOutcome::UserActionRequired {
            owner: active_owner.into(),
            owner_epoch,
            remedy: MacosOwnerRemedy::RestartStandalone {
                pid: record.active_identity.pid,
            },
        });
    }
    let incarnation = record.incarnation();
    store
        .request_stop_if_current(&incarnation, || executor.flush_and_stop(&incarnation))
        .map_err(anyhow::Error::from)?;
    let restart = (|| {
        if !executor
            .wait_for_guard_release(hypercolor_macos_owner::MACOS_MANAGED_HANDOVER_TIMEOUT)
            .map_err(anyhow::Error::from)?
        {
            anyhow::bail!(
                "macOS capture owner did not release the daemon guard within ten seconds"
            );
        }
        executor.start(active_owner).map_err(anyhow::Error::from)?;
        if !executor
            .wait_for_owner(
                active_owner,
                owner_epoch,
                hypercolor_macos_owner::MACOS_MANAGED_HANDOVER_TIMEOUT,
            )
            .map_err(anyhow::Error::from)?
        {
            anyhow::bail!("restarted macOS capture owner did not publish within ten seconds");
        }
        let restarted = store
            .load_owner_record()?
            .filter(|record| {
                record.active_owner == active_owner && record.owner_epoch > owner_epoch
            })
            .ok_or_else(|| {
                anyhow::anyhow!("restarted macOS capture owner did not publish a new epoch")
            })?;
        Ok(MacosCaptureOwnerRestartOutcome::Restarted {
            owner: active_owner.into(),
            previous_owner_epoch: owner_epoch,
            owner_epoch: restarted.owner_epoch,
        })
    })();
    if restart.is_err() && active_owner == MacosDaemonOwner::AppSidecar {
        executor.start(active_owner).map_err(|rearm_error| {
            anyhow::anyhow!("failed to rearm the app-sidecar supervisor: {rearm_error}")
        })?;
    }
    restart
}

pub(super) fn choose_daemon_owner_inner(
    app: &AppHandle,
    state: SupervisorState,
    requested_owner: MacosDaemonOwner,
) -> Result<MacosOwnerCoordinatorOutcome, anyhow::Error> {
    use hypercolor_core::config::paths::data_dir;

    let store = MacosOwnerStore::new(data_dir());
    let daemon_url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| crate::DEFAULT_DAEMON_URL.to_owned())
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid HYPERCOLOR_URL: {error}"))?;
    let mut executor =
        AppOwnerExecutor::new(app.clone(), state.clone(), daemon_url, store.clone())?;
    let transaction_id = MacosHandoverTransactionId::new(format!(
        "owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))?;
    let outcome = hypercolor_macos_owner::choose_daemon_owner(
        &store,
        &mut executor,
        requested_owner,
        transaction_id,
    )
    .map_err(anyhow::Error::from)?;
    apply_owner_choice_outcome(&state, &outcome);
    Ok(outcome)
}

pub(super) fn apply_owner_choice_outcome(
    state: &SupervisorState,
    outcome: &MacosOwnerCoordinatorOutcome,
) {
    match outcome {
        MacosOwnerCoordinatorOutcome::Active { owner, .. } => {
            state.set_macos_external_owner(match owner {
                MacosDaemonOwner::DirectLaunchd => {
                    Some(hypercolor_macos_owner::MacosExternalOwnerMode::DirectLaunchd)
                }
                MacosDaemonOwner::Homebrew => {
                    Some(hypercolor_macos_owner::MacosExternalOwnerMode::Homebrew)
                }
                MacosDaemonOwner::AppSidecar | MacosDaemonOwner::Standalone => None,
            });
            state.set_macos_owner_offline(None);
        }
        MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: MacosDaemonOwner::AppSidecar,
            ..
        } => release_app_sidecar_supervisor(state),
        MacosOwnerCoordinatorOutcome::PendingStandalone { .. }
        | MacosOwnerCoordinatorOutcome::RolledBack { .. }
        | MacosOwnerCoordinatorOutcome::RecoveryRequired { .. } => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacosStartupRecoveryDisposition {
    Continue,
    SupervisorStarted,
    SuppressSupervisor,
}

pub(crate) fn recover_daemon_owner_before_supervisor<R: Runtime>(
    app: &AppHandle<R>,
    state: SupervisorState,
    daemon_url: url::Url,
    store: MacosOwnerStore,
) -> Result<MacosStartupRecoveryDisposition, anyhow::Error> {
    let mut executor = AppOwnerExecutor::new(app.clone(), state, daemon_url, store.clone())?;
    if store.load_handover_journal()?.is_some_and(|journal| {
        app_sidecar_recovery_needs_rearm(journal.requested_owner, journal.phase)
    }) {
        executor.start(MacosDaemonOwner::AppSidecar)?;
    }
    let outcome = hypercolor_macos_owner::recover_daemon_owner(&store, &mut executor)?;
    Ok(startup_recovery_disposition(
        outcome.as_ref(),
        executor.app_sidecar_supervisor_started,
    ))
}

pub(super) const fn app_sidecar_recovery_needs_rearm(
    requested_owner: MacosDaemonOwner,
    phase: MacosHandoverPhase,
) -> bool {
    matches!(
        (requested_owner, phase),
        (
            MacosDaemonOwner::AppSidecar,
            MacosHandoverPhase::RequestedOwnerStarted
        )
    )
}

pub(super) fn startup_recovery_disposition(
    outcome: Option<&MacosOwnerCoordinatorOutcome>,
    app_sidecar_supervisor_started: bool,
) -> MacosStartupRecoveryDisposition {
    if app_sidecar_supervisor_started {
        MacosStartupRecoveryDisposition::SupervisorStarted
    } else if matches!(
        outcome,
        Some(MacosOwnerCoordinatorOutcome::PendingStandalone { .. })
    ) {
        MacosStartupRecoveryDisposition::SuppressSupervisor
    } else {
        MacosStartupRecoveryDisposition::Continue
    }
}
