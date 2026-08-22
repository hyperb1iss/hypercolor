use crate::coordinator_error::{MacosOwnerCoordinatorError, MacosOwnerExecutionError};
use crate::error::MacosOwnerStoreError;
use crate::executor::MacosOwnerExecutor;
use crate::journal::{MacosHandoverJournal, flush_stop_operation};
use crate::model::{MacosDaemonOwner, MacosHandoverOperation, MacosHandoverPhase};
use crate::store::MacosOwnerStore;

pub(crate) fn inspect_autostart(
    executor: &mut impl MacosOwnerExecutor,
    owner: MacosDaemonOwner,
) -> Result<bool, MacosOwnerCoordinatorError> {
    executor
        .autostart_enabled(owner)
        .map_err(|source| MacosOwnerCoordinatorError::InspectAutostart { owner, source })
}

pub(crate) fn preflight_forward_stop_authority(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    journal: &MacosHandoverJournal,
) -> Result<ForwardStopPreflight, MacosOwnerCoordinatorError> {
    if journal.requested_owner == journal.prior_owner
        || journal.prior_owner == MacosDaemonOwner::Standalone
    {
        return Ok(ForwardStopPreflight::Ready);
    }
    let operation = flush_stop_operation(journal.prior_owner)?;
    let record =
        store
            .load_owner_record()?
            .ok_or_else(|| MacosOwnerCoordinatorError::Operation {
                operation,
                source: MacosOwnerExecutionError::new(
                    "macOS owner record is unavailable during stop-authority preflight",
                ),
            })?;
    if record.active_owner == journal.prior_owner && record.owner_epoch > journal.active_epoch {
        return Ok(ForwardStopPreflight::PriorOwnerReplaced);
    }
    if record.active_owner != journal.prior_owner || record.owner_epoch != journal.active_epoch {
        return Err(MacosOwnerCoordinatorError::Operation {
            operation,
            source: MacosOwnerExecutionError::new(
                "macOS owner incarnation changed before stop-authority preflight",
            ),
        });
    }
    executor
        .preflight_stop_authority(&record.incarnation())
        .map_err(|source| MacosOwnerCoordinatorError::Operation { operation, source })?;
    Ok(ForwardStopPreflight::Ready)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwardStopPreflight {
    Ready,
    PriorOwnerReplaced,
}

pub(crate) fn preflight_exact_stop_authority(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    owner: MacosDaemonOwner,
    owner_epoch: u64,
) -> Result<(), MacosOwnerCoordinatorError> {
    let operation = flush_stop_operation(owner)?;
    let incarnation = store
        .load_owner_record()?
        .filter(|record| record.active_owner == owner && record.owner_epoch == owner_epoch)
        .map(|record| record.incarnation())
        .ok_or_else(|| MacosOwnerCoordinatorError::Operation {
            operation,
            source: MacosOwnerExecutionError::new(
                "macOS owner incarnation changed before stop-authority preflight",
            ),
        })?;
    executor
        .preflight_stop_authority(&incarnation)
        .map_err(|source| MacosOwnerCoordinatorError::Operation { operation, source })
}

pub(crate) fn clear_conflict_if_present(
    store: &MacosOwnerStore,
) -> Result<(), MacosOwnerStoreError> {
    if store.load_owner_record()?.is_some() {
        store.clear_conflict()?;
    }
    Ok(())
}

pub(crate) fn advance(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
    phase: MacosHandoverPhase,
) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
    match store.advance_handover_from(&journal.transaction_id, journal.phase, phase) {
        Ok(advanced) => Ok(advanced),
        Err(MacosOwnerStoreError::HandoverPhaseChanged { .. }) => store
            .load_handover_journal()?
            .filter(|current| current.transaction_id == journal.transaction_id)
            .ok_or(MacosOwnerStoreError::HandoverTransactionMismatch),
        Err(error) => Err(error),
    }
}

pub(crate) fn begin_rollback(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
    advance(store, journal, MacosHandoverPhase::RollbackPending)
}

pub(crate) fn execute_operation(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    journal: &MacosHandoverJournal,
    operation: MacosHandoverOperation,
    forward: bool,
) -> Result<(), MacosOwnerCoordinatorError> {
    let allowed = if forward {
        &journal.allowed_forward_operations
    } else {
        &journal.allowed_rollback_operations
    };
    require_operation(allowed, operation)?;
    match operation {
        MacosHandoverOperation::SetAppSidecarAutostart { enabled } => {
            executor.set_autostart(MacosDaemonOwner::AppSidecar, enabled)
        }
        MacosHandoverOperation::FlushAndStopAppSidecar {} => flush_and_stop_owner(
            store,
            executor,
            journal,
            MacosDaemonOwner::AppSidecar,
            forward,
        ),
        MacosHandoverOperation::StartAppSidecar {} => executor.start(MacosDaemonOwner::AppSidecar),
        MacosHandoverOperation::SetDirectLaunchdAutostart { enabled } => {
            executor.set_autostart(MacosDaemonOwner::DirectLaunchd, enabled)
        }
        MacosHandoverOperation::FlushAndStopDirectLaunchd {} => flush_and_stop_owner(
            store,
            executor,
            journal,
            MacosDaemonOwner::DirectLaunchd,
            forward,
        ),
        MacosHandoverOperation::StartDirectLaunchd {} => {
            executor.start(MacosDaemonOwner::DirectLaunchd)
        }
        MacosHandoverOperation::SetHomebrewAutostart { enabled } => {
            executor.set_autostart(MacosDaemonOwner::Homebrew, enabled)
        }
        MacosHandoverOperation::FlushAndStopHomebrew {} => flush_and_stop_owner(
            store,
            executor,
            journal,
            MacosDaemonOwner::Homebrew,
            forward,
        ),
        MacosHandoverOperation::StartHomebrew {} => executor.start(MacosDaemonOwner::Homebrew),
        MacosHandoverOperation::AwaitStandaloneExit { .. } => Ok(()),
    }
    .map_err(|source| MacosOwnerCoordinatorError::Operation { operation, source })
}

fn flush_and_stop_owner(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    journal: &MacosHandoverJournal,
    owner: MacosDaemonOwner,
    forward: bool,
) -> Result<(), MacosOwnerExecutionError> {
    let incarnation = if forward {
        if owner != journal.prior_owner {
            return Err(MacosOwnerExecutionError::new(
                "forward stop does not target the journal's prior owner",
            ));
        }
        store
            .load_owner_record()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .filter(|record| {
                record.active_owner == owner && record.owner_epoch == journal.active_epoch
            })
            .map(|record| record.incarnation())
            .ok_or_else(|| {
                MacosOwnerExecutionError::new(
                    "handover journal has no matching prior owner incarnation",
                )
            })?
    } else {
        if owner != journal.requested_owner {
            return Err(MacosOwnerExecutionError::new(
                "rollback stop does not target the journal's requested owner",
            ));
        }
        let Some(requested_epoch) = journal
            .contender_epoch
            .filter(|epoch| *epoch > journal.active_epoch)
        else {
            return Ok(());
        };
        store
            .load_owner_record()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .filter(|record| record.active_owner == owner && record.owner_epoch == requested_epoch)
            .map(|record| record.incarnation())
            .ok_or_else(|| {
                MacosOwnerExecutionError::new(
                    "handover journal has no matching requested owner incarnation",
                )
            })?
    };
    store.request_stop_if_current(&incarnation, || executor.flush_and_stop(&incarnation))
}

pub(crate) fn bind_requested_epoch_from_record(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<MacosHandoverJournal, MacosOwnerStoreError> {
    let requested = store
        .load_owner_record()?
        .filter(|record| {
            record.active_owner == journal.requested_owner
                && record.owner_epoch > journal.active_epoch
        })
        .map(|record| (record.active_owner, record.owner_epoch));
    requested.map_or_else(
        || Ok(journal.clone()),
        |(owner, epoch)| store.bind_requested_epoch(&journal.transaction_id, owner, epoch),
    )
}

pub(crate) fn require_operation(
    allowed: &[MacosHandoverOperation],
    operation: MacosHandoverOperation,
) -> Result<(), MacosOwnerCoordinatorError> {
    if allowed.contains(&operation) {
        Ok(())
    } else {
        Err(MacosOwnerCoordinatorError::UnauthorizedOperation { operation })
    }
}
