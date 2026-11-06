//! Where the SMBus broker service writes its own log.
//!
//! A Windows service has no console, so tracing output written to stdout
//! goes nowhere, and a broker that failed or stopped used to leave no
//! trace at all. The service writes `smbus-broker.log` in the machine log
//! directory, `%ProgramData%\Hypercolor\logs`, outside the release it runs
//! from, which nothing may change once installed. The directory is opened
//! through handles that refuse links and any component a standard user
//! could change (see `log_directory`), so no unprivileged user can plant a
//! link there for the LocalSystem service to write through. The
//! diagnostics bundle, `diagnose-windows.ps1`, and `hypercolor diagnose`
//! all read the log from that one place.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::PathBuf;

use super::log_directory::{machine_log_directory, open_machine_log_directory};

/// The broker's own log file.
const LOG_FILE_NAME: &str = "smbus-broker.log";
/// The previous log, kept once the current one reaches [`ROTATE_AT_BYTES`].
const ROTATED_LOG_FILE_NAME: &str = "smbus-broker.log.1";
/// Size at which the log is rotated when the broker starts. The broker
/// logs its lifecycle and the first failure of each kind, not every
/// request, so a megabyte holds many restarts of history.
const ROTATE_AT_BYTES: u64 = 1024 * 1024;

/// Path of the log the broker writes. `None` when `ProgramData` is not set
/// to an absolute path.
#[must_use]
pub fn broker_log_path() -> Option<PathBuf> {
    machine_log_directory().map(|directory| directory.join(LOG_FILE_NAME))
}

/// Open the broker log for appending, rotating it first when it has grown
/// past [`ROTATE_AT_BYTES`]. Returns the log's path with the file.
pub(super) fn open_broker_log() -> io::Result<(PathBuf, File)> {
    let directory = open_machine_log_directory()?;
    let name = OsStr::new(LOG_FILE_NAME);
    if directory
        .entry_metadata(name)?
        .is_some_and(|metadata| metadata.size() > ROTATE_AT_BYTES)
    {
        // A failed rotation only means the log keeps growing past the cap
        // until the next start; it must never keep the broker from logging.
        let _ = directory.rename(name, OsStr::new(ROTATED_LOG_FILE_NAME), true);
    }
    let file = directory.open_append_file(name)?;
    Ok((directory.path().join(LOG_FILE_NAME), file))
}

#[cfg(test)]
mod tests {
    use super::broker_log_path;
    use crate::machine_log_directory;

    #[test]
    fn broker_log_lives_in_the_machine_log_directory() {
        let path = broker_log_path().expect("ProgramData is set on Windows");
        assert_eq!(
            path,
            machine_log_directory()
                .expect("machine log directory")
                .join("smbus-broker.log")
        );
        assert!(
            path.ends_with(r"Hypercolor\logs\smbus-broker.log"),
            "{}",
            path.display()
        );
    }
}
