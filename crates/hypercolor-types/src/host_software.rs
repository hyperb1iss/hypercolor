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
