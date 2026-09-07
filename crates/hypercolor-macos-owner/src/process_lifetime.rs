//! Kernel-backed parent lifetime for a supervised macOS daemon.
//!
//! A supervised child must die with its supervisor even when the supervisor
//! is SIGKILLed and never reaps. Linux has `prctl(PR_SET_PDEATHSIG)`; macOS
//! has no equivalent, so the child registers a kqueue `EVFILT_PROC`
//! `NOTE_EXIT` watch on the parent pid and treats the event as a shutdown
//! signal. The watch is a kernel fact, not a poll.

use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};

use crate::MacosOwnerExecutionError;

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
    let mut events = [KEvent::new(
        0,
        EventFilter::EVFILT_PROC,
        EvFlags::empty(),
        FilterFlag::empty(),
        0,
        0,
    )];
    loop {
        match queue.kevent(std::slice::from_ref(&watch), &mut events, None) {
            Ok(0) => continue,
            Ok(_) => {
                let event = &events[0];
                if event.flags().contains(EvFlags::EV_ERROR) {
                    // ESRCH in `data`: the parent was already gone when the
                    // watch was registered, which is the exit we wanted.
                    if event.data() == i64::from(nix::errno::Errno::ESRCH as i32) as isize {
                        return Ok(());
                    }
                    return Err(MacosOwnerExecutionError::new(format!(
                        "kqueue process watch for pid {pid} failed with errno {}",
                        event.data()
                    )));
                }
                return Ok(());
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(nix::errno::Errno::ESRCH) => return Ok(()),
            Err(error) => {
                return Err(MacosOwnerExecutionError::new(format!(
                    "kqueue process watch for pid {pid} failed: {error}"
                )));
            }
        }
    }
}
