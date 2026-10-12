//! What the host is running, as the platform crates report it.
//!
//! Platform crates fill a [`HostSoftwareSnapshot`] from procfs on Linux or
//! WMI on Windows, and core matches it against the catalog of RGB tools
//! that compete with Hypercolor for devices. A command line can carry
//! anything a program was launched with, so snapshots stay inside the
//! daemon: only matched product and process names reach the API.

/// One running process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostProcess {
    /// Process name as the OS reports it (`SignalRgb.exe`, `openrgb`).
    pub name: String,
    /// Full command line, when the OS lets the daemon read it.
    pub command_line: Option<String>,
}

impl HostProcess {
    /// A process known only by name.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command_line: None,
        }
    }

    /// A process with its full command line.
    #[must_use]
    pub fn with_command_line(name: impl Into<String>, command_line: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command_line: Some(command_line.into()),
        }
    }
}

/// Running processes and services at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostSoftwareSnapshot {
    /// Every process the daemon could see.
    pub processes: Vec<HostProcess>,
    /// Names of running OS services (Windows service names). Empty where the
    /// platform has no separate service inventory; Linux daemons show up as
    /// processes.
    pub services: Vec<String>,
}

/// What one attempt to take a host inventory produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostInventory {
    /// The processes and services running now.
    Listed(HostSoftwareSnapshot),
    /// This platform has no inventory, so nothing can be known.
    Unsupported,
    /// The platform has an inventory, but this attempt failed (WMI was
    /// unreachable, `/proc` could not be listed). It says nothing about
    /// what is running.
    Failed,
}

/// The privileged broker an unprivileged daemon goes through to reach
/// hardware buses (`SMBus` controllers, CPU sensor registers), as the host
/// service manager reports it. Platforms without such a broker report
/// none at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareBrokerReport {
    /// Service-manager name of the broker.
    pub service_name: String,
    /// What the service manager says the broker is doing.
    pub state: HardwareBrokerState,
    /// Where the broker writes its own log, when the platform knows.
    pub log_path: Option<String>,
    /// How to bring a missing or stopped broker back, in the platform's
    /// own terms.
    pub remedy: String,
}

/// Broker lifecycle in platform-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardwareBrokerState {
    /// The daemon reaches the hardware itself, so no broker is needed.
    NotNeeded,
    /// No broker is registered with the service manager.
    NotInstalled,
    /// Registered and running. `answering` says whether it replied on its
    /// local channel when asked just now.
    Running {
        /// Whether the broker replied to a liveness probe.
        answering: bool,
    },
    /// Registered and starting, stopping, or paused.
    Transitioning {
        /// The service manager's word for the state.
        state: String,
    },
    /// Registered but not running.
    Stopped {
        /// What the service manager recorded about the last exit.
        exit: String,
        /// Whether the service manager starts the broker at boot.
        starts_at_boot: Option<bool>,
    },
    /// The service manager could not be asked.
    Unknown {
        /// Why the query failed.
        detail: String,
    },
}
