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
    process::{Child, Command},
};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;

/// Open (append) a supervised child's log file under `<data>/logs`.
pub(crate) fn supervised_log_file(file_name: &str) -> io::Result<File> {
    supervised_log_file_in(&data_dir(), file_name)
}

/// Open a child log alongside the configuration of its verified daemon.
pub(crate) fn supervised_log_file_in(directory: &Path, file_name: &str) -> io::Result<File> {
    let log_dir = directory.join("logs");
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

#[cfg(test)]
mod tests {
    use super::supervised_log_file_in;
    use std::io::Write;

    #[test]
    fn custom_daemon_directory_owns_the_child_log() {
        let directory = tempfile::tempdir().expect("temporary daemon directory");
        let mut log = supervised_log_file_in(directory.path(), "openrgb.log").expect("open log");
        log.write_all(b"custom daemon log\n").expect("write log");
        assert_eq!(
            std::fs::read(directory.path().join("logs/openrgb.log")).expect("read log"),
            b"custom daemon log\n"
        );
    }
}
