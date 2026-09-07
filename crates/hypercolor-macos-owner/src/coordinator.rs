use serde::{Deserialize, Serialize};

use crate::coordinator_error::MacosOwnerCoordinatorError;
use crate::effects::{
    ForwardStopPreflight, advance, begin_rollback, bind_requested_epoch_from_record,
    clear_conflict_if_present, execute_operation, inspect_autostart,
    preflight_exact_stop_authority, preflight_forward_stop_authority, require_operation,
};
use crate::error::MacosOwnerStoreError;
use crate::executor::MacosOwnerExecutor;
use crate::journal::{
    MacosHandoverJournal, MacosHandoverTransactionId, autostart_operations_for,
    autostart_operations_from, external_owner_mode, flush_stop_operation, forward_operations,
    rollback_operations, start_operation,
};
use crate::model::{
    MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION, MACOS_MANAGED_HANDOVER_TIMEOUT,
    MACOS_STANDALONE_HANDOVER_TIMEOUT, MacosAutostartStates, MacosDaemonOwner,
    MacosHandoverOperation, MacosHandoverPhase, MacosOwnerRecord,
};
use crate::store::MacosOwnerStore;

/// A topology-specific user action returned by the local coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacosOwnerRemedy {
    /// The standalone owner must be stopped by its terminal user.
    StopStandaloneOwner { pid: u32 },
    /// The standalone capture owner must be restarted by its terminal user.
    RestartStandalone { pid: u32 },
    /// Start the packaged app sidecar locally.
    StartAppSidecar,
    /// Start the direct launchd service locally.
    StartLaunchdService,
    /// Start the Homebrew service locally.
    StartHomebrewService,
}

/// Synchronous result of a local owner selection or recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosOwnerCoordinatorOutcome {
    /// The requested owner published a matching durable epoch.
    Active {
        owner: MacosDaemonOwner,
        owner_epoch: u64,
    },
    /// A standalone process still owns the guard and must exit voluntarily.
    PendingStandalone {
        requested_owner: MacosDaemonOwner,
        remedy: MacosOwnerRemedy,
    },
    /// Forward progress failed and the prior managed owner was restored.
    RolledBack {
        prior_owner: MacosDaemonOwner,
        failure: String,
    },
    /// A validated journal belongs to another owner and remains pending.
    RecoveryRequired {
        requested_owner: MacosDaemonOwner,
        prior_owner: MacosDaemonOwner,
        phase: MacosHandoverPhase,
    },
}

impl MacosHandoverJournal {
    /// Construct a complete path-free handover journal for a local owner choice.
    pub fn for_owner_choice(
        transaction_id: MacosHandoverTransactionId,
        requested_owner: MacosDaemonOwner,
        prior_record: &MacosOwnerRecord,
        prior_autostart_states: MacosAutostartStates,
    ) -> Result<Self, MacosOwnerCoordinatorError> {
        if requested_owner == MacosDaemonOwner::Standalone {
            return Err(MacosOwnerCoordinatorError::StandaloneCannotBeSelected);
        }
        let pending_standalone_pid = (prior_record.active_owner == MacosDaemonOwner::Standalone)
            .then_some(prior_record.active_identity.pid);
        let allowed_forward_operations = forward_operations(
            requested_owner,
            prior_record.active_owner,
            pending_standalone_pid,
        );
        let allowed_rollback_operations = rollback_operations(
            requested_owner,
            prior_record.active_owner,
            prior_autostart_states,
        );
        Ok(Self {
            schema_version: MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION,
            journal_revision: 0,
            transaction_id,
            requested_owner,
            prior_owner: prior_record.active_owner,
            prior_autostart_states,
            allowed_forward_operations,
            allowed_rollback_operations,
            phase: MacosHandoverPhase::Prepared,
            active_epoch: prior_record.owner_epoch,
            contender_epoch: None,
            pending_standalone_pid,
        })
    }
}

/// Run a local, synchronous daemon-owner choice.
pub fn choose_daemon_owner(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    requested_owner: MacosDaemonOwner,
    transaction_id: MacosHandoverTransactionId,
) -> Result<MacosOwnerCoordinatorOutcome, MacosOwnerCoordinatorError> {
    if let Some(existing) = store.load_handover_journal()?
        && !existing.phase.is_terminal()
    {
        let recovered = run_handover(store, executor, existing)?;
        if !matches!(recovered, MacosOwnerCoordinatorOutcome::Active { .. }) {
            return Ok(recovered);
        }
    }

    let prior_record = store
        .load_owner_record()?
        .ok_or(MacosOwnerCoordinatorError::MissingActiveOwner)?;
    if requested_owner != prior_record.active_owner
        && prior_record.active_owner != MacosDaemonOwner::Standalone
    {
        preflight_exact_stop_authority(
            store,
            executor,
            prior_record.active_owner,
            prior_record.owner_epoch,
        )?;
    }
    let prior_autostart_states = MacosAutostartStates::new(
        inspect_autostart(executor, MacosDaemonOwner::AppSidecar)?,
        inspect_autostart(executor, MacosDaemonOwner::DirectLaunchd)?,
        inspect_autostart(executor, MacosDaemonOwner::Homebrew)?,
    );
    let journal = MacosHandoverJournal::for_owner_choice(
        transaction_id,
        requested_owner,
        &prior_record,
        prior_autostart_states,
    )?;
    let journal = store.begin_handover(journal)?;
    run_handover(store, executor, journal)
}

/// Resume the current local transaction before accepting another owner choice.
pub fn recover_daemon_owner(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, MacosOwnerCoordinatorError> {
    let Some(journal) = store.load_handover_journal()? else {
        return Ok(None);
    };
    if journal.phase.is_terminal() {
        return Ok(None);
    }
    run_handover(store, executor, journal).map(Some)
}

/// Reconcile a journal from a daemon that already holds the process guard.
pub fn recover_incoming_daemon_owner(
    store: &MacosOwnerStore,
    current_owner: MacosDaemonOwner,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, MacosOwnerCoordinatorError> {
    let Some(mut journal) = store.load_handover_journal()? else {
        return Ok(None);
    };
    if journal.phase.is_terminal() {
        return Ok(None);
    }
    if current_owner == journal.requested_owner && requested_owner_can_complete(&journal) {
        return complete_requested_owner_recovery(store, journal).map(Some);
    }
    if current_owner == journal.prior_owner
        && matches!(
            journal.phase,
            MacosHandoverPhase::RollbackStartRequested
                | MacosHandoverPhase::PriorOwnerStarted
                | MacosHandoverPhase::RollbackCommitPending
        )
    {
        if journal.phase == MacosHandoverPhase::RollbackStartRequested {
            journal = advance(store, &journal, MacosHandoverPhase::PriorOwnerStarted)?;
            if let Some(outcome) = terminal_outcome(store, &journal)? {
                return Ok(Some(outcome));
            }
        }
        if journal.phase == MacosHandoverPhase::PriorOwnerStarted {
            journal = advance(store, &journal, MacosHandoverPhase::RollbackCommitPending)?;
            if let Some(outcome) = terminal_outcome(store, &journal)? {
                return Ok(Some(outcome));
            }
        }
        store.set_external_owner_mode(external_owner_mode(journal.prior_owner))?;
        clear_conflict_if_present(store)?;
        let journal = advance(store, &journal, MacosHandoverPhase::RolledBack)?;
        return Ok(Some(terminal_outcome(store, &journal)?.unwrap_or(
            MacosOwnerCoordinatorOutcome::RecoveryRequired {
                requested_owner: journal.requested_owner,
                prior_owner: journal.prior_owner,
                phase: journal.phase,
            },
        )));
    }
    Ok(Some(recovery_required(&journal)))
}

const fn requested_owner_can_complete(journal: &MacosHandoverJournal) -> bool {
    match journal.phase {
        MacosHandoverPhase::AutostartsConfigured
        | MacosHandoverPhase::StartRequested
        | MacosHandoverPhase::RequestedOwnerStarted
        | MacosHandoverPhase::CommitPending => true,
        MacosHandoverPhase::StopRequested
        | MacosHandoverPhase::OutgoingOwnerStopped
        | MacosHandoverPhase::AwaitingGuardRelease
        | MacosHandoverPhase::GuardReleased => journal.pending_standalone_pid.is_none(),
        MacosHandoverPhase::Prepared
        | MacosHandoverPhase::Committed
        | MacosHandoverPhase::RollbackPending
        | MacosHandoverPhase::RollbackAutostartsRestored
        | MacosHandoverPhase::RollbackStopRequested
        | MacosHandoverPhase::RollbackOwnerStopped
        | MacosHandoverPhase::RollbackAwaitingGuardRelease
        | MacosHandoverPhase::RollbackGuardReleased
        | MacosHandoverPhase::RollbackStartRequested
        | MacosHandoverPhase::PriorOwnerStarted
        | MacosHandoverPhase::RollbackCommitPending
        | MacosHandoverPhase::RolledBack => false,
    }
}

fn complete_requested_owner_recovery(
    store: &MacosOwnerStore,
    mut journal: MacosHandoverJournal,
) -> Result<MacosOwnerCoordinatorOutcome, MacosOwnerCoordinatorError> {
    loop {
        if !requested_owner_can_complete(&journal) {
            return Ok(recovery_required(&journal));
        }
        journal = match journal.phase {
            MacosHandoverPhase::AutostartsConfigured => advance(
                store,
                &journal,
                if journal.requested_owner == journal.prior_owner {
                    MacosHandoverPhase::CommitPending
                } else if journal.pending_standalone_pid.is_some() {
                    MacosHandoverPhase::StartRequested
                } else {
                    MacosHandoverPhase::StopRequested
                },
            )?,
            MacosHandoverPhase::StopRequested => {
                advance(store, &journal, MacosHandoverPhase::OutgoingOwnerStopped)?
            }
            MacosHandoverPhase::OutgoingOwnerStopped => {
                advance(store, &journal, MacosHandoverPhase::AwaitingGuardRelease)?
            }
            MacosHandoverPhase::AwaitingGuardRelease => {
                advance(store, &journal, MacosHandoverPhase::GuardReleased)?
            }
            MacosHandoverPhase::GuardReleased => {
                advance(store, &journal, MacosHandoverPhase::StartRequested)?
            }
            MacosHandoverPhase::StartRequested => {
                advance(store, &journal, MacosHandoverPhase::RequestedOwnerStarted)?
            }
            MacosHandoverPhase::RequestedOwnerStarted => {
                advance(store, &journal, MacosHandoverPhase::CommitPending)?
            }
            MacosHandoverPhase::CommitPending => {
                store.set_external_owner_mode(external_owner_mode(journal.requested_owner))?;
                clear_conflict_if_present(store)?;
                let committed = advance(store, &journal, MacosHandoverPhase::Committed)?;
                return Ok(terminal_outcome(store, &committed)?
                    .unwrap_or_else(|| recovery_required(&committed)));
            }
            MacosHandoverPhase::Prepared
            | MacosHandoverPhase::Committed
            | MacosHandoverPhase::RollbackPending
            | MacosHandoverPhase::RollbackAutostartsRestored
            | MacosHandoverPhase::RollbackStopRequested
            | MacosHandoverPhase::RollbackOwnerStopped
            | MacosHandoverPhase::RollbackAwaitingGuardRelease
            | MacosHandoverPhase::RollbackGuardReleased
            | MacosHandoverPhase::RollbackStartRequested
            | MacosHandoverPhase::PriorOwnerStarted
            | MacosHandoverPhase::RollbackCommitPending
            | MacosHandoverPhase::RolledBack => return Ok(recovery_required(&journal)),
        };
    }
}

fn recovery_required(journal: &MacosHandoverJournal) -> MacosOwnerCoordinatorOutcome {
    MacosOwnerCoordinatorOutcome::RecoveryRequired {
        requested_owner: journal.requested_owner,
        prior_owner: journal.prior_owner,
        phase: journal.phase,
    }
}

fn rollback_stop_authority_is_unbound(journal: &MacosHandoverJournal) -> bool {
    journal
        .contender_epoch
        .is_none_or(|epoch| epoch <= journal.active_epoch)
}

fn newer_prior_owner_is_published(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<bool, MacosOwnerStoreError> {
    Ok(store.load_owner_record()?.is_some_and(|record| {
        record.active_owner == journal.prior_owner && record.owner_epoch > journal.active_epoch
    }))
}

fn terminal_outcome(
    store: &MacosOwnerStore,
    journal: &MacosHandoverJournal,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, MacosOwnerStoreError> {
    match journal.phase {
        MacosHandoverPhase::Committed => {
            let owner_epoch = store
                .load_owner_record()?
                .filter(|record| record.active_owner == journal.requested_owner)
                .map_or(journal.active_epoch, |record| record.owner_epoch);
            Ok(Some(MacosOwnerCoordinatorOutcome::Active {
                owner: journal.requested_owner,
                owner_epoch,
            }))
        }
        MacosHandoverPhase::RolledBack => Ok(Some(MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: journal.prior_owner,
            failure: "requested owner failed to become active".to_owned(),
        })),
        _ => Ok(None),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the match is the auditable one-to-one encoding of all durable phases"
)]
fn run_handover(
    store: &MacosOwnerStore,
    executor: &mut impl MacosOwnerExecutor,
    mut journal: MacosHandoverJournal,
) -> Result<MacosOwnerCoordinatorOutcome, MacosOwnerCoordinatorError> {
    loop {
        match journal.phase {
            MacosHandoverPhase::Prepared => {
                if let Some(pid) = journal.pending_standalone_pid {
                    require_operation(
                        &journal.allowed_forward_operations,
                        MacosHandoverOperation::AwaitStandaloneExit { pid },
                    )?;
                    journal = advance(store, &journal, MacosHandoverPhase::AwaitingGuardRelease)?;
                } else {
                    if preflight_forward_stop_authority(store, executor, &journal)?
                        == ForwardStopPreflight::PriorOwnerReplaced
                    {
                        journal = begin_rollback(store, &journal)?;
                        continue;
                    }
                    for operation in autostart_operations_for(journal.requested_owner) {
                        if execute_operation(store, executor, &journal, operation, true).is_err() {
                            journal = begin_rollback(store, &journal)?;
                            break;
                        }
                    }
                    if journal.phase == MacosHandoverPhase::Prepared {
                        journal =
                            advance(store, &journal, MacosHandoverPhase::AutostartsConfigured)?;
                    }
                }
            }
            MacosHandoverPhase::AutostartsConfigured => {
                if preflight_forward_stop_authority(store, executor, &journal)?
                    == ForwardStopPreflight::PriorOwnerReplaced
                {
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(
                    store,
                    &journal,
                    if journal.requested_owner == journal.prior_owner {
                        MacosHandoverPhase::CommitPending
                    } else if journal.pending_standalone_pid.is_some() {
                        MacosHandoverPhase::StartRequested
                    } else {
                        MacosHandoverPhase::StopRequested
                    },
                )?;
            }
            MacosHandoverPhase::StopRequested => {
                if preflight_forward_stop_authority(store, executor, &journal)?
                    == ForwardStopPreflight::PriorOwnerReplaced
                {
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                let operation = flush_stop_operation(journal.prior_owner)?;
                if execute_operation(store, executor, &journal, operation, true).is_err() {
                    journal = begin_rollback(store, &journal)?;
                } else {
                    journal = advance(store, &journal, MacosHandoverPhase::OutgoingOwnerStopped)?;
                }
            }
            MacosHandoverPhase::OutgoingOwnerStopped => {
                journal = advance(store, &journal, MacosHandoverPhase::AwaitingGuardRelease)?;
            }
            MacosHandoverPhase::AwaitingGuardRelease => {
                let standalone_pid = journal.pending_standalone_pid;
                let timeout = if journal.pending_standalone_pid.is_some() {
                    MACOS_STANDALONE_HANDOVER_TIMEOUT
                } else {
                    MACOS_MANAGED_HANDOVER_TIMEOUT
                };
                let released = executor.wait_for_guard_release(timeout);
                if released
                    .as_ref()
                    .is_err_and(|_| journal.pending_standalone_pid.is_some())
                {
                    return Err(MacosOwnerCoordinatorError::Operation {
                        operation: MacosHandoverOperation::AwaitStandaloneExit {
                            pid: standalone_pid.expect("checked standalone handover"),
                        },
                        source: released.expect_err("checked error result"),
                    });
                }
                if released.is_err() {
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                if !released.expect("checked successful result") {
                    if journal.pending_standalone_pid.is_some() {
                        return Ok(MacosOwnerCoordinatorOutcome::PendingStandalone {
                            requested_owner: journal.requested_owner,
                            remedy: MacosOwnerRemedy::StopStandaloneOwner {
                                pid: standalone_pid.expect("checked standalone handover"),
                            },
                        });
                    }
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(store, &journal, MacosHandoverPhase::GuardReleased)?;
            }
            MacosHandoverPhase::GuardReleased => {
                if journal.pending_standalone_pid.is_some() {
                    for operation in autostart_operations_for(journal.requested_owner) {
                        if let Err(error) =
                            execute_operation(store, executor, &journal, operation, true)
                        {
                            return Err(error);
                        }
                    }
                    journal = advance(store, &journal, MacosHandoverPhase::AutostartsConfigured)?;
                } else {
                    journal = advance(store, &journal, MacosHandoverPhase::StartRequested)?;
                }
            }
            MacosHandoverPhase::StartRequested => {
                let operation = start_operation(journal.requested_owner)?;
                if let Err(error) = execute_operation(store, executor, &journal, operation, true) {
                    if journal.prior_owner == MacosDaemonOwner::Standalone {
                        return Err(error);
                    }
                    journal = bind_requested_epoch_from_record(store, &journal)?;
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(store, &journal, MacosHandoverPhase::RequestedOwnerStarted)?;
            }
            MacosHandoverPhase::RequestedOwnerStarted => {
                let started = executor.wait_for_owner(
                    journal.requested_owner,
                    journal.active_epoch,
                    MACOS_MANAGED_HANDOVER_TIMEOUT,
                );
                journal = bind_requested_epoch_from_record(store, &journal)?;
                if started.is_err() && journal.prior_owner == MacosDaemonOwner::Standalone {
                    return Err(MacosOwnerCoordinatorError::Operation {
                        operation: start_operation(journal.requested_owner)
                            .expect("validated requested owner is managed"),
                        source: started.expect_err("checked error result"),
                    });
                }
                if !started.unwrap_or(false) {
                    if journal.prior_owner == MacosDaemonOwner::Standalone {
                        return Err(MacosOwnerCoordinatorError::OwnerStartupTimeout);
                    }
                    journal = begin_rollback(store, &journal)?;
                    continue;
                }
                journal = advance(store, &journal, MacosHandoverPhase::CommitPending)?;
            }
            MacosHandoverPhase::CommitPending => {
                store.set_external_owner_mode(external_owner_mode(journal.requested_owner))?;
                clear_conflict_if_present(store)?;
                journal = advance(store, &journal, MacosHandoverPhase::Committed)?;
            }
            MacosHandoverPhase::Committed => {
                let owner_epoch = store
                    .load_owner_record()?
                    .filter(|record| record.active_owner == journal.requested_owner)
                    .map_or(journal.active_epoch, |record| record.owner_epoch);
                return Ok(MacosOwnerCoordinatorOutcome::Active {
                    owner: journal.requested_owner,
                    owner_epoch,
                });
            }
            MacosHandoverPhase::RollbackPending => {
                if journal.requested_owner != journal.prior_owner
                    && !newer_prior_owner_is_published(store, &journal)?
                    && !rollback_stop_authority_is_unbound(&journal)
                {
                    preflight_exact_stop_authority(
                        store,
                        executor,
                        journal.requested_owner,
                        journal
                            .contender_epoch
                            .expect("checked bound contender epoch"),
                    )?;
                }
                for operation in autostart_operations_from(journal.prior_autostart_states) {
                    execute_operation(store, executor, &journal, operation, false)?;
                }
                journal = advance(
                    store,
                    &journal,
                    MacosHandoverPhase::RollbackAutostartsRestored,
                )?;
            }
            MacosHandoverPhase::RollbackAutostartsRestored => {
                let prior_owner_is_active = newer_prior_owner_is_published(store, &journal)?;
                if journal.requested_owner != journal.prior_owner && !prior_owner_is_active {
                    if rollback_stop_authority_is_unbound(&journal) {
                        return Ok(recovery_required(&journal));
                    }
                    preflight_exact_stop_authority(
                        store,
                        executor,
                        journal.requested_owner,
                        journal
                            .contender_epoch
                            .expect("checked bound contender epoch"),
                    )?;
                }
                journal = advance(
                    store,
                    &journal,
                    if journal.requested_owner == journal.prior_owner || prior_owner_is_active {
                        MacosHandoverPhase::RollbackCommitPending
                    } else {
                        MacosHandoverPhase::RollbackStopRequested
                    },
                )?;
            }
            MacosHandoverPhase::RollbackStopRequested => {
                if rollback_stop_authority_is_unbound(&journal) {
                    return Ok(recovery_required(&journal));
                }
                preflight_exact_stop_authority(
                    store,
                    executor,
                    journal.requested_owner,
                    journal
                        .contender_epoch
                        .expect("checked bound contender epoch"),
                )?;
                let operation = flush_stop_operation(journal.requested_owner)?;
                execute_operation(store, executor, &journal, operation, false)?;
                journal = advance(store, &journal, MacosHandoverPhase::RollbackOwnerStopped)?;
            }
            MacosHandoverPhase::RollbackOwnerStopped => {
                if rollback_stop_authority_is_unbound(&journal) {
                    return Ok(recovery_required(&journal));
                }
                journal = advance(
                    store,
                    &journal,
                    MacosHandoverPhase::RollbackAwaitingGuardRelease,
                )?;
            }
            MacosHandoverPhase::RollbackAwaitingGuardRelease => {
                if rollback_stop_authority_is_unbound(&journal) {
                    return Ok(recovery_required(&journal));
                }
                if !executor
                    .wait_for_guard_release(MACOS_MANAGED_HANDOVER_TIMEOUT)
                    .map_err(|source| MacosOwnerCoordinatorError::Operation {
                        operation: flush_stop_operation(journal.requested_owner)
                            .expect("validated requested owner is managed"),
                        source,
                    })?
                {
                    return Err(MacosOwnerCoordinatorError::GuardReleaseTimeout);
                }
                journal = advance(store, &journal, MacosHandoverPhase::RollbackGuardReleased)?;
            }
            MacosHandoverPhase::RollbackGuardReleased => {
                journal = advance(store, &journal, MacosHandoverPhase::RollbackStartRequested)?;
            }
            MacosHandoverPhase::RollbackStartRequested => {
                let operation = start_operation(journal.prior_owner)?;
                execute_operation(store, executor, &journal, operation, false)?;
                journal = advance(store, &journal, MacosHandoverPhase::PriorOwnerStarted)?;
            }
            MacosHandoverPhase::PriorOwnerStarted => {
                if !executor
                    .wait_for_owner(
                        journal.prior_owner,
                        journal.active_epoch,
                        MACOS_MANAGED_HANDOVER_TIMEOUT,
                    )
                    .map_err(|source| MacosOwnerCoordinatorError::Operation {
                        operation: start_operation(journal.prior_owner)
                            .expect("rollback prior owner is managed"),
                        source,
                    })?
                {
                    return Err(MacosOwnerCoordinatorError::OwnerStartupTimeout);
                }
                journal = advance(store, &journal, MacosHandoverPhase::RollbackCommitPending)?;
            }
            MacosHandoverPhase::RollbackCommitPending => {
                store.set_external_owner_mode(external_owner_mode(journal.prior_owner))?;
                clear_conflict_if_present(store)?;
                journal = advance(store, &journal, MacosHandoverPhase::RolledBack)?;
            }
            MacosHandoverPhase::RolledBack => {
                return Ok(MacosOwnerCoordinatorOutcome::RolledBack {
                    prior_owner: journal.prior_owner,
                    failure: "requested owner failed to become active".to_owned(),
                });
            }
        }
    }
}
