//! Waiting for a service to start, through the service control manager.
//!
//! `NotifyServiceStatusChangeW` delivers each change as an asynchronous
//! procedure call to the thread that asked, during an alertable wait, so
//! the caller blocks in the wait itself. On one service handle the manager
//! remembers what it already reported: a second wait for the same state
//! returns only once the service has entered that state again, and at once
//! when it has done so since the last report. Holding the handle between
//! waits is what makes every start reported exactly once.

use std::ffi::c_void;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_MARKED_FOR_DELETE,
    ERROR_SERVICE_NOTIFY_CLIENT_LAGGING, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Services::{
    NotifyServiceStatusChangeW, OpenSCManagerW, OpenServiceW, SC_HANDLE, SC_MANAGER_CONNECT,
    SERVICE_NOTIFY_2W, SERVICE_NOTIFY_DELETE_PENDING, SERVICE_NOTIFY_RUNNING,
    SERVICE_NOTIFY_STATUS_CHANGE, SERVICE_QUERY_STATUS,
};
use windows::Win32::System::Threading::{INFINITE, SleepEx};
use windows::core::{HSTRING, Owned, PCWSTR};

/// One service's starts, each reported once to whoever waits.
pub struct ServiceStarts {
    name: HSTRING,
    /// The service handle that remembers what was reported, and the
    /// manager it came from. Opened on the first wait.
    handles: Option<(Owned<SC_HANDLE>, Owned<SC_HANDLE>)>,
}

// SAFETY: Service control manager handles belong to the process, not to a
// thread, and no notification is outstanding between calls to `wait`, which
// waits for the one it registers before returning.
unsafe impl Send for ServiceStarts {}

impl fmt::Debug for ServiceStarts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceStarts")
            .field("name", &self.name)
            .field("open", &self.handles.is_some())
            .finish()
    }
}

/// What one registration heard.
enum Heard {
    Running,
    Deleted,
    Lagging,
}

impl ServiceStarts {
    /// Watch the service `name`. Nothing is asked until the first wait.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: HSTRING::from(name),
            handles: None,
        }
    }

    /// Block until the service enters `SERVICE_RUNNING`. The first wait
    /// returns at once when it already runs; every later one returns once
    /// it has started again since the last, at once when that start has
    /// already happened. Returns `false` when no such service is registered
    /// or it is marked for deletion, since it will then never start again.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the service control manager
    /// cannot be reached or refuses the wait.
    pub fn wait(&mut self) -> io::Result<bool> {
        loop {
            let handles = match self.handles.take() {
                Some(handles) => handles,
                None => {
                    let manager = open_manager()?;
                    let Some(service) = open_service(&manager, &self.name)? else {
                        return Ok(false);
                    };
                    (service, manager)
                }
            };
            match wait_once(&handles.0)? {
                Heard::Running => {
                    self.handles = Some(handles);
                    return Ok(true);
                }
                Heard::Deleted => return Ok(false),
                // The manager dropped changes this watcher had not taken. A
                // fresh handle starts over from the current state, which
                // may report a start once more.
                Heard::Lagging => {}
            }
        }
    }
}

fn open_manager() -> io::Result<Owned<SC_HANDLE>> {
    // SAFETY: Null names the local manager's active database; the call
    // takes plain values.
    let manager = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(|error| os_error(&error))?;
    // SAFETY: Nothing else owns the handle the call just returned.
    Ok(unsafe { Owned::new(manager) })
}

fn open_service(
    manager: &Owned<SC_HANDLE>,
    name: &HSTRING,
) -> io::Result<Option<Owned<SC_HANDLE>>> {
    // SAFETY: The manager handle and the NUL-terminated name are live for
    // the call, which transfers ownership of the returned handle.
    let service = match unsafe { OpenServiceW(**manager, name, SERVICE_QUERY_STATUS) } {
        Ok(service) => service,
        Err(error) if WIN32_ERROR::from_error(&error) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => {
            return Ok(None);
        }
        Err(error) => return Err(os_error(&error)),
    };
    // SAFETY: Nothing else owns the handle the call just returned.
    Ok(Some(unsafe { Owned::new(service) }))
}

/// Register for the next start and wait for it.
///
/// The system writes into `notify` and queues the callback exactly once,
/// and both must outlive that, so nothing returns between a successful
/// registration and the callback having run.
fn wait_once(service: &Owned<SC_HANDLE>) -> io::Result<Heard> {
    let delivered = AtomicBool::new(false);
    let mut notify = SERVICE_NOTIFY_2W {
        dwVersion: SERVICE_NOTIFY_STATUS_CHANGE,
        pfnNotifyCallback: Some(notified),
        pContext: (&raw const delivered).cast_mut().cast::<c_void>(),
        ..SERVICE_NOTIFY_2W::default()
    };
    // SAFETY: The service handle is live, and `notify` and the flag it
    // names stay in place until the callback runs, which the loop below
    // waits for.
    let registered = unsafe {
        NotifyServiceStatusChangeW(
            **service,
            SERVICE_NOTIFY_RUNNING | SERVICE_NOTIFY_DELETE_PENDING,
            &raw mut notify,
        )
    };
    if let Some(heard) = refusal(registered)? {
        return Ok(heard);
    }
    while !delivered.load(Ordering::Acquire) {
        // SAFETY: An alertable sleep takes plain values; the callback runs
        // inside it, on this thread.
        unsafe { SleepEx(INFINITE, true) };
    }
    if let Some(heard) = refusal(notify.dwNotificationStatus)? {
        return Ok(heard);
    }
    if notify.dwNotificationTriggered & SERVICE_NOTIFY_DELETE_PENDING.0 != 0 {
        Ok(Heard::Deleted)
    } else {
        Ok(Heard::Running)
    }
}

/// What a status the manager returned means, when it is not success.
fn refusal(status: u32) -> io::Result<Option<Heard>> {
    match WIN32_ERROR(status) {
        ERROR_SUCCESS => Ok(None),
        ERROR_SERVICE_MARKED_FOR_DELETE => Ok(Some(Heard::Deleted)),
        ERROR_SERVICE_NOTIFY_CLIENT_LAGGING => Ok(Some(Heard::Lagging)),
        _ => Err(io::Error::from_raw_os_error(status.cast_signed())),
    }
}

/// Runs as an asynchronous procedure call on the waiting thread.
unsafe extern "system" fn notified(parameter: *const c_void) {
    // SAFETY: The system passes the `SERVICE_NOTIFY_2W` that `wait_once`
    // registered, whose context is the flag on that frame, and the frame
    // is still waiting for exactly this call.
    let delivered = unsafe {
        &*(*parameter.cast::<SERVICE_NOTIFY_2W>())
            .pContext
            .cast::<AtomicBool>()
    };
    delivered.store(true, Ordering::Release);
}

fn os_error(error: &windows::core::Error) -> io::Error {
    WIN32_ERROR::from_error(error).map_or_else(
        || io::Error::other(error.to_string()),
        |code| io::Error::from_raw_os_error(code.0.cast_signed()),
    )
}
