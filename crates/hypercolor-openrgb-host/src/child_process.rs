//! Stop an owned OpenRGB process group without releasing its identity early.

use std::{
    io,
    process::{Child, ExitStatus},
    time::Duration,
};

#[cfg(unix)]
use std::time::Instant;

/// Stop a retained child and its descendants before reaping the group leader.
/// The caller must retain exclusive ownership of the unreaped child handle.
/// On Unix, spawn the child with `CommandExt::process_group(0)` so its PID
/// identifies the owned group; other spawn modes cannot guarantee tree stop.
pub fn stop_owned_server(child: &mut Child, grace: Duration) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        if let Err(error) = signal_group(child, nix::sys::signal::Signal::SIGTERM)
            && error.raw_os_error() != Some(nix::errno::Errno::ESRCH as i32)
        {
            child.kill()?;
        }
        let deadline = Instant::now() + grace;
        loop {
            if exited_without_reaping(child)? {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        // The leader is still unreaped, even if it exited during the grace
        // period. Its group identity remains reserved through this signal.
        let _ = signal_group(child, nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = grace;
    let _ = child.kill();
    child.wait()
}

#[cfg(unix)]
fn signal_group(child: &Child, signal: nix::sys::signal::Signal) -> io::Result<()> {
    let pid = i32::try_from(child.id()).map_err(io::Error::other)?;
    nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal).map_err(io::Error::from)
}

/// Reap a completed child only after terminating any remaining descendants.
pub fn reap_owned_server(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    #[cfg(unix)]
    {
        if exited_without_reaping(child)? {
            return stop_owned_server(child, Duration::ZERO).map(Some);
        }
        Ok(None)
    }
    #[cfg(not(unix))]
    child.try_wait()
}

#[cfg(unix)]
fn exited_without_reaping(child: &Child) -> io::Result<bool> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let pid = Pid::from_raw(i32::try_from(child.id()).map_err(io::Error::other)?)
        .ok_or_else(|| io::Error::other("child has no process identity"))?;
    waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )
    .map(|status| status.is_some())
    .map_err(io::Error::from)
}
