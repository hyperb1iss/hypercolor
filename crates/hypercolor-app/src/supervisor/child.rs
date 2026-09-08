//! Platform plumbing shared by every child process the app supervises.
//!
//! The daemon and the managed OpenRGB server both need the same lifetime
//! binding: stdio appended to a log under the data directory, a hidden
//! console on Windows, a private process group plus parent-death signal on
//! Unix, a kill-on-close Job object on Windows, and a kill unless the child
//! has provably exited when the handle drops. This module owns those pieces
//! so the two children cannot drift apart.

use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
    process::{Child, Command, ExitStatus},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;

/// Poll cadence while waiting for a child to honor a termination request.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Open (append) a supervised child's log file under `<data>/logs`.
pub(crate) fn supervised_log_file(file_name: &str) -> io::Result<File> {
    let log_dir = data_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join(file_name))
}

/// Spawn a configured command as a supervised child and bind it to the app
/// lifetime.
///
/// Applies the platform command configuration, spawns, and attaches the
/// platform guard. A guard failure kills the fresh child before returning so
/// nothing leaks past the error.
pub(crate) fn spawn_supervised(
    process: &mut Command,
    program: &Path,
) -> Result<(Child, PlatformGuard)> {
    configure_platform_command(process);

    let mut child = process
        .spawn()
        .with_context(|| format!("failed to spawn {}", program.display()))?;

    match attach_platform_guard(&child) {
        Ok(platform_guard) => Ok((child, platform_guard)),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

/// Kill a child unless it has provably exited.
///
/// A `try_wait` error (EINTR, ECHILD) must not leak a live process, so only
/// a confirmed exit skips the kill.
pub(crate) fn kill_unless_exited(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(Some(_))) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Deliver `signal` to the child's whole process group, falling back to the
/// child pid alone when the group cannot be signalled.
///
/// Every supervised child is spawned with `process_group(0)`, so its pgid is
/// its own pid and `killpg` reaches the forks an AppImage or Flatpak runtime
/// leaves between us and the real OpenRGB process.
#[cfg(unix)]
fn signal_child_group(child: &Child, signal: libc::c_int) -> io::Result<()> {
    let pid = libc::pid_t::try_from(child.id())
        .map_err(|_| io::Error::other("child pid exceeds the platform process range"))?;
    // SAFETY: `killpg` and `kill` take a pid/pgid and a signal number and
    // have no memory-safety preconditions; the pid belongs to a child this
    // process spawned and has not reaped, so it cannot have been recycled.
    let group = unsafe { libc::killpg(pid, signal) };
    if group == 0 {
        return Ok(());
    }
    let group_error = io::Error::last_os_error();
    // SAFETY: as above; the direct-pid path is the fallback for a child that
    // changed its own process group.
    let direct = unsafe { libc::kill(pid, signal) };
    if direct == 0 {
        return Ok(());
    }
    Err(group_error)
}

/// Ask a child to terminate gracefully.
///
/// Unix delivers `SIGTERM` to the child's process group; Windows has no
/// graceful console signal for a GUI child, so the request is a
/// `TerminateProcess` there.
pub(crate) fn request_graceful_termination(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        signal_child_group(child, libc::SIGTERM)
    }
    #[cfg(not(unix))]
    {
        child.kill()
    }
}

/// Kill a child and, on Unix, its whole process group, unless it has
/// provably exited. The tree variant of [`kill_unless_exited`] for children
/// whose runtime forks (AppImage, Flatpak).
pub(crate) fn kill_tree_unless_exited(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    #[cfg(unix)]
    let _ = signal_child_group(child, libc::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
}

/// Stop a child: request graceful termination, wait up to `grace`, then kill.
///
/// Returns the exit status when the child was reaped.
pub(crate) fn stop_child(child: &mut Child, grace: Duration) -> Option<ExitStatus> {
    if let Err(error) = request_graceful_termination(child) {
        tracing::warn!(pid = child.id(), %error, "graceful termination request failed");
    }
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(pid = child.id(), %error, "child wait failed while stopping");
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(EXIT_POLL_INTERVAL);
    }
    #[cfg(unix)]
    let _ = signal_child_group(child, libc::SIGKILL);
    let _ = child.kill();
    child.wait().ok()
}

#[cfg(target_os = "windows")]
pub(crate) type PlatformGuard = win32job::Job;

#[cfg(target_os = "windows")]
fn configure_platform_command(command: &mut Command) {
    crate::process_ext::hide_console_window(command);
}

#[cfg(target_os = "windows")]
fn attach_platform_guard(child: &Child) -> Result<PlatformGuard> {
    use std::os::windows::io::AsRawHandle;

    let mut limits = win32job::ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&limits)?;
    job.assign_process(child.as_raw_handle() as isize)?;
    Ok(job)
}

#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct PlatformGuard;

#[cfg(unix)]
fn configure_platform_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    // Linux: the kernel delivers SIGTERM to the child when this process
    // exits (pdeathsig), so supervisor death is a kernel fact rather than
    // something the child has to notice. macOS arms the equivalent on the
    // daemon side with a kqueue EVFILT_PROC watch on the parent pid.
    #[cfg(target_os = "linux")]
    hypercolor_linux_session::arm_parent_death(command, std::process::id());
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "keeps the platform helper signature aligned with Windows"
)]
fn attach_platform_guard(_child: &Child) -> Result<PlatformGuard> {
    Ok(PlatformGuard)
}
