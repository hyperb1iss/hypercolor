use super::model::{
    InstallAction, InstallDisposition, InstallJournalV1, InstallModelError, InstallOutcome,
    InstallRequest, InstallationState, PlatformCheckpoint, PlatformOwnerReceipt, PlatformState,
    PlatformTransactionRecord, PreparedPlatformTransaction, UnitId, UnitRecord,
};
use super::store::{InstallLock, InstallStore, InstallStoreError};

const MAX_FAILURE_DETAIL_BYTES: usize = 4_096;

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
        self.store.write_journal(&journal, lock)?;
        self.drive_forward(journal, lock)
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
                }
                continue;
            }

            match self.reconcile_action(&journal, action, lock) {
                Ok(()) => {
                    if action == InstallAction::RestoreCandidateRuntime {
                        self.capture_candidate_owner_receipt(&mut journal, lock)?;
                    }
                    journal.advance(InstallDisposition::Forward, Some(next_forward(action)?))?;
                    self.store.write_journal(&journal, lock)?;
                }
                Err(StepError::Effect(error)) => {
                    let failure = truncate_detail(error.to_string());
                    let next_action = self.rollback_entry_action(&journal, action, lock)?;
                    if next_action == InstallAction::UnloadCandidateRuntime {
                        self.capture_candidate_owner_receipt(&mut journal, lock)?;
                    }
                    journal.failure = Some(failure);
                    journal.advance(InstallDisposition::Rollback, Some(next_action))?;
                    self.store.write_journal(&journal, lock)?;
                    return self.drive_rollback(journal, lock);
                }
                Err(StepError::Fatal(error)) => return Err(error),
            }
        }
    }

    fn capture_candidate_owner_receipt(
        &mut self,
        journal: &mut InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<(), InstallCoordinatorError> {
        if journal.target_platform.running_unit.is_none()
            || journal.candidate_owner_receipt.is_some()
        {
            return Ok(());
        }
        let actual = self.inspect_state(lock)?;
        let expected = Checkpoints::new(journal).candidate_runtime;
        if !self.matches_checkpoint_at(
            &actual,
            &expected,
            PlatformCheckpoint::CandidateRuntime,
            journal.layout_operation_index,
            &journal.platform_record,
            None,
        )? {
            return Err(state_drift(
                InstallAction::RestoreCandidateRuntime,
                expected.clone(),
                expected,
                actual,
            ));
        }
        let receipt = self
            .platform
            .capture_candidate_owner_receipt(&journal.target_platform, &journal.platform_record)
            .map_err(|source| platform_error(InstallAction::RestoreCandidateRuntime, source))?;
        receipt.validate()?;
        journal.candidate_owner_receipt = Some(receipt);
        Ok(())
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
                    Err(StepError::Effect(error)) => return Err(error),
                    Err(StepError::Fatal(error)) => return Err(error),
                }
                continue;
            }

            match self.reconcile_action(&journal, action, lock) {
                Ok(()) => {
                    journal.advance(InstallDisposition::Rollback, Some(next_rollback(action)?))?;
                    self.store.write_journal(&journal, lock)?;
                }
                Err(StepError::Effect(error) | StepError::Fatal(error)) => return Err(error),
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

        if transition.kind != TransitionKind::Mutation {
            if !before_matches {
                return Err(StepError::Fatal(state_drift(
                    action,
                    transition.before,
                    transition.after,
                    actual,
                )));
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
        let actual = self.inspect_state(lock)?;
        let checkpoints = Checkpoints::new(journal);
        let mut candidate_unload = None;
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
                &actual,
                state,
                checkpoint,
                journal.layout_operation_index,
                &journal.platform_record,
                journal.candidate_owner_receipt.as_ref(),
            )? {
                candidate_unload = Some(action);
                break;
            }
        }
        let action = if let Some(action) = candidate_unload {
            action
        } else if self.matches_checkpoint_at(
            &actual,
            &checkpoints.candidate_launcher,
            PlatformCheckpoint::CandidateLauncher,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            InstallAction::RestorePriorLauncher
        } else if journal.layout_operation_index > 0
            && self.matches_checkpoint_at(
                &actual,
                &checkpoints.candidate_layout,
                PlatformCheckpoint::CandidateLayout,
                journal.layout_operation_index,
                &journal.platform_record,
                journal.candidate_owner_receipt.as_ref(),
            )?
        {
            InstallAction::RestorePriorLayout
        } else if self.matches_checkpoint_at(
            &actual,
            &checkpoints.prior_unloaded,
            PlatformCheckpoint::PriorUnloaded,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            InstallAction::ReloadPriorManager
        } else if self.matches_checkpoint_at(
            &actual,
            &checkpoints.prior,
            PlatformCheckpoint::PriorOriginal,
            journal.layout_operation_index,
            &journal.platform_record,
            journal.candidate_owner_receipt.as_ref(),
        )? {
            InstallAction::FinishRollback
        } else {
            return Err(state_drift(
                failed_action,
                Transition::for_action(journal, failed_action)?.before,
                Transition::for_action(journal, failed_action)?.after,
                actual,
            ));
        };
        Ok(action)
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

#[derive(Debug)]
enum StepError {
    Effect(InstallCoordinatorError),
    Fatal(InstallCoordinatorError),
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
    #[error("install action {0:?} is invalid for the current disposition")]
    InvalidAction(InstallAction),
}
