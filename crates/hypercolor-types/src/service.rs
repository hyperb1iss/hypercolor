//! Neutral daemon service identity shared by every launcher and client.
//!
//! A launcher (desktop app supervisor, launchd, systemd, the Windows
//! Service Control Manager, Homebrew, or a terminal user) declares who it
//! is through the launcher metadata channel. The daemon corroborates that
//! claim against the platform authority before it reports the identity in
//! the status API and on the event bus. These types are data only: the
//! corroboration rules live in the platform adapters.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Environment variable through which a launcher declares the neutral
/// daemon identity it speaks for.
///
/// The value is the bounded declaration form produced by
/// [`ServiceIdentity::declaration`]: `<run_mode>[:<manager>[:<unit>]]`.
pub const SERVICE_IDENTITY_ENV: &str = "HYPERCOLOR_SERVICE_IDENTITY";

/// Environment variable a supervising app sets to its own pid when it
/// spawns the daemon as a managed child. The daemon corroborates a
/// supervised-child claim against it (the value must equal the live
/// parent) and keeps it as a diagnostic beside the kernel lifetime guard.
pub const SUPERVISED_PARENT_PID_ENV: &str = "HYPERCOLOR_SUPERVISED_PARENT_PID";

/// Upper bound on the opaque unit label carried by a declaration.
pub const MAX_SERVICE_UNIT_BYTES: usize = 128;

/// The direct launchd agent label and unit.
pub const LAUNCHD_DIRECT_UNIT: &str = "tech.hyperbliss.hypercolor";
/// The Homebrew services launchd label and unit.
pub const HOMEBREW_UNIT: &str = "homebrew.mxcl.hypercolor";
/// The systemd unit name shared by the user and system definitions.
pub const SYSTEMD_UNIT: &str = "hypercolor.service";
/// The Windows Service Control Manager registration name.
pub const WINDOWS_SCM_UNIT: &str = "Hypercolor";

/// Who launched and supervises the daemon process. Neutral across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DaemonRunMode {
    /// Child of the desktop app supervisor (pdeathsig, kqueue, job object).
    SupervisedChild,
    /// Session-scoped service manager: launchd gui domain, systemd `--user`,
    /// or a per-user SCM registration.
    UserService,
    /// Machine-scoped service manager: systemd system unit or SCM system
    /// service.
    SystemService,
    /// Started by a terminal user or an unknown launcher.
    Standalone,
}

impl DaemonRunMode {
    /// Wire name used by the declaration form and serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SupervisedChild => "supervised_child",
            Self::UserService => "user_service",
            Self::SystemService => "system_service",
            Self::Standalone => "standalone",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "supervised_child" => Some(Self::SupervisedChild),
            "user_service" => Some(Self::UserService),
            "system_service" => Some(Self::SystemService),
            "standalone" => Some(Self::Standalone),
            _ => None,
        }
    }
}

impl fmt::Display for DaemonRunMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which service manager registration (if any) the launcher speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ServiceManager {
    Launchd,
    Systemd,
    WindowsScm,
    Homebrew,
}

impl ServiceManager {
    /// Wire name used by the declaration form and serde.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Launchd => "launchd",
            Self::Systemd => "systemd",
            Self::WindowsScm => "windows_scm",
            Self::Homebrew => "homebrew",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "launchd" => Some(Self::Launchd),
            "systemd" => Some(Self::Systemd),
            "windows_scm" => Some(Self::WindowsScm),
            "homebrew" => Some(Self::Homebrew),
            _ => None,
        }
    }
}

impl fmt::Display for ServiceManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Neutral identity of the daemon's launcher, declared by the launcher and
/// corroborated by the platform authority before the daemon reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ServiceIdentity {
    pub run_mode: DaemonRunMode,
    /// Absent for supervised children and standalone launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager: Option<ServiceManager>,
    /// Opaque bounded label: launchd label, systemd unit name, SCM service
    /// name, or Homebrew formula label. Diagnostic, never an authority input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl ServiceIdentity {
    /// Daemon spawned and supervised by the desktop app.
    pub const APP_SIDECAR: Self = Self::plain(DaemonRunMode::SupervisedChild);
    /// Daemon started by a terminal user or an unknown launcher.
    pub const STANDALONE: Self = Self::plain(DaemonRunMode::Standalone);

    const fn plain(run_mode: DaemonRunMode) -> Self {
        Self {
            run_mode,
            manager: None,
            unit: None,
        }
    }

    fn managed(run_mode: DaemonRunMode, manager: ServiceManager, unit: &str) -> Self {
        Self {
            run_mode,
            manager: Some(manager),
            unit: Some(unit.to_owned()),
        }
    }

    /// Direct launchd user agent (`tech.hyperbliss.hypercolor`).
    #[must_use]
    pub fn launchd_direct() -> Self {
        Self::managed(
            DaemonRunMode::UserService,
            ServiceManager::Launchd,
            LAUNCHD_DIRECT_UNIT,
        )
    }

    /// Homebrew services launchd agent (`homebrew.mxcl.hypercolor`).
    #[must_use]
    pub fn homebrew() -> Self {
        Self::managed(
            DaemonRunMode::UserService,
            ServiceManager::Homebrew,
            HOMEBREW_UNIT,
        )
    }

    /// systemd `--user` unit.
    #[must_use]
    pub fn systemd_user() -> Self {
        Self::managed(
            DaemonRunMode::UserService,
            ServiceManager::Systemd,
            SYSTEMD_UNIT,
        )
    }

    /// systemd system unit.
    #[must_use]
    pub fn systemd_system() -> Self {
        Self::managed(
            DaemonRunMode::SystemService,
            ServiceManager::Systemd,
            SYSTEMD_UNIT,
        )
    }

    /// Windows Service Control Manager system service.
    #[must_use]
    pub fn windows_scm() -> Self {
        Self::managed(
            DaemonRunMode::SystemService,
            ServiceManager::WindowsScm,
            WINDOWS_SCM_UNIT,
        )
    }

    /// Whether a service manager registration owns this identity.
    #[must_use]
    pub const fn is_managed(&self) -> bool {
        self.manager.is_some()
    }

    /// Bounded declaration form carried by [`SERVICE_IDENTITY_ENV`]:
    /// `<run_mode>[:<manager>[:<unit>]]`.
    #[must_use]
    pub fn declaration(&self) -> String {
        let mut out = self.run_mode.as_str().to_owned();
        if let Some(manager) = self.manager {
            out.push(':');
            out.push_str(manager.as_str());
            if let Some(unit) = self.unit.as_deref() {
                out.push(':');
                out.push_str(unit);
            }
        }
        out
    }

    /// Parse a declaration produced by [`Self::declaration`].
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, malformed, oversized, or unknown
    /// declaration so a launcher typo never silently becomes standalone.
    pub fn parse_declaration(value: &str) -> Result<Self, ServiceIdentityParseError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ServiceIdentityParseError::Empty);
        }
        let mut parts = value.splitn(3, ':');
        let run_mode = parts
            .next()
            .and_then(DaemonRunMode::parse)
            .ok_or_else(|| ServiceIdentityParseError::UnknownRunMode(value.to_owned()))?;
        let manager = match parts.next() {
            None | Some("") => None,
            Some(manager) => Some(
                ServiceManager::parse(manager)
                    .ok_or_else(|| ServiceIdentityParseError::UnknownManager(manager.to_owned()))?,
            ),
        };
        let unit = match parts.next() {
            None | Some("") => None,
            Some(unit) if unit.len() > MAX_SERVICE_UNIT_BYTES => {
                return Err(ServiceIdentityParseError::UnitTooLong(unit.len()));
            }
            Some(unit) if unit.chars().any(char::is_control) => {
                return Err(ServiceIdentityParseError::UnitNotPrintable);
            }
            Some(unit) => Some(unit.to_owned()),
        };
        if manager.is_none() && unit.is_some() {
            return Err(ServiceIdentityParseError::UnitWithoutManager);
        }
        if manager.is_some() && run_mode.is_unmanaged() {
            return Err(ServiceIdentityParseError::ManagerOnUnmanagedMode(run_mode));
        }
        Ok(Self {
            run_mode,
            manager,
            unit,
        })
    }
}

impl DaemonRunMode {
    /// Supervised children and standalone launches never carry a manager.
    #[must_use]
    pub const fn is_unmanaged(self) -> bool {
        matches!(self, Self::SupervisedChild | Self::Standalone)
    }
}

impl fmt::Display for ServiceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.declaration())
    }
}

/// Why a launcher declaration could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceIdentityParseError {
    #[error("service identity declaration is empty")]
    Empty,
    #[error("unknown daemon run mode in service identity declaration {0:?}")]
    UnknownRunMode(String),
    #[error("unknown service manager {0:?} in service identity declaration")]
    UnknownManager(String),
    #[error("service identity unit label of {0} bytes exceeds {MAX_SERVICE_UNIT_BYTES}")]
    UnitTooLong(usize),
    #[error("service identity unit label contains control characters")]
    UnitNotPrintable,
    #[error("service identity unit label requires a service manager")]
    UnitWithoutManager,
    #[error("daemon run mode {0} never carries a service manager")]
    ManagerOnUnmanagedMode(DaemonRunMode),
}

/// A losing launcher observed beside the active daemon identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ServiceConflict {
    pub active: ServiceIdentity,
    pub contender: ServiceIdentity,
    pub observed_at_ms: u64,
}

/// A handover the active daemon could not complete; the phase is the
/// opaque diagnostic name of the durable journal step it stopped at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ServiceRecoveryRequired {
    pub requested: ServiceIdentity,
    pub prior: ServiceIdentity,
    pub phase: String,
}

/// The daemon's self-reported launcher identity and ownership epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ServiceStatus {
    pub identity: ServiceIdentity,
    /// Monotonic ownership epoch; bumps whenever the active launcher changes.
    #[serde(default)]
    pub owner_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ServiceConflict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_required: Option<ServiceRecoveryRequired>,
}

impl ServiceStatus {
    /// Status for a freshly corroborated identity with no contention.
    #[must_use]
    pub const fn new(identity: ServiceIdentity, owner_epoch: u64) -> Self {
        Self {
            identity,
            owner_epoch,
            conflict: None,
            recovery_required: None,
        }
    }
}

/// Who may stop the running daemon, derived from its service identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "identity")]
pub enum StopAuthority {
    /// The supervising app owns the child's lifetime; nobody else stops it.
    SupervisedChild,
    /// The named service manager registration owns start and stop.
    ServiceManager(ServiceIdentity),
    /// A terminal user started it; only that user stops it.
    UserDirected,
}

impl StopAuthority {
    /// Derive the stop authority for a corroborated identity.
    #[must_use]
    pub fn for_identity(identity: &ServiceIdentity) -> Self {
        match identity.run_mode {
            DaemonRunMode::SupervisedChild => Self::SupervisedChild,
            DaemonRunMode::Standalone => Self::UserDirected,
            DaemonRunMode::UserService | DaemonRunMode::SystemService => {
                if identity.is_managed() {
                    Self::ServiceManager(identity.clone())
                } else {
                    Self::UserDirected
                }
            }
        }
    }
}
