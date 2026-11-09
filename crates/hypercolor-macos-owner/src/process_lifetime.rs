//! Kernel-backed process exit watches.
//!
//! A supervised child must die with its supervisor even when the supervisor
//! is SIGKILLed and never reaps. Linux has `prctl(PR_SET_PDEATHSIG)`; macOS
//! has no equivalent, so the child registers a kqueue `EVFILT_PROC`
//! `NOTE_EXIT` watch on the parent pid and treats the event as a shutdown
//! signal. The watch is a kernel fact, not a poll.
//!
//! The same watch proves a launchd job gone: `launchctl bootout` returns
//! while launchd is still stopping the job's process, so the caller arms a
//! [`ProcessExitWatch`] before it submits the bootout and waits on it after.

use std::time::{Duration, Instant};

use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
use nix::sys::time::TimeSpec;

use crate::MacosOwnerExecutionError;

/// A kqueue `NOTE_EXIT` watch on one process, registered when it is armed.
///
/// Arming before the step that ends the process means its exit can never
/// fall between that step and the wait.
#[derive(Debug)]
pub struct ProcessExitWatch {
    queue: Kqueue,
    pid: u32,
    exited: bool,
}

impl ProcessExitWatch {
    /// Register a watch on the exit of process `pid`. A process that is
    /// already gone (the kernel reports ESRCH) counts as exited.
    ///
    /// # Errors
    ///
    /// Returns an error when the kqueue cannot be created or the watch cannot
    /// be registered for a reason other than the process being gone.
    pub fn arm(pid: u32) -> Result<Self, MacosOwnerExecutionError> {
        let queue = Kqueue::new().map_err(|error| {
            MacosOwnerExecutionError::new(format!("kqueue creation failed: {error}"))
        })?;
        let watch = KEvent::new(
            pid as usize,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
            FilterFlag::NOTE_EXIT,
            0,
            0,
        );
        // With no room for receipts, a failed registration comes back as the
        // call's own errno.
        let exited = match queue.kevent(
            std::slice::from_ref(&watch),
            &mut [],
            Some(*TimeSpec::from_duration(Duration::ZERO).as_ref()),
        ) {
            Ok(_) => false,
            Err(nix::errno::Errno::ESRCH) => true,
            Err(error) => {
                return Err(MacosOwnerExecutionError::new(format!(
                    "kqueue process watch for pid {pid} failed: {error}"
                )));
            }
        };
        Ok(Self { queue, pid, exited })
    }

    /// Wait until the process exits or `deadline` passes, and report whether
    /// it exited.
    ///
    /// # Errors
    ///
    /// Returns an error when the kqueue fails.
    pub fn wait_until(&mut self, deadline: Instant) -> Result<bool, MacosOwnerExecutionError> {
        self.wait(Some(deadline))
    }

    fn wait(&mut self, deadline: Option<Instant>) -> Result<bool, MacosOwnerExecutionError> {
        let mut events = [KEvent::new(
            0,
            EventFilter::EVFILT_PROC,
            EvFlags::empty(),
            FilterFlag::empty(),
            0,
            0,
        )];
        while !self.exited {
            let remaining = deadline.map(|deadline| {
                let remaining = deadline.saturating_duration_since(Instant::now());
                *TimeSpec::from_duration(remaining).as_ref()
            });
            match self.queue.kevent(&[], &mut events, remaining) {
                Ok(0) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                    return Ok(false);
                }
                Ok(0) | Err(nix::errno::Errno::EINTR) => {}
                Ok(_) => {
                    let event = &events[0];
                    if event.flags().contains(EvFlags::EV_ERROR)
                        && event.data() != i64::from(nix::errno::Errno::ESRCH as i32) as isize
                    {
                        return Err(MacosOwnerExecutionError::new(format!(
                            "kqueue process watch for pid {} failed with errno {}",
                            self.pid,
                            event.data()
                        )));
                    }
                    self.exited = true;
                }
                Err(error) => {
                    return Err(MacosOwnerExecutionError::new(format!(
                        "kqueue process watch for pid {} failed: {error}",
                        self.pid
                    )));
                }
            }
        }
        Ok(true)
    }
}

/// Block until the process with `pid` exits.
///
/// Returns immediately when the process has already exited by the time the
/// watch is registered (the kernel reports ESRCH), so a parent that died
/// between exec and registration is never missed.
///
/// # Errors
///
/// Returns an error when the kqueue cannot be created or the watch cannot
/// be registered for a reason other than the process being gone.
pub fn wait_for_process_exit(pid: u32) -> Result<(), MacosOwnerExecutionError> {
    ProcessExitWatch::arm(pid)?.wait(None).map(drop)
}

/// Block until the process with `pid` exits or `timeout` passes, and report
/// whether it exited.
///
/// # Errors
///
/// Returns the errors of [`wait_for_process_exit`], and an error when the
/// deadline cannot be represented.
pub fn wait_for_process_exit_within(
    pid: u32,
    timeout: Duration,
) -> Result<bool, MacosOwnerExecutionError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| MacosOwnerExecutionError::new("process watch deadline overflowed"))?;
    ProcessExitWatch::arm(pid)?.wait_until(deadline)
}
