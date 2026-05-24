//! Cross-platform helpers for `std::process::Command`.

use std::process::Command;

/// Hide the console window that Windows would otherwise pop up when a GUI
/// process spawns a console-subsystem child (e.g. `sc.exe`, `powershell.exe`).
///
/// No-op on non-Windows platforms.
pub fn hide_console_window(command: &mut Command) -> &mut Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW from winbase.h; avoids a winapi/windows-sys dep
        // just for one constant.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
