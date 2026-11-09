//! What ties everything the service runs to the service's process.

use std::io;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, IntoRawHandle as _, OwnedHandle};

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

/// Put this process in a new job whose last handle closes with it. Every
/// child and grandchild starts inside it, so when the process ends, however
/// it ends, nothing it ran survives to race the recovery that follows.
///
/// The process joins before it starts any child, so no child can slip out.
/// Only one that asks to leave with `CREATE_BREAKAWAY_FROM_JOB` gets out,
/// which is how the app and daemon outlive the service that started them.
///
/// The job's handle is deliberately never closed. Closing it would end this
/// process too, before it could report how the run ended.
pub(crate) fn enclose_this_process_in_kill_on_close_job() -> io::Result<()> {
    // SAFETY: Null attributes and a null name ask for a new anonymous job,
    // whose handle is not inheritable.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateJobObjectW returned a new handle nothing else owns.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    // SAFETY: The job is live, and the pointer and size describe `limits`
    // for the information class it belongs to.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The job is live, and GetCurrentProcess returns a pseudo
    // handle that needs no closing.
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _ = job.into_raw_handle();
    Ok(())
}

/// End the process `process`, for a stop the service manager asked for.
pub(crate) fn terminate(process: &OwnedHandle) {
    // SAFETY: The handle is live for the call. A process that already ended
    // refuses, which leaves nothing to do.
    unsafe { TerminateProcess(process.as_raw_handle(), 1) };
}
