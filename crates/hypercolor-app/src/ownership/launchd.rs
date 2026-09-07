use std::process::Output;

use hypercolor_macos_owner::{MACOS_APP_PRODUCT_NAME, MacosDaemonOwner, MacosOwnerExecutionError};

pub(super) fn service_label(
    owner: MacosDaemonOwner,
) -> Result<&'static str, MacosOwnerExecutionError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MACOS_APP_PRODUCT_NAME),
        MacosDaemonOwner::DirectLaunchd => Ok(hypercolor_macos_owner::MACOS_DIRECT_LAUNCHD_LABEL),
        MacosDaemonOwner::Homebrew => Ok(hypercolor_types::service::HOMEBREW_UNIT),
        MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
            "owner does not use a service label",
        )),
    }
}

fn command_output(program: &str, args: &[&str]) -> Result<Output, MacosOwnerExecutionError> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

pub(super) fn run_command(program: &str, args: &[&str]) -> Result<(), MacosOwnerExecutionError> {
    let output = command_output(program, args)?;
    if output.status.success() {
        Ok(())
    } else {
        let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        stderr.truncate(4_096);
        Err(MacosOwnerExecutionError::new(format!(
            "{program} failed with {}: {}",
            output.status,
            stderr.trim()
        )))
    }
}
