//! The pure launcher plan: one decision table for every platform tenant.
//!
//! Each platform probes its service manager (systemd, the Windows Service
//! Control Manager, the macOS owner store) and folds what it learned into a
//! [`LauncherProbe`]. The plan function turns that probe plus the owner
//! preference into a payload-bearing [`LauncherPlan`] arm. Nothing here
//! touches the OS: the arms are executed by the supervisor's platform
//! composition.
//!
//! The same file holds the OpenRGB fallback-server plan (Spec 81 §3.2): the
//! supervisor gathers the binary, the SDK probe, the permission checks, and
//! the bridge config, and [`openrgb_plan`] decides adopt, spawn, or hold.

use std::net::SocketAddr;

use hypercolor_openrgb_host::{
    InstallHint, OpenRgbBinary, PermissionCheck, ProcessSpec, ServerProbe,
};
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

/// Why the supervisor is holding instead of adopting or spawning an OpenRGB
/// SDK server (Spec 81 §3.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenRgbHoldReason {
    /// No OpenRGB binary was found; the hints say how to install one here.
    NotInstalled { hints: Vec<InstallHint> },
    /// The binary exists but host permission checks fail; each check carries
    /// its remedy.
    PermissionsMissing { checks: Vec<PermissionCheck> },
    /// The daemon's OpenRGB bridge driver is disabled, so a server would have
    /// no consumer.
    BridgeDisabled,
    /// Something answers on the SDK port without speaking the SDK protocol;
    /// spawning beside it would only collide.
    PortOwnedByUnknown { addr: SocketAddr },
    /// The server this app spawned is still coming up (OpenRGB enumerates
    /// devices before it answers the SDK handshake); nothing to do but wait.
    Starting { pid: u32 },
}

/// The supervisor action selected for the OpenRGB fallback server.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenRgbPlan {
    /// Connect the bridge to the server already answering on `addr`; never
    /// spawn beside a live server.
    Adopt {
        addr: SocketAddr,
        probe: ServerProbe,
    },
    /// Launch a headless loopback server from the detected binary.
    Spawn { spec: ProcessSpec },
    /// Do nothing and surface the reason.
    Hold { reason: OpenRgbHoldReason },
}

/// Everything the OpenRGB plan decides over. Gathering these touches the OS;
/// deciding does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbPlanInputs {
    /// Whether the daemon's OpenRGB bridge driver is enabled.
    pub bridge_enabled: bool,
    /// The OpenRGB installation found on the host, if any.
    pub binary: Option<OpenRgbBinary>,
    /// The loopback SDK endpoint the plan targets.
    pub addr: SocketAddr,
    /// What answered the SDK handshake on `addr`.
    pub probe: ServerProbe,
    /// Whether a plain TCP connect to `addr` succeeded. Together with an
    /// unreachable probe this means a foreign listener owns the port.
    pub port_open: bool,
    /// Host permission and driver prerequisites.
    pub checks: Vec<PermissionCheck>,
    /// Install hints for this host, surfaced when no binary exists.
    pub hints: Vec<InstallHint>,
    /// The launch spec to use when spawning is the answer.
    pub spawn: Option<ProcessSpec>,
    /// Pid of a server this app already spawned and still holds.
    pub managed_pid: Option<u32>,
}

/// Select the supervisor action for the OpenRGB fallback server.
///
/// Order: a disabled bridge holds first (nothing would consume the server);
/// a server that completed the SDK handshake is adopted, never doubled; a
/// child this app already spawned but which has not answered yet holds as
/// starting (never as a foreign listener, and never spawns a sibling); an
/// open port that failed the handshake holds as foreign; a missing binary
/// holds with install hints; any failing permission check holds with the
/// failing checks; otherwise spawn. A binary with no launch spec collapses
/// to `NotInstalled`, which cannot happen when the caller builds the spec
/// from the detected binary.
#[must_use]
pub fn openrgb_plan(inputs: OpenRgbPlanInputs) -> OpenRgbPlan {
    if !inputs.bridge_enabled {
        return OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::BridgeDisabled,
        };
    }
    if inputs.probe.reachable {
        return OpenRgbPlan::Adopt {
            addr: inputs.addr,
            probe: inputs.probe,
        };
    }
    if let Some(pid) = inputs.managed_pid {
        return OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::Starting { pid },
        };
    }
    if inputs.port_open {
        return OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::PortOwnedByUnknown { addr: inputs.addr },
        };
    }
    let Some(spawn) = inputs.binary.and(inputs.spawn) else {
        return OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::NotInstalled {
                hints: inputs.hints,
            },
        };
    };
    let failing: Vec<PermissionCheck> = inputs
        .checks
        .into_iter()
        .filter(|check| !check.ok)
        .collect();
    if !failing.is_empty() {
        return OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::PermissionsMissing { checks: failing },
        };
    }
    OpenRgbPlan::Spawn { spec: spawn }
}
