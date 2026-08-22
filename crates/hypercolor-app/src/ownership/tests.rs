use std::time::Duration;

use super::executor::{
    AppOwnerStopAuthority, app_owner_stop_authority, hold_app_sidecar_supervisor,
    release_app_sidecar_supervisor, update_app_sidecar_gate_for_autostart,
};
use super::launchd::{launchctl_service_disabled, service_label};
use super::model::{
    MacosCaptureOwner, MacosCaptureOwnerRestartOutcome, MacosDaemonOwnerRemedyOutcome,
};
use super::planning::{
    MacosStartupRecoveryDisposition, app_sidecar_recovery_needs_rearm, apply_owner_choice_outcome,
    restart_capture_owner_with, startup_recovery_disposition,
};
use super::remediation::{complete_offline_remedy_with, execute_offline_remedy_with};
use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosHandoverOperation, MacosHandoverPhase, MacosOwnerCoordinatorOutcome,
    MacosOwnerExecutionError, MacosOwnerExecutor, MacosOwnerIdentity, MacosOwnerIncarnation,
    MacosOwnerRemedy, MacosOwnerStore,
};

use crate::supervisor::{MacosDaemonOwnerOfflineStatus, SupervisorState};

struct RestartFixtureExecutor {
    store: MacosOwnerStore,
    operations: Vec<MacosHandoverOperation>,
    stopped_incarnations: Vec<MacosOwnerIncarnation>,
    next_pid: u32,
    guard_released: bool,
}

impl RestartFixtureExecutor {
    fn new(store: MacosOwnerStore) -> Self {
        Self {
            store,
            operations: Vec::new(),
            stopped_incarnations: Vec::new(),
            next_pid: 1_000,
            guard_released: true,
        }
    }
}

impl MacosOwnerExecutor for RestartFixtureExecutor {
    fn autostart_enabled(
        &mut self,
        _owner: MacosDaemonOwner,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(false)
    }

    fn set_autostart(
        &mut self,
        _owner: MacosDaemonOwner,
        _enabled: bool,
    ) -> Result<(), MacosOwnerExecutionError> {
        Err(MacosOwnerExecutionError::new(
            "restart must not mutate autostart",
        ))
    }

    fn preflight_stop_authority(
        &mut self,
        _incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        Ok(())
    }

    fn flush_and_stop(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        self.stopped_incarnations.push(incarnation.clone());
        self.operations.push(match incarnation.owner {
            MacosDaemonOwner::AppSidecar => MacosHandoverOperation::FlushAndStopAppSidecar {},
            MacosDaemonOwner::DirectLaunchd => MacosHandoverOperation::FlushAndStopDirectLaunchd {},
            MacosDaemonOwner::Homebrew => MacosHandoverOperation::FlushAndStopHomebrew {},
            MacosDaemonOwner::Standalone => {
                return Err(MacosOwnerExecutionError::new(
                    "standalone restart must remain user-directed",
                ));
            }
        });
        Ok(())
    }

    fn start(&mut self, owner: MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError> {
        self.operations.push(match owner {
            MacosDaemonOwner::AppSidecar => MacosHandoverOperation::StartAppSidecar {},
            MacosDaemonOwner::DirectLaunchd => MacosHandoverOperation::StartDirectLaunchd {},
            MacosDaemonOwner::Homebrew => MacosHandoverOperation::StartHomebrew {},
            MacosDaemonOwner::Standalone => {
                return Err(MacosOwnerExecutionError::new(
                    "standalone restart must remain user-directed",
                ));
            }
        });
        self.next_pid += 1;
        self.store
            .publish_owner(
                owner,
                MacosOwnerIdentity::new(
                    "restart-audit",
                    "/fixture/hypercolor-daemon",
                    "restart-requirement",
                    self.next_pid,
                )
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?,
            )
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        Ok(())
    }

    fn wait_for_guard_release(
        &mut self,
        _timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(self.guard_released)
    }

    fn wait_for_owner(
        &mut self,
        owner: MacosDaemonOwner,
        after_epoch: u64,
        _timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(self.store.load_owner_record().is_ok_and(|record| {
            record.is_some_and(|record| {
                record.active_owner == owner && record.owner_epoch > after_epoch
            })
        }))
    }
}

fn restart_identity(pid: u32) -> MacosOwnerIdentity {
    MacosOwnerIdentity::new(
        "active-audit",
        "/fixture/active-hypercolor-daemon",
        "active-requirement",
        pid,
    )
    .expect("fixture identity should build")
}

#[test]
fn disabled_service_parser_is_exact_to_the_requested_label() {
    let output = r#"disabled services = {
        "tech.hyperbliss.hypercolor" => true
        "homebrew.mxcl.hypercolor" => false
    }"#;
    assert!(launchctl_service_disabled(
        output,
        "tech.hyperbliss.hypercolor"
    ));
    assert!(!launchctl_service_disabled(
        output,
        "homebrew.mxcl.hypercolor"
    ));
}

#[test]
fn app_sidecar_service_label_matches_tauri_product_name() {
    assert_eq!(
        service_label(MacosDaemonOwner::AppSidecar).expect("app label should resolve"),
        hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME
    );
}

#[test]
fn app_sidecar_gate_stays_held_until_explicit_start() {
    let state = SupervisorState::default();
    update_app_sidecar_gate_for_autostart(&state, true);
    assert!(!state.owner_handover_stop());

    hold_app_sidecar_supervisor(&state);
    update_app_sidecar_gate_for_autostart(&state, true);
    assert!(state.owner_handover_stop());

    release_app_sidecar_supervisor(&state);
    assert!(!state.owner_handover_stop());
    update_app_sidecar_gate_for_autostart(&state, false);
    assert!(state.owner_handover_stop());
}

#[test]
fn requested_app_sidecar_recovery_rearms_the_supervisor() {
    assert!(app_sidecar_recovery_needs_rearm(
        MacosDaemonOwner::AppSidecar,
        MacosHandoverPhase::RequestedOwnerStarted,
    ));
    assert!(!app_sidecar_recovery_needs_rearm(
        MacosDaemonOwner::AppSidecar,
        MacosHandoverPhase::StartRequested,
    ));
    assert!(!app_sidecar_recovery_needs_rearm(
        MacosDaemonOwner::DirectLaunchd,
        MacosHandoverPhase::RequestedOwnerStarted,
    ));
}

#[test]
fn newer_prior_app_sidecar_rollback_releases_only_proven_terminal_gate() {
    let state = SupervisorState::default();
    hold_app_sidecar_supervisor(&state);

    apply_owner_choice_outcome(
        &state,
        &MacosOwnerCoordinatorOutcome::RecoveryRequired {
            requested_owner: MacosDaemonOwner::DirectLaunchd,
            prior_owner: MacosDaemonOwner::AppSidecar,
            phase: hypercolor_macos_owner::MacosHandoverPhase::RollbackAutostartsRestored,
        },
    );
    assert!(state.owner_handover_stop());

    apply_owner_choice_outcome(
        &state,
        &MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: MacosDaemonOwner::DirectLaunchd,
            failure: "fixture rollback".to_owned(),
        },
    );
    assert!(state.owner_handover_stop());

    apply_owner_choice_outcome(
        &state,
        &MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: MacosDaemonOwner::AppSidecar,
            failure: "newer prior publication restored ownership".to_owned(),
        },
    );
    assert!(!state.owner_handover_stop());
}

#[test]
fn managed_topologies_resolve_only_their_exact_stop_authority() {
    assert_eq!(
        app_owner_stop_authority(MacosDaemonOwner::AppSidecar, "501")
            .expect("app sidecar authority should resolve"),
        AppOwnerStopAuthority::SupervisorChild
    );
    assert_eq!(
        app_owner_stop_authority(MacosDaemonOwner::DirectLaunchd, "501")
            .expect("launchd authority should resolve"),
        AppOwnerStopAuthority::LaunchctlService("gui/501/tech.hyperbliss.hypercolor".to_owned())
    );
    assert_eq!(
        app_owner_stop_authority(MacosDaemonOwner::Homebrew, "501")
            .expect("Homebrew authority should resolve"),
        AppOwnerStopAuthority::HomebrewService("hypercolor")
    );
}

#[test]
fn exact_offline_remedy_clears_only_after_new_healthy_owner_epoch() {
    let state = SupervisorState::default();
    let status = MacosDaemonOwnerOfflineStatus {
        code: "macos_daemon_owner_offline",
        selected_owner: MacosDaemonOwner::DirectLaunchd,
        remedy: MacosOwnerRemedy::StartLaunchdService,
    };
    state.set_macos_owner_offline(Some(status));

    let pending =
        execute_offline_remedy_with(&state, MacosOwnerRemedy::StartLaunchdService, 7, |owner| {
            assert_eq!(owner, MacosDaemonOwner::DirectLaunchd);
            Ok(())
        })
        .expect("matching remedy should start");

    assert_eq!(pending.status, status);
    assert_eq!(pending.owner, MacosDaemonOwner::DirectLaunchd);
    assert_eq!(pending.after_epoch, 7);
    assert_eq!(state.macos_owner_offline(), Some(status));
    assert!(complete_offline_remedy_with(&state, pending, false).is_err());
    assert_eq!(state.macos_owner_offline(), Some(status));
    let outcome = complete_offline_remedy_with(&state, pending, true)
        .expect("new authoritative owner epoch should complete the remedy");
    assert_eq!(
        outcome,
        MacosDaemonOwnerRemedyOutcome::Started {
            owner: MacosDaemonOwner::DirectLaunchd
        }
    );
    assert_eq!(state.macos_owner_offline(), None);
}

#[test]
fn failed_or_stale_offline_remedy_preserves_status() {
    let state = SupervisorState::default();
    let status = MacosDaemonOwnerOfflineStatus {
        code: "macos_daemon_owner_offline",
        selected_owner: MacosDaemonOwner::Homebrew,
        remedy: MacosOwnerRemedy::StartHomebrewService,
    };
    state.set_macos_owner_offline(Some(status));

    assert!(
        execute_offline_remedy_with(
            &state,
            MacosOwnerRemedy::StartLaunchdService,
            7,
            |_| panic!("stale remedy must not execute"),
        )
        .is_err()
    );
    assert_eq!(state.macos_owner_offline(), Some(status));
    assert!(
        execute_offline_remedy_with(&state, MacosOwnerRemedy::StartHomebrewService, 7, |_| Err(
            MacosOwnerExecutionError::new("injected start failure")
        ),)
        .is_err()
    );
    assert_eq!(state.macos_owner_offline(), Some(status));
}

#[test]
fn capture_owner_restart_revalidates_and_publishes_a_new_epoch() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let record = store
        .publish_owner(MacosDaemonOwner::DirectLaunchd, restart_identity(42))
        .expect("fixture owner should publish");
    let mut executor = RestartFixtureExecutor::new(store.clone());

    let outcome = restart_capture_owner_with(
        &store,
        &mut executor,
        record.active_owner,
        record.owner_epoch,
    )
    .expect("managed owner should restart");

    assert_eq!(
        outcome,
        MacosCaptureOwnerRestartOutcome::Restarted {
            owner: MacosCaptureOwner::LaunchdService,
            previous_owner_epoch: 1,
            owner_epoch: 2,
        }
    );
    assert_eq!(
        executor.operations,
        [
            MacosHandoverOperation::FlushAndStopDirectLaunchd {},
            MacosHandoverOperation::StartDirectLaunchd {},
        ]
    );
    assert_eq!(executor.stopped_incarnations, [record.incarnation()]);
}

#[test]
fn capture_owner_restart_rejects_stale_epoch_and_wrong_owner_without_mutation() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let record = store
        .publish_owner(MacosDaemonOwner::Homebrew, restart_identity(42))
        .expect("fixture owner should publish");
    let mut executor = RestartFixtureExecutor::new(store.clone());

    assert!(
        restart_capture_owner_with(
            &store,
            &mut executor,
            MacosDaemonOwner::Homebrew,
            record.owner_epoch + 1,
        )
        .is_err()
    );
    assert!(
        restart_capture_owner_with(
            &store,
            &mut executor,
            MacosDaemonOwner::DirectLaunchd,
            record.owner_epoch,
        )
        .is_err()
    );
    assert!(executor.operations.is_empty());
}

#[test]
fn failed_app_sidecar_restart_rearms_the_supervisor_after_stop() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let record = store
        .publish_owner(MacosDaemonOwner::AppSidecar, restart_identity(42))
        .expect("fixture owner should publish");
    let mut executor = RestartFixtureExecutor::new(store.clone());
    executor.guard_released = false;

    assert!(
        restart_capture_owner_with(
            &store,
            &mut executor,
            record.active_owner,
            record.owner_epoch,
        )
        .is_err()
    );
    assert_eq!(
        executor.operations,
        [
            MacosHandoverOperation::FlushAndStopAppSidecar {},
            MacosHandoverOperation::StartAppSidecar {},
        ]
    );
    assert!(
        store
            .load_owner_record()
            .expect("owner record should load")
            .is_some_and(|current| {
                current.active_owner == MacosDaemonOwner::AppSidecar
                    && current.owner_epoch > record.owner_epoch
            })
    );
}

#[test]
fn standalone_capture_owner_restart_returns_typed_user_remedy() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let record = store
        .publish_owner(MacosDaemonOwner::Standalone, restart_identity(77))
        .expect("fixture owner should publish");
    let mut executor = RestartFixtureExecutor::new(store.clone());

    let outcome = restart_capture_owner_with(
        &store,
        &mut executor,
        record.active_owner,
        record.owner_epoch,
    )
    .expect("standalone owner should return a local remedy");

    assert_eq!(
        outcome,
        MacosCaptureOwnerRestartOutcome::UserActionRequired {
            owner: MacosCaptureOwner::Standalone,
            owner_epoch: 1,
            remedy: MacosOwnerRemedy::RestartStandalone { pid: 77 },
        }
    );
    assert_eq!(
        serde_json::to_value(outcome).expect("restart outcome should serialize"),
        serde_json::json!({
            "status": "user_action_required",
            "owner": "standalone",
            "owner_epoch": 1,
            "remedy": {
                "kind": "restart_standalone",
                "pid": 77
            }
        })
    );
    assert!(executor.operations.is_empty());
}

#[test]
fn startup_recovery_suppresses_normal_watchdog_until_pending_standalone_exits() {
    let pending = MacosOwnerCoordinatorOutcome::PendingStandalone {
        requested_owner: MacosDaemonOwner::Homebrew,
        remedy: MacosOwnerRemedy::StopStandaloneOwner { pid: 42 },
    };
    assert_eq!(
        startup_recovery_disposition(Some(&pending), false),
        MacosStartupRecoveryDisposition::SuppressSupervisor
    );
    assert_eq!(
        startup_recovery_disposition(None, false),
        MacosStartupRecoveryDisposition::Continue
    );
    assert_eq!(
        startup_recovery_disposition(Some(&pending), true),
        MacosStartupRecoveryDisposition::SupervisorStarted
    );
}
