use std::path::{Path, PathBuf};
use std::process::Output;

use hypercolor_macos_owner::{MACOS_APP_PRODUCT_NAME, MacosDaemonOwner, MacosOwnerExecutionError};

pub(super) fn service_label(
    owner: MacosDaemonOwner,
) -> Result<&'static str, MacosOwnerExecutionError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MACOS_APP_PRODUCT_NAME),
        MacosDaemonOwner::DirectLaunchd => Ok("tech.hyperbliss.hypercolor"),
        MacosDaemonOwner::Homebrew => Ok("homebrew.mxcl.hypercolor"),
        MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
            "owner does not use a service label",
        )),
    }
}

pub(super) fn service_target(
    owner: MacosDaemonOwner,
    uid: &str,
) -> Result<String, MacosOwnerExecutionError> {
    Ok(format!("gui/{uid}/{}", service_label(owner)?))
}

pub(super) fn service_plist(
    owner: MacosDaemonOwner,
    launch_agents: &Path,
) -> Result<PathBuf, MacosOwnerExecutionError> {
    Ok(launch_agents.join(format!("{}.plist", service_label(owner)?)))
}

pub(super) fn service_autostart_enabled(
    owner: MacosDaemonOwner,
    uid: &str,
    launch_agents: &Path,
) -> Result<bool, MacosOwnerExecutionError> {
    let plist = service_plist(owner, launch_agents)?;
    if !plist.is_file() {
        return Ok(false);
    }
    let output = command_output("/bin/launchctl", &["print-disabled", &format!("gui/{uid}")])?;
    if !output.status.success() {
        return Err(MacosOwnerExecutionError::new(
            "launchctl failed to inspect service autostart state",
        ));
    }
    Ok(!launchctl_service_disabled(
        &String::from_utf8_lossy(&output.stdout),
        service_label(owner)?,
    ))
}

pub(super) fn launchctl_service_disabled(output: &str, label: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        line.contains(&format!("\"{label}\"")) && line.ends_with("=> true")
    })
}

pub(super) fn command_stdout(program: &str, args: &[&str]) -> Result<String, anyhow::Error> {
    let output = std::process::Command::new(program).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("{program} failed with {}", output.status);
    }
    if output.stdout.len() > 64 * 1024 {
        anyhow::bail!("{program} output exceeds 64 KiB");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub(super) fn command_output(
    program: &str,
    args: &[&str],
) -> Result<Output, MacosOwnerExecutionError> {
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
