//! Local-only macOS daemon ownership coordination.

mod commands;
mod model;

#[cfg(target_os = "macos")]
mod executor;
#[cfg(target_os = "macos")]
mod homebrew;
#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "macos")]
mod planning;
#[cfg(target_os = "macos")]
mod remediation;

#[doc(hidden)]
pub use commands::{
    __cmd__choose_daemon_owner, __cmd__execute_macos_daemon_owner_offline_remedy,
    __cmd__macos_daemon_owner_offline_status, __cmd__restart_macos_capture_owner,
};
pub use commands::{
    choose_daemon_owner, execute_macos_daemon_owner_offline_remedy,
    macos_daemon_owner_offline_status, restart_macos_capture_owner,
};
pub use model::{
    MacosCaptureOwner, MacosCaptureOwnerRestartOutcome, MacosDaemonOwnerRemedyOutcome,
};

#[cfg(target_os = "macos")]
pub(crate) use planning::{
    MacosStartupRecoveryDisposition, recover_daemon_owner_before_supervisor,
};

#[cfg(all(test, target_os = "macos"))]
mod tests;
