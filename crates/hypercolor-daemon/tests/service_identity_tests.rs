#![cfg(target_os = "macos")]

use hypercolor_daemon::macos_owner::{
    MacosDaemonOwner, MacosHandoverPhase, MacosOwnerConflict, MacosOwnerRecoveryRequired,
    MacosOwnerSnapshot,
};
use hypercolor_daemon::macos_service_identity::{
    handover_phase_name, macos_owner, service_identity, service_status,
};
use hypercolor_types::service::{DaemonRunMode, ServiceIdentity, ServiceManager};

const OWNERS: [MacosDaemonOwner; 4] = [
    MacosDaemonOwner::AppSidecar,
    MacosDaemonOwner::DirectLaunchd,
    MacosDaemonOwner::Homebrew,
    MacosDaemonOwner::Standalone,
];

#[test]
fn every_macos_owner_round_trips_through_the_neutral_identity() {
    for owner in OWNERS {
        let identity = service_identity(owner);
        assert_eq!(macos_owner(&identity), Some(owner), "{owner:?}");
    }
}

#[test]
fn macos_owners_map_to_the_documented_identities() {
    assert_eq!(
        service_identity(MacosDaemonOwner::AppSidecar),
        ServiceIdentity::APP_SIDECAR
    );
    assert_eq!(
        service_identity(MacosDaemonOwner::DirectLaunchd),
        ServiceIdentity::launchd_direct()
    );
    assert_eq!(
        service_identity(MacosDaemonOwner::Homebrew),
        ServiceIdentity::homebrew()
    );
    assert_eq!(
        service_identity(MacosDaemonOwner::Standalone),
        ServiceIdentity::STANDALONE
    );
}

#[test]
fn foreign_identities_never_name_a_macos_owner() {
    assert_eq!(macos_owner(&ServiceIdentity::systemd_user()), None);
    assert_eq!(macos_owner(&ServiceIdentity::systemd_system()), None);
    assert_eq!(macos_owner(&ServiceIdentity::windows_scm()), None);
    let managerless_service = ServiceIdentity {
        run_mode: DaemonRunMode::UserService,
        manager: None,
        unit: None,
    };
    assert_eq!(macos_owner(&managerless_service), None);
    let sidecar_with_manager = ServiceIdentity {
        run_mode: DaemonRunMode::SupervisedChild,
        manager: Some(ServiceManager::Launchd),
        unit: None,
    };
    assert_eq!(macos_owner(&sidecar_with_manager), None);
}

#[test]
fn handover_phase_names_are_the_serde_wire_names() {
    assert_eq!(
        handover_phase_name(MacosHandoverPhase::RollbackStopRequested),
        "rollback_stop_requested"
    );
    assert_eq!(
        handover_phase_name(MacosHandoverPhase::Committed),
        "committed"
    );
}

#[test]
fn service_status_carries_conflict_and_recovery() {
    let status = service_status(&MacosOwnerSnapshot {
        active_owner: MacosDaemonOwner::DirectLaunchd,
        owner_epoch: 42,
        conflict: Some(MacosOwnerConflict {
            active_owner: MacosDaemonOwner::DirectLaunchd,
            active_epoch: 42,
            contender_owner: MacosDaemonOwner::Homebrew,
            observed_at_ms: 1_725_000_000_789,
        }),
        recovery_required: Some(MacosOwnerRecoveryRequired {
            requested_owner: MacosDaemonOwner::AppSidecar,
            prior_owner: MacosDaemonOwner::Homebrew,
            phase: MacosHandoverPhase::RollbackStopRequested,
        }),
    });
    let json = serde_json::to_value(&status).expect("serialize");
    assert_eq!(json["identity"]["unit"], "tech.hyperbliss.hypercolor");
    assert_eq!(
        json["recovery_required"]["phase"],
        "rollback_stop_requested"
    );

    assert_eq!(status.identity, ServiceIdentity::launchd_direct());
    assert_eq!(status.owner_epoch, 42);
    let conflict = status.conflict.clone().expect("conflict maps");
    assert_eq!(conflict.contender, ServiceIdentity::homebrew());
    assert_eq!(conflict.observed_at_ms, 1_725_000_000_789);
    let recovery = status.recovery_required.clone().expect("recovery maps");
    assert_eq!(recovery.requested, ServiceIdentity::APP_SIDECAR);
    assert_eq!(recovery.prior, ServiceIdentity::homebrew());
    assert_eq!(recovery.phase, "rollback_stop_requested");

    let json = serde_json::to_value(&status).expect("serialize");
    assert_eq!(json["identity"]["unit"], "tech.hyperbliss.hypercolor");
    assert_eq!(
        json["recovery_required"]["phase"],
        "rollback_stop_requested"
    );
}
