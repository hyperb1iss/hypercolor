//! macOS owner vocabulary mapped onto the neutral service identity.
//!
//! The durable owner store keeps its own `MacosDaemonOwner` schema (spec 76
//! treats the record as diagnostic evidence, so its shape stays frozen).
//! This module is the one place where that platform vocabulary crosses
//! into the neutral `ServiceIdentity` that the status API, the event bus,
//! and every client speak.

use hypercolor_types::service::{
    ServiceConflict, ServiceIdentity, ServiceManager, ServiceRecoveryRequired, ServiceStatus,
};

use crate::macos_owner::{MacosDaemonOwner, MacosHandoverPhase, MacosOwnerSnapshot};

/// The neutral identity that a corroborated macOS owner reports.
#[must_use]
pub fn service_identity(owner: MacosDaemonOwner) -> ServiceIdentity {
    match owner {
        MacosDaemonOwner::AppSidecar => ServiceIdentity::APP_SIDECAR,
        MacosDaemonOwner::DirectLaunchd => ServiceIdentity::launchd_direct(),
        MacosDaemonOwner::Homebrew => ServiceIdentity::homebrew(),
        MacosDaemonOwner::Standalone => ServiceIdentity::STANDALONE,
    }
}

/// The macOS owner a neutral identity names, when it names one at all.
///
/// Identities that do not exist on macOS (systemd, the Windows SCM, or a
/// managed run mode without a manager) map to `None` so a launcher cannot
/// declare a topology the platform authority can never corroborate.
#[must_use]
pub fn macos_owner(identity: &ServiceIdentity) -> Option<MacosDaemonOwner> {
    match (identity.run_mode, identity.manager) {
        (hypercolor_types::service::DaemonRunMode::SupervisedChild, None) => {
            Some(MacosDaemonOwner::AppSidecar)
        }
        (hypercolor_types::service::DaemonRunMode::Standalone, None) => {
            Some(MacosDaemonOwner::Standalone)
        }
        (hypercolor_types::service::DaemonRunMode::UserService, Some(ServiceManager::Launchd)) => {
            Some(MacosDaemonOwner::DirectLaunchd)
        }
        (hypercolor_types::service::DaemonRunMode::UserService, Some(ServiceManager::Homebrew)) => {
            Some(MacosDaemonOwner::Homebrew)
        }
        _ => None,
    }
}

/// Opaque diagnostic name of a durable handover phase (its serde wire name).
#[must_use]
pub fn handover_phase_name(phase: MacosHandoverPhase) -> String {
    match serde_json::to_value(phase) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{phase:?}"),
    }
}

/// The neutral status the daemon reports for a durable owner snapshot.
#[must_use]
pub fn service_status(snapshot: &MacosOwnerSnapshot) -> ServiceStatus {
    ServiceStatus {
        identity: service_identity(snapshot.active_owner),
        owner_epoch: snapshot.owner_epoch,
        conflict: snapshot.conflict.map(|conflict| ServiceConflict {
            active: service_identity(conflict.active_owner),
            contender: service_identity(conflict.contender_owner),
            observed_at_ms: conflict.observed_at_ms,
        }),
        recovery_required: snapshot
            .recovery_required
            .map(|recovery| ServiceRecoveryRequired {
                requested: service_identity(recovery.requested_owner),
                prior: service_identity(recovery.prior_owner),
                phase: handover_phase_name(recovery.phase),
            }),
    }
}
