use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerRemedy};
use hypercolor_types::service::{DaemonRunMode, ServiceIdentity, ServiceManager};

/// Result of executing an app-local offline-owner remedy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosDaemonOwnerRemedyOutcome {
    /// The selected external owner published a newer healthy epoch.
    Started { owner: MacosDaemonOwner },
}

/// The neutral identity a macOS owner reports (the same mapping the daemon
/// publishes in `SystemStatus.service`).
#[must_use]
pub fn service_identity(owner: MacosDaemonOwner) -> ServiceIdentity {
    match owner {
        MacosDaemonOwner::AppSidecar => ServiceIdentity::APP_SIDECAR,
        MacosDaemonOwner::DirectLaunchd => ServiceIdentity::launchd_direct(),
        MacosDaemonOwner::Homebrew => ServiceIdentity::homebrew(),
        MacosDaemonOwner::Standalone => ServiceIdentity::STANDALONE,
    }
}

/// The macOS owner a neutral identity names; `None` for launchers that do
/// not exist on macOS, so a caller can never address one.
#[must_use]
pub fn macos_owner(identity: &ServiceIdentity) -> Option<MacosDaemonOwner> {
    match (identity.run_mode, identity.manager) {
        (DaemonRunMode::SupervisedChild, None) => Some(MacosDaemonOwner::AppSidecar),
        (DaemonRunMode::Standalone, None) => Some(MacosDaemonOwner::Standalone),
        (DaemonRunMode::UserService, Some(ServiceManager::Launchd)) => {
            Some(MacosDaemonOwner::DirectLaunchd)
        }
        (DaemonRunMode::UserService, Some(ServiceManager::Homebrew)) => {
            Some(MacosDaemonOwner::Homebrew)
        }
        _ => None,
    }
}

/// Resolve a requested neutral identity to the macOS owner it names.
///
/// # Errors
///
/// Returns an error naming the declaration when it is not a macOS topology.
pub fn require_macos_owner(identity: &ServiceIdentity) -> Result<MacosDaemonOwner, String> {
    macos_owner(identity).ok_or_else(|| format!("{identity} is not a macOS daemon topology"))
}

/// Result of an explicit local restart of the authoritative capture owner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosCaptureOwnerRestartOutcome {
    /// A managed owner published a new authoritative epoch after restart.
    Restarted {
        owner: ServiceIdentity,
        previous_owner_epoch: u64,
        owner_epoch: u64,
    },
    /// A standalone owner must be stopped by the terminal user.
    UserActionRequired {
        owner: ServiceIdentity,
        owner_epoch: u64,
        remedy: MacosOwnerRemedy,
    },
}
