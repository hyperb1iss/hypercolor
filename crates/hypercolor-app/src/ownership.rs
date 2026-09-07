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

// Tauri's command macro emits two hidden companions per command (the
// wrapper and, since tauri-macros 2.6, the name macro); `generate_handler!`
// resolves both through this module's path.
#[doc(hidden)]
pub use commands::{
    __cmd__choose_daemon_owner, __cmd__execute_macos_daemon_owner_offline_remedy,
    __cmd__macos_daemon_owner_offline_status, __cmd__restart_macos_capture_owner,
    __tauri_command_name_choose_daemon_owner,
    __tauri_command_name_execute_macos_daemon_owner_offline_remedy,
    __tauri_command_name_macos_daemon_owner_offline_status,
    __tauri_command_name_restart_macos_capture_owner,
};
pub use commands::{
    choose_daemon_owner, execute_macos_daemon_owner_offline_remedy,
    macos_daemon_owner_offline_status, restart_macos_capture_owner,
};
pub use model::{
    MacosCaptureOwnerRestartOutcome, MacosDaemonOwnerRemedyOutcome, macos_owner,
    require_macos_owner, service_identity,
};

#[cfg(target_os = "macos")]
pub(crate) use planning::{
    MacosStartupRecoveryDisposition, recover_daemon_owner_before_supervisor,
};

#[cfg(all(test, target_os = "macos"))]
mod tests;
