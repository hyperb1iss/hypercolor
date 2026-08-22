use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerRemedy};

/// Result of executing an app-local offline-owner remedy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosDaemonOwnerRemedyOutcome {
    /// The selected external owner published a newer healthy epoch.
    Started { owner: MacosDaemonOwner },
}

/// Owner vocabulary matching `SystemStatus.macos_daemon_ownership.active_owner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosCaptureOwner {
    AppSidecar,
    LaunchdService,
    HomebrewService,
    Standalone,
}

impl From<MacosCaptureOwner> for MacosDaemonOwner {
    fn from(owner: MacosCaptureOwner) -> Self {
        match owner {
            MacosCaptureOwner::AppSidecar => Self::AppSidecar,
            MacosCaptureOwner::LaunchdService => Self::DirectLaunchd,
            MacosCaptureOwner::HomebrewService => Self::Homebrew,
            MacosCaptureOwner::Standalone => Self::Standalone,
        }
    }
}

impl From<MacosDaemonOwner> for MacosCaptureOwner {
    fn from(owner: MacosDaemonOwner) -> Self {
        match owner {
            MacosDaemonOwner::AppSidecar => Self::AppSidecar,
            MacosDaemonOwner::DirectLaunchd => Self::LaunchdService,
            MacosDaemonOwner::Homebrew => Self::HomebrewService,
            MacosDaemonOwner::Standalone => Self::Standalone,
        }
    }
}

/// Result of an explicit local restart of the authoritative capture owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosCaptureOwnerRestartOutcome {
    /// A managed owner published a new authoritative epoch after restart.
    Restarted {
        owner: MacosCaptureOwner,
        previous_owner_epoch: u64,
        owner_epoch: u64,
    },
    /// A standalone owner must be stopped by the terminal user.
    UserActionRequired {
        owner: MacosCaptureOwner,
        owner_epoch: u64,
        remedy: MacosOwnerRemedy,
    },
}
