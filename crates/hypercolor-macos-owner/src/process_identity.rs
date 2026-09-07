use crate::coordinator_error::MacosOwnerExecutionError;

/// Request graceful termination through a retained, unreaped child handle.
///
/// # Errors
///
/// Returns an error when the child state cannot be inspected, its identifier
/// cannot be represented by the platform API, or `SIGTERM` cannot be delivered.
#[cfg(target_os = "macos")]
pub fn request_macos_child_termination(
    child: &mut std::process::Child,
) -> Result<(), MacosOwnerExecutionError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    if child
        .try_wait()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
        .is_some()
    {
        return Ok(());
    }
    let pid = i32::try_from(child.id()).map_err(|_| {
        MacosOwnerExecutionError::new("retained child identifier exceeds the macOS process range")
    })?;
    kill(Pid::from_raw(pid), Signal::SIGTERM)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

/// Request graceful termination of a recorded owner process by pid.
///
/// The caller must have verified the exact live process identity against the
/// owner record (including audit token) and observed guard contention before
/// calling: this function delivers `SIGTERM` to whatever currently holds the pid.
///
/// # Errors
///
/// Returns an error when the identifier cannot be represented by the
/// platform API or `SIGTERM` cannot be delivered.
#[cfg(target_os = "macos")]
pub fn request_macos_pid_termination(pid: u32) -> Result<(), MacosOwnerExecutionError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let pid = i32::try_from(pid).map_err(|_| {
        MacosOwnerExecutionError::new("recorded owner identifier exceeds the macOS process range")
    })?;
    kill(Pid::from_raw(pid), Signal::SIGTERM)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}
