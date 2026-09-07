//! Neutral daemon service identity shared by every launcher and client.
//!
//! A launcher (desktop app supervisor, launchd, systemd, the Windows
//! Service Control Manager, Homebrew, or a terminal user) declares who it
//! is through the launcher metadata channel. The daemon corroborates that
//! claim against the platform authority before it reports the identity in
//! the status API and on the event bus. These types are data only: the
//! corroboration rules live in the platform adapters.

use std::fmt::{self, Write as _};

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

/// Environment variable carrying the private protected-control credential
/// from a trusted launcher into its supervised daemon child.
pub const PROTECTED_CONTROL_CREDENTIAL_ENV: &str = "HYPERCOLOR_PROTECTED_CONTROL_CREDENTIAL";

/// Entropy bytes encoded in one protected-control credential.
pub const PROTECTED_CONTROL_CREDENTIAL_BYTES: usize = 32;

const PROTECTED_CONTROL_CREDENTIAL_PREFIX: &str = "hc_pc_";

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

/// Private bearer credential for one trusted daemon session.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ProtectedControlCredential(String);

impl ProtectedControlCredential {
    /// Construct a canonical protected-control credential from 256 bits.
    #[must_use]
    pub fn from_bytes(bytes: [u8; PROTECTED_CONTROL_CREDENTIAL_BYTES]) -> Self {
        let mut value = String::with_capacity(
            PROTECTED_CONTROL_CREDENTIAL_PREFIX.len() + PROTECTED_CONTROL_CREDENTIAL_BYTES * 2,
        );
        value.push_str(PROTECTED_CONTROL_CREDENTIAL_PREFIX);
        for byte in bytes {
            write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(value)
    }

    /// Generate a fresh process-session credential from operating-system
    /// randomness through UUID v4.
    #[must_use]
    pub fn generate() -> Self {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let replacement = uuid::Uuid::new_v4();
        let mut bytes = [0_u8; PROTECTED_CONTROL_CREDENTIAL_BYTES];
        bytes[..16].copy_from_slice(first.as_bytes());
        bytes[16..].copy_from_slice(second.as_bytes());
        for (index, value) in [6, 8, 22, 24]
            .into_iter()
            .zip(replacement.as_bytes().iter().copied())
        {
            bytes[index] = value;
        }
        Self::from_bytes(bytes)
    }

    /// Parse and validate a canonical protected-control credential.
    ///
    /// # Errors
    ///
    /// Returns an error without echoing the secret when the prefix, length,
    /// or lowercase hexadecimal payload is invalid.
    pub fn parse(value: &str) -> Result<Self, ProtectedControlCredentialParseError> {
        let valid = value
            .strip_prefix(PROTECTED_CONTROL_CREDENTIAL_PREFIX)
            .filter(|hex| hex.len() == PROTECTED_CONTROL_CREDENTIAL_BYTES * 2)
            .is_some_and(|hex| {
                hex.bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            });
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(ProtectedControlCredentialParseError)
        }
    }

    /// Explicitly expose the bearer value for authenticated local transport.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProtectedControlCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProtectedControlCredential([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for ProtectedControlCredential {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// A protected-control credential failed canonical validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("protected-control credential is not a canonical 256-bit token")]
pub struct ProtectedControlCredentialParseError;

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
