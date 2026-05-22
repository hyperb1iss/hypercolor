//! Allowlisted verb enum and dispatch.
//!
//! Helper only accepts verbs from this fixed enum; arbitrary commands are
//! impossible because the verb field deserializes via serde and unknown
//! variants are rejected (`#[serde(rename_all = "kebab-case")]` with no
//! `#[serde(other)]` catch-all).

use std::fmt;

use serde::Deserialize;
use thiserror::Error;

use crate::request::Request;

mod repair_smbus_service;

/// Allowlisted operations the elevated helper can perform.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Verb {
    InstallPawnio,
    InstallSmbusService,
    /// Composite: install PawnIO + register SMBus service in one transaction
    /// so the caller sees a single UAC prompt for the full hardware setup.
    InstallHardwareSupport,
    RepairSmbusService,
    /// Replace `hypercolor-smbus-service.exe` under
    /// `C:\Program Files\Hypercolor\smbus-service\`. Invoked by
    /// `hypercolor-updater` per RFC 52.
    SwapSmbusBinary,
    UninstallSmbusService,
    UninstallPawnio,
    /// Composite: stop+delete the SMBus service and optionally remove
    /// PawnIO (gated by `flags.also_remove_pawnio`) in one UAC.
    UninstallHardwareSupport,
}

impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::InstallPawnio => "install-pawnio",
            Self::InstallSmbusService => "install-smbus-service",
            Self::InstallHardwareSupport => "install-hardware-support",
            Self::RepairSmbusService => "repair-smbus-service",
            Self::SwapSmbusBinary => "swap-smbus-binary",
            Self::UninstallSmbusService => "uninstall-smbus-service",
            Self::UninstallPawnio => "uninstall-pawnio",
            Self::UninstallHardwareSupport => "uninstall-hardware-support",
        };
        f.write_str(s)
    }
}

/// Per-verb structured error returned from [`dispatch`].
#[derive(Debug, Error)]
pub enum VerbError {
    #[error("verb `{verb}` is not yet implemented")]
    NotImplemented { verb: Verb },
    #[error("could not connect to the Windows service manager: {detail}")]
    ServiceManager { detail: String },
    #[error("could not open service `{service}`: {detail}")]
    ServiceOpen { service: String, detail: String },
    #[error("could not query status of service `{service}`: {detail}")]
    ServiceQuery { service: String, detail: String },
    #[error("could not stop service `{service}`: {detail}")]
    ServiceStop { service: String, detail: String },
    #[error("could not start service `{service}`: {detail}")]
    ServiceStart { service: String, detail: String },
    #[error(
        "service `{service}` did not reach expected state `{expected}` within timeout (observed `{observed}`)"
    )]
    ServiceTimeout {
        service: String,
        expected: String,
        observed: String,
    },
}

impl VerbError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotImplemented { .. } => "verb_not_implemented",
            Self::ServiceManager { .. } => "service_manager_unavailable",
            Self::ServiceOpen { .. } => "service_open_failed",
            Self::ServiceQuery { .. } => "service_query_failed",
            Self::ServiceStop { .. } => "service_stop_failed",
            Self::ServiceStart { .. } => "service_start_failed",
            Self::ServiceTimeout { .. } => "service_timeout",
        }
    }
}

/// Dispatch a validated request to its verb handler.
///
/// Verbs that still return `VerbError::NotImplemented` are intentional —
/// they'll land as their downstream Phase tickets become unblocked.
pub fn dispatch(request: &Request) -> Result<(), VerbError> {
    match request.verb {
        Verb::RepairSmbusService => repair_smbus_service::run(),
        verb => Err(VerbError::NotImplemented { verb }),
    }
}
