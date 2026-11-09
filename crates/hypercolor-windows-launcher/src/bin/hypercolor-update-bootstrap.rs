//! The image of the `HypercolorUpdate` service. An installer lays this
//! binary down as `update\slot-a.exe` and `update\slot-b.exe`.

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    hypercolor_windows_launcher::bootstrap::run()
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("hypercolor-update-bootstrap only runs on Windows");
    ExitCode::from(2)
}
