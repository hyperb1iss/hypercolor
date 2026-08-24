//! Kernel-backed parent lifetime for a supervised Linux daemon.
//!
//! `prctl(PR_SET_PDEATHSIG)` makes the kernel deliver SIGTERM to the child
//! the moment its parent thread exits, so a supervisor that crashes or is
//! SIGKILLed can never orphan the daemon on the port and the ownership
//! guard. The flag is armed between fork and exec and followed by a parent
//! re-check, because a parent that died before the flag was set never
//! triggers it.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

use nix::errno::Errno;
use nix::sys::prctl::set_pdeathsig;
use nix::sys::signal::Signal;
use nix::unistd::getppid;

/// Arm the parent-death signal on a command about to be spawned by the
/// process with `parent_pid` (normally the caller's own pid).
///
/// The child receives SIGTERM when the spawning thread exits. If the parent
/// is already gone when the flag is armed, the child exits before exec so
/// it never runs unsupervised.
pub fn arm_parent_death(command: &mut Command, parent_pid: u32) {
    // SAFETY: the closure runs in the forked child before exec and only
    // performs async-signal-safe work: two raw syscalls (prctl, getppid) and
    // an error return; it allocates nothing and touches no locks.
    unsafe {
        command.pre_exec(move || {
            set_pdeathsig(Signal::SIGTERM).map_err(io::Error::from)?;
            if getppid().as_raw() as u32 != parent_pid {
                // std relays only a raw errno out of the forked child, so the
                // refusal is reported as ESRCH ("no such process"): the
                // supervisor exited before the parent-death signal was armed.
                return Err(io::Error::from(Errno::ESRCH));
            }
            Ok(())
        });
    }
}
