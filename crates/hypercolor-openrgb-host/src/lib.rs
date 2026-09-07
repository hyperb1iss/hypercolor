//! Host-side OpenRGB integration for Hypercolor.
//!
//! This crate answers the questions the desktop app and the CLI ask before
//! they lean on the OpenRGB fallback bridge: is OpenRGB installed, is its
//! SDK server reachable, how do you install it here, which permissions are
//! missing, and how do we launch a headless server that leaves natively
//! supported hardware alone.
//!
//! It follows the platform-crate pattern: every type is neutral and
//! serializable on every target, and only the functions that touch the OS
//! are gated behind `cfg(target_os)` inside this crate.

mod error;
mod types;

pub use error::{HostError, Result};
pub use types::{
    BinaryKind, InstallHint, InstallMethod, ManagedConfigDir, OpenRgbBinary, PermissionCheck,
    Platform, ProcessSpec, ServerProbe,
};
