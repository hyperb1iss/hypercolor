//! What the Windows machine platform observes and journals.
//!
//! Everything here is plain data, so the platform's decisions compile and
//! run on any host against fakes.

use std::path::Path;
use std::time::Duration;

use hypercolor_platform_fs::windows_file_name_is_valid;
use serde::{Deserialize, Serialize};

use super::machine_layout::{BootstrapSlot, WINDOWS_APP_PROGRAM, WINDOWS_DAEMON_PROGRAM};
use super::manifest::fold_case;
use crate::{InstallPlatformError, UnitId};

/// The arguments the app starts with when an update starts it.
pub const WINDOWS_APP_ARGUMENTS: [&str; 1] = ["--minimized"];
/// How long a proven candidate must keep running before it commits.
pub const WINDOWS_PROBATION_WINDOW: Duration = Duration::from_secs(90);
/// How long the app gets to quit, and the daemon to become ready.
pub const WINDOWS_RUNTIME_DEADLINE: Duration = Duration::from_secs(180);

const MAX_UNITS_ROOT_BYTES: usize = 512;
const MAX_SERVICE_IMAGE_BYTES: usize = 1024;
const SHA256_DIGITS: usize = 64;
const FILE_ID_DIGITS: usize = 32;

pub(super) fn error(detail: impl Into<String>) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}

/// Where a Windows installation keeps its units, and the deadlines its
/// transactions run under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsInstallConfig {
    /// `%ProgramFiles%\Hypercolor\units`, as a canonical drive path.
    pub units_root: String,
    /// How long the app gets to quit before it is ended by force.
    pub stop_deadline: Duration,
    /// How long a started app gets to bring its daemon to ready.
    pub start_deadline: Duration,
    /// How long a proven candidate must keep running.
    pub probation: Duration,
}

impl WindowsInstallConfig {
    /// A configuration under the standard deadlines.
    #[must_use]
    pub fn new(units_root: impl Into<String>) -> Self {
        Self {
            units_root: units_root.into(),
            stop_deadline: WINDOWS_RUNTIME_DEADLINE,
            start_deadline: WINDOWS_RUNTIME_DEADLINE,
            probation: WINDOWS_PROBATION_WINDOW,
        }
    }

    /// Require a canonical, bounded drive path.
    ///
    /// # Errors
    /// Returns an error naming the root when it is not one.
    pub fn validate(&self) -> Result<(), InstallPlatformError> {
        let root = self.units_root.as_str();
        let canonical = root.len() <= MAX_UNITS_ROOT_BYTES
            && root.split_once(":\\").is_some_and(|(drive, rest)| {
                drive.len() == 1
                    && drive.bytes().all(|byte| byte.is_ascii_alphabetic())
                    && rest.split('\\').all(windows_file_name_is_valid)
            });
        if canonical {
            Ok(())
        } else {
            Err(error(format!(
                "the Windows units root {root:?} is not a canonical drive path"
            )))
        }
    }

    /// The installed path of one unit member, given its `/`-separated path.
    #[must_use]
    pub fn member_path(&self, unit: &UnitId, member: &str) -> String {
        member_path(&self.units_root, unit, member)
    }
}

/// The installed path of `member` in `unit`, beneath `units_root`.
pub(super) fn member_path(units_root: &str, unit: &UnitId, member: &str) -> String {
    format!(
        "{units_root}\\{}\\{}",
        unit.as_str(),
        member.replace('/', "\\")
    )
}

/// Whether two Windows paths name one file: NTFS folds case, and a process
/// image may carry the `\\?\` prefix.
pub(super) fn same_windows_path(left: &str, right: &str) -> bool {
    let plain = |path: &str| fold_case(path.strip_prefix(r"\\?\").unwrap_or(path));
    plain(left) == plain(right)
}

/// The one program a service's registered command line runs, or `None` for
/// a line that is anything else.
pub(super) fn service_program(image: &str) -> Option<&str> {
    let program = image
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(image);
    (image.len() <= MAX_SERVICE_IMAGE_BYTES
        && !program.contains('"')
        && Path::new(program)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")))
    .then_some(program)
}

/// What identifies one installed file: where it lives on its volume, and
/// what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsFileIdentity {
    pub volume_serial: u64,
    /// The 128-bit file ID, as 32 lowercase hex digits.
    pub file_id: String,
    /// SHA-256 of the file's bytes, as lowercase hex.
    pub sha256: String,
}

impl WindowsFileIdentity {
    /// # Errors
    /// Returns an error for a malformed file ID or digest.
    pub fn validate(&self) -> Result<(), InstallPlatformError> {
        if is_lower_hex(&self.file_id, FILE_ID_DIGITS) && is_lower_hex(&self.sha256, SHA256_DIGITS)
        {
            Ok(())
        } else {
            Err(error("Windows file identity is malformed"))
        }
    }
}

fn is_lower_hex(value: &str, digits: usize) -> bool {
    value.len() == digits
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A retained unit, as its own manifest and files state it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsUnitBinding {
    pub unit: UnitId,
    pub version: String,
    pub app: WindowsFileIdentity,
    pub daemon: WindowsFileIdentity,
    pub broker: WindowsFileIdentity,
    pub bootstrap: WindowsFileIdentity,
}

impl WindowsUnitBinding {
    /// # Errors
    /// Returns an error for a malformed identity or an unbounded version.
    pub fn validate(&self) -> Result<(), InstallPlatformError> {
        if self.version.is_empty() || self.version.len() > 128 {
            return Err(error("Windows unit version is not bounded"));
        }
        self.app.validate()?;
        self.daemon.validate()?;
        self.broker.validate()?;
        self.bootstrap.validate()
    }
}

/// One process, told apart from any later process that reuses its ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsProcessId {
    pub pid: u32,
    /// Its creation time, in `FILETIME` ticks.
    pub created: u64,
}

/// A live process and the image path the operating system reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsRunningProcess {
    pub id: WindowsProcessId,
    pub image: String,
}

/// What a running process's image file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsProcessImage {
    /// The path from `QueryFullProcessImageNameW`.
    pub path: String,
    pub identity: WindowsFileIdentity,
}

/// Every process running under the app's or the daemon's file name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowsRuntimeObservation {
    pub apps: Vec<WindowsRunningProcess>,
    pub daemons: Vec<WindowsRunningProcess>,
}

impl WindowsRuntimeObservation {
    /// Whether nothing runs.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.apps.is_empty() && self.daemons.is_empty()
    }

    /// Every process, apps first, because ending an app ends the daemon it
    /// holds in its job.
    pub(super) fn processes(&self) -> impl Iterator<Item = &WindowsRunningProcess> {
        self.apps.iter().chain(&self.daemons)
    }

    /// The app and daemon, when exactly one of each runs.
    pub(super) fn pair(&self) -> Option<(&WindowsRunningProcess, &WindowsRunningProcess)> {
        match (self.apps.as_slice(), self.daemons.as_slice()) {
            ([app], [daemon]) => Some((app, daemon)),
            _ => None,
        }
    }
}

/// The app and the daemon it supervises, as one running release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsRuntime {
    pub app: WindowsProcessId,
    pub daemon: WindowsProcessId,
}

impl WindowsRuntime {
    /// Whether either process of `other` is one of these.
    pub(super) fn shares_a_process(&self, other: &Self) -> bool {
        self.app == other.app || self.daemon == other.daemon
    }
}

/// What the service control manager reports about the SMBus broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsBrokerObservation {
    /// The registered command line (`ImagePath`), verbatim.
    pub image: String,
    /// The service is anything but stopped.
    pub running: bool,
}

/// The Windows Firewall program rules a unit needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsFirewallRule {
    Daemon,
    App,
}

impl WindowsFirewallRule {
    /// The rule's name, as the installer registers it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Daemon => "Hypercolor Daemon",
            Self::App => "Hypercolor App",
        }
    }

    /// The unit member the rule allows.
    #[must_use]
    pub const fn member(self) -> &'static str {
        match self {
            Self::Daemon => WINDOWS_DAEMON_PROGRAM,
            Self::App => WINDOWS_APP_PROGRAM,
        }
    }
}

/// The program each rule allows, or `None` for a rule that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsFirewallObservation {
    pub daemon: Option<String>,
    pub app: Option<String>,
}

impl WindowsFirewallObservation {
    #[must_use]
    pub fn program(&self, rule: WindowsFirewallRule) -> Option<&str> {
        match rule {
            WindowsFirewallRule::Daemon => self.daemon.as_deref(),
            WindowsFirewallRule::App => self.app.as_deref(),
        }
    }
}

/// What names a unit from outside it, as the machine holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsLayoutObservation {
    /// The slot the update service's `ImagePath` names; `None` when the
    /// service is not registered.
    pub bootstrap: Option<BootstrapSlot>,
    /// `None` when no broker is registered.
    pub broker: Option<WindowsBrokerObservation>,
    /// `None` when the firewall service does not run.
    pub firewall: Option<WindowsFirewallObservation>,
}

/// How a request to start the app ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsAppStart {
    /// The app runs in the recorded user's session.
    Started(WindowsRunningProcess),
    /// The recorded user has no session to start it in.
    NoSession,
}

#[cfg(test)]
mod tests {
    use super::{WindowsInstallConfig, same_windows_path, service_program};

    #[test]
    fn units_roots_must_be_canonical_drive_paths() {
        for root in [
            r"C:\Program Files\Hypercolor\units",
            r"D:\Hypercolor\units",
            r"c:\units",
        ] {
            WindowsInstallConfig::new(root)
                .validate()
                .unwrap_or_else(|_| panic!("{root} is canonical"));
        }
        for root in [
            "",
            "units",
            r"C:",
            r"C:\",
            r"C:\units\",
            r"\\server\share\units",
            r"CC:\units",
            r"C:/units",
            r"C:\units\..\windows",
            r"C:\units\con",
            r"C:\units:stream",
        ] {
            assert!(
                WindowsInstallConfig::new(root).validate().is_err(),
                "{root:?} must be refused"
            );
        }
        let long = format!(r"C:\{}", "a".repeat(600));
        assert!(WindowsInstallConfig::new(long).validate().is_err());
    }

    #[test]
    fn paths_compare_as_ntfs_does() {
        assert!(same_windows_path(
            r"C:\Program Files\Hypercolor\units\ab\hypercolor-daemon.exe",
            r"c:\PROGRAM FILES\hypercolor\UNITS\AB\Hypercolor-Daemon.EXE",
        ));
        assert!(same_windows_path(r"\\?\C:\a\b.exe", r"C:\a\b.exe"));
        assert!(!same_windows_path(r"C:\a\b.exe", r"C:\a\c.exe"));
    }

    #[test]
    fn a_service_line_must_be_exactly_one_program() {
        assert_eq!(
            service_program(r#""C:\Program Files\a.exe""#),
            Some(r"C:\Program Files\a.exe")
        );
        assert_eq!(service_program(r"C:\a.EXE"), Some(r"C:\a.EXE"));
        for line in [
            "",
            r#""C:\a.exe" --flag"#,
            r"C:\a.exe --flag",
            r#""C:\a.exe"#,
            r#"C:\a.exe""#,
        ] {
            assert_eq!(service_program(line), None, "{line:?}");
        }
        let long = format!(r"C:\{}.exe", "a".repeat(2048));
        assert_eq!(service_program(&long), None);
    }
}
