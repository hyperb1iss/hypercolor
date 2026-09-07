//! Linux launcher authority: who positively owns this daemon process.
//!
//! systemd is corroborated through the unit's `MainPID` (the same exact
//! pid identity launchctl gives on macOS), the desktop app through its
//! supervised-parent claim matching the live parent, and everything else
//! falls through to the standalone residual.

use std::process::Command;

use anyhow::{Context, Result};
use hypercolor_types::service::SYSTEMD_UNIT;

use crate::launcher_claim::{LinuxLauncherEvidence, parse_systemd_main_pid};
use crate::startup::SUPERVISED_PARENT_PID_ENV;

/// systemd exports this to every service it starts; its absence means no
/// `systemctl` query can name this process and the probe is skipped.
const SYSTEMD_INVOCATION_ENV: &str = "INVOCATION_ID";
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

/// Measure which launchers own the current process.
///
/// # Errors
///
/// Returns an error when a `systemctl` probe that systemd itself invited
/// (the invocation id is set) cannot run.
pub fn inspect_linux_launcher_authority() -> Result<LinuxLauncherEvidence> {
    let current_pid = std::process::id();
    let parent_pid = std::os::unix::process::parent_id();
    let supervised_child = std::env::var(SUPERVISED_PARENT_PID_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|claimed| claimed == parent_pid);
    let invoked_by_systemd =
        std::env::var_os(SYSTEMD_INVOCATION_ENV).is_some_and(|invocation| !invocation.is_empty());
    let (systemd_user, systemd_system) = if invoked_by_systemd {
        (
            systemd_main_pid(true)? == Some(current_pid),
            systemd_main_pid(false)? == Some(current_pid),
        )
    } else {
        (false, false)
    };
    Ok(LinuxLauncherEvidence {
        supervised_child,
        systemd_user,
        systemd_system,
    })
}

fn systemd_main_pid(user_scope: bool) -> Result<Option<u32>> {
    let mut command = Command::new("systemctl");
    if user_scope {
        command.arg("--user");
    }
    let output = command
        .args(["show", "--property=MainPID", "--value", SYSTEMD_UNIT])
        .output()
        .with_context(|| {
            format!(
                "failed to query systemd{} for the {SYSTEMD_UNIT} main pid",
                if user_scope { " --user" } else { "" }
            )
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = &output.stdout[..output.stdout.len().min(MAX_COMMAND_OUTPUT_BYTES)];
    Ok(parse_systemd_main_pid(&String::from_utf8_lossy(stdout)))
}
