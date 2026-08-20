use super::model::{
    InstallAction, InstallDisposition, InstallJournalV1, InstallModelError, InstallOutcome,
    InstallRequest, InstallationState, PlatformCheckpoint, PlatformState,
    PlatformTransactionRecord, UnitId, UnitRecord,
};
use super::store::{InstallStore, InstallStoreError};

const MAX_FAILURE_DETAIL_BYTES: usize = 4_096;

pub trait InstallPlatform {
    fn inspect(&mut self) -> Result<PlatformState, InstallPlatformError>;

    fn prepare_transaction(
        &mut self,
        candidate: &UnitRecord,
        prior: &InstallationState,
    ) -> Result<PlatformTransactionRecord, InstallPlatformError>;

    fn matches_exact_state(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallPlatformError>;

    fn preflight_authority(
        &mut self,
        candidate: &UnitId,
        prior: &InstallationState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn unload(&mut self, record: &PlatformTransactionRecord) -> Result<(), InstallPlatformError>;

    fn wait_for_guard_release(
        &mut self,
        unloaded: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn install_launcher(
        &mut self,
        unit: Option<&UnitId>,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn restore_loaded_state(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
    ) -> Result<(), InstallPlatformError>;

    fn wait_for_newer_owner(
        &mut self,
        expected: &PlatformState,
        record: &PlatformTransactionRecord,
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
        let lock = self.store.acquire_lock()?;
        if let Some(journal) = self.store.load_journal(&lock)?
            && matches!(
                journal.disposition,
                InstallDisposition::Forward | InstallDisposition::Rollback
            )
        {
            return self.resume(journal, &lock);
        }

        let prior_active_unit = self.store.active_unit(&lock)?;
        let prior_platform = self
            .platform
            .inspect()
            .map_err(InstallCoordinatorError::InspectPlatform)?;
        prior_platform.validate()?;
        let prior_state = InstallationState {
            active_unit: prior_active_unit.clone(),
            platform: prior_platform.clone(),
        };
        let platform_record = self
            .platform
            .prepare_transaction(&request.candidate, &prior_state)
            .map_err(InstallCoordinatorError::PreparePlatform)?;
        platform_record.validate()?;

        let journal = InstallJournalV1::new(
            request.transaction_id,
            prior_active_unit,
            request.candidate.id,
            prior_platform,
            platform_record,
        )?;
        self.store.write_journal(&journal, &lock)?;
        self.drive_forward(journal, &lock)
    }

    pub fn recover(&mut self) -> Result<Option<InstallOutcome>, InstallCoordinatorError> {
        let lock = self.store.acquire_lock()?;
        let Some(journal) = self.store.load_journal(&lock)? else {
            return Ok(None);
        };
        self.resume(journal, &lock).map(Some)
    }

    fn resume(
        &mut self,
        journal: InstallJournalV1,
        lock: &super::store::InstallLock,
    ) -> Result<InstallOutcome, InstallCoordinatorError> {
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

            match self.reconcile_action(&journal, action, lock) {
                Ok(()) => {
                    journal.advance(InstallDisposition::Forward, Some(next_forward(action)?))?;
                    self.store.write_journal(&journal, lock)?;
                }
                Err(StepError::Effect(error)) => {
                    let failure = truncate_detail(error.to_string());
                    let next_action = self.rollback_entry_action(&journal, action, lock)?;
                    journal.failure = Some(failure);
                    journal.advance(InstallDisposition::Rollback, Some(next_action))?;
                    self.store.write_journal(&journal, lock)?;
                    return self.drive_rollback(journal, lock);
                }
                Err(StepError::Fatal(error)) => return Err(error),
            }
        }
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

            match self.reconcile_action(&journal, action, lock) {
                Ok(()) => {
                    journal.advance(InstallDisposition::Rollback, Some(next_rollback(action)?))?;
                    self.store.write_journal(&journal, lock)?;
                }
                Err(StepError::Effect(error) | StepError::Fatal(error)) => return Err(error),
            }
        }
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
            .matches_checkpoint(
                &actual,
                &transition.before,
                transition.before_checkpoint,
                &journal.platform_record,
            )
            .map_err(StepError::Fatal)?;
        let after_matches = self
            .matches_checkpoint(
                &actual,
                &transition.after,
                transition.after_checkpoint,
                &journal.platform_record,
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
                TransitionKind::OwnerPublication => self
                    .platform
                    .wait_for_newer_owner(&transition.after.platform, &journal.platform_record),
                TransitionKind::Mutation => unreachable!("mutation handled below"),
            };
            check.map_err(|source| StepError::Effect(platform_error(action, source)))?;
            return self
                .require_state(
                    action,
                    &transition.before,
                    &transition.after,
                    transition.after_checkpoint,
                    &journal.platform_record,
                    lock,
                )
                .map_err(StepError::Fatal);
        }

        if after_matches {
            return Ok(());
        }
        if !before_matches {
            return Err(StepError::Fatal(state_drift(
                action,
                transition.before,
                transition.after,
                actual,
            )));
        }

        self.apply_effect(journal, action, &transition.after, lock)
            .map_err(StepError::Effect)?;
        self.require_state(
            action,
            &transition.before,
            &transition.after,
            transition.after_checkpoint,
            &journal.platform_record,
            lock,
        )
        .map_err(StepError::Fatal)
    }

    fn apply_effect(
        &mut self,
        journal: &InstallJournalV1,
        action: InstallAction,
        after: &InstallationState,
        lock: &super::store::InstallLock,
    ) -> Result<(), InstallCoordinatorError> {
        match action {
            InstallAction::UnloadPrior | InstallAction::UnloadCandidate => self
                .platform
                .unload(&journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::InstallCandidateLauncher => self
                .platform
                .install_launcher(Some(&journal.candidate_unit), &journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::SwitchToCandidate => self
                .store
                .set_active(Some(&journal.candidate_unit), lock)
                .map_err(InstallCoordinatorError::Store),
            InstallAction::ReloadCandidate | InstallAction::ReloadPrior => self
                .platform
                .restore_loaded_state(&after.platform, &journal.platform_record)
                .map_err(|source| platform_error(action, source)),
            InstallAction::RestorePriorActive => self
                .store
                .set_active(journal.prior_active_unit.as_ref(), lock)
                .map_err(InstallCoordinatorError::Store),
            InstallAction::RestorePriorLauncher => self
                .platform
                .install_launcher(
                    journal.prior_platform.launcher_unit.as_ref(),
                    &journal.platform_record,
                )
                .map_err(|source| platform_error(action, source)),
            InstallAction::ProveCandidate
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
        let action = if self.matches_checkpoint(
            &actual,
            &checkpoints.candidate_running,
            PlatformCheckpoint::CandidateRunning,
            &journal.platform_record,
        )? {
            InstallAction::UnloadCandidate
        } else if self.matches_checkpoint(
            &actual,
            &checkpoints.candidate_active,
            PlatformCheckpoint::CandidateActive,
            &journal.platform_record,
        )? {
            InstallAction::RestorePriorActive
        } else if self.matches_checkpoint(
            &actual,
            &checkpoints.candidate_launcher,
            PlatformCheckpoint::CandidateLauncher,
            &journal.platform_record,
        )? {
            InstallAction::RestorePriorLauncher
        } else if self.matches_checkpoint(
            &actual,
            &checkpoints.prior_quiescent,
            PlatformCheckpoint::PriorQuiescent,
            &journal.platform_record,
        )? {
            InstallAction::ReloadPrior
        } else if self.matches_checkpoint(
            &actual,
            &checkpoints.prior,
            PlatformCheckpoint::PriorRestored,
            &journal.platform_record,
        )? {
            InstallAction::ProvePrior
        } else if self.matches_checkpoint(
            &actual,
            &checkpoints.prior,
            PlatformCheckpoint::PriorOriginal,
            &journal.platform_record,
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
        record: &PlatformTransactionRecord,
        lock: &super::store::InstallLock,
    ) -> Result<(), InstallCoordinatorError> {
        let actual = self.inspect_state(lock)?;
        if self.matches_checkpoint(&actual, after, after_checkpoint, record)? {
            Ok(())
        } else {
            Err(state_drift(action, before.clone(), after.clone(), actual))
        }
    }

    fn matches_checkpoint(
        &mut self,
        actual: &InstallationState,
        expected: &InstallationState,
        checkpoint: PlatformCheckpoint,
        record: &PlatformTransactionRecord,
    ) -> Result<bool, InstallCoordinatorError> {
        if actual.active_unit != expected.active_unit || actual.platform != expected.platform {
            return Ok(false);
        }
        self.platform
            .matches_exact_state(checkpoint, &expected.platform, record)
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

impl Transition {
    fn for_action(
        journal: &InstallJournalV1,
        action: InstallAction,
    ) -> Result<Self, InstallCoordinatorError> {
        let checkpoints = Checkpoints::new(journal);
        let (before, after, kind) = match action {
            InstallAction::PreflightCandidate => (
                checkpoints.prior.clone(),
                checkpoints.prior,
                TransitionKind::Preflight,
            ),
            InstallAction::UnloadPrior => (
                checkpoints.prior,
                checkpoints.prior_quiescent,
                TransitionKind::Mutation,
            ),
            InstallAction::ProvePriorGuardReleased => (
                checkpoints.prior_quiescent.clone(),
                checkpoints.prior_quiescent,
                TransitionKind::GuardRelease,
            ),
            InstallAction::InstallCandidateLauncher => (
                checkpoints.prior_quiescent,
                checkpoints.candidate_launcher,
                TransitionKind::Mutation,
            ),
            InstallAction::SwitchToCandidate => (
                checkpoints.candidate_launcher,
                checkpoints.candidate_active,
                TransitionKind::Mutation,
            ),
            InstallAction::ReloadCandidate => (
                checkpoints.candidate_active,
                checkpoints.candidate_running,
                TransitionKind::Mutation,
            ),
            InstallAction::ProveCandidate => (
                checkpoints.candidate_running.clone(),
                checkpoints.candidate_running,
                TransitionKind::OwnerPublication,
            ),
            InstallAction::UnloadCandidate => (
                checkpoints.candidate_running,
                checkpoints.candidate_active,
                TransitionKind::Mutation,
            ),
            InstallAction::ProveCandidateGuardReleased => (
                checkpoints.candidate_active.clone(),
                checkpoints.candidate_active,
                TransitionKind::GuardRelease,
            ),
            InstallAction::RestorePriorActive => (
                checkpoints.candidate_active,
                checkpoints.candidate_launcher,
                TransitionKind::Mutation,
            ),
            InstallAction::RestorePriorLauncher => (
                checkpoints.candidate_launcher,
                checkpoints.prior_quiescent,
                TransitionKind::Mutation,
            ),
            InstallAction::ReloadPrior => (
                checkpoints.prior_quiescent,
                checkpoints.prior,
                TransitionKind::Mutation,
            ),
            InstallAction::ProvePrior => (
                checkpoints.prior.clone(),
                checkpoints.prior,
                TransitionKind::OwnerPublication,
            ),
            InstallAction::Commit | InstallAction::FinishRollback => {
                return Err(InstallCoordinatorError::InvalidAction(action));
            }
        };
        let (before_checkpoint, after_checkpoint) = match action {
            InstallAction::PreflightCandidate => (
                PlatformCheckpoint::PriorOriginal,
                PlatformCheckpoint::PriorOriginal,
            ),
            InstallAction::UnloadPrior => (
                PlatformCheckpoint::PriorOriginal,
                PlatformCheckpoint::PriorQuiescent,
            ),
            InstallAction::ProvePriorGuardReleased => (
                PlatformCheckpoint::PriorQuiescent,
                PlatformCheckpoint::PriorQuiescent,
            ),
            InstallAction::InstallCandidateLauncher => (
                PlatformCheckpoint::PriorQuiescent,
                PlatformCheckpoint::CandidateLauncher,
            ),
            InstallAction::SwitchToCandidate => (
                PlatformCheckpoint::CandidateLauncher,
                PlatformCheckpoint::CandidateActive,
            ),
            InstallAction::ReloadCandidate => (
                PlatformCheckpoint::CandidateActive,
                PlatformCheckpoint::CandidateRunning,
            ),
            InstallAction::ProveCandidate => (
                PlatformCheckpoint::CandidateRunning,
                PlatformCheckpoint::CandidateRunning,
            ),
            InstallAction::UnloadCandidate => (
                PlatformCheckpoint::CandidateRunning,
                PlatformCheckpoint::CandidateActive,
            ),
            InstallAction::ProveCandidateGuardReleased => (
                PlatformCheckpoint::CandidateActive,
                PlatformCheckpoint::CandidateActive,
            ),
            InstallAction::RestorePriorActive => (
                PlatformCheckpoint::CandidateActive,
                PlatformCheckpoint::CandidateLauncher,
            ),
            InstallAction::RestorePriorLauncher => (
                PlatformCheckpoint::CandidateLauncher,
                PlatformCheckpoint::PriorQuiescent,
            ),
            InstallAction::ReloadPrior => (
                PlatformCheckpoint::PriorQuiescent,
                PlatformCheckpoint::PriorRestored,
            ),
            InstallAction::ProvePrior => (
                PlatformCheckpoint::PriorRestored,
                PlatformCheckpoint::PriorRestored,
            ),
            InstallAction::Commit | InstallAction::FinishRollback => {
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
    prior_quiescent: InstallationState,
    candidate_launcher: InstallationState,
    candidate_active: InstallationState,
    candidate_running: InstallationState,
}

impl Checkpoints {
    fn new(journal: &InstallJournalV1) -> Self {
        let prior = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: journal.prior_platform.clone(),
        };
        let prior_quiescent = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: journal.prior_platform.quiescent(),
        };
        let candidate_launcher = InstallationState {
            active_unit: journal.prior_active_unit.clone(),
            platform: PlatformState {
                launcher_unit: Some(journal.candidate_unit.clone()),
                ..journal.prior_platform.quiescent()
            },
        };
        let candidate_active = InstallationState {
            active_unit: Some(journal.candidate_unit.clone()),
            platform: candidate_launcher.platform.clone(),
        };
        let candidate_running = InstallationState {
            active_unit: Some(journal.candidate_unit.clone()),
            platform: journal.target_platform.clone(),
        };
        Self {
            prior,
            prior_quiescent,
            candidate_launcher,
            candidate_active,
            candidate_running,
        }
    }
}

fn next_forward(action: InstallAction) -> Result<InstallAction, InstallCoordinatorError> {
    match action {
        InstallAction::PreflightCandidate => Ok(InstallAction::UnloadPrior),
        InstallAction::UnloadPrior => Ok(InstallAction::ProvePriorGuardReleased),
        InstallAction::ProvePriorGuardReleased => Ok(InstallAction::InstallCandidateLauncher),
        InstallAction::InstallCandidateLauncher => Ok(InstallAction::SwitchToCandidate),
        InstallAction::SwitchToCandidate => Ok(InstallAction::ReloadCandidate),
        InstallAction::ReloadCandidate => Ok(InstallAction::ProveCandidate),
        InstallAction::ProveCandidate => Ok(InstallAction::Commit),
        _ => Err(InstallCoordinatorError::InvalidAction(action)),
    }
}

fn next_rollback(action: InstallAction) -> Result<InstallAction, InstallCoordinatorError> {
    match action {
        InstallAction::UnloadCandidate => Ok(InstallAction::ProveCandidateGuardReleased),
        InstallAction::ProveCandidateGuardReleased => Ok(InstallAction::RestorePriorActive),
        InstallAction::RestorePriorActive => Ok(InstallAction::RestorePriorLauncher),
        InstallAction::RestorePriorLauncher => Ok(InstallAction::ReloadPrior),
        InstallAction::ReloadPrior => Ok(InstallAction::ProvePrior),
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
