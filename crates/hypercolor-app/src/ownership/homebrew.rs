use std::path::PathBuf;

use hypercolor_macos_owner::MacosOwnerExecutionError;

use super::launchd::run_command;

/// The Homebrew formula whose service runs the daemon.
pub(super) const FORMULA: &str = "hypercolor";

pub(super) fn homebrew_binary() -> Result<PathBuf, MacosOwnerExecutionError> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| MacosOwnerExecutionError::new("Homebrew executable is unavailable"))
}

pub(super) fn start_service() -> Result<(), MacosOwnerExecutionError> {
    let brew = homebrew_binary()?;
    run_command(&brew.to_string_lossy(), &["services", "start", FORMULA])
}

pub(super) fn stop_service(formula: &str) -> Result<(), MacosOwnerExecutionError> {
    let brew = homebrew_binary()?;
    run_command(&brew.to_string_lossy(), &["services", "stop", formula])
}
