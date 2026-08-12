use std::collections::VecDeque;
use std::fs;
use std::time::Duration;

use hypercolor_macos_owner::{
    MACOS_MANAGED_HANDOVER_TIMEOUT, MACOS_STANDALONE_HANDOVER_TIMEOUT, MacosDaemonOwner,
    MacosHandoverJournal, MacosHandoverOperation, MacosHandoverPhase, MacosHandoverTransactionId,
    MacosOwnerCoordinatorOutcome, MacosOwnerExecutionError, MacosOwnerExecutor, MacosOwnerIdentity,
    MacosOwnerRemedy, MacosOwnerStore, choose_daemon_owner, recover_daemon_owner,
    recover_incoming_daemon_owner,
};

fn identity(label: &str, pid: u32) -> MacosOwnerIdentity {
    MacosOwnerIdentity::new(
        format!("audit-{label}"),
        format!("/Applications/{label}/hypercolor-daemon"),
        format!("requirement-{label}"),
        pid,
    )
    .expect("fixture identity should be valid")
}

fn transaction(label: &str) -> MacosHandoverTransactionId {
    MacosHandoverTransactionId::new(label).expect("fixture transaction should be valid")
}

struct FixtureExecutor {
    store: MacosOwnerStore,
    autostarts: [bool; 3],
    operations: Vec<MacosHandoverOperation>,
    guard_results: VecDeque<bool>,
    owner_results: VecDeque<bool>,
    fail_operation: Option<MacosHandoverOperation>,
    next_pid: u32,
    incoming_recovers_on_start: bool,
}

impl FixtureExecutor {
    fn new(store: MacosOwnerStore) -> Self {
        Self {
            store,
            autostarts: [true, false, false],
            operations: Vec::new(),
            guard_results: VecDeque::from([true]),
            owner_results: VecDeque::from([true, true]),
            fail_operation: None,
            next_pid: 1_000,
            incoming_recovers_on_start: false,
        }
    }

    fn index(owner: MacosDaemonOwner) -> Result<usize, MacosOwnerExecutionError> {
        match owner {
            MacosDaemonOwner::AppSidecar => Ok(0),
            MacosDaemonOwner::DirectLaunchd => Ok(1),
            MacosDaemonOwner::Homebrew => Ok(2),
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone has no autostart state",
            )),
        }
    }

    fn push(&mut self, operation: MacosHandoverOperation) -> Result<(), MacosOwnerExecutionError> {
        self.operations.push(operation);
        if self.fail_operation == Some(operation) {
            Err(MacosOwnerExecutionError::new("injected operation failure"))
        } else {
            Ok(())
        }
    }
}

impl MacosOwnerExecutor for FixtureExecutor {
    fn autostart_enabled(
        &mut self,
        owner: MacosDaemonOwner,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(self.autostarts[Self::index(owner)?])
    }

    fn set_autostart(
        &mut self,
        owner: MacosDaemonOwner,
        enabled: bool,
    ) -> Result<(), MacosOwnerExecutionError> {
        let operation = match owner {
            MacosDaemonOwner::AppSidecar => {
                MacosHandoverOperation::SetAppSidecarAutostart { enabled }
            }
            MacosDaemonOwner::DirectLaunchd => {
                MacosHandoverOperation::SetDirectLaunchdAutostart { enabled }
            }
            MacosDaemonOwner::Homebrew => MacosHandoverOperation::SetHomebrewAutostart { enabled },
            MacosDaemonOwner::Standalone => {
                return Err(MacosOwnerExecutionError::new(
                    "standalone has no autostart state",
                ));
            }
        };
        self.push(operation)?;
        self.autostarts[Self::index(owner)?] = enabled;
        Ok(())
    }

    fn flush_and_stop(
        &mut self,
        owner: MacosDaemonOwner,
        _pid: Option<u32>,
    ) -> Result<(), MacosOwnerExecutionError> {
        let operation = match owner {
            MacosDaemonOwner::AppSidecar => MacosHandoverOperation::FlushAndStopAppSidecar {},
            MacosDaemonOwner::DirectLaunchd => MacosHandoverOperation::FlushAndStopDirectLaunchd {},
            MacosDaemonOwner::Homebrew => MacosHandoverOperation::FlushAndStopHomebrew {},
            MacosDaemonOwner::Standalone => {
                return Err(MacosOwnerExecutionError::new(
                    "standalone cannot be stopped remotely",
                ));
            }
        };
        self.push(operation)
    }

    fn start(&mut self, owner: MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError> {
        let operation = match owner {
            MacosDaemonOwner::AppSidecar => MacosHandoverOperation::StartAppSidecar {},
            MacosDaemonOwner::DirectLaunchd => MacosHandoverOperation::StartDirectLaunchd {},
            MacosDaemonOwner::Homebrew => MacosHandoverOperation::StartHomebrew {},
            MacosDaemonOwner::Standalone => {
                return Err(MacosOwnerExecutionError::new(
                    "standalone cannot be started by a launcher",
                ));
            }
        };
        self.push(operation)?;
        if self.incoming_recovers_on_start {
            self.next_pid += 1;
            self.store
                .publish_owner(owner, identity("racing-incoming", self.next_pid))
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            recover_incoming_daemon_owner(&self.store, owner)
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        }
        Ok(())
    }

    fn wait_for_guard_release(
        &mut self,
        _pid: u32,
        _timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(self.guard_results.pop_front().unwrap_or(true))
    }

    fn wait_for_owner(
        &mut self,
        owner: MacosDaemonOwner,
        _after_epoch: u64,
        _timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let result = self.owner_results.pop_front().unwrap_or(true);
        if result {
            self.next_pid += 1;
            self.store
                .publish_owner(owner, identity("incoming", self.next_pid))
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        }
        Ok(result)
    }
}

#[test]
fn managed_handover_is_synchronous_and_commits_every_forward_phase() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());

    let outcome = choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::DirectLaunchd,
        transaction("managed-success"),
    )
    .expect("handover should succeed");

    assert!(matches!(
        outcome,
        MacosOwnerCoordinatorOutcome::Active {
            owner: MacosDaemonOwner::DirectLaunchd,
            ..
        }
    ));
    assert_eq!(executor.autostarts, [false, true, false]);
    let journal = store
        .load_handover_journal()
        .expect("journal should load")
        .expect("journal should exist");
    assert_eq!(journal.phase, MacosHandoverPhase::Committed);
    assert_eq!(journal.journal_revision, 10);
    assert_eq!(
        store
            .load_owner_record()
            .expect("owner should load")
            .expect("owner should exist")
            .selected_external_owner,
        Some(hypercolor_macos_owner::MacosExternalOwnerMode::DirectLaunchd)
    );
}

#[test]
fn same_owner_choice_journals_competing_autostart_reconciliation() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let record = store
        .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("direct", 101))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());
    executor.autostarts = [true, true, true];

    let outcome = choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::DirectLaunchd,
        transaction("same-owner-reconcile"),
    )
    .expect("same-owner reconciliation should commit");

    assert_eq!(
        outcome,
        MacosOwnerCoordinatorOutcome::Active {
            owner: MacosDaemonOwner::DirectLaunchd,
            owner_epoch: record.owner_epoch,
        }
    );
    assert_eq!(executor.autostarts, [false, true, false]);
    assert_eq!(
        executor.operations,
        [
            MacosHandoverOperation::SetAppSidecarAutostart { enabled: false },
            MacosHandoverOperation::SetDirectLaunchdAutostart { enabled: true },
            MacosHandoverOperation::SetHomebrewAutostart { enabled: false },
        ]
    );
    let journal = store
        .load_handover_journal()
        .expect("journal should load")
        .expect("same-owner reconciliation should be journaled");
    assert_eq!(journal.phase, MacosHandoverPhase::Committed);
    assert_eq!(journal.journal_revision, 4);
    let record = store
        .load_owner_record()
        .expect("owner record should load")
        .expect("owner record should remain present");
    assert_eq!(
        record.selected_external_owner,
        Some(hypercolor_macos_owner::MacosExternalOwnerMode::DirectLaunchd)
    );
}

#[test]
fn failed_start_runs_the_complete_rollback_and_restores_prior_mode() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());
    executor.fail_operation = Some(MacosHandoverOperation::StartDirectLaunchd {});

    let outcome = choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::DirectLaunchd,
        transaction("managed-rollback"),
    )
    .expect("rollback should complete");

    assert!(matches!(
        outcome,
        MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: MacosDaemonOwner::AppSidecar,
            ..
        }
    ));
    assert_eq!(executor.autostarts, [true, false, false]);
    let journal = store
        .load_handover_journal()
        .expect("journal should load")
        .expect("journal should exist");
    assert_eq!(journal.phase, MacosHandoverPhase::RolledBack);
    assert_eq!(journal.journal_revision, 17);
}

#[test]
fn rollback_preserves_an_all_disabled_launcher_state() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());
    executor.autostarts = [false, false, false];
    executor.fail_operation = Some(MacosHandoverOperation::StartDirectLaunchd {});

    let outcome = choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::DirectLaunchd,
        transaction("all-disabled-rollback"),
    )
    .expect("rollback should complete");

    assert!(matches!(
        outcome,
        MacosOwnerCoordinatorOutcome::RolledBack { .. }
    ));
    assert_eq!(executor.autostarts, [false, false, false]);
}

#[test]
fn incoming_daemon_and_surviving_coordinator_converge_without_phase_regression() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());
    executor.incoming_recovers_on_start = true;

    let outcome = choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::DirectLaunchd,
        transaction("racing-incoming"),
    )
    .expect("both recovery participants should converge");

    assert!(matches!(
        outcome,
        MacosOwnerCoordinatorOutcome::Active {
            owner: MacosDaemonOwner::DirectLaunchd,
            ..
        }
    ));
    assert_eq!(
        store
            .load_handover_journal()
            .expect("journal should load")
            .expect("journal should exist")
            .phase,
        MacosHandoverPhase::Committed
    );
}

#[test]
fn standalone_handover_never_mutates_autostart_before_user_exit() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::Standalone, identity("standalone", 4242))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());
    executor.guard_results = VecDeque::from([false]);

    let outcome = choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::Homebrew,
        transaction("standalone-pending"),
    )
    .expect("pending handover should be typed");

    assert_eq!(executor.autostarts, [true, false, false]);
    assert!(executor.operations.is_empty());
    assert_eq!(
        outcome,
        MacosOwnerCoordinatorOutcome::PendingStandalone {
            requested_owner: MacosDaemonOwner::Homebrew,
            remedy: MacosOwnerRemedy::StopStandaloneOwner { pid: 4242 },
        }
    );
    assert_eq!(
        store
            .load_handover_journal()
            .expect("journal should load")
            .expect("journal should exist")
            .phase,
        MacosHandoverPhase::AwaitingGuardRelease
    );
}

#[test]
fn standalone_pending_resumes_after_native_guard_notification() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::Standalone, identity("standalone", 4242))
        .expect("initial owner should publish");
    let mut executor = FixtureExecutor::new(store.clone());
    executor.guard_results = VecDeque::from([false]);
    choose_daemon_owner(
        &store,
        &mut executor,
        MacosDaemonOwner::Homebrew,
        transaction("standalone-resume"),
    )
    .expect("first invocation should remain pending");

    executor.guard_results = VecDeque::from([true]);
    let outcome = recover_daemon_owner(&store, &mut executor)
        .expect("recovery should succeed")
        .expect("pending journal should recover");

    assert!(matches!(
        outcome,
        MacosOwnerCoordinatorOutcome::Active {
            owner: MacosDaemonOwner::Homebrew,
            ..
        }
    ));
    assert_eq!(executor.autostarts, [false, false, true]);
}

#[test]
fn incoming_daemon_only_commits_its_matching_journal_role() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let prior = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let journal = MacosHandoverJournal::for_owner_choice(
        transaction("incoming-recovery"),
        MacosDaemonOwner::DirectLaunchd,
        &prior,
        hypercolor_macos_owner::MacosAutostartStates::new(true, false, false),
    )
    .expect("journal should build");
    let journal = store.begin_handover(journal).expect("journal should begin");
    store
        .advance_handover(&journal.transaction_id, MacosHandoverPhase::StartRequested)
        .expect("start request should persist");
    store
        .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("launchd", 202))
        .expect("incoming owner should publish");

    let outcome = recover_incoming_daemon_owner(&store, MacosDaemonOwner::DirectLaunchd)
        .expect("incoming recovery should succeed")
        .expect("journal should reconcile");
    assert!(matches!(
        outcome,
        MacosOwnerCoordinatorOutcome::Active {
            owner: MacosDaemonOwner::DirectLaunchd,
            ..
        }
    ));

    let unrelated = recover_incoming_daemon_owner(&store, MacosDaemonOwner::Homebrew)
        .expect("terminal journal should be inert");
    assert!(unrelated.is_none());
}

#[test]
fn requested_incoming_daemon_completes_every_applicable_forward_phase() {
    for (index, phase) in [
        MacosHandoverPhase::AutostartsConfigured,
        MacosHandoverPhase::StopRequested,
        MacosHandoverPhase::OutgoingOwnerStopped,
        MacosHandoverPhase::AwaitingGuardRelease,
        MacosHandoverPhase::GuardReleased,
        MacosHandoverPhase::StartRequested,
        MacosHandoverPhase::RequestedOwnerStarted,
        MacosHandoverPhase::CommitPending,
    ]
    .into_iter()
    .enumerate()
    {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let prior = store
            .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
            .expect("initial owner should publish");
        let journal = MacosHandoverJournal::for_owner_choice(
            transaction(&format!("incoming-forward-{index}")),
            MacosDaemonOwner::DirectLaunchd,
            &prior,
            hypercolor_macos_owner::MacosAutostartStates::new(true, false, false),
        )
        .expect("journal should build");
        let journal = store.begin_handover(journal).expect("journal should begin");
        store
            .advance_handover(&journal.transaction_id, phase)
            .expect("fixture phase should persist");
        store
            .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("launchd", 202))
            .expect("requested incoming owner should publish");

        let outcome = recover_incoming_daemon_owner(&store, MacosDaemonOwner::DirectLaunchd)
            .expect("incoming recovery should succeed")
            .expect("journal should reconcile");
        assert!(
            matches!(
                outcome,
                MacosOwnerCoordinatorOutcome::Active {
                    owner: MacosDaemonOwner::DirectLaunchd,
                    ..
                }
            ),
            "phase {phase:?} should commit for the active requested owner"
        );
        assert_eq!(
            store
                .load_handover_journal()
                .expect("journal should load")
                .expect("journal should exist")
                .phase,
            MacosHandoverPhase::Committed
        );
    }
}

#[test]
fn incoming_requested_owner_does_not_skip_unconfigured_standalone_autostarts() {
    for phase in [
        MacosHandoverPhase::Prepared,
        MacosHandoverPhase::AwaitingGuardRelease,
        MacosHandoverPhase::GuardReleased,
    ] {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let prior = store
            .publish_owner(MacosDaemonOwner::Standalone, identity("standalone", 101))
            .expect("initial owner should publish");
        let journal = MacosHandoverJournal::for_owner_choice(
            transaction(&format!("incoming-standalone-{phase:?}")),
            MacosDaemonOwner::DirectLaunchd,
            &prior,
            hypercolor_macos_owner::MacosAutostartStates::new(false, false, false),
        )
        .expect("journal should build");
        let journal = store.begin_handover(journal).expect("journal should begin");
        if phase != MacosHandoverPhase::Prepared {
            store
                .advance_handover(&journal.transaction_id, phase)
                .expect("fixture phase should persist");
        }
        store
            .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("launchd", 202))
            .expect("requested incoming owner should publish");

        let outcome = recover_incoming_daemon_owner(&store, MacosDaemonOwner::DirectLaunchd)
            .expect("incoming recovery should inspect the journal")
            .expect("journal should require coordinator recovery");
        assert_eq!(
            outcome,
            MacosOwnerCoordinatorOutcome::RecoveryRequired {
                requested_owner: MacosDaemonOwner::DirectLaunchd,
                prior_owner: MacosDaemonOwner::Standalone,
                phase,
            }
        );
    }
}

#[test]
fn unrelated_incoming_daemon_returns_path_free_recovery_status() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let prior = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let journal = MacosHandoverJournal::for_owner_choice(
        transaction("unrelated-incoming"),
        MacosDaemonOwner::DirectLaunchd,
        &prior,
        hypercolor_macos_owner::MacosAutostartStates::new(false, false, false),
    )
    .expect("journal should build");
    store.begin_handover(journal).expect("journal should begin");
    store
        .publish_owner(MacosDaemonOwner::Homebrew, identity("homebrew", 303))
        .expect("unrelated incoming owner should publish");

    let outcome = recover_incoming_daemon_owner(&store, MacosDaemonOwner::Homebrew)
        .expect("incoming recovery should inspect the journal")
        .expect("nonterminal journal should publish recovery status");
    assert_eq!(
        outcome,
        MacosOwnerCoordinatorOutcome::RecoveryRequired {
            requested_owner: MacosDaemonOwner::DirectLaunchd,
            prior_owner: MacosDaemonOwner::AppSidecar,
            phase: MacosHandoverPhase::Prepared,
        }
    );
    let encoded = serde_json::to_string(&outcome).expect("recovery status should serialize");
    assert!(!encoded.contains("Applications"));
    assert!(!encoded.contains("executable"));
}

#[test]
fn timeout_contracts_remain_ten_and_sixty_seconds() {
    assert_eq!(MACOS_MANAGED_HANDOVER_TIMEOUT, Duration::from_secs(10));
    assert_eq!(MACOS_STANDALONE_HANDOVER_TIMEOUT, Duration::from_mins(1));
}

#[test]
fn every_nonterminal_phase_recovers_to_one_viable_owner() {
    for (index, phase) in MacosHandoverPhase::ALL.into_iter().enumerate() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let prior = store
            .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
            .expect("initial owner should publish");
        let journal = MacosHandoverJournal::for_owner_choice(
            transaction(&format!("recover-phase-{index}")),
            MacosDaemonOwner::DirectLaunchd,
            &prior,
            hypercolor_macos_owner::MacosAutostartStates::new(true, false, false),
        )
        .expect("journal should build");
        let journal = store.begin_handover(journal).expect("journal should begin");
        if phase != MacosHandoverPhase::Prepared {
            store
                .advance_handover(&journal.transaction_id, phase)
                .expect("fixture phase should persist");
        }

        if matches!(
            phase,
            MacosHandoverPhase::RequestedOwnerStarted
                | MacosHandoverPhase::CommitPending
                | MacosHandoverPhase::RollbackPending
                | MacosHandoverPhase::RollbackAutostartsRestored
                | MacosHandoverPhase::RollbackStopRequested
                | MacosHandoverPhase::RollbackOwnerStopped
                | MacosHandoverPhase::RollbackAwaitingGuardRelease
                | MacosHandoverPhase::RollbackGuardReleased
                | MacosHandoverPhase::RollbackStartRequested
        ) {
            store
                .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("direct", 202))
                .expect("requested owner fixture should publish");
        }

        let mut executor = FixtureExecutor::new(store.clone());
        executor.autostarts = if phase == MacosHandoverPhase::Prepared {
            [true, false, false]
        } else if matches!(
            phase,
            MacosHandoverPhase::AutostartsConfigured
                | MacosHandoverPhase::StopRequested
                | MacosHandoverPhase::OutgoingOwnerStopped
                | MacosHandoverPhase::AwaitingGuardRelease
                | MacosHandoverPhase::GuardReleased
                | MacosHandoverPhase::StartRequested
                | MacosHandoverPhase::RequestedOwnerStarted
                | MacosHandoverPhase::CommitPending
                | MacosHandoverPhase::RollbackPending
        ) {
            [false, true, false]
        } else {
            [true, false, false]
        };

        let recovered =
            recover_daemon_owner(&store, &mut executor).expect("phase recovery should not fail");
        if phase.is_terminal() {
            assert!(
                recovered.is_none(),
                "terminal phase {phase:?} must be inert"
            );
            continue;
        }
        let outcome = recovered.expect("nonterminal phase should recover");
        if matches!(
            phase,
            MacosHandoverPhase::RollbackPending
                | MacosHandoverPhase::RollbackAutostartsRestored
                | MacosHandoverPhase::RollbackStopRequested
                | MacosHandoverPhase::RollbackOwnerStopped
                | MacosHandoverPhase::RollbackAwaitingGuardRelease
                | MacosHandoverPhase::RollbackGuardReleased
                | MacosHandoverPhase::RollbackStartRequested
                | MacosHandoverPhase::PriorOwnerStarted
                | MacosHandoverPhase::RollbackCommitPending
        ) {
            assert!(matches!(
                outcome,
                MacosOwnerCoordinatorOutcome::RolledBack {
                    prior_owner: MacosDaemonOwner::AppSidecar,
                    ..
                }
            ));
        } else {
            assert!(matches!(
                outcome,
                MacosOwnerCoordinatorOutcome::Active {
                    owner: MacosDaemonOwner::DirectLaunchd,
                    ..
                }
            ));
        }
    }
}

#[test]
fn stop_and_guard_failures_enter_durable_rollback() {
    for (label, failure, guard_results) in [
        (
            "stop-failure",
            Some(MacosHandoverOperation::FlushAndStopAppSidecar {}),
            VecDeque::from([true]),
        ),
        ("guard-timeout", None, VecDeque::from([false, true])),
    ] {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        store
            .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
            .expect("initial owner should publish");
        let mut executor = FixtureExecutor::new(store.clone());
        executor.fail_operation = failure;
        executor.guard_results = guard_results;

        let outcome = choose_daemon_owner(
            &store,
            &mut executor,
            MacosDaemonOwner::DirectLaunchd,
            transaction(label),
        )
        .expect("rollback should complete");

        assert!(matches!(
            outcome,
            MacosOwnerCoordinatorOutcome::RolledBack {
                prior_owner: MacosDaemonOwner::AppSidecar,
                ..
            }
        ));
        assert_eq!(
            store
                .load_handover_journal()
                .expect("journal should load")
                .expect("journal should exist")
                .phase,
            MacosHandoverPhase::RolledBack
        );
    }
}

#[test]
fn forward_operation_payloads_and_oversized_lists_are_rejected() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let prior = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let journal = MacosHandoverJournal::for_owner_choice(
        transaction("forward-payload"),
        MacosDaemonOwner::DirectLaunchd,
        &prior,
        hypercolor_macos_owner::MacosAutostartStates::new(true, false, false),
    )
    .expect("journal should build");
    store.begin_handover(journal).expect("journal should begin");
    let mut value: serde_json::Value = serde_json::from_slice(
        &fs::read(store.handover_journal_path()).expect("journal should read"),
    )
    .expect("journal should decode as JSON");
    value["allowed_forward_operations"] = serde_json::json!([{
        "kind": "start_direct_launchd",
        "command": "/bin/sh",
        "argv": ["-c", "forbidden"],
        "executable_path": "/tmp/forbidden"
    }]);
    fs::write(
        store.handover_journal_path(),
        serde_json::to_vec(&value).expect("fixture should encode"),
    )
    .expect("fixture should write");
    assert!(store.load_handover_journal().is_err());

    value["allowed_forward_operations"] = serde_json::Value::Array(
        (0..=hypercolor_macos_owner::MAX_MACOS_HANDOVER_OPERATIONS)
            .map(|_| serde_json::json!({ "kind": "start_direct_launchd" }))
            .collect(),
    );
    fs::write(
        store.handover_journal_path(),
        serde_json::to_vec(&value).expect("fixture should encode"),
    )
    .expect("fixture should write");
    assert!(store.load_handover_journal().is_err());
}

#[test]
fn conditional_phase_advance_cannot_regress_concurrent_recovery() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let prior = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let journal = MacosHandoverJournal::for_owner_choice(
        transaction("phase-cas"),
        MacosDaemonOwner::DirectLaunchd,
        &prior,
        hypercolor_macos_owner::MacosAutostartStates::new(true, false, false),
    )
    .expect("journal should build");
    let journal = store.begin_handover(journal).expect("journal should begin");
    store
        .advance_handover_from(
            &journal.transaction_id,
            MacosHandoverPhase::Prepared,
            MacosHandoverPhase::AutostartsConfigured,
        )
        .expect("first participant should advance");

    assert!(matches!(
        store.advance_handover_from(
            &journal.transaction_id,
            MacosHandoverPhase::Prepared,
            MacosHandoverPhase::RollbackPending,
        ),
        Err(
            hypercolor_macos_owner::MacosOwnerStoreError::HandoverPhaseChanged {
                expected: MacosHandoverPhase::Prepared,
                found: MacosHandoverPhase::AutostartsConfigured,
            }
        )
    ));
    assert_eq!(
        store
            .load_handover_journal()
            .expect("journal should load")
            .expect("journal should exist")
            .phase,
        MacosHandoverPhase::AutostartsConfigured
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_guard_waiter_reacquires_only_after_final_guard_release() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let guard_path = directory.path().join("daemon.lock");
    let guard_name = guard_path.to_string_lossy().into_owned();
    let winner =
        single_instance::SingleInstance::new(&guard_name).expect("winner guard should open");
    assert!(winner.is_single());
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let waiter_name = guard_name.clone();
    let waiter = std::thread::spawn(move || {
        sender
            .send(hypercolor_macos_owner::acquire_macos_daemon_guard(
                &waiter_name,
            ))
            .expect("guard result should send");
    });

    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    drop(winner);
    let reacquired = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("waiter should observe guard release")
        .expect("waiter should acquire the guard");
    let contender =
        single_instance::SingleInstance::new(&guard_name).expect("contender guard should open");
    assert!(!contender.is_single());
    drop(reacquired);
    drop(contender);
    waiter.join().expect("guard waiter should finish");
    assert!(
        single_instance::SingleInstance::new(&guard_name)
            .expect("final guard should open")
            .is_single()
    );
}
