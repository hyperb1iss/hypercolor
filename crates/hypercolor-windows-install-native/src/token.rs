//! Who an installer acts for, and who owns what it creates.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;

use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    CreateWellKnownSid, GetTokenInformation, PSID, SECURITY_MAX_SID_SIZE, SetTokenInformation,
    TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER, TokenOwner,
    TokenUser, WinBuiltinAdministratorsSid,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows::core::{Owned, PWSTR};

/// `TOKEN_USER` and the SID behind it fit well inside this.
const TOKEN_USER_WORDS: usize = 64;

fn os_error(error: &windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0)
}

fn process_token(process: HANDLE, access: TOKEN_ACCESS_MASK) -> io::Result<Owned<HANDLE>> {
    let mut token = HANDLE::default();
    // SAFETY: `process` is live for the call and `token` is a live local.
    unsafe { OpenProcessToken(process, access, &raw mut token) }
        .map_err(|error| os_error(&error))?;
    // SAFETY: OpenProcessToken returned a handle this function owns.
    Ok(unsafe { Owned::new(token) })
}

fn user_of(token: &Owned<HANDLE>) -> io::Result<String> {
    let mut buffer = [0_u64; TOKEN_USER_WORDS];
    let mut needed = 0_u32;
    // SAFETY: The buffer is live, writable and aligned for TOKEN_USER, and
    // its byte length is passed exactly.
    unsafe {
        GetTokenInformation(
            **token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            size_of::<[u64; TOKEN_USER_WORDS]>() as u32,
            &raw mut needed,
        )
    }
    .map_err(|error| os_error(&error))?;
    // SAFETY: GetTokenInformation wrote a TOKEN_USER at the buffer's start.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    sid_text(sid)
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid for the call and `text` is a live local.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.map_err(|error| os_error(&error))?;
    // SAFETY: The conversion returned a NUL-terminated string.
    let result = unsafe { text.to_string() }
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a SID's text is not UTF-16"));
    // SAFETY: The conversion allocated the string for the caller to free
    // once, and nothing uses it after this.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    result
}

/// The user whose desktop this process runs on, as a SID in its textual
/// form: the owner of the shell, whoever elevated this process. Without a
/// shell (a deployment tool, a service) it is the user this process runs as.
///
/// # Errors
///
/// Returns the operating-system error when neither token can be read.
pub fn desktop_user_sid() -> io::Result<String> {
    // SAFETY: GetShellWindow takes no arguments and may return null.
    let shell = unsafe { GetShellWindow() };
    let mut pid = 0_u32;
    if !shell.is_invalid() {
        // SAFETY: A stale window handle only makes this return zero, and
        // `pid` is a live local.
        unsafe { GetWindowThreadProcessId(shell, Some(&raw mut pid)) };
    }
    if pid != 0 {
        // SAFETY: Opening a process by ID has no preconditions.
        if let Ok(process) = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            // SAFETY: OpenProcess returned a handle this function owns.
            let process = unsafe { Owned::new(process) };
            if let Ok(token) = process_token(*process, TOKEN_QUERY) {
                return user_of(&token);
            }
        }
    }
    // SAFETY: The pseudo handle needs no closing and is always valid.
    let token = process_token(unsafe { GetCurrentProcess() }, TOKEN_QUERY)?;
    user_of(&token)
}

/// Make the Administrators group the owner of everything this process and
/// its children create from here on.
///
/// Windows otherwise gives what an elevated account creates to that account
/// wherever the system's own default says so, and a machine-wide
/// installation trusts only machine principals as owners of its units and
/// records.
///
/// # Errors
///
/// Returns the operating-system error, which is `ERROR_INVALID_OWNER` for a
/// process that is not elevated.
pub fn own_new_objects_as_administrators() -> io::Result<()> {
    let mut sid = [0_u8; SECURITY_MAX_SID_SIZE as usize];
    let mut length = SECURITY_MAX_SID_SIZE;
    // SAFETY: The buffer is live, writable and `length` bytes long, and no
    // domain SID is needed for a built-in group.
    unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            None,
            Some(PSID(sid.as_mut_ptr().cast())),
            &raw mut length,
        )
    }
    .map_err(|error| os_error(&error))?;
    // SAFETY: The pseudo handle needs no closing and is always valid.
    let token = process_token(unsafe { GetCurrentProcess() }, TOKEN_ADJUST_DEFAULT)?;
    let owner = TOKEN_OWNER {
        Owner: PSID(sid.as_mut_ptr().cast()),
    };
    // SAFETY: The token is live, `owner` and the SID it names outlive the
    // call, and the size is exact.
    unsafe {
        SetTokenInformation(
            *token,
            TokenOwner,
            (&raw const owner).cast::<c_void>(),
            size_of::<TOKEN_OWNER>() as u32,
        )
    }
    .map_err(|error| os_error(&error))
}

#[cfg(test)]
mod tests {
    use super::desktop_user_sid;

    #[test]
    fn someone_owns_this_desktop() {
        let sid = desktop_user_sid().expect("a user");
        assert!(sid.starts_with("S-1-"), "{sid}");
    }
}
