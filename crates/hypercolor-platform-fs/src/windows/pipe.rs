//! Which process serves a named pipe.

use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle as _;

use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

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
