use super::model::{
    InstallAction, InstallDisposition, InstallJournalV1, InstallModelError, InstallOutcome,
    InstallRequest, InstallationState, PlatformCheckpoint, PlatformOwnerReceipt, PlatformState,
    PlatformTransactionRecord, PreparedPlatformTransaction, UnitId, UnitRecord,
};
use super::store::{InstallLock, InstallStore, InstallStoreError};

const MAX_FAILURE_DETAIL_BYTES: usize = 4_096;
/// A candidate that keeps changing state while rollback binds its stop
/// authority gets this many attempts before recovery reports it.
const MAX_ROLLBACK_ENTRY_ATTEMPTS: usize = 3;

pub trait InstallPlatform {
    fn inspect(&mut self) -> Result<PlatformState, InstallPlatformError>;

    fn prepare_transaction(
        &mut self,
        candidate: &UnitRecord,
        prior: &InstallationState,
        target: &PlatformState,
    ) -> Result<PreparedPlatformTransaction, InstallPlatformError>;

    /// Layout checkpoints must prove the exact operation prefix because the
    /// coarse logical layout unit is platform-specific during itemized mutation.
    fn matches_exact_state(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        layout_operation_index: u16,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<bool, InstallPlatformError>;

    fn capture_candidate_owner_receipt(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<PlatformOwnerReceipt, InstallPlatformError>;

    fn validate_transaction_plan(
        &mut self,
        prior: &PlatformState,
        target: &PlatformState,
        transitions: &super::model::PlatformTransitionStates,
        layout_operation_count: u16,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn preflight_authority(
        &mut self,
        candidate: &UnitId,
        prior: &InstallationState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn install_launcher(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn install_layout_operation(
        &mut self,
        checkpoint: PlatformCheckpoint,
        unit: Option<&UnitId>,
        operation_index: u16,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn reload_manager(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn restore_autostart(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn restore_runtime(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError>;

    fn wait_for_newer_owner(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<(), InstallPlatformError>;

    /// Stop a service that runs, or is still changing state, where the next
    /// checkpoint expects it stopped and nothing else differs from it (the
    /// coordinator proves that first through
    /// [`Self::matches_exact_state_except_runtime`]).
    ///
    /// This is how recovery meets a service the platform started on its own
    /// from the on-disk active pointer and service definition, for example
    /// at login after a power loss, or a candidate that restarted after its
    /// owner receipt. Implementations must first prove the running service
    /// is exactly the one those on-disk records name, as either side of this
    /// transaction, and refuse anything else.
    ///
    /// `Ok(false)` declines without any effect, and the coordinator reports
    /// the drift as before. `Ok(true)` means the service is now stopped.
    ///
    /// # Errors
    /// Returns an error when the running service is not provably this
    /// transaction's, or when stopping it fails.
    fn stop_unjournaled_runtime(
        &mut self,
        record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallPlatformError> {
        let _ = record;
        Ok(false)
    }

    /// Whether the last inspection matches `expected` at `checkpoint` in
    /// everything except the service runtime: whether it runs, and under
    /// which invocation.
    ///
    /// The coordinator relies on it to tell a service that merely restarted,
    /// stopped or started on its own from any other drift, and never stops a
    /// service unless stopping it would reach the expected checkpoint. The
    /// default declines, so such drift stays drift.
    ///
    /// # Errors
    /// Returns an error when the platform record cannot be read.
    fn matches_exact_state_except_runtime(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        layout_operation_index: u16,
        record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallPlatformError> {
        let _ = (checkpoint, expected, layout_operation_index, record);
        Ok(false)
    }

    /// Whether the platform still holds the prior exactly as prepared apart
    /// from its runtime: its launcher, public layout and service definition.
    /// The coordinator checks the active pointer itself.
    ///
    /// A transaction whose baseline runtime identity was lost before the
    /// prior was unloaded (a crash and restart, or a power loss) cannot prove
    /// its baseline any more; when this holds it is abandoned without any
    /// effect instead of stopping with drift. With `restarted`, the prior
    /// must also be running, steadily, under an identity other than its
    /// baseline, which proves the transaction's own unload never took hold.
    ///
    /// # Errors
    /// Returns an error when the platform cannot be inspected.
    fn matches_untouched_prior(
        &mut self,
        prior: &PlatformState,
        record: &PlatformTransactionRecord,
        restarted: bool,
    ) -> Result<bool, InstallPlatformError> {
        let _ = (prior, record, restarted);
        Ok(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct InstallPlatformError {
    detail: String,
}

impl InstallPlatformError {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

pub struct InstallCoordinator<'a, P> {
    store: &'a InstallStore,
    platform: &'a mut P,
}

impl<'a, P: InstallPlatform> InstallCoordinator<'a, P> {
    #[must_use]
    pub fn new(store: &'a InstallStore, platform: &'a mut P) -> Self {
        Self { store, platform }
    }

    pub fn install(
        &mut self,
        request: InstallRequest,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
        let mut lock = self.store.acquire_lock()?;
        self.install_with_lock(request, &mut lock)
    }

    /// Install one candidate while using an already-held transaction lock.
    ///
    /// Platform adapters may derive public layout capabilities from `lock`
    /// before entering the coordinator. Every mutation then shares the same
    /// retained lock and operation gate.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::install`], plus
    /// [`InstallStoreError::WrongLock`] when `lock` belongs to another store.
    pub fn install_with_lock(
        &mut self,
        request: InstallRequest,
        lock: &mut InstallLock,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
        if let Some(journal) = self.store.load_journal(lock)?
            && matches!(
                journal.disposition,
                InstallDisposition::Forward | InstallDisposition::Rollback
            )
        {
            return self.resume(journal, lock);
        }

        let journal = self.prepare_with_lock(request, lock)?;
        self.store.write_journal(&journal, lock)?;
        self.drive_forward(journal, lock)
    }

    /// Build an exact initial journal without publishing or driving it.
    ///
    /// Platform preparation retains the original inspection and candidate
    /// bindings. Callers may durably bind that journal to an authority handoff
    /// before writing it and entering the existing recovery path.
    ///
    /// # Errors
    /// Returns an error when another transaction needs recovery, the lock is
    /// foreign, or the original platform and transaction plan cannot be proven.
    pub fn prepare_with_lock(
        &mut self,
        request: InstallRequest,
        lock: &InstallLock,
    ) -> Result<InstallJournalV1, InstallCoordinatorError> {
        if let Some(journal) = self.store.load_journal(lock)?
            && matches!(
                journal.disposition,
                InstallDisposition::Forward | InstallDisposition::Rollback
            )
        {
            return Err(InstallCoordinatorError::PendingPreparation);
        }

        let prior_active_unit = self.store.active_unit(lock)?;
        let prior_platform = self
            .platform
            .inspect()
            .map_err(InstallCoordinatorError::InspectPlatform)?;
        prior_platform.validate()?;
        let prior_state = InstallationState {
            active_unit: prior_active_unit.clone(),
            platform: prior_platform.clone(),
        };
        let target_platform = request
            .target_policy
            .target_platform(&prior_platform, request.candidate.id());
        let prepared_platform = self
            .platform
            .prepare_transaction(&request.candidate, &prior_state, &target_platform)
            .map_err(InstallCoordinatorError::PreparePlatform)?;
        prepared_platform.record.validate()?;
        prepared_platform
            .transitions
            .validate(&prior_platform, &target_platform)?;
        self.platform
            .validate_transaction_plan(
                &prior_platform,
                &target_platform,
                &prepared_platform.transitions,
                prepared_platform.layout_operation_count,
                &prepared_platform.record,
            )
            .map_err(InstallCoordinatorError::PreparePlatform)?;

        let journal = InstallJournalV1::new(
            request.transaction_id,
            prior_active_unit,
            request.candidate.id().clone(),
            prior_platform,
            request.target_policy,
            prepared_platform.transitions,
            prepared_platform.layout_operation_count,
            prepared_platform.record,
        )?;
        Ok(journal)
    }

    pub fn recover(&mut self) -> Result<Option<InstallOutcome>, InstallCoordinatorError> {
        let mut lock = self.store.acquire_lock()?;
        self.recover_with_lock(&mut lock)
    }

    /// Recover a journal while using an already-held transaction lock.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::recover`], plus
    /// [`InstallStoreError::WrongLock`] when `lock` belongs to another store.
    pub fn recover_with_lock(
        &mut self,
        lock: &mut InstallLock,
    ) -> Result<Option<InstallOutcome>, InstallCoordinatorError> {
        let Some(journal) = self.store.load_journal(lock)? else {
            return Ok(None);
        };
        self.resume(journal, lock).map(Some)
    }

    fn resume(
        &mut self,
        journal: InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
        self.platform
            .validate_transaction_plan(
                &journal.prior_platform,
                &journal.target_platform,
                &journal.transition_states,
                journal.layout_operation_count,
                &journal.platform_record,
            )
            .map_err(InstallCoordinatorError::PreparePlatform)?;
        match journal.disposition {
            InstallDisposition::Forward => self.drive_forward(journal, lock),
            InstallDisposition::Rollback => self.drive_rollback(journal, lock),
            InstallDisposition::Committed => Ok(InstallOutcome::Committed {
                active_unit: journal.candidate_unit,
            }),
            InstallDisposition::RolledBack => Ok(InstallOutcome::RolledBack {
                active_unit: journal.prior_active_unit,
                failure: journal.failure.unwrap_or_default(),
                abandoned: journal.abandoned,
            }),
        }
    }

    fn drive_forward(
        &mut self,
        mut journal: InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
        loop {
            let action = journal
                .next_action
                .ok_or(InstallCoordinatorError::MissingNextAction)?;
            if action == InstallAction::Commit {
                journal.advance(InstallDisposition::Committed, None)?;
                self.store.write_journal(&journal, lock)?;
                return Ok(InstallOutcome::Committed {
                    active_unit: journal.candidate_unit,
                });
            }

            if action == InstallAction::InstallCandidateLayout {
                match self.reconcile_layout_operation(&journal, true, lock) {
                    Ok(()) => {
                        journal.layout_operation_index += 1;
                        let next_action =
                            if journal.layout_operation_index == journal.layout_operation_count {
                                next_forward(action)?
                            } else {
                                action
                            };
                        journal.advance(InstallDisposition::Forward, Some(next_action))?;
                        self.store.write_journal(&journal, lock)?;
                    }
                    Err(StepError::Effect(error)) => {
                        let failure = truncate_detail(error.to_string());
                        self.reconcile_forward_layout_progress_after_error(&mut journal, lock)?;
                        journal.failure = Some(failure);
                        let next_action = if journal.layout_operation_index == 0 {
                            InstallAction::ReloadPriorManager
                        } else {
                            InstallAction::RestorePriorLayout
                        };
                        journal.advance(InstallDisposition::Rollback, Some(next_action))?;
                        self.store.write_journal(&journal, lock)?;
                        return self.drive_rollback(journal, lock);
                    }
                    Err(StepError::Fatal(error)) => return Err(error),
                    Err(StepError::Abandon(detail)) => {
                        return Err(InstallCoordinatorError::InvalidAbandonment(detail));
                    }
                }
                continue;
            }

            let mut step = self.reconcile_action(&journal, action, lock);
            if step.is_ok() && action == InstallAction::RestoreCandidateRuntime {
                // A candidate that dies between its start job and this
                // receipt is a candidate failure, not drift.
                step = self.capture_candidate_owner_receipt(&mut journal, lock);
            }
            match step {
                Ok(()) => {
                    journal.advance(InstallDisposition::Forward, Some(next_forward(action)?))?;
                    self.store.write_journal(&journal, lock)?;
                }
                Err(StepError::Effect(error)) => {
                    let failure = truncate_detail(error.to_string());
                    match self.enter_rollback(&mut journal, action, lock)? {
                        RollbackEntry::Action(next_action) => {
                            journal.failure = Some(failure);
                            journal.advance(InstallDisposition::Rollback, Some(next_action))?;
                            self.store.write_journal(&journal, lock)?;
                            return self.drive_rollback(journal, lock);
                        }
                        RollbackEntry::Abandon(detail) => {
                            return self.abandon(journal, detail, lock);
                        }
                    }
                }
                Err(StepError::Abandon(detail)) => return self.abandon(journal, detail, lock),
                Err(StepError::Fatal(error)) => return Err(error),
            }
        }
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        journal: &mut InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<(), StepError> {
        if journal.target_platform.running_unit.is_none()
            || journal.candidate_owner_receipt.is_some()
        {
            return Ok(());
        }
        let actual = self.inspect_state(lock).map_err(StepError::Fatal)?;
        let expected = Checkpoints::new(journal).candidate_runtime;
        if !self
            .matches_checkpoint_at(
                &actual,
                &expected,
                PlatformCheckpoint::CandidateRuntime,
                journal.layout_operation_index,
                &journal.platform_record,
                None,
            )
            .map_err(StepError::Fatal)?
        {
            return Err(StepError::Effect(state_drift(
                InstallAction::RestoreCandidateRuntime,
                expected.clone(),
                expected,
                actual,
            )));
        }
        // The candidate matched just above; a failure to read its receipt
        // now is the platform's, not the candidate's, and recovery retries it.
        let receipt = self
            .platform
            .capture_candidate_owner_receipt(&journal.target_platform, &journal.platform_record)
            .map_err(|source| {
                StepError::Fatal(platform_error(
                    InstallAction::RestoreCandidateRuntime,
                    source,
                ))
            })?;
        receipt
            .validate()
            .map_err(|source| StepError::Fatal(source.into()))?;
        journal.candidate_owner_receipt = Some(receipt);
        Ok(())
    }

    /// Choose the rollback entry for a failed forward action and bind the
    /// candidate's stop authority when the entry needs it.
    ///
    /// Before the prior is unloaded, a failure whose platform no longer shows
    /// the prepared baseline abandons the transaction instead: nothing was
    /// changed, and the baseline a rollback would restore cannot be proven.
    fn enter_rollback(
        &mut self,
        journal: &mut InstallJournalV1,
        failed_action: InstallAction,
        lock: &super::store::InstallLock,
    ) -> Result<RollbackEntry, InstallCoordinatorError> {
        let mut attempts = 0;
        loop {
            let next_action = match self.rollback_entry_action(journal, failed_action, lock) {
                Ok(next_action) => next_action,
                Err(drift @ InstallCoordinatorError::StateDrift { .. })
                    if is_baseline_step(failed_action) =>
                {
                    let actual = self.inspect_state(lock)?;
                    return match self.abandonment(journal, failed_action, &actual)? {
                        Some(detail) => Ok(RollbackEntry::Abandon(detail)),
                        None => Err(drift),
                    };
                }
                Err(error) => return Err(error),
            };
            if next_action != InstallAction::UnloadCandidateRuntime {
                return Ok(RollbackEntry::Action(next_action));
            }
            match self.capture_candidate_owner_receipt(journal, lock) {
                Ok(()) => return Ok(RollbackEntry::Action(next_action)),
                // The candidate stopped or restarted after it matched; choose
                // again, which stops it whatever its invocation.
                Err(StepError::Effect(error)) => {
                    attempts += 1;
                    if attempts == MAX_ROLLBACK_ENTRY_ATTEMPTS {
                        return Err(error);
                    }
                }
                Err(StepError::Fatal(error)) => return Err(error),
                Err(StepError::Abandon(detail)) => {
                    return Err(InstallCoordinatorError::InvalidAbandonment(detail));
                }
            }
        }
    }

    /// End a transaction that never unloaded the prior, with no effect.
    fn abandon(
        &mut self,
        mut journal: InstallJournalV1,
        detail: String,
        lock: &super::store::InstallLock,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
        journal.failure = Some(truncate_detail(detail));
        journal.abandoned = true;
        journal.advance(InstallDisposition::RolledBack, None)?;
        self.store.write_journal(&journal, lock)?;
        Ok(InstallOutcome::RolledBack {
            active_unit: journal.prior_active_unit,
            failure: journal.failure.unwrap_or_default(),
            abandoned: true,
        })
    }

    fn drive_rollback(
        &mut self,
        mut journal: InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
        loop {
            let action = journal
                .next_action
                .ok_or(InstallCoordinatorError::MissingNextAction)?;
            if action == InstallAction::FinishRollback {
                journal.advance(InstallDisposition::RolledBack, None)?;
                self.store.write_journal(&journal, lock)?;
                return Ok(InstallOutcome::RolledBack {
                    active_unit: journal.prior_active_unit,
                    failure: journal.failure.unwrap_or_default(),
                    abandoned: false,
                });
            }

            if action == InstallAction::RestorePriorLayout {
                match self.reconcile_layout_operation(&journal, false, lock) {
                    Ok(()) => {
                        journal.layout_operation_index -= 1;
                        let next_action = if journal.layout_operation_index == 0 {
                            next_rollback(action)?
                        } else {
                            action
                        };
                        journal.advance(InstallDisposition::Rollback, Some(next_action))?;
                        self.store.write_journal(&journal, lock)?;
                    }
                    Err(StepError::Effect(error) | StepError::Fatal(error)) => return Err(error),
                    Err(StepError::Abandon(detail)) => {
                        return Err(InstallCoordinatorError::InvalidAbandonment(detail));
                    }
                }
                continue;
            }

            match self.reconcile_action(&journal, action, lock) {
                Ok(()) => {
                    journal.advance(InstallDisposition::Rollback, Some(next_rollback(action)?))?;
                    self.store.write_journal(&journal, lock)?;
                }
                Err(StepError::Effect(error) | StepError::Fatal(error)) => return Err(error),
                Err(StepError::Abandon(detail)) => {
                    return Err(InstallCoordinatorError::InvalidAbandonment(detail));
                }
            }
        }
    }

    fn reconcile_layout_operation(
        &mut self,
        journal: &InstallJournalV1,
        installing: bool,
        lock: &super::store::InstallLock,
    ) -> Result<(), StepError> {
        let checkpoints = Checkpoints::new(journal);
        let current_index = journal.layout_operation_index;
        let (before, after, before_checkpoint, after_checkpoint, after_index, unit, action) =
            if installing {
                let before = if current_index == 0 {
                    &checkpoints.prior_unloaded
                } else {
                    &checkpoints.candidate_layout
                };
                let before_checkpoint = if current_index == 0 {
                    PlatformCheckpoint::PriorUnloaded
                } else {
                    PlatformCheckpoint::CandidateLayout
                };
                (
                    before,
                    &checkpoints.candidate_layout,
                    before_checkpoint,
                    PlatformCheckpoint::CandidateLayout,
                    current_index + 1,
                    Some(&journal.candidate_unit),
                    InstallAction::InstallCandidateLayout,
                )
            } else {
                if current_index == 0 {
                    return Err(StepError::Fatal(
                        InstallModelError::InvalidLayoutOperationCursor.into(),
                    ));
                }
                let before = if current_index == journal.layout_operation_count {
                    &checkpoints.prior_launcher_restored
                } else {
                    &checkpoints.prior_layout_restored
                };
                let before_checkpoint = if current_index == journal.layout_operation_count {
                    PlatformCheckpoint::PriorLauncherRestored
                } else {
                    PlatformCheckpoint::PriorLayoutRestored
                };
                (
                    before,
                    &checkpoints.prior_layout_restored,
                    before_checkpoint,
                    PlatformCheckpoint::PriorLayoutRestored,
                    current_index - 1,
                    journal.prior_platform.layout_unit.as_ref(),
                    InstallAction::RestorePriorLayout,
                )
            };
        let mut stopped = false;
        let (actual, before_matches, after_matches) = loop {
            let actual = self.inspect_state(lock).map_err(StepError::Fatal)?;
            let before_matches = self
                .matches_checkpoint_at(
                    &actual,
                    before,
                    before_checkpoint,
                    current_index,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                )
                .map_err(StepError::Fatal)?;
            let after_matches = self
                .matches_checkpoint_at(
                    &actual,
                    after,
                    after_checkpoint,
                    after_index,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                )
                .map_err(StepError::Fatal)?;
            if before_matches
                || after_matches
                || stopped
                || !self
                    .stop_first(
                        journal,
                        action,
                        &actual,
                        &[
                            (before, before_checkpoint, current_index),
                            (after, after_checkpoint, after_index),
                        ],
                    )
                    .map_err(StepError::Fatal)?
            {
                break (actual, before_matches, after_matches);
            }
            stopped = true;
        };
        if after_matches {
            return Ok(());
        }
        if !before_matches {
            return Err(StepError::Fatal(state_drift(
                action,
                before.clone(),
                after.clone(),
                actual,
            )));
        }
        self.platform
            .install_layout_operation(
                after_checkpoint,
                unit,
                current_index.min(after_index),
                &journal.platform_record,
            )
            .map_err(|source| StepError::Effect(platform_error(action, source)))?;
        self.require_state(
            action,
            before,
            after,
            after_checkpoint,
            after_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
            lock,
        )
        .map_err(StepError::Fatal)
    }

    fn reconcile_forward_layout_progress_after_error(
        &mut self,
        journal: &mut InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<(), InstallCoordinatorError> {
        let checkpoints = Checkpoints::new(journal);
        let actual = self.inspect_state(lock)?;
        let current = journal.layout_operation_index;
        let progressed = current + 1;
        if self.matches_checkpoint_at(
            &actual,
            &checkpoints.candidate_layout,
            PlatformCheckpoint::CandidateLayout,
            progressed,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            journal.layout_operation_index = progressed;
            return Ok(());
        }
        let (before, before_checkpoint) = if current == 0 {
            (
                &checkpoints.prior_unloaded,
                PlatformCheckpoint::PriorUnloaded,
            )
        } else {
            (
                &checkpoints.candidate_layout,
                PlatformCheckpoint::CandidateLayout,
            )
        };
        if !self.matches_checkpoint_at(
            &actual,
            before,
            before_checkpoint,
            current,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            return Err(state_drift(
                InstallAction::InstallCandidateLayout,
                before.clone(),
                checkpoints.candidate_layout,
                actual,
            ));
        }
        Ok(())
    }

    fn reconcile_action(
        &mut self,
        journal: &InstallJournalV1,
        action: InstallAction,
        lock: &super::store::InstallLock,
    ) -> Result<(), StepError> {
        let transition = Transition::for_action(journal, action).map_err(StepError::Fatal)?;
        // Before the prior is unloaded, a lost baseline abandons the
        // transaction instead; everywhere else a service that the platform
        // restarted on its own is stopped before the step resumes.
        let baseline_step = is_baseline_step(action);
        // The stopped states this step can resume from once a service the
        // platform started on its own is stopped.
        let stopped_targets: Vec<_> = [
            (&transition.before, transition.before_checkpoint),
            (&transition.after, transition.after_checkpoint),
        ]
        .into_iter()
        .filter(|(state, _)| !baseline_step && state.platform.running_unit.is_none())
        .map(|(state, checkpoint)| (state, checkpoint, journal.layout_operation_index))
        .collect();
        let mut stopped = false;
        let (actual, before_matches, after_matches) = loop {
            let actual = self.inspect_state(lock).map_err(StepError::Fatal)?;
            let before_matches = self
                .matches_checkpoint_at(
                    &actual,
                    &transition.before,
                    transition.before_checkpoint,
                    journal.layout_operation_index,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                )
                .map_err(StepError::Fatal)?;
            let after_matches = self
                .matches_checkpoint_at(
                    &actual,
                    &transition.after,
                    transition.after_checkpoint,
                    journal.layout_operation_index,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                )
                .map_err(StepError::Fatal)?;
            if before_matches
                || after_matches
                || stopped
                || !self
                    .stop_first(journal, action, &actual, &stopped_targets)
                    .map_err(StepError::Fatal)?
            {
                break (actual, before_matches, after_matches);
            }
            stopped = true;
        };
        if baseline_step
            && !before_matches
            && !after_matches
            && let Some(detail) = self
                .abandonment(journal, action, &actual)
                .map_err(StepError::Fatal)?
        {
            return Err(StepError::Abandon(detail));
        }

        if transition.kind != TransitionKind::Mutation {
            if !before_matches {
                // A candidate that restarted, stopped or is still changing
                // state after its receipt, with nothing else changed, failed
                // its proof. Any other difference stays drift.
                let candidate_failed = action == InstallAction::ProveCandidate
                    && self
                        .matches_except_runtime(
                            &actual,
                            &transition.before,
                            transition.before_checkpoint,
                            journal.layout_operation_index,
                            &journal.platform_record,
                        )
                        .map_err(StepError::Fatal)?;
                let drift = state_drift(action, transition.before, transition.after, actual);
                return Err(if candidate_failed {
                    StepError::Effect(drift)
                } else {
                    StepError::Fatal(drift)
                });
            }
            let check = match transition.kind {
                TransitionKind::Preflight => self.platform.preflight_authority(
                    &journal.candidate_unit,
                    &transition.before,
                    &journal.platform_record,
                ),
                TransitionKind::GuardRelease => self
                    .platform
                    .wait_for_guard_release(&transition.after.platform, &journal.platform_record),
                TransitionKind::OwnerPublication => self.platform.wait_for_newer_owner(
                    transition.after_checkpoint,
                    &transition.after.platform,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                ),
                TransitionKind::Mutation => unreachable!("mutation handled below"),
            };
            check.map_err(|source| StepError::Effect(platform_error(action, source)))?;
            return self
                .require_state(
                    action,
                    &transition.before,
                    &transition.after,
                    transition.after_checkpoint,
                    journal.layout_operation_index,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                    lock,
                )
                .map_err(StepError::Fatal);
        }

        let replay_policy = ReplayPolicy::for_action(action);
        if replay_policy == ReplayPolicy::ObservedTransition && after_matches {
            return Ok(());
        }
        if !before_matches && !after_matches {
            return Err(StepError::Fatal(state_drift(
                action,
                transition.before,
                transition.after,
                actual,
            )));
        }

        self.apply_effect(
            journal,
            action,
            transition.after_checkpoint,
            &transition.after,
            lock,
        )
        .map_err(StepError::Effect)?;
        self.require_state(
            action,
            &transition.before,
            &transition.after,
            transition.after_checkpoint,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
            lock,
        )
        .map_err(StepError::Fatal)
    }

    fn apply_effect(
        &mut self,
        journal: &InstallJournalV1,
        action: InstallAction,
        checkpoint: PlatformCheckpoint,
        after: &InstallationState,
        lock: &super::store::InstallLock,
    ) -> Result<(), InstallCoordinatorError> {
        match action {
            InstallAction::UnloadPrior | InstallAction::UnloadCandidateRuntime => self
                .platform
                .restore_runtime(
                    &after.platform,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                )
                .map_err(|source| platform_error(action, source)),
            InstallAction::UnloadCandidateAutostart => self
                .platform
                .restore_autostart(&after.platform, &journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::UnloadCandidateManager => self
                .platform
                .reload_manager(&after.platform, &journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::InstallCandidateLauncher => self
                .platform
                .install_launcher(
                    checkpoint,
                    journal.target_platform.launcher_unit.as_ref(),
                    &journal.platform_record,
                )
                .map_err(|source| platform_error(action, source)),
            InstallAction::SwitchToCandidate => self
                .store
                .set_active(Some(&journal.candidate_unit), lock)
                .map_err(InstallCoordinatorError::Store),
            InstallAction::ReloadCandidateManager | InstallAction::ReloadPriorManager => self
                .platform
                .reload_manager(&after.platform, &journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::RestoreCandidateAutostart | InstallAction::RestorePriorAutostart => self
                .platform
                .restore_autostart(&after.platform, &journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::RestoreCandidateRuntime | InstallAction::RestorePriorRuntime => self
                .platform
                .restore_runtime(
                    &after.platform,
                    &journal.platform_record,
                    journal.candidate_owner_receipt.as_ref(),
                )
                .map_err(|source| platform_error(action, source)),
            InstallAction::RestorePriorActive => self
                .store
                .set_active(journal.prior_active_unit.as_ref(), lock)
                .map_err(InstallCoordinatorError::Store),
            InstallAction::RestorePriorLauncher => self
                .platform
                .install_launcher(
                    checkpoint,
                    journal.prior_platform.launcher_unit.as_ref(),
                    &journal.platform_record,
                )
                .map_err(|source| platform_error(action, source)),
            InstallAction::InstallCandidateLayout
            | InstallAction::RestorePriorLayout
            | InstallAction::ProveCandidate
            | InstallAction::ProvePrior
            | InstallAction::PreflightCandidate
            | InstallAction::ProvePriorGuardReleased
            | InstallAction::ProveCandidateGuardReleased
            | InstallAction::Commit
            | InstallAction::FinishRollback => Err(InstallCoordinatorError::InvalidAction(action)),
        }
    }

    fn rollback_entry_action(
        &mut self,
        journal: &InstallJournalV1,
        failed_action: InstallAction,
        lock: &super::store::InstallLock,
    ) -> Result<InstallAction, InstallCoordinatorError> {
        let mut stopped = false;
        loop {
            let actual = self.inspect_state(lock)?;
            if let Some(action) = self.rollback_entry_for(journal, &actual)? {
                return Ok(action);
            }
            // No entry names a service that restarted, flaps or is still
            // changing state. When stopping it would reach a stopped entry,
            // stop it whatever its invocation, then choose. Never before the
            // prior is unloaded: there a lost baseline abandons instead.
            let checkpoints = Checkpoints::new(journal);
            let index = journal.layout_operation_index;
            let mut targets = vec![
                (
                    &checkpoints.candidate_autostart,
                    PlatformCheckpoint::CandidateAutostart,
                    index,
                ),
                (
                    &checkpoints.candidate_manager,
                    PlatformCheckpoint::CandidateManager,
                    index,
                ),
                (
                    &checkpoints.candidate_active,
                    PlatformCheckpoint::CandidateActive,
                    index,
                ),
                (
                    &checkpoints.candidate_launcher,
                    PlatformCheckpoint::CandidateLauncher,
                    index,
                ),
            ];
            if index > 0 {
                targets.push((
                    &checkpoints.candidate_layout,
                    PlatformCheckpoint::CandidateLayout,
                    index,
                ));
            }
            targets.push((
                &checkpoints.prior_unloaded,
                PlatformCheckpoint::PriorUnloaded,
                index,
            ));
            if stopped
                || is_baseline_step(failed_action)
                || !self.stop_first(journal, failed_action, &actual, &targets)?
            {
                return Err(state_drift(
                    failed_action,
                    Transition::for_action(journal, failed_action)?.before,
                    Transition::for_action(journal, failed_action)?.after,
                    actual,
                ));
            }
            stopped = true;
        }
    }

    fn rollback_entry_for(
        &mut self,
        journal: &InstallJournalV1,
        actual: &InstallationState,
    ) -> Result<Option<InstallAction>, InstallCoordinatorError> {
        let checkpoints = Checkpoints::new(journal);
        for (state, checkpoint, action) in [
            (
                &checkpoints.candidate_runtime,
                PlatformCheckpoint::CandidateRuntime,
                InstallAction::UnloadCandidateRuntime,
            ),
            (
                &checkpoints.candidate_autostart,
                PlatformCheckpoint::CandidateAutostart,
                InstallAction::UnloadCandidateAutostart,
            ),
            (
                &checkpoints.candidate_manager,
                PlatformCheckpoint::CandidateManager,
                InstallAction::UnloadCandidateManager,
            ),
            (
                &checkpoints.candidate_active,
                PlatformCheckpoint::CandidateActive,
                InstallAction::ProveCandidateGuardReleased,
            ),
        ] {
            if self.matches_checkpoint_at(
                actual,
                state,
                checkpoint,
                journal.layout_operation_index,
                &journal.platform_record,
                journal.candidate_owner_receipt.as_ref(),
            )? {
                return Ok(Some(action));
            }
        }
        let action = if self.matches_checkpoint_at(
            actual,
            &checkpoints.candidate_launcher,
            PlatformCheckpoint::CandidateLauncher,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            InstallAction::RestorePriorLauncher
        } else if journal.layout_operation_index > 0
            && self.matches_checkpoint_at(
                actual,
                &checkpoints.candidate_layout,
                PlatformCheckpoint::CandidateLayout,
                journal.layout_operation_index,
                &journal.platform_record,
                journal.candidate_owner_receipt.as_ref(),
            )?
        {
            InstallAction::RestorePriorLayout
        } else if self.matches_checkpoint_at(
            actual,
            &checkpoints.prior_unloaded,
            PlatformCheckpoint::PriorUnloaded,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            InstallAction::ReloadPriorManager
        } else if self.matches_checkpoint_at(
            actual,
            &checkpoints.prior,
            PlatformCheckpoint::PriorOriginal,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            InstallAction::FinishRollback
        } else {
            return Ok(None);
        };
        Ok(Some(action))
    }

    /// Stop a service the platform started outside the journal (A-10) when
    /// that alone would reach one of `targets`, stopped states this step
    /// could resume from. Returns whether it stopped one.
    fn stop_first(
        &mut self,
        journal: &InstallJournalV1,
        action: InstallAction,
        actual: &InstallationState,
        targets: &[(&InstallationState, PlatformCheckpoint, u16)],
    ) -> Result<bool, InstallCoordinatorError> {
        if actual.platform.running_unit.is_none() {
            return Ok(false);
        }
        let mut reachable = false;
        for (target, checkpoint, index) in targets {
            if target.platform.running_unit.is_none()
                && self.matches_except_runtime(
                    actual,
                    target,
                    *checkpoint,
                    *index,
                    &journal.platform_record,
                )?
            {
                reachable = true;
                break;
            }
        }
        if !reachable {
            return Ok(false);
        }
        self.platform
            .stop_unjournaled_runtime(&journal.platform_record)
            .map_err(|source| platform_error(action, source))
    }

    /// Whether `actual` matches `expected` in everything but the runtime.
    fn matches_except_runtime(
        &mut self,
        actual: &InstallationState,
        expected: &InstallationState,
        checkpoint: PlatformCheckpoint,
        layout_operation_index: u16,
        record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallCoordinatorError> {
        let layout_checkpoint = matches!(
            checkpoint,
            PlatformCheckpoint::CandidateLayout | PlatformCheckpoint::PriorLayoutRestored
        );
        if actual.active_unit != expected.active_unit
            || actual.platform.launcher_unit != expected.platform.launcher_unit
            || actual.platform.autostart_enabled != expected.platform.autostart_enabled
            || !layout_checkpoint && actual.platform.layout_unit != expected.platform.layout_unit
        {
            return Ok(false);
        }
        self.platform
            .matches_exact_state_except_runtime(
                checkpoint,
                &expected.platform,
                layout_operation_index,
                record,
            )
            .map_err(InstallCoordinatorError::InspectPlatform)
    }

    /// The failure detail for abandoning a transaction whose prior was never
    /// unloaded and whose baseline service identity is gone (A-11).
    fn abandonment(
        &mut self,
        journal: &InstallJournalV1,
        action: InstallAction,
        actual: &InstallationState,
    ) -> Result<Option<String>, InstallCoordinatorError> {
        // At UnloadPrior a stopped or still stopping prior may be this
        // transaction's own stop, so only a prior running steadily under a
        // new invocation proves nothing was changed; any other difference
        // there is drift.
        let at_unload = action == InstallAction::UnloadPrior;
        if journal.disposition != InstallDisposition::Forward
            || !is_baseline_step(action)
            || journal.layout_operation_index != 0
            || journal.candidate_owner_receipt.is_some()
            || at_unload && actual.platform.running_unit.is_none()
            || actual.active_unit != journal.prior_active_unit
            || !self
                .platform
                .matches_untouched_prior(
                    &journal.prior_platform,
                    &journal.platform_record,
                    at_unload,
                )
                .map_err(InstallCoordinatorError::InspectPlatform)?
        {
            return Ok(None);
        }
        Ok(Some(format!(
            "abandoned at {action:?}: the running release restarted or stopped before \
             it was unloaded, so its original service could no longer be proven; \
             nothing had changed"
        )))
    }

    fn require_state(
        &mut self,
        action: InstallAction,
        before: &InstallationState,
        after: &InstallationState,
        after_checkpoint: PlatformCheckpoint,
        layout_operation_index: u16,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
        lock: &super::store::InstallLock,
    ) -> Result<(), InstallCoordinatorError> {
        let actual = self.inspect_state(lock)?;
        if self.matches_checkpoint_at(
            &actual,
            after,
            after_checkpoint,
            layout_operation_index,
            record,
            candidate_owner_receipt,
        )? {
            Ok(())
        } else {
            Err(state_drift(action, before.clone(), after.clone(), actual))
        }
    }

    fn matches_checkpoint_at(
        &mut self,
        actual: &InstallationState,
        expected: &InstallationState,
        checkpoint: PlatformCheckpoint,
        layout_operation_index: u16,
        record: &PlatformTransactionRecord,
        candidate_owner_receipt: Option<&PlatformOwnerReceipt>,
    ) -> Result<bool, InstallCoordinatorError> {
        let platform_matches = if matches!(
            checkpoint,
            PlatformCheckpoint::CandidateLayout | PlatformCheckpoint::PriorLayoutRestored
        ) {
            actual.platform.launcher_unit == expected.platform.launcher_unit
                && actual.platform.loaded == expected.platform.loaded
                && actual.platform.running_unit == expected.platform.running_unit
                && actual.platform.autostart_enabled == expected.platform.autostart_enabled
        } else {
            actual.platform == expected.platform
        };
        if actual.active_unit != expected.active_unit || !platform_matches {
            return Ok(false);
        }
        self.platform
            .matches_exact_state(
                checkpoint,
                &expected.platform,
                layout_operation_index,
                record,
                candidate_owner_receipt,
            )
            .map_err(InstallCoordinatorError::InspectPlatform)
    }

    fn inspect_state(
        &mut self,
        lock: &super::store::InstallLock,
    ) -> Result<InstallationState, InstallCoordinatorError> {
        let active_unit = self.store.active_unit(lock)?;
        let platform = self
            .platform
            .inspect()
            .map_err(InstallCoordinatorError::InspectPlatform)?;
        platform.validate()?;
        Ok(InstallationState {
            active_unit,
            platform,
        })
    }
}

/// Where a failed forward action goes next.
#[derive(Debug)]
enum RollbackEntry {
    Action(InstallAction),
    Abandon(String),
}

/// The steps before the prior is unloaded, where a lost baseline abandons
/// the transaction and no service is ever stopped outside the journal.
fn is_baseline_step(action: InstallAction) -> bool {
    matches!(
        action,
        InstallAction::PreflightCandidate | InstallAction::UnloadPrior
    )
}

#[derive(Debug)]
enum StepError {
    Effect(InstallCoordinatorError),
    Fatal(InstallCoordinatorError),
    /// The transaction never unloaded the prior, lost the baseline it would
    /// need to, and can end without any effect.
    Abandon(String),
}

#[derive(Debug)]
struct Transition {
    before: InstallationState,
    after: InstallationState,
    before_checkpoint: PlatformCheckpoint,
    after_checkpoint: PlatformCheckpoint,
    kind: TransitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransitionKind {
    Mutation,
    Preflight,
    GuardRelease,
    OwnerPublication,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayPolicy {
    ObservedTransition,
    IdempotentCommand,
}

impl ReplayPolicy {
    fn for_action(action: InstallAction) -> Self {
        match action {
            InstallAction::ReloadCandidateManager
            | InstallAction::UnloadCandidateManager
            | InstallAction::ReloadPriorManager => Self::IdempotentCommand,
            _ => Self::ObservedTransition,
        }
    }
}

impl Transition {
    fn for_action(
        journal: &InstallJournalV1,
        action: InstallAction,
    ) -> Result<Self, InstallCoordinatorError> {
        let checkpoints = Checkpoints::new(journal);
        let (before, after, before_checkpoint, after_checkpoint, kind) = match action {
            InstallAction::PreflightCandidate => (
                checkpoints.prior.clone(),
                checkpoints.prior,
                PlatformCheckpoint::PriorOriginal,
                PlatformCheckpoint::PriorOriginal,
                TransitionKind::Preflight,
            ),
            InstallAction::UnloadPrior => (
                checkpoints.prior,
                checkpoints.prior_unloaded,
                PlatformCheckpoint::PriorOriginal,
                PlatformCheckpoint::PriorUnloaded,
                TransitionKind::Mutation,
            ),
            InstallAction::ProvePriorGuardReleased => (
                checkpoints.prior_unloaded.clone(),
                checkpoints.prior_unloaded,
                PlatformCheckpoint::PriorUnloaded,
                PlatformCheckpoint::PriorUnloaded,
                TransitionKind::GuardRelease,
            ),
            InstallAction::InstallCandidateLauncher => (
                checkpoints.candidate_layout,
                checkpoints.candidate_launcher,
                PlatformCheckpoint::CandidateLayout,
                PlatformCheckpoint::CandidateLauncher,
                TransitionKind::Mutation,
            ),
            InstallAction::SwitchToCandidate => (
                checkpoints.candidate_launcher,
                checkpoints.candidate_active,
                PlatformCheckpoint::CandidateLauncher,
                PlatformCheckpoint::CandidateActive,
                TransitionKind::Mutation,
            ),
            InstallAction::ReloadCandidateManager => (
                checkpoints.candidate_active,
                checkpoints.candidate_manager,
                PlatformCheckpoint::CandidateActive,
                PlatformCheckpoint::CandidateManager,
                TransitionKind::Mutation,
            ),
            InstallAction::RestoreCandidateAutostart => (
                checkpoints.candidate_manager,
                checkpoints.candidate_autostart,
                PlatformCheckpoint::CandidateManager,
                PlatformCheckpoint::CandidateAutostart,
                TransitionKind::Mutation,
            ),
            InstallAction::RestoreCandidateRuntime => (
                checkpoints.candidate_autostart,
                checkpoints.candidate_runtime,
                PlatformCheckpoint::CandidateAutostart,
                PlatformCheckpoint::CandidateRuntime,
                TransitionKind::Mutation,
            ),
            InstallAction::ProveCandidate => (
                checkpoints.candidate_runtime.clone(),
                checkpoints.candidate_runtime,
                PlatformCheckpoint::CandidateRuntime,
                PlatformCheckpoint::CandidateRuntime,
                TransitionKind::OwnerPublication,
            ),
            InstallAction::UnloadCandidateRuntime => (
                checkpoints.candidate_runtime,
                checkpoints.candidate_autostart,
                PlatformCheckpoint::CandidateRuntime,
                PlatformCheckpoint::CandidateAutostart,
                TransitionKind::Mutation,
            ),
            InstallAction::UnloadCandidateAutostart => (
                checkpoints.candidate_autostart,
                checkpoints.candidate_manager,
                PlatformCheckpoint::CandidateAutostart,
                PlatformCheckpoint::CandidateManager,
                TransitionKind::Mutation,
            ),
            InstallAction::UnloadCandidateManager => (
                checkpoints.candidate_manager,
                checkpoints.candidate_active,
                PlatformCheckpoint::CandidateManager,
                PlatformCheckpoint::CandidateActive,
                TransitionKind::Mutation,
            ),
            InstallAction::ProveCandidateGuardReleased => (
                checkpoints.candidate_active.clone(),
                checkpoints.candidate_active,
                PlatformCheckpoint::CandidateActive,
                PlatformCheckpoint::CandidateActive,
                TransitionKind::GuardRelease,
            ),
            InstallAction::RestorePriorActive => (
                checkpoints.candidate_active,
                checkpoints.prior_active_restored,
                PlatformCheckpoint::CandidateActive,
                PlatformCheckpoint::PriorActiveRestored,
                TransitionKind::Mutation,
            ),
            InstallAction::RestorePriorLauncher => (
                checkpoints.prior_active_restored,
                checkpoints.prior_launcher_restored,
                PlatformCheckpoint::PriorActiveRestored,
                PlatformCheckpoint::PriorLauncherRestored,
                TransitionKind::Mutation,
            ),
            InstallAction::ReloadPriorManager => (
                checkpoints.prior_layout_restored,
                checkpoints.prior_manager,
                PlatformCheckpoint::PriorLayoutRestored,
                PlatformCheckpoint::PriorManager,
                TransitionKind::Mutation,
            ),
            InstallAction::RestorePriorAutostart => (
                checkpoints.prior_manager,
                checkpoints.prior_autostart,
                PlatformCheckpoint::PriorManager,
                PlatformCheckpoint::PriorAutostart,
                TransitionKind::Mutation,
            ),
            InstallAction::RestorePriorRuntime => (
                checkpoints.prior_autostart,
                checkpoints.prior,
                PlatformCheckpoint::PriorAutostart,
                PlatformCheckpoint::PriorRestored,
                TransitionKind::Mutation,
            ),
            InstallAction::ProvePrior => (
                checkpoints.prior.clone(),
                checkpoints.prior,
                PlatformCheckpoint::PriorRestored,
                PlatformCheckpoint::PriorRestored,
                TransitionKind::OwnerPublication,
            ),
            InstallAction::InstallCandidateLayout
            | InstallAction::RestorePriorLayout
            | InstallAction::Commit
            | InstallAction::FinishRollback => {
                return Err(InstallCoordinatorError::InvalidAction(action));
            }
        };
        Ok(Self {
            before,
            after,
            before_checkpoint,
            after_checkpoint,
            kind,
        })
    }
}

struct Checkpoints {
    prior: InstallationState,
    prior_unloaded: InstallationState,
    candidate_layout: InstallationState,
    candidate_launcher: InstallationState,
    candidate_active: InstallationState,
    candidate_manager: InstallationState,
    candidate_autostart: InstallationState,
    candidate_runtime: InstallationState,
    prior_active_restored: InstallationState,
    prior_launcher_restored: InstallationState,
    prior_layout_restored: InstallationState,
    prior_manager: InstallationState,
    prior_autostart: InstallationState,
}

impl Checkpoints {
    fn new(journal: &InstallJournalV1) -> Self {
        let prior = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: journal.prior_platform.clone(),
        };
        let prior_unloaded = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: journal.transition_states.prior_unloaded.clone(),
        };
        let candidate_layout = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: PlatformState {
                layout_unit: journal.prior_active_unit.clone(),
                ..prior_unloaded.platform.clone()
            },
        };
        let candidate_launcher = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: PlatformState {
                launcher_unit: journal
                    .target_platform
                    .launcher_unit
                    .as_ref()
                    .and(journal.prior_active_unit.clone()),
                ..candidate_layout.platform.clone()
            },
        };
        let candidate_active = InstallationState {
            active_unit: Some(journal.candidate_unit.clone()),
            platform: PlatformState {
                layout_unit: Some(journal.candidate_unit.clone()),
                launcher_unit: journal
                    .target_platform
                    .launcher_unit
                    .as_ref()
                    .map(|_| journal.candidate_unit.clone()),
                ..candidate_launcher.platform.clone()
            },
        };
        let candidate_manager = InstallationState {
            active_unit: Some(journal.candidate_unit.clone()),
            platform: journal.transition_states.candidate_manager.clone(),
        };
        let candidate_autostart = InstallationState {
            active_unit: Some(journal.candidate_unit.clone()),
            platform: journal.transition_states.candidate_autostart.clone(),
        };
        let candidate_runtime = InstallationState {
            active_unit: Some(journal.candidate_unit.clone()),
            platform: journal.target_platform.clone(),
        };
        let prior_active_restored = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: PlatformState {
                layout_unit: journal.prior_active_unit.clone(),
                launcher_unit: journal
                    .target_platform
                    .launcher_unit
                    .as_ref()
                    .and(journal.prior_active_unit.clone()),
                ..candidate_active.platform.clone()
            },
        };
        let prior_launcher_restored = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: PlatformState {
                launcher_unit: journal.prior_platform.launcher_unit.clone(),
                ..prior_active_restored.platform.clone()
            },
        };
        let prior_layout_restored = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: PlatformState {
                layout_unit: journal.prior_platform.layout_unit.clone(),
                ..prior_launcher_restored.platform.clone()
            },
        };
        let prior_manager = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: journal.transition_states.prior_manager.clone(),
        };
        let prior_autostart = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: journal.transition_states.prior_autostart.clone(),
        };
        Self {
            prior,
            prior_unloaded,
            candidate_layout,
            candidate_launcher,
            candidate_active,
            candidate_manager,
            candidate_autostart,
            candidate_runtime,
            prior_active_restored,
            prior_launcher_restored,
            prior_layout_restored,
            prior_manager,
            prior_autostart,
        }
    }
}

fn next_forward(action: InstallAction) -> Result<InstallAction, InstallCoordinatorError> {
    match action {
        InstallAction::PreflightCandidate => Ok(InstallAction::UnloadPrior),
        InstallAction::UnloadPrior => Ok(InstallAction::ProvePriorGuardReleased),
        InstallAction::ProvePriorGuardReleased => Ok(InstallAction::InstallCandidateLayout),
        InstallAction::InstallCandidateLayout => Ok(InstallAction::InstallCandidateLauncher),
        InstallAction::InstallCandidateLauncher => Ok(InstallAction::SwitchToCandidate),
        InstallAction::SwitchToCandidate => Ok(InstallAction::ReloadCandidateManager),
        InstallAction::ReloadCandidateManager => Ok(InstallAction::RestoreCandidateAutostart),
        InstallAction::RestoreCandidateAutostart => Ok(InstallAction::RestoreCandidateRuntime),
        InstallAction::RestoreCandidateRuntime => Ok(InstallAction::ProveCandidate),
        InstallAction::ProveCandidate => Ok(InstallAction::Commit),
        _ => Err(InstallCoordinatorError::InvalidAction(action)),
    }
}

fn next_rollback(action: InstallAction) -> Result<InstallAction, InstallCoordinatorError> {
    match action {
        InstallAction::UnloadCandidateRuntime => Ok(InstallAction::UnloadCandidateAutostart),
        InstallAction::UnloadCandidateAutostart => Ok(InstallAction::UnloadCandidateManager),
        InstallAction::UnloadCandidateManager => Ok(InstallAction::ProveCandidateGuardReleased),
        InstallAction::ProveCandidateGuardReleased => Ok(InstallAction::RestorePriorActive),
        InstallAction::RestorePriorActive => Ok(InstallAction::RestorePriorLauncher),
        InstallAction::RestorePriorLauncher => Ok(InstallAction::RestorePriorLayout),
        InstallAction::RestorePriorLayout => Ok(InstallAction::ReloadPriorManager),
        InstallAction::ReloadPriorManager => Ok(InstallAction::RestorePriorAutostart),
        InstallAction::RestorePriorAutostart => Ok(InstallAction::RestorePriorRuntime),
        InstallAction::RestorePriorRuntime => Ok(InstallAction::ProvePrior),
        InstallAction::ProvePrior => Ok(InstallAction::FinishRollback),
        _ => Err(InstallCoordinatorError::InvalidAction(action)),
    }
}

fn state_drift(
    action: InstallAction,
    before: InstallationState,
    after: InstallationState,
    actual: InstallationState,
) -> InstallCoordinatorError {
    InstallCoordinatorError::StateDrift {
        action,
        before: Box::new(before),
        after: Box::new(after),
        actual: Box::new(actual),
    }
}

fn platform_error(action: InstallAction, source: InstallPlatformError) -> InstallCoordinatorError {
    InstallCoordinatorError::Platform { action, source }
}

fn truncate_detail(mut detail: String) -> String {
    if detail.len() <= MAX_FAILURE_DETAIL_BYTES {
        return detail;
    }
    let mut boundary = MAX_FAILURE_DETAIL_BYTES;
    while !detail.is_char_boundary(boundary) {
        boundary -= 1;
    }
    detail.truncate(boundary);
    detail
}

#[derive(Debug, thiserror::Error)]
pub enum InstallCoordinatorError {
    #[error("an existing install transaction must be recovered before preparing another")]
    PendingPreparation,
    #[error(transparent)]
    Store(#[from] InstallStoreError),
    #[error(transparent)]
    Model(#[from] InstallModelError),
    #[error("install platform failed during {action:?}: {source}")]
    Platform {
        action: InstallAction,
        source: InstallPlatformError,
    },
    #[error("failed to inspect install platform state: {0}")]
    InspectPlatform(InstallPlatformError),
    #[error("failed to prepare exact install platform record: {0}")]
    PreparePlatform(InstallPlatformError),
    #[error(
        "install state drift before {action:?}: expected {before:?} or {after:?}, observed {actual:?}"
    )]
    StateDrift {
        action: InstallAction,
        before: Box<InstallationState>,
        after: Box<InstallationState>,
        actual: Box<InstallationState>,
    },
    #[error("install journal has no next action while nonterminal")]
    MissingNextAction,
    #[error("install transaction cannot be abandoned after it changed the platform: {0}")]
    InvalidAbandonment(String),
    #[error("install action {0:?} is invalid for the current disposition")]
    InvalidAction(InstallAction),
}
