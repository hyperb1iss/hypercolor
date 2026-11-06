//! The daemon's launchd agent, as an app bundle installation runs it.
//!
//! [`MacosBundleLaunchdService`] holds every decision and compiles on every
//! unix host. Each macOS fact or effect is one call on [`MacosBundleAgent`],
//! so the proof and its probation window are tested against fakes anywhere.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::super::InstallPlatformError;
use super::model::{
    MacosBundleConfig, MacosBundleRunningDaemon, MacosBundleServiceObservation, error,
};
use super::service::MacosBundleService;

/// The longest a daemon may take from its start to answering its proof.
pub const MACOS_BUNDLE_START_DEADLINE: Duration = Duration::from_mins(3);

/// Pause between proof attempts while a daemon comes up. The pause is a wait
/// on the daemon's exit, so a crash ends it at once.
const READINESS_RETRY: Duration = Duration::from_millis(250);

/// Reads of a job that keeps changing under its own inspection.
const IDENTITY_ATTEMPTS: usize = 3;

/// What launchd holds for the daemon agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosBundleAgentJob {
    /// The agent is not loaded.
    NotLoaded,
    /// The agent is loaded with no process: its daemon exited, and launchd
    /// may start it again.
    Idle,
    /// The agent runs this process.
    Running { pid: u32 },
}

/// The macOS calls behind [`MacosBundleLaunchdService`].
pub trait MacosBundleAgent {
    /// Whether the agent's definition is installed.
    ///
    /// # Errors
    /// Returns an error when the definition cannot be inspected.
    fn registered(&mut self) -> Result<bool, InstallPlatformError>;

    /// Whether launchd may load the agent.
    ///
    /// # Errors
    /// Returns an error when launchd cannot be queried.
    fn autostart_enabled(&mut self) -> Result<bool, InstallPlatformError>;

    /// `launchctl enable` or `disable`, confirmed.
    ///
    /// # Errors
    /// Returns an error when launchd refuses or the change is not confirmed.
    fn set_autostart(&mut self, enabled: bool) -> Result<(), InstallPlatformError>;

    /// What launchd holds for the agent now.
    ///
    /// # Errors
    /// Returns an error when launchd cannot be queried.
    fn job(&mut self) -> Result<MacosBundleAgentJob, InstallPlatformError>;

    /// The executable path and cdhash of process `pid`, or `None` once it is
    /// gone.
    ///
    /// # Errors
    /// Returns an error when the running code cannot be inspected.
    fn process_image(
        &mut self,
        pid: u32,
    ) -> Result<Option<(PathBuf, String)>, InstallPlatformError>;

    /// Bootstrap the agent and wait up to `timeout` for the daemon at
    /// `executable` with `cdhash` to publish its ownership.
    ///
    /// # Errors
    /// Returns an error when launchd refuses or that publication never comes.
    fn start(
        &mut self,
        executable: &Path,
        cdhash: &str,
        timeout: Duration,
    ) -> Result<(), InstallPlatformError>;

    /// `launchctl bootout`, confirmed. An agent that is not loaded is done.
    ///
    /// # Errors
    /// Returns an error when the agent stays loaded.
    fn unload(&mut self) -> Result<(), InstallPlatformError>;

    /// Wait for the daemon guard to be released, and report whether it was.
    ///
    /// # Errors
    /// Returns an error when the guard cannot be inspected.
    fn wait_for_guard_release(&mut self) -> Result<bool, InstallPlatformError>;

    /// Wait up to `timeout` for `daemon` to publish its ownership, and report
    /// whether it did.
    ///
    /// # Errors
    /// Returns an error when a publication names any other code.
    fn wait_for_publication(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        timeout: Duration,
    ) -> Result<bool, InstallPlatformError>;

    /// One request of the daemon's local API, which must answer healthy and
    /// as `version`.
    ///
    /// # Errors
    /// Returns an error when it does not answer, or answers anything else.
    fn prove_local_api(&mut self, version: &str) -> Result<(), InstallPlatformError>;

    /// Wait up to `timeout` for process `pid` to exit, and report whether it
    /// did. A process that is already gone has exited.
    ///
    /// # Errors
    /// Returns an error when the process cannot be watched.
    fn wait_for_exit(&mut self, pid: u32, timeout: Duration) -> Result<bool, InstallPlatformError>;
}

/// [`MacosBundleService`] over launchd: the decisions, with every macOS call
/// behind `A`.
pub struct MacosBundleLaunchdService<A> {
    agent: A,
    installed_daemon: PathBuf,
    start_deadline: Duration,
}

impl<A: MacosBundleAgent> MacosBundleLaunchdService<A> {
    /// A service for `config`'s installed daemon.
    #[must_use]
    pub fn new(agent: A, config: &MacosBundleConfig) -> Self {
        Self {
            agent,
            installed_daemon: config.installed_daemon(),
            start_deadline: MACOS_BUNDLE_START_DEADLINE,
        }
    }

    /// The same service under a shorter start deadline.
    #[must_use]
    pub const fn with_start_deadline(mut self, start_deadline: Duration) -> Self {
        self.start_deadline = start_deadline;
        self
    }

    #[must_use]
    pub fn into_agent(self) -> A {
        self.agent
    }

    /// The daemon launchd runs now, read twice around its identity so a
    /// restart during the read is never reported as one daemon.
    fn running(&mut self) -> Result<Option<MacosBundleRunningDaemon>, InstallPlatformError> {
        for _ in 0..IDENTITY_ATTEMPTS {
            let MacosBundleAgentJob::Running { pid } = self.agent.job()? else {
                return Ok(None);
            };
            let Some((executable, cdhash)) = self.agent.process_image(pid)? else {
                continue;
            };
            if self.agent.job()? == (MacosBundleAgentJob::Running { pid }) {
                return Ok(Some(MacosBundleRunningDaemon {
                    pid,
                    executable,
                    cdhash,
                }));
            }
        }
        Err(error("the macOS daemon agent keeps restarting"))
    }

    /// One whole proof: the daemon claimed the installation, still runs as
    /// itself, and its local API answers as `version`. Readiness may take
    /// until `deadline`; a daemon that exits first has failed.
    fn prove_once(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        version: &str,
        deadline: Instant,
    ) -> Result<(), InstallPlatformError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !self.agent.wait_for_publication(daemon, remaining)? {
            return Err(error("the macOS daemon never claimed the installation"));
        }
        self.require_running(daemon)?;
        loop {
            let answer = self.agent.prove_local_api(version);
            let Err(failure) = answer else {
                return self.require_running(daemon);
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(failure);
            }
            if self
                .agent
                .wait_for_exit(daemon.pid, remaining.min(READINESS_RETRY))?
            {
                return Err(error(format!(
                    "the macOS daemon exited before it answered: {failure}"
                )));
            }
        }
    }

    fn require_running(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
    ) -> Result<(), InstallPlatformError> {
        if self.running()?.as_ref() == Some(daemon) {
            Ok(())
        } else {
            Err(error("the macOS daemon changed during its proof"))
        }
    }

    /// Hold a proven daemon through its probation window, then repeat the
    /// whole proof with no readiness allowance, which is what catches a
    /// daemon whose API hung while its process lived on.
    ///
    /// The wait is the kernel's exit event, so a crash ends the window at
    /// once. An installer that dies mid-window leaves the journal at the
    /// same step with the same receipt, and the next run holds the daemon
    /// through a whole window again.
    fn hold_probation(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        version: &str,
        window: Duration,
    ) -> Result<(), InstallPlatformError> {
        let started = Instant::now();
        if self.agent.wait_for_exit(daemon.pid, window)? {
            return Err(error(format!(
                "candidate failed probation {:.1} s into its {} s window: the daemon exited",
                started.elapsed().as_secs_f64(),
                window.as_secs()
            )));
        }
        self.prove_once(daemon, version, Instant::now())
            .map_err(|source| {
                error(format!(
                    "candidate failed the proof at the end of its {} s probation window: {source}",
                    window.as_secs()
                ))
            })
    }
}

impl<A: MacosBundleAgent> MacosBundleService for MacosBundleLaunchdService<A> {
    fn observe(&mut self) -> Result<MacosBundleServiceObservation, InstallPlatformError> {
        Ok(MacosBundleServiceObservation {
            registered: self.agent.registered()?,
            enabled: self.agent.autostart_enabled()?,
            running: self.running()?,
        })
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        self.agent.set_autostart(enabled)
    }

    fn start(&mut self, cdhash: &str) -> Result<(), InstallPlatformError> {
        self.agent
            .start(&self.installed_daemon, cdhash, self.start_deadline)
    }

    fn stop(
        &mut self,
        daemon: Option<&MacosBundleRunningDaemon>,
    ) -> Result<(), InstallPlatformError> {
        if self.running()?.as_ref() != daemon {
            return Err(error(
                "the running macOS daemon is not the one this stop names",
            ));
        }
        self.agent.unload()
    }

    fn wait_for_release(&mut self) -> Result<(), InstallPlatformError> {
        if self.agent.wait_for_guard_release()? {
            Ok(())
        } else {
            Err(error("the macOS daemon guard was not released in time"))
        }
    }

    fn prove(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        version: &str,
        probation: Duration,
    ) -> Result<(), InstallPlatformError> {
        let deadline = Instant::now() + self.start_deadline;
        self.prove_once(daemon, version, deadline)?;
        if probation.is_zero() {
            Ok(())
        } else {
            self.hold_probation(daemon, version, probation)
        }
    }
}
