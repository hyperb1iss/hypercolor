use std::fs::{File, OpenOptions};
use std::time::Duration;

use crate::coordinator_error::MacosOwnerExecutionError;

/// Owning handle for the same macOS `flock` used by the final daemon guard.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct MacosDaemonGuard {
    _lock: nix::fcntl::Flock<File>,
}

/// Block until the final macOS daemon guard is acquired.
#[cfg(target_os = "macos")]
pub fn acquire_macos_daemon_guard(
    instance_name: &str,
) -> Result<MacosDaemonGuard, MacosOwnerExecutionError> {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg};

    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(instance_name)
        .map_err(|error| {
            MacosOwnerExecutionError::new(format!("failed to open daemon guard: {error}"))
        })?;
    loop {
        match Flock::lock(file, FlockArg::LockExclusive) {
            Ok(lock) => return Ok(MacosDaemonGuard { _lock: lock }),
            Err((returned, Errno::EINTR)) => file = returned,
            Err((_, error)) => {
                return Err(MacosOwnerExecutionError::new(format!(
                    "failed to acquire daemon guard: {error}"
                )));
            }
        }
    }
}

/// Attempt to acquire the final macOS daemon guard without blocking.
#[cfg(target_os = "macos")]
pub fn try_acquire_macos_daemon_guard(
    instance_name: &str,
) -> Result<Option<MacosDaemonGuard>, MacosOwnerExecutionError> {
    use nix::errno::Errno;
    use nix::fcntl::{Flock, FlockArg};

    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(instance_name)
        .map_err(|error| {
            MacosOwnerExecutionError::new(format!("failed to open daemon guard: {error}"))
        })?;
    loop {
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => return Ok(Some(MacosDaemonGuard { _lock: lock })),
            Err((returned, Errno::EINTR)) => file = returned,
            Err((_, Errno::EAGAIN)) => return Ok(None),
            Err((_, error)) => {
                return Err(MacosOwnerExecutionError::new(format!(
                    "failed to acquire daemon guard: {error}"
                )));
            }
        }
    }
}

/// Wait until the final single-instance guard can be acquired.
#[cfg(target_os = "macos")]
pub fn wait_for_macos_guard_release(
    timeout: Duration,
    instance_name: &str,
) -> Result<bool, MacosOwnerExecutionError> {
    use std::time::Instant;

    let started = Instant::now();
    loop {
        if try_acquire_macos_daemon_guard(instance_name)?.is_some() {
            return Ok(true);
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(false);
        };
        std::thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}
