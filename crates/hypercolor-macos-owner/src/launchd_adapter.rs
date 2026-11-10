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
#[cfg(target_os = "macos")]
use std::time::Duration;
#[cfg(any(test, target_os = "macos"))]
use std::time::Instant;

use crate::MacosOwnerExecutionError;
#[cfg(any(test, target_os = "macos"))]
use crate::{MacosLaunchdJob, parse_launchd_job};

/// The launchctl binary every adapter call executes.
pub const LAUNCHCTL_PATH: &str = "/bin/launchctl";

/// Upper bound on captured launchctl output.
const MAX_LAUNCHCTL_OUTPUT_BYTES: usize = 64 * 1024;

/// Bound on a bootout's wait for the job's process. launchd kills a job at
/// its own exit timeout (5 s for these agents), so this only bounds a
/// launchd that never does.
#[cfg(target_os = "macos")]
const BOOTOUT_TIMEOUT: Duration = Duration::from_secs(30);

/// A watch on one process's exit, armed before anything can end it.
#[cfg(any(test, target_os = "macos"))]
pub(crate) trait ExitWatch {
    /// Whether the watched process exited by `deadline`.
    fn exited_by(&mut self, deadline: Instant) -> Result<bool, MacosOwnerExecutionError>;
}

#[cfg(target_os = "macos")]
impl ExitWatch for crate::ProcessExitWatch {
    fn exited_by(&mut self, deadline: Instant) -> Result<bool, MacosOwnerExecutionError> {
        self.wait_until(deadline)
    }
}

/// One finished launchctl call.
#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchctlReply {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// What launchd and the kernel answer the adapter's proofs.
#[cfg(any(test, target_os = "macos"))]
trait LaunchdHost {
    type ExitWatch: ExitWatch;

    fn launchctl(&mut self, args: &[&str]) -> Result<LaunchctlReply, MacosOwnerExecutionError>;

    /// Arm a watch on the exit of process `pid`. A process already gone has
    /// exited.
    fn watch_exit(&mut self, pid: u32) -> Result<Self::ExitWatch, MacosOwnerExecutionError>;
}

#[cfg(target_os = "macos")]
struct NativeLaunchdHost;

#[cfg(target_os = "macos")]
impl LaunchdHost for NativeLaunchdHost {
    type ExitWatch = crate::ProcessExitWatch;

    fn launchctl(&mut self, args: &[&str]) -> Result<LaunchctlReply, MacosOwnerExecutionError> {
        let output = launchctl_output(args)?;
        Ok(LaunchctlReply {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn watch_exit(
        &mut self,
        pid: u32,
    ) -> Result<crate::ProcessExitWatch, MacosOwnerExecutionError> {
        crate::ProcessExitWatch::arm(pid)
    }
}

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
    /// it if it runs, and return once launchd no longer holds it. A target
    /// that is not loaded is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error when launchctl refuses the bootout, or when the job
    /// is still loaded once its process has exited or the wait ran out.
    #[cfg(target_os = "macos")]
    pub fn bootout(&self, target: &LaunchdTarget) -> Result<(), MacosOwnerExecutionError> {
        let deadline = Instant::now()
            .checked_add(BOOTOUT_TIMEOUT)
            .ok_or_else(|| MacosOwnerExecutionError::new("bootout deadline overflowed"))?;
        bootout_until_gone(&mut NativeLaunchdHost, target, deadline)
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

/// Unload `target` and prove launchd dropped it by `deadline`.
///
/// launchctl answers a bootout once launchd has signalled the job, which
/// runs on until it exits or launchd kills it at its exit timeout, and
/// launchd drops the label when that process ends. So the proof arms a watch
/// on the job's process before submitting, waits on its exit, and only then
/// asks launchd. A loaded job with no process leaves with the bootout itself.
#[cfg(any(test, target_os = "macos"))]
fn bootout_until_gone(
    host: &mut impl LaunchdHost,
    target: &LaunchdTarget,
    deadline: Instant,
) -> Result<(), MacosOwnerExecutionError> {
    let mut exit = match launchd_job(host, target)? {
        MacosLaunchdJob::NotLoaded => return Ok(()),
        MacosLaunchdJob::Idle => None,
        MacosLaunchdJob::Running { pid } => Some(host.watch_exit(pid)?),
    };
    let service = target.service();
    let reply = host.launchctl(&["bootout", &service])?;
    if reply.status != Some(0) {
        // `Boot-out failed: 3: No such process`: it left on its own.
        if launchd_job(host, target)? == MacosLaunchdJob::NotLoaded {
            return Ok(());
        }
        return Err(launchctl_failure(&["bootout", &service], &reply));
    }
    if let Some(exit) = exit.as_mut()
        && !exit.exited_by(deadline)?
    {
        return Err(MacosOwnerExecutionError::new(format!(
            "{} did not exit before its bootout deadline",
            target.label()
        )));
    }
    match launchd_job(host, target)? {
        MacosLaunchdJob::NotLoaded => Ok(()),
        job => Err(MacosOwnerExecutionError::new(format!(
            "launchd still holds {} after its bootout: {job:?}",
            target.label()
        ))),
    }
}

/// What launchd holds for `target`, read from exact `launchctl print` output.
#[cfg(any(test, target_os = "macos"))]
fn launchd_job(
    host: &mut impl LaunchdHost,
    target: &LaunchdTarget,
) -> Result<MacosLaunchdJob, MacosOwnerExecutionError> {
    let uid = target
        .uid
        .parse()
        .map_err(|_| MacosOwnerExecutionError::new("launchd user id is not numeric"))?;
    let reply = host.launchctl(&["print", &target.service()])?;
    parse_launchd_job(
        target.label(),
        uid,
        reply.status,
        &reply.stdout,
        &reply.stderr,
    )
}

#[cfg(any(test, target_os = "macos"))]
fn launchctl_failure(args: &[&str], reply: &LaunchctlReply) -> MacosOwnerExecutionError {
    let mut stderr = String::from_utf8_lossy(&reply.stderr).into_owned();
    stderr.truncate(4_096);
    MacosOwnerExecutionError::new(format!(
        "launchctl {} failed with status {:?}: {}",
        args.join(" "),
        reply.status,
        stderr.trim()
    ))
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
        line.contains(&format!("\"{label}\""))
            && (line.ends_with("=> disabled") || line.ends_with("=> true"))
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
        .env("LANG", "C")
        .env("LC_ALL", "C")
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

#[cfg(test)]
mod tests;
