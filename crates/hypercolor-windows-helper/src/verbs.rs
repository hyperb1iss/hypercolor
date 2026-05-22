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
}

impl VerbError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotImplemented { .. } => "verb_not_implemented",
        }
    }
}

/// Dispatch a validated request to its verb handler.
///
/// Every verb returns `Err(VerbError::NotImplemented)` until the
/// corresponding handler lands — this is intentional scaffolding so the
/// CLI/request/auth plumbing can be validated end-to-end with smoke tests
/// before any privileged operation is wired.
pub fn dispatch(request: &Request) -> Result<(), VerbError> {
    Err(VerbError::NotImplemented {
        verb: request.verb,
    })
}
