//! The `HypercolorUpdate` service and its recovery bootstrap.
//!
//! The service's image is a bootstrap, a small program that picks the unit
//! the activator runs from and supervises it. It lives in one of two fixed
//! slots outside the units. `ImagePath` names the active slot, and the
//! service's final failure action runs the other one, which always holds
//! the last bootstrap known to work, with `--recover-fallback`.
//!
//! The service manager supervises recovery on its own: a run that ends
//! badly is restarted with no argument, which means `recover`, and a
//! restart that ends badly again falls back to the retained slot. That only
//! covers an image that reached the service manager. One that never does
//! (it will not load, or exits first) gets a failed start and no failure
//! action at all, so a new bootstrap proves it loads before `ImagePath` may
//! name it.
//!
//! Everything here that decides what happens sits behind
//! [`UpdateServiceConfig`] and [`UpdateServiceHost`], so it runs against
//! fakes on any platform.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::Duration;

use super::machine_layout::BootstrapSlot;

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::{ScmUpdateService, run_slot};

/// The flag the final failure action runs the retained slot with.
pub const RECOVER_FALLBACK_FLAG: &str = "--recover-fallback";
/// The flag an installer registers the service with, from the slot that
/// becomes its image.
pub const REGISTER_SERVICE_FLAG: &str = "--register-service";
/// The flag that proves a bootstrap image loads, by exiting zero.
pub const PROBE_FLAG: &str = "--probe";
/// Names, in the activator's environment, the server end of the update
/// pipe it inherited. The bootstrap creates the pipe because a client
/// accepts only the process the service manager registered; the activator
/// serves it through this handle.
pub const INHERITED_PIPE_VARIABLE: &str = "HYPERCOLOR_UPDATE_PIPE_HANDLE";
/// How soon a failed run is restarted.
pub const RESTART_DELAY: Duration = Duration::from_secs(1);
/// How long a run that failed again waits before the retained slot takes
/// over. The service manager repeats its last action for as long as a
/// failure persists, so this is also the pace of those retries.
pub const FALLBACK_DELAY: Duration = Duration::from_secs(30);
/// Two failures further apart than this are unrelated: the count restarts,
/// so a bootstrap only loses its slot to failures in a row.
pub const FAILURE_RESET_PERIOD: Duration = Duration::from_secs(60);

/// Service exit code when no release could be named.
pub const EXIT_NO_RELEASE: u32 = 1001;
/// Service exit code when the update pipe could not be created or published.
pub const EXIT_NO_PIPE: u32 = 1002;
/// Service exit code when the activator could not be started.
pub const EXIT_NO_ACTIVATOR: u32 = 1003;

/// What one start of the service asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateServiceMode {
    Prepare,
    Activate,
    Recover,
}

impl UpdateServiceMode {
    /// The mode that the arguments of one start name: `prepare` or
    /// `activate`, and none at all for `recover`, which is how boot and the
    /// service manager's own restarts arrive.
    #[must_use]
    pub fn parse(arguments: &[OsString]) -> Option<Self> {
        match arguments {
            [] => Some(Self::Recover),
            [mode] if mode == "prepare" => Some(Self::Prepare),
            [mode] if mode == "activate" => Some(Self::Activate),
            _ => None,
        }
    }

    /// The arguments that run this mode through the unit's CLI.
    #[must_use]
    pub const fn activator_arguments(self) -> [&'static str; 3] {
        let mode = match self {
            Self::Prepare => "prepare",
            Self::Activate => "activate",
            Self::Recover => "recover",
        };
        ["update", "activator", mode]
    }
}

/// The line the bootstrap writes to the activator's standard input when a
/// user signs in. Session notifications only reach the process the service
/// manager registered, and a recovery waiting for its owner needs them.
#[must_use]
pub fn session_logon_notice(session_id: u32) -> String {
    format!("session-logon {session_id}\n")
}

/// How one run of the service ends, as the service manager sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateServiceExit {
    /// `SERVICE_STOPPED` with no error, which queues no failure action.
    Stopped,
    /// `SERVICE_STOPPED` with this service-specific code, which queues the
    /// failure actions.
    Failed(u32),
}

impl UpdateServiceExit {
    /// The end of a run whose activator exited with `code`.
    ///
    /// Only zero stops quietly. The service manager queues failure actions
    /// for every other exit, so an activator that wants no recovery
    /// (another holder of the lock, a pending transaction, a blocked
    /// rollback) exits zero and says why in its own records.
    #[must_use]
    pub const fn of_activator(code: i32) -> Self {
        if code == 0 {
            Self::Stopped
        } else {
            // A process killed by an exception exits with an NTSTATUS, which
            // only reads as a negative number.
            Self::Failed(code.cast_unsigned())
        }
    }
}

/// What one run of the service needs from its host.
pub trait UpdateServiceHost {
    /// What the bootstrap holds for the activator to serve.
    type Pipe;
    /// What names the activator: the unit it runs from.
    type Activator;
    /// Why no activator can be named.
    type Error;

    /// Name the activator of the unit that may run now.
    ///
    /// # Errors
    ///
    /// Returns the host's error when no release can be named.
    fn select_activator(&mut self) -> Result<Self::Activator, Self::Error>;

    /// Create the update pipe in this process and publish its name.
    ///
    /// # Errors
    ///
    /// Returns the host's error when it cannot be created or published.
    fn open_pipe(&mut self) -> io::Result<Self::Pipe>;

    /// Run the activator to its end and return its exit code.
    ///
    /// # Errors
    ///
    /// Returns the host's error when it cannot be started.
    fn run_activator(
        &mut self,
        activator: Self::Activator,
        mode: UpdateServiceMode,
        pipe: Self::Pipe,
    ) -> io::Result<i32>;
}

/// One run of the service: read what must run, open the pipe, run it.
///
/// Arguments no mode names end the run quietly. Whoever may start the
/// service can pass anything, and that must not count as a failure the
/// service manager acts on.
pub fn run_update_service<H: UpdateServiceHost>(
    host: &mut H,
    arguments: &[OsString],
) -> UpdateServiceExit {
    let Some(mode) = UpdateServiceMode::parse(arguments) else {
        return UpdateServiceExit::Stopped;
    };
    let Ok(activator) = host.select_activator() else {
        return UpdateServiceExit::Failed(EXIT_NO_RELEASE);
    };
    let Ok(pipe) = host.open_pipe() else {
        return UpdateServiceExit::Failed(EXIT_NO_PIPE);
    };
    match host.run_activator(activator, mode, pipe) {
        Ok(code) => UpdateServiceExit::of_activator(code),
        Err(_) => UpdateServiceExit::Failed(EXIT_NO_ACTIVATOR),
    }
}

/// The command line the final failure action runs: the retained slot, in
/// `--recover-fallback` mode. The image path holds spaces, so it is quoted.
#[must_use]
pub fn fallback_command(image: &Path) -> OsString {
    let mut command = OsString::from("\"");
    command.push(image);
    command.push("\" ");
    command.push(RECOVER_FALLBACK_FLAG);
    command
}

/// What a bootstrap process was started to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapCommand {
    /// Connect to the service manager and run as the service.
    Service,
    /// Take the service back to this slot and start it.
    RecoverFallback,
    /// Register the service on this slot.
    RegisterService,
    /// Exit zero, which proves the image loads.
    Probe,
}

impl BootstrapCommand {
    /// The command in a process's own arguments. A service's start arguments
    /// never arrive here, so nobody allowed only to start the service can
    /// reach anything but [`Self::Service`].
    #[must_use]
    pub fn parse(arguments: &[OsString]) -> Option<Self> {
        match arguments {
            [] => Some(Self::Service),
            [flag] if flag == RECOVER_FALLBACK_FLAG => Some(Self::RecoverFallback),
            [flag] if flag == REGISTER_SERVICE_FLAG => Some(Self::RegisterService),
            [flag] if flag == PROBE_FLAG => Some(Self::Probe),
            _ => None,
        }
    }
}

/// The service's security descriptor: the machine's administrators, and the
/// recorded user, who may start it and watch it and nothing else.
#[must_use]
pub fn update_service_sddl(user_sid: &str) -> String {
    format!(
        "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;LCRP;;;{user_sid})"
    )
}

/// The service's registration, one change at a time.
///
/// The service control manager keeps `ImagePath` and the failure actions in
/// the registry, which Windows writes to disk lazily: a power cut can take
/// back either change until [`flush`](Self::flush) returns. A slot's file
/// is durable on return.
pub trait UpdateServiceConfig {
    /// Point `ImagePath` at `slot`.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the change is refused.
    fn set_image(&mut self, slot: BootstrapSlot) -> io::Result<()>;

    /// Point the final failure action at `slot`.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the change is refused.
    fn set_fallback(&mut self, slot: BootstrapSlot) -> io::Result<()>;

    /// Write every registration change made so far to disk.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the registry cannot be written.
    fn flush(&mut self) -> io::Result<()>;

    /// Durably replace the bootstrap in `slot` with the one at `source`.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the replacement fails.
    fn write_slot(&mut self, slot: BootstrapSlot, source: &Path) -> io::Result<()>;

    /// Prove the bootstrap in `slot` loads and runs.
    ///
    /// # Errors
    ///
    /// Returns an error when it does not start or does not exit zero.
    fn probe_slot(&mut self, slot: BootstrapSlot) -> io::Result<()>;

    /// Start the service with no argument. A service that already runs
    /// counts as started.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the service cannot be started.
    fn start(&mut self) -> io::Result<()>;
}

/// `--recover-fallback`, run from the retained slot `own`: take the service
/// back to this slot and start it. No recovery happens in this process, so
/// recovery always runs as the registered service, under its supervision.
///
/// The registration is on disk before the start, so the recovery that start
/// runs never journals under an image a power cut could take back. Killed
/// before the start, this leaves the service stopped on its last image, and
/// the next boot's auto-start fails into this same fallback.
///
/// # Errors
///
/// Returns the first change the service manager refused.
pub fn recover_fallback<S: UpdateServiceConfig>(
    service: &mut S,
    own: BootstrapSlot,
) -> io::Result<()> {
    service.set_image(own)?;
    service.set_fallback(own)?;
    service.flush()?;
    service.start()
}

/// Replace the bootstrap as part of a transaction, resuming wherever a
/// crash left it. `proven` is the slot `ImagePath` named when the
/// transaction began, recorded so a resumed step cannot mistake the new
/// slot for the proven one.
///
/// The failure action reaches the disk before the slot it used to name is
/// written, so at every crash and power cut the failure action names the
/// proven slot, and the slot it names is never the one being written.
/// `ImagePath` moves last and only to a bootstrap that loaded: the service
/// manager runs no failure action for an image that never reaches it, so
/// nothing could recover from naming one. That last change reaches the disk
/// when the caller next flushes, before anything records it.
///
/// # Errors
///
/// Returns the first step that failed.
pub fn install_bootstrap<S: UpdateServiceConfig>(
    service: &mut S,
    proven: BootstrapSlot,
    source: &Path,
) -> io::Result<()> {
    let next = proven.other();
    service.set_fallback(proven)?;
    service.flush()?;
    service.write_slot(next, source)?;
    service.probe_slot(next)?;
    service.set_image(next)
}

/// Take back [`install_bootstrap`]: `ImagePath` returns to `proven`, which
/// the failure action already names, so no crash between the two leaves
/// both pointing at the new bootstrap.
///
/// # Errors
///
/// Returns the service manager's refusal.
pub fn restore_bootstrap<S: UpdateServiceConfig>(
    service: &mut S,
    proven: BootstrapSlot,
) -> io::Result<()> {
    service.set_image(proven)
}

#[cfg(test)]
#[path = "update_service_tests.rs"]
mod tests;
