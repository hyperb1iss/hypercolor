//! Named pipes that only their creator can stand behind.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_TYPE_PIPE, GetFileType, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, NMPWAIT_WAIT_FOREVER,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};

use super::directory::SddlDescriptor;

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

/// The ID of the process that created the server end of the named pipe
/// `pipe` is connected to. A client checks it against the process it
/// expects before it trusts anything the pipe says, since anyone who knows
/// a pipe's name and holds the right to create it can serve one.
///
/// # Errors
///
/// Returns the operating-system error when `pipe` is not a named pipe
/// handle or the query fails.
pub fn windows_named_pipe_server_process_id(pipe: &File) -> io::Result<u32> {
    let mut process_id = 0_u32;
    // SAFETY: The handle is live for the call and `process_id` is writable
    // storage for the one value the function returns.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut process_id) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(process_id)
}

/// Create the first and only instance of the local byte pipe `name`, with
/// exactly the security descriptor `sddl`.
///
/// The calling process stays the pipe's server for
/// [`windows_named_pipe_server_process_id`] whoever holds the handle later.
/// The handle is inheritable, so a child started with handle inheritance
/// can serve the pipe through [`windows_adopt_inherited_pipe`].
///
/// # Errors
///
/// Returns the operating-system error for an invalid name or descriptor,
/// and when any process already serves `name`.
pub fn windows_create_pipe_server(name: &str, sddl: &str) -> io::Result<File> {
    let wide = wide_pipe_name(name)?;
    let descriptor = SddlDescriptor::parse(sddl)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.cast::<c_void>(),
        bInheritHandle: 1,
    };
    // SAFETY: The name is NUL-terminated, and it and the attributes, whose
    // descriptor `descriptor` owns, outlive the call.
    let raw = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            &attributes,
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateNamedPipeW returned a new handle that nothing else owns.
    Ok(File::from(unsafe { OwnedHandle::from_raw_handle(raw) }))
}

/// Wait, with no time limit, until an instance of the local pipe `name`
/// listens for a client. A client that found every instance busy waits
/// here before it connects again.
///
/// # Errors
///
/// Returns the operating-system error when nothing serves `name`, which is
/// also how the wait ends when the pipe's last instance closes during it.
pub fn windows_wait_for_pipe(name: &str) -> io::Result<()> {
    let wide = wide_pipe_name(name)?;
    // SAFETY: The name is NUL-terminated and outlives the call.
    if unsafe { WaitNamedPipeW(wide.as_ptr(), NMPWAIT_WAIT_FOREVER) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wide_pipe_name(name: &str) -> io::Result<Vec<u16>> {
    let mut wide = name.encode_utf16().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a pipe name holds a NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

/// Wait until a client reaches the server end `server`.
///
/// # Errors
///
/// Returns the operating-system error when `server` is not a waiting pipe
/// server.
pub fn windows_connect_pipe_client(server: &File) -> io::Result<()> {
    // SAFETY: The handle is live for the call and was not opened for
    // overlapped I/O, so the null OVERLAPPED makes the call synchronous.
    if unsafe { ConnectNamedPipe(server.as_raw_handle(), std::ptr::null_mut()) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // A client that arrived before the call is connected already.
    if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
        return Ok(());
    }
    Err(error)
}

/// Take ownership of the pipe handle a parent passed down by value, and stop
/// it travelling any further: a helper this process runs never holds it.
///
/// # Errors
///
/// Returns an error when `value` does not name an open pipe.
///
/// # Safety
///
/// `value` must be a handle this process inherited and nothing else in the
/// process owns, since the returned file closes it.
pub unsafe fn windows_adopt_inherited_pipe(value: usize) -> io::Result<File> {
    let raw = value as *mut c_void;
    // SAFETY: GetFileType only inspects the handle value and reports an
    // unknown type for one that is not open.
    if unsafe { GetFileType(raw) } != FILE_TYPE_PIPE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the inherited handle is not a pipe",
        ));
    }
    // SAFETY: The caller guarantees the handle is this process's to own.
    let pipe = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: The handle is live, and clearing its flag only affects which
    // processes inherit it.
    if unsafe { SetHandleInformation(pipe.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(File::from(pipe))
}
