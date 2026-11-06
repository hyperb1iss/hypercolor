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
//! the programs outside a unit make: which release runs, which process may
//! serve the update pipe, and how the service manager recovers the update
//! service. So does every step of [`WindowsInstallPlatform`]. Only their
//! Win32 hosts and everything that touches the store are Windows-only.

mod error;
mod executor;
mod installation;
mod layout;
mod machine_layout;
mod manifest;
mod model;
#[cfg(windows)]
mod ownership;
#[cfg(windows)]
mod payload;
mod platform;
#[cfg(all(test, unix))]
mod platform_tests;
mod record;
mod release_selection;
mod session;
#[cfg(windows)]
pub(crate) mod store;
mod unit;
mod update_pipe;
mod update_service;

pub use error::ReleasePayloadError;
pub use executor::WindowsInstallExecutor;
pub use installation::{WINDOWS_INSTALLATION_FILE, WindowsInstallation, WindowsRuntimeMode};
pub use machine_layout::{
    BootstrapSlot, WINDOWS_APP_PROGRAM, WINDOWS_CLI_PROGRAM, WindowsInstallLayout,
};
pub use manifest::{
    MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES, MAX_VENDOR_SIGNED, PermissionClass,
    WindowsVendor,
};
pub use model::{
    WINDOWS_APP_ARGUMENTS, WINDOWS_APP_MEMBER, WINDOWS_BROKER_MEMBER, WINDOWS_DAEMON_MEMBER,
    WINDOWS_PROBATION_WINDOW, WINDOWS_RUNTIME_DEADLINE, WindowsAppStart, WindowsBrokerObservation,
    WindowsFileIdentity, WindowsFirewallObservation, WindowsFirewallRule, WindowsInstallConfig,
    WindowsProcessId, WindowsProcessImage, WindowsRunningProcess, WindowsRuntime,
    WindowsRuntimeObservation, WindowsUnitBinding,
};
pub use platform::WindowsInstallPlatform;
pub use session::WindowsSessionHost;
pub use release_selection::{ReleaseRecords, ReleaseSelectionError, select_release};
pub use unit::{
    WINDOWS_DURABLE_STORES_PATH, WindowsUnit, WindowsUnitError, WindowsUnitSources,
    produce_windows_unit,
};
pub use update_pipe::{
    UPDATE_PIPE_RECORD, UPDATE_SERVICE_NAME, UpdatePipeError, UpdatePipeHost, UpdatePipeName,
    VerifiedUpdatePipe, connect_update_pipe,
};
pub use update_service::{
    BootstrapCommand, EXIT_NO_ACTIVATOR, EXIT_NO_PIPE, EXIT_NO_RELEASE, FAILURE_RESET_PERIOD,
    FALLBACK_DELAY, INHERITED_PIPE_VARIABLE, PROBE_FLAG, RECOVER_FALLBACK_FLAG,
    REGISTER_SERVICE_FLAG, RESTART_DELAY, UpdateServiceConfig, UpdateServiceExit,
    UpdateServiceHost, UpdateServiceMode, fallback_command, install_bootstrap, recover_fallback,
    restore_bootstrap, run_update_service, session_logon_notice, update_service_sddl,
};

#[cfg(windows)]
pub use ownership::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};
#[cfg(windows)]
pub use payload::{retain_installed_release_unit, stage_release_payload, validate_release_payload};
#[cfg(windows)]
pub use store::WindowsReleaseRecords;
#[cfg(windows)]
pub use update_pipe::WindowsUpdatePipeHost;
#[cfg(windows)]
pub use update_service::ScmUpdateService;
