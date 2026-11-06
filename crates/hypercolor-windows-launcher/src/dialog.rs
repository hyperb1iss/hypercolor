//! The one thing the launcher ever shows.

use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

/// Show `message` and wait for the person to dismiss it. The launcher has
/// no console, so this is the only way a failed launch is ever seen.
pub(crate) fn show_failure(title: &str, message: &str) {
    let title = wide(title);
    let message = wide(message);
    // SAFETY: Both strings are NUL-terminated and outlive the call, and a
    // null owner window is allowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
