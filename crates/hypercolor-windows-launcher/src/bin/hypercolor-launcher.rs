//! `Hypercolor.exe`: the name shortcuts, sign-in and links all run.
//!
//! An installer lays this binary down as `Hypercolor.exe` beside `units\`.
//! Cargo cannot build it under that name, because `hypercolor.exe`, the
//! CLI, lands in the same directory on a file system that ignores case.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    hypercolor_windows_launcher::launcher::run()
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("hypercolor-launcher only runs on Windows");
    ExitCode::from(2)
}
