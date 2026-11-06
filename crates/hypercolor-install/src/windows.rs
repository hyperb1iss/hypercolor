//! The Windows machine layout of the release store (RFC 57, B1.4.1).
//!
//! Releases live as immutable units under `%ProgramFiles%\Hypercolor\units`,
//! and the transaction state under `%ProgramData%\Hypercolor\update`: the
//! journal, the lock and `active.json`, the pointer that names the active
//! unit. Both roots are machine-wide. `WindowsOwnershipPolicy` proves that
//! only the principals it trusts can change them, or replace anything on
//! the path to them, before a transaction relies on either.
//!
//! The unit manifest is plain data, so its parser and the unit producer
//! compile on every host, and a unit produced or checked on another
//! platform meets the rules the Windows store enforces. So do the decisions
//! the programs outside a unit make: which release runs and which process
//! may serve the update pipe. Only their Win32 hosts and everything that touches the store
//! are Windows-only.

mod error;
mod installation;
mod machine_layout;
mod manifest;
#[cfg(windows)]
mod ownership;
#[cfg(windows)]
mod payload;
mod release_selection;
#[cfg(windows)]
pub(crate) mod store;
mod unit;
mod update_pipe;

pub use error::ReleasePayloadError;
pub use installation::{WINDOWS_INSTALLATION_FILE, WindowsInstallation, WindowsRuntimeMode};
pub use machine_layout::{
    BootstrapSlot, WINDOWS_APP_PROGRAM, WINDOWS_CLI_PROGRAM, WindowsInstallLayout,
};
pub use manifest::{
    MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES, MAX_VENDOR_SIGNED, PermissionClass,
    WindowsVendor,
};
pub use release_selection::{ReleaseRecords, ReleaseSelectionError, select_release};
pub use unit::{
    WINDOWS_DURABLE_STORES_PATH, WindowsUnit, WindowsUnitError, WindowsUnitSources,
    produce_windows_unit,
};
pub use update_pipe::{
    UPDATE_PIPE_RECORD, UPDATE_SERVICE_NAME, UpdatePipeError, UpdatePipeHost, UpdatePipeName,
    VerifiedUpdatePipe, connect_update_pipe,
};

#[cfg(windows)]
pub use ownership::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};
#[cfg(windows)]
pub use payload::{retain_installed_release_unit, stage_release_payload, validate_release_payload};
#[cfg(windows)]
pub use store::WindowsReleaseRecords;
#[cfg(windows)]
pub use update_pipe::WindowsUpdatePipeHost;
