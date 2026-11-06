//! Processes named and held through handles.
//!
//! A process ID alone is reused once its process ends, so a process is
//! always named with its creation time, and every handle is proved to be
//! that process before it is waited on or ended.

use std::io;
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, INFINITE, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
    WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::{Owned, PWSTR};

/// The longest path `QueryFullProcessImageNameW` can return.
const MAX_IMAGE_PATH_UNITS: usize = 32_768;
/// Exit code of a process ended by [`terminate_processes`].
const TERMINATED_EXIT_CODE: u32 = 1;
/// `MAXIMUM_WAIT_OBJECTS`: how many handles one wait can hold.
const MAX_WAIT_HANDLES: usize = 64;

/// One process, told apart from any later process that reuses its ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessId {
    /// The process ID.
    pub pid: u32,
    /// Its creation time, in `FILETIME` ticks.
    pub created: u64,
}

/// A live process and the image file it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProcess {
    /// Which process it is.
    pub id: ProcessId,
    /// Its image path from `QueryFullProcessImageNameW`.
    pub image: String,
}

fn os_error(error: &windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0)
}

/// Open `pid` for naming, waiting and ending, or `None` when no such
/// process exists.
fn open(pid: u32) -> io::Result<Option<Owned<HANDLE>>> {
    let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
    // SAFETY: The call takes plain values and transfers ownership of the
    // returned handle, which `Owned` closes.
    match unsafe { OpenProcess(access, false, pid) } {
        Ok(handle) => {
            // SAFETY: `handle` is a live process handle no one else owns.
            Ok(Some(unsafe { Owned::new(handle) }))
        }
        Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => Ok(None),
        Err(error) => Err(os_error(&error)),
    }
}

fn creation_time(handle: HANDLE) -> io::Result<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: `handle` is live for the call and all four out-parameters are
    // writable storage of the type the call fills.
    unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(|error| os_error(&error))?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn image_path(handle: HANDLE) -> io::Result<String> {
    let mut buffer = vec![0_u16; MAX_IMAGE_PATH_UNITS];
    let mut length = u32::try_from(buffer.len()).map_err(io::Error::other)?;
    // SAFETY: `buffer` is writable for `length` UTF-16 units, the call writes
    // at most that many and stores the count it wrote in `length`.
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .map_err(|error| os_error(&error))?;
    let length = usize::try_from(length).map_err(io::Error::other)?;
    String::from_utf16(&buffer[..length]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "process image path is not UTF-16",
        )
    })
}

fn has_exited(handle: HANDLE) -> io::Result<bool> {
    // SAFETY: `handle` is live for the call and holds `SYNCHRONIZE`.
    let outcome = unsafe { WaitForSingleObject(handle, 0) };
    match outcome {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        other => Err(io::Error::other(format!(
            "unexpected process wait result {}",
            other.0
        ))),
    }
}

/// A live process's description, or `None` when it has already exited and
/// only a handle someone holds keeps it visible.
fn describe(pid: u32, handle: HANDLE) -> io::Result<Option<RunningProcess>> {
    if has_exited(handle)? {
        return Ok(None);
    }
    Ok(Some(RunningProcess {
        id: ProcessId {
            pid,
            created: creation_time(handle)?,
        },
        image: image_path(handle)?,
    }))
}

/// A handle to exactly `id`, or `None` when that process is gone, even if
/// another process now uses its ID.
fn retain(id: ProcessId) -> io::Result<Option<Owned<HANDLE>>> {
    let Some(handle) = open(id.pid)? else {
        return Ok(None);
    };
    if creation_time(*handle)? != id.created {
        return Ok(None);
    }
    Ok(Some(handle))
}

/// `id` while it still runs, with the image it runs.
///
/// # Errors
///
/// Returns the operating-system error when the process cannot be opened or
/// named.
pub fn process(id: ProcessId) -> io::Result<Option<RunningProcess>> {
    let Some(handle) = retain(id)? else {
        return Ok(None);
    };
    describe(id.pid, *handle)
}

/// The process running under `pid` now, whichever it is.
///
/// # Errors
///
/// Returns the operating-system error when the process cannot be opened or
/// named.
pub fn process_with_pid(pid: u32) -> io::Result<Option<RunningProcess>> {
    let Some(handle) = open(pid)? else {
        return Ok(None);
    };
    describe(pid, *handle)
}

/// Every live process whose image file name is one of `names`, compared
/// without regard to ASCII case.
///
/// # Errors
///
/// Returns the operating-system error when the process list cannot be read
/// or a matching process cannot be opened or named.
pub fn processes_named(names: &[&str]) -> io::Result<Vec<RunningProcess>> {
    // SAFETY: The call takes plain values and transfers ownership of the
    // returned handle, which `Owned` closes.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map(|handle| {
            // SAFETY: `handle` is a live snapshot handle no one else owns.
            unsafe { Owned::new(handle) }
        })
        .map_err(|error| os_error(&error))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).map_err(io::Error::other)?,
        ..PROCESSENTRY32W::default()
    };
    let mut found = Vec::new();
    // SAFETY: `snapshot` is live and `entry` is writable with `dwSize` set
    // to its own size, as the call requires.
    let mut more = unsafe { Process32FirstW(*snapshot, &raw mut entry) };
    while more.is_ok() {
        let end = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&name))
            && let Some(handle) = open(entry.th32ProcessID)?
            && let Some(running) = describe(entry.th32ProcessID, *handle)?
        {
            found.push(running);
        }
        // SAFETY: As for `Process32FirstW` above.
        more = unsafe { Process32NextW(*snapshot, &raw mut entry) };
    }
    Ok(found)
}

fn wait_milliseconds(timeout: Duration) -> u32 {
    u32::try_from(timeout.as_millis())
        .unwrap_or(u32::MAX)
        .min(INFINITE - 1)
}

/// Handles to whichever of `processes` still run.
fn retain_all(processes: &[ProcessId]) -> io::Result<Vec<(ProcessId, Owned<HANDLE>)>> {
    let mut retained = Vec::with_capacity(processes.len());
    for id in processes {
        if let Some(handle) = retain(*id)? {
            retained.push((*id, handle));
        }
    }
    if retained.len() > MAX_WAIT_HANDLES {
        return Err(io::Error::other("too many processes to wait on at once"));
    }
    Ok(retained)
}

/// Wait until every one of `processes` has exited, or `timeout` has
/// passed. Returns whether they all exited.
///
/// # Errors
///
/// Returns the operating-system error when a process cannot be opened or
/// the wait fails.
pub fn wait_for_every_exit(processes: &[ProcessId], timeout: Duration) -> io::Result<bool> {
    let retained = retain_all(processes)?;
    if retained.is_empty() {
        return Ok(true);
    }
    let handles: Vec<HANDLE> = retained.iter().map(|(_, handle)| **handle).collect();
    // SAFETY: Every handle stays live in `retained` for the call, and there
    // are at most `MAX_WAIT_HANDLES` of them.
    let outcome = unsafe { WaitForMultipleObjects(&handles, true, wait_milliseconds(timeout)) };
    match outcome {
        WAIT_TIMEOUT => Ok(false),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        _ => Ok(true),
    }
}

/// Wait until any one of `processes` exits, or `timeout` passes. Returns
/// the one that exited, which may be one that was already gone.
///
/// # Errors
///
/// Returns the operating-system error when a process cannot be opened or
/// the wait fails.
pub fn wait_for_any_exit(
    processes: &[ProcessId],
    timeout: Duration,
) -> io::Result<Option<ProcessId>> {
    let retained = retain_all(processes)?;
    if let Some(gone) = processes
        .iter()
        .find(|id| !retained.iter().any(|(held, _)| held == *id))
    {
        return Ok(Some(*gone));
    }
    if retained.is_empty() {
        return Ok(None);
    }
    let handles: Vec<HANDLE> = retained.iter().map(|(_, handle)| **handle).collect();
    // SAFETY: Every handle stays live in `retained` for the call, and there
    // are at most `MAX_WAIT_HANDLES` of them.
    let outcome = unsafe { WaitForMultipleObjects(&handles, false, wait_milliseconds(timeout)) };
    match outcome {
        WAIT_TIMEOUT => Ok(None),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        signaled => {
            let index = usize::try_from(signaled.0 - WAIT_OBJECT_0.0).map_err(io::Error::other)?;
            retained
                .get(index)
                .map(|(id, _)| Some(*id))
                .ok_or_else(|| io::Error::other("process wait named no process"))
        }
    }
}

/// End every one of `processes` still running, without waiting for them.
///
/// # Errors
///
/// Returns the operating-system error when a process that still runs
/// cannot be opened or ended.
pub fn terminate_processes(processes: &[ProcessId]) -> io::Result<()> {
    for (_, handle) in retain_all(processes)? {
        // SAFETY: `handle` is live for the call and holds `PROCESS_TERMINATE`.
        if let Err(error) = unsafe { TerminateProcess(*handle, TERMINATED_EXIT_CODE) }
            && !has_exited(*handle)?
        {
            return Err(os_error(&error));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use super::{
        ProcessId, process, process_with_pid, processes_named, terminate_processes,
        wait_for_any_exit, wait_for_every_exit,
    };

    const SHORT: Duration = Duration::from_millis(100);
    const LONG: Duration = Duration::from_secs(30);

    /// A process that runs until it is ended: `cmd` waiting on a pipe the
    /// test holds open.
    fn sleeper() -> (Child, ProcessId) {
        let child = Command::new("cmd.exe")
            .args(["/d", "/q", "/k"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn a sleeping process");
        let running = processes_named(&["CMD.EXE"])
            .expect("list processes")
            .into_iter()
            .find(|running| running.id.pid == child.id())
            .expect("the child is listed by its name");
        assert!(
            running.image.to_ascii_lowercase().ends_with("cmd.exe"),
            "{running:?}"
        );
        (child, running.id)
    }

    #[test]
    fn a_live_process_is_named_by_id_and_creation_time() {
        let (mut child, id) = sleeper();
        let running = process(id)
            .expect("name the process")
            .expect("it is running");
        assert_eq!(running.id, id);
        assert_eq!(
            process_with_pid(id.pid).expect("name the ID"),
            Some(running)
        );
        let reused = ProcessId {
            pid: id.pid,
            created: id.created + 1,
        };
        assert_eq!(
            process(reused).expect("ask about a reused ID"),
            None,
            "a process with another creation time is not this one"
        );
        terminate_processes(&[id]).expect("end the process");
        child.wait().expect("reap the child");
    }

    #[test]
    fn waits_report_what_happened_to_the_processes() {
        let (mut first, first_id) = sleeper();
        let (mut second, second_id) = sleeper();
        let both = [first_id, second_id];

        assert!(!wait_for_every_exit(&both, SHORT).expect("timed wait"));
        assert_eq!(wait_for_any_exit(&both, SHORT).expect("timed wait"), None);

        terminate_processes(&[first_id]).expect("end the first");
        assert_eq!(
            wait_for_any_exit(&both, LONG).expect("wait for one"),
            Some(first_id)
        );
        assert!(!wait_for_every_exit(&both, SHORT).expect("one still runs"));

        terminate_processes(&both).expect("ending a gone process is harmless");
        assert!(wait_for_every_exit(&both, LONG).expect("wait for both"));
        first.wait().expect("reap the first");
        second.wait().expect("reap the second");
        assert_eq!(process(first_id).expect("ask after exit"), None);
    }

    #[test]
    fn nothing_to_wait_on_is_already_done() {
        assert!(wait_for_every_exit(&[], SHORT).expect("empty wait"));
        assert_eq!(wait_for_any_exit(&[], SHORT).expect("empty wait"), None);
    }

    #[test]
    fn a_process_that_never_existed_counts_as_gone() {
        let gone = ProcessId {
            pid: 0xFFFF_FFF0,
            created: 1,
        };
        assert_eq!(process(gone).expect("ask"), None);
        assert_eq!(process_with_pid(gone.pid).expect("ask"), None);
        assert_eq!(
            wait_for_any_exit(&[gone], LONG).expect("wait"),
            Some(gone),
            "a process that is gone ends the wait at once"
        );
        assert!(wait_for_every_exit(&[gone], LONG).expect("wait"));
        terminate_processes(&[gone]).expect("nothing to end");
    }
}
