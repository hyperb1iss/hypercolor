//! The one launchd adapter: every launchd interaction in the app and the
//! CLI speaks through it, with modern verbs only.
//!
//! Verb semantics, kept distinct on purpose:
//!
//! - `bootstrap` / `bootout` load and unload an agent in the user's gui
//!   domain (the legacy `load` / `unload` pair is never used).
//! - `kickstart` starts a loaded agent, `stop` asks a running one to exit
//!   with SIGTERM and leaves it loaded.
//! - `enable` / `disable` flip launchd's persistent autostart gate, which
//!   is independent of whether the agent is loaded right now.
//! - `print` and `print-disabled` are the read side.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::MacosOwnerExecutionError;

/// The launchctl binary every adapter call executes.
pub const LAUNCHCTL_PATH: &str = "/bin/launchctl";

/// Upper bound on captured launchctl output.
const MAX_LAUNCHCTL_OUTPUT_BYTES: usize = 64 * 1024;

/// A launchd agent label inside one user's gui domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdTarget {
    uid: String,
    label: String,
}

impl LaunchdTarget {
    /// The `gui/<uid>` domain the agent belongs to.
    #[must_use]
    pub fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    /// The `gui/<uid>/<label>` service target launchctl addresses.
    #[must_use]
    pub fn service(&self) -> String {
        format!("gui/{}/{}", self.uid, self.label)
    }

    /// The agent label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The agent's plist file name inside `~/Library/LaunchAgents`.
    #[must_use]
    pub fn plist_file_name(&self) -> String {
        format!("{}.plist", self.label)
    }
}

/// Per-user launchd adapter bound to one gui domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdAdapter {
    uid: String,
}

impl LaunchdAdapter {
    /// Adapter for the user whose uid `id -u` reports.
    ///
    /// # Errors
    ///
    /// Returns an error when the uid cannot be read.
    pub fn for_current_user() -> Result<Self, MacosOwnerExecutionError> {
        let output = Command::new("/usr/bin/id")
            .arg("-u")
            .output()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if !output.status.success() {
            return Err(MacosOwnerExecutionError::new(
                "failed to resolve the current user id for launchd",
            ));
        }
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MacosOwnerExecutionError::new(
                "current user id for launchd is not numeric",
            ));
        }
        Ok(Self::with_uid(uid))
    }

    /// Adapter for an explicit numeric uid.
    #[must_use]
    pub fn with_uid(uid: impl Into<String>) -> Self {
        Self { uid: uid.into() }
    }

    /// The uid this adapter addresses.
    #[must_use]
    pub fn uid(&self) -> &str {
        &self.uid
    }

    /// Address an agent label in this user's gui domain.
    #[must_use]
    pub fn target(&self, label: &str) -> LaunchdTarget {
        LaunchdTarget {
            uid: self.uid.clone(),
            label: label.to_owned(),
        }
    }

    /// Whether launchd currently has the agent loaded (`launchctl print`).
    ///
    /// # Errors
    ///
    /// Returns an error only when launchctl cannot be executed.
    pub fn is_loaded(&self, target: &LaunchdTarget) -> Result<bool, MacosOwnerExecutionError> {
        Ok(launchctl_output(&["print", &target.service()])?
            .status
            .success())
    }

    /// The pid launchd reports for the agent, when it is loaded and running.
    ///
    /// # Errors
    ///
    /// Returns an error only when launchctl cannot be executed.
    pub fn service_pid(
        &self,
        target: &LaunchdTarget,
    ) -> Result<Option<u32>, MacosOwnerExecutionError> {
        let output = launchctl_output(&["print", &target.service()])?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(parse_launchctl_print_pid(&bounded_text(&output.stdout)))
    }

    /// Load the agent plist into the gui domain (`launchctl bootstrap`).
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl rejects the plist.
    pub fn bootstrap(&self, plist: &Path) -> Result<(), MacosOwnerExecutionError> {
        run_launchctl(&["bootstrap", &self.domain(), &plist.to_string_lossy()])
    }

    /// Unload the agent from the gui domain (`launchctl bootout`), stopping
    /// it if it runs. A target that is not loaded is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl fails for a loaded agent.
    pub fn bootout(&self, target: &LaunchdTarget) -> Result<(), MacosOwnerExecutionError> {
        if !self.is_loaded(target)? {
            return Ok(());
        }
        run_launchctl(&["bootout", &target.service()])
    }

    /// Start a loaded agent now (`launchctl kickstart`).
    ///
    /// # Errors
    ///
    /// Returns an error when the agent is not loaded or launchctl fails.
    pub fn kickstart(&self, target: &LaunchdTarget) -> Result<(), MacosOwnerExecutionError> {
        run_launchctl(&["kickstart", &target.service()])
    }

    /// Restart a loaded agent now (`launchctl kickstart -k`), killing the
    /// running instance first.
    ///
    /// # Errors
    ///
    /// Returns an error when the agent is not loaded or launchctl fails.
    pub fn kickstart_restart(
        &self,
        target: &LaunchdTarget,
    ) -> Result<(), MacosOwnerExecutionError> {
        run_launchctl(&["kickstart", "-k", &target.service()])
    }

    /// Start the agent: kickstart when loaded, bootstrap the plist otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl fails.
    pub fn start(
        &self,
        target: &LaunchdTarget,
        plist: &Path,
    ) -> Result<(), MacosOwnerExecutionError> {
        if self.is_loaded(target)? {
            self.kickstart(target)
        } else {
            self.bootstrap(plist)
        }
    }

    /// Ask a running agent to exit with SIGTERM, leaving it loaded. An
    /// agent that is not loaded is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl fails for a loaded agent.
    pub fn stop(&self, target: &LaunchdTarget) -> Result<(), MacosOwnerExecutionError> {
        if !self.is_loaded(target)? {
            return Ok(());
        }
        run_launchctl(&["kill", "SIGTERM", &target.service()])
    }

    /// Flip launchd's persistent autostart gate for the agent.
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl fails.
    pub fn set_autostart(
        &self,
        target: &LaunchdTarget,
        enabled: bool,
    ) -> Result<(), MacosOwnerExecutionError> {
        let verb = if enabled { "enable" } else { "disable" };
        run_launchctl(&[verb, &target.service()])
    }

    /// Whether launchd's autostart gate is open for the agent.
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl cannot report the domain's disabled set.
    pub fn autostart_enabled(
        &self,
        target: &LaunchdTarget,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let output = launchctl_output(&["print-disabled", &self.domain()])?;
        if !output.status.success() {
            return Err(MacosOwnerExecutionError::new(
                "launchctl failed to inspect service autostart state",
            ));
        }
        Ok(!launchctl_service_disabled(
            &bounded_text(&output.stdout),
            target.label(),
        ))
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }
}

/// Resolve the agent plist path for a target inside a LaunchAgents dir.
#[must_use]
pub fn launch_agent_plist(launch_agents: &Path, target: &LaunchdTarget) -> PathBuf {
    launch_agents.join(target.plist_file_name())
}

/// Parse `launchctl print-disabled gui/<uid>` output for one label.
#[must_use]
pub fn launchctl_service_disabled(output: &str, label: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        line.contains(&format!("\"{label}\"")) && line.ends_with("=> true")
    })
}

/// Parse the `pid = <n>` line of `launchctl print gui/<uid>/<label>`.
#[must_use]
pub fn parse_launchctl_print_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("pid = ")
            .and_then(|pid| pid.trim().parse::<u32>().ok())
    })
}

fn bounded_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_LAUNCHCTL_OUTPUT_BYTES)]).into_owned()
}

fn launchctl_output(args: &[&str]) -> Result<Output, MacosOwnerExecutionError> {
    Command::new(LAUNCHCTL_PATH)
        .args(args)
        .output()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

fn run_launchctl(args: &[&str]) -> Result<(), MacosOwnerExecutionError> {
    let output = launchctl_output(args)?;
    if output.status.success() {
        return Ok(());
    }
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    stderr.truncate(4_096);
    Err(MacosOwnerExecutionError::new(format!(
        "launchctl {} failed with {}: {}",
        args.join(" "),
        output.status,
        stderr.trim()
    )))
}
