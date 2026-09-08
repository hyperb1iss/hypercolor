//! Shared decision for adopting, starting, or holding an OpenRGB server.
use crate::{InstallHint, OpenRgbBinary, PermissionCheck, ProcessSpec, ServerProbe};
use std::net::SocketAddr;

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
