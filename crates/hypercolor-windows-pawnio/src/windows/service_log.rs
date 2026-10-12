//! Where the SMBus broker service writes its own log.
//!
//! A Windows service has no console, so tracing output written to stdout
//! goes nowhere, and a broker that failed or stopped used to leave no
//! trace at all. The service writes `logs\smbus-broker.log` beside its
//! executable instead. The installer registers the broker from
//! `Program Files`, and the install script refuses broker paths under a
//! user profile, so the log directory inherits an administrator-owned ACL
//! that users can read but not write: no unprivileged user can plant a
//! link there for the LocalSystem service to write through. The
//! diagnostics bundle, `diagnose-windows.ps1`, and `hypercolor diagnose`
//! all read the log from that one place.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Directory beside the broker executable that holds its logs.
const LOG_DIR_NAME: &str = "logs";
/// The broker's own log file.
const LOG_FILE_NAME: &str = "smbus-broker.log";
/// The previous log, kept once the current one reaches [`ROTATE_AT_BYTES`].
const ROTATED_LOG_FILE_NAME: &str = "smbus-broker.log.1";
/// Size at which the log is rotated when the broker starts. The broker
/// logs its lifecycle and the first failure of each kind, not every
/// request, so a megabyte holds many restarts of history.
const ROTATE_AT_BYTES: u64 = 1024 * 1024;

/// Path of the log the broker at `broker_executable` writes.
#[must_use]
pub fn broker_log_path(broker_executable: &Path) -> Option<PathBuf> {
    broker_executable
        .parent()
        .map(|dir| dir.join(LOG_DIR_NAME).join(LOG_FILE_NAME))
}

/// Open the broker log for appending, rotating it first when it has grown
/// past [`ROTATE_AT_BYTES`].
pub(super) fn open_broker_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() > ROTATE_AT_BYTES) {
        // A failed rotation only means the log keeps growing past the cap
        // until the next start; it must never keep the broker from logging.
        let _ = fs::rename(path, path.with_file_name(ROTATED_LOG_FILE_NAME));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::broker_log_path;

    #[test]
    fn broker_log_lives_in_logs_beside_the_executable() {
        let path = broker_log_path(Path::new(
            r"C:\Program Files\Hypercolor\tools\hypercolor-smbus-service.exe",
        ))
        .expect("an executable path has a parent directory");

        assert_eq!(
            path,
            Path::new(r"C:\Program Files\Hypercolor\tools\logs\smbus-broker.log")
        );
    }
}
