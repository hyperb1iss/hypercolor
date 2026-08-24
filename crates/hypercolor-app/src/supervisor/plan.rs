//! The pure launcher plan: one decision table for every platform tenant.
//!
//! Each platform probes its service manager (systemd, the Windows Service
//! Control Manager, the macOS owner store) and folds what it learned into a
//! [`LauncherProbe`]. The plan function turns that probe plus the owner
//! preference into a payload-bearing [`LauncherPlan`] arm. Nothing here
//! touches the OS: the arms are executed by the supervisor's platform
//! composition.

use hypercolor_types::service::ServiceIdentity;
use url::Url;

use super::DaemonCommand;

/// What the supervisor learned about the launcher already registered for
/// the daemon on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherProbe {
    /// The registered service-manager launcher, or
    /// [`ServiceIdentity::STANDALONE`] when none is registered (an
    /// unidentified daemon answering on the endpoint is also standalone).
    pub identity: ServiceIdentity,
    /// Whether a daemon currently answers on the endpoint under that identity.
    pub online: bool,
    /// Whether the service manager can start the registered launcher on the
    /// supervisor's request (enabled-but-inactive unit, stopped SCM service).
    pub startable: bool,
}

impl LauncherProbe {
    /// No registered launcher and nothing answering on the endpoint.
    pub const NOTHING: Self = Self {
        identity: ServiceIdentity::STANDALONE,
        online: false,
        startable: false,
    };

    /// A registered launcher that is currently running the daemon.
    #[must_use]
    pub const fn online(identity: ServiceIdentity) -> Self {
        Self {
            identity,
            online: true,
            startable: false,
        }
    }

    /// A registered launcher the service manager could start.
    #[must_use]
    pub const fn startable(identity: ServiceIdentity) -> Self {
        Self {
            identity,
            online: false,
            startable: true,
        }
    }

    /// A registered launcher that is neither running nor startable here.
    #[must_use]
    pub const fn offline(identity: ServiceIdentity) -> Self {
        Self {
            identity,
            online: false,
            startable: false,
        }
    }
}

/// Which launcher arms the user's owner selection permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerPreference {
    /// Reuse or start whatever is registered, and spawn a supervised child
    /// when nothing can serve.
    Flexible,
    /// The user selected this external owner. The supervisor reuses it when
    /// it is online and otherwise holds with a remedy; it never spawns a
    /// child beside a selected owner (spec 76 §external owners).
    Selected(ServiceIdentity),
}

/// Why the supervisor is holding instead of running a daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    /// The selected external owner is not running; the user (or an explicit
    /// remedy action) must start it.
    SelectedOwnerOffline,
    /// A different launcher than the selected one is running the daemon.
    SelectedOwnerDisplaced,
    /// A standalone owner still holds the daemon guard after a handover and
    /// must exit before any supervisor may run.
    PendingStandaloneExit,
}

/// The supervisor action selected for a launcher probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherPlan {
    /// Connect to the daemon the registered launcher already runs.
    Reuse {
        identity: ServiceIdentity,
        endpoint: Url,
    },
    /// Ask the service manager to start its registered launcher.
    Start {
        identity: ServiceIdentity,
        unit: String,
    },
    /// Spawn the bundled daemon as a supervised child.
    SpawnChild { command: DaemonCommand },
    /// Do nothing and surface a remedy for the selected owner.
    Hold {
        identity: ServiceIdentity,
        reason: HoldReason,
    },
}

/// Whether two identities name the same launcher for plan purposes (the
/// unit label is diagnostic).
fn same_launcher(left: &ServiceIdentity, right: &ServiceIdentity) -> bool {
    left.run_mode == right.run_mode && left.manager == right.manager
}

/// Select the supervisor action for a launcher probe.
#[must_use]
pub fn launcher_plan(
    probe: &LauncherProbe,
    preference: &OwnerPreference,
    endpoint: &Url,
    spawn: DaemonCommand,
) -> LauncherPlan {
    match preference {
        OwnerPreference::Selected(selected) => {
            if probe.online && same_launcher(&probe.identity, selected) {
                LauncherPlan::Reuse {
                    identity: probe.identity.clone(),
                    endpoint: endpoint.clone(),
                }
            } else if probe.online {
                LauncherPlan::Hold {
                    identity: selected.clone(),
                    reason: HoldReason::SelectedOwnerDisplaced,
                }
            } else {
                LauncherPlan::Hold {
                    identity: selected.clone(),
                    reason: HoldReason::SelectedOwnerOffline,
                }
            }
        }
        OwnerPreference::Flexible => {
            if probe.online {
                return LauncherPlan::Reuse {
                    identity: probe.identity.clone(),
                    endpoint: endpoint.clone(),
                };
            }
            if probe.startable
                && let Some(unit) = probe.identity.unit.clone()
                && probe.identity.is_managed()
            {
                return LauncherPlan::Start {
                    identity: probe.identity.clone(),
                    unit,
                };
            }
            LauncherPlan::SpawnChild { command: spawn }
        }
    }
}
