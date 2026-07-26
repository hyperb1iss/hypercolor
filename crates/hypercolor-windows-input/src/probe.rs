//! Whether this process can observe user input at all.
//!
//! Raw Input fails *silently* across a session boundary: `CreateWindowExW`
//! succeeds, `RegisterRawInputDevices` succeeds, and `WM_INPUT` simply never
//! arrives. There is no error to report and no call to check, so the condition
//! has to be probed explicitly — waiting to see whether input shows up could
//! not distinguish an isolated session from an idle user.
//!
//! The probe is the window station, not the session id. The intuitive test,
//! `ProcessIdToSessionId(...) == 0`, is not sufficient: a service or a
//! scheduled task configured to run whether the user is logged on or not gets
//! a non-interactive window station inside a perfectly ordinary non-zero
//! session, and sees exactly as little input as a session-0 service. Requiring
//! a *visible* window station subsumes the session-0 case, catches the
//! scheduled-task case the session id misses, and is the thing Raw Input
//! actually depends on.

use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::StationsAndDesktops::{
    GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
};

use crate::shared::SessionState;

/// `WSF_VISIBLE` from `winuser.h`: the window station is visible on screen and
/// can therefore receive user input.
const WSF_VISIBLE: u32 = 0x0001;

/// Probe whether this process has a visible window station.
///
/// Deliberately does not consult `WTSGetActiveConsoleSessionId`: comparing
/// against the console session would wrongly condemn a legitimate RDP user,
/// who has their own interactive session and their own visible `WinSta0`.
#[must_use]
pub fn interactive_session_state() -> SessionState {
    // SAFETY: returns a non-owned pseudo-handle to the calling process's
    // window station; it must not be closed and stays valid for the call
    // below. Failure is reported through the `Result`.
    let Ok(station) = (unsafe { GetProcessWindowStation() }) else {
        return SessionState::NoInteractiveSession;
    };

    let mut flags = USEROBJECTFLAGS::default();
    let length = u32::try_from(size_of::<USEROBJECTFLAGS>()).unwrap_or(u32::MAX);
    // SAFETY: `station` is a live window-station handle, `flags` is a properly
    // sized and aligned `USEROBJECTFLAGS` owned by this frame, and `length`
    // describes exactly that allocation. `UOI_FLAGS` is the index that
    // populates it.
    let queried = unsafe {
        GetUserObjectInformationW(
            HANDLE(station.0),
            UOI_FLAGS,
            Some(std::ptr::from_mut(&mut flags).cast::<c_void>()),
            length,
            None,
        )
    };
    if queried.is_err() {
        return SessionState::NoInteractiveSession;
    }

    if flags.dwFlags & WSF_VISIBLE == 0 {
        SessionState::NoInteractiveSession
    } else {
        SessionState::Interactive
    }
}
