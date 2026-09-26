//! The systemd user unit a managed installation runs its daemon under.
//!
//! A managed installation's unit names one release directory: its daemon,
//! and that same release's UI and bundled effects. Every install renders a
//! new unit for its candidate and writes it inside the transaction, and a
//! rollback restores the prior unit's exact bytes, so switching `active`
//! never changes what a running or restarting service starts, and no start
//! can mix one release's executable with another's assets.
//!
//! [`LinuxServiceRenderer`] is the one seam another build can use to render
//! a different unit. Each renderer carries a contract ID, which the
//! installer writes as the unit's first line. Validating a transaction
//! re-renders its candidate unit with the renderer that contract names, so
//! a build validates any transaction whose contract it knows, including one
//! a newer build prepared, and refuses one it does not know instead of
//! misjudging it. A unit without that line is the historical direct unit,
//! which runs the daemon through `active` without a sandbox.

use std::path::Path;

use super::super::InstallPlatformError;
use super::LinuxInstallLocation;
use super::model::error;

/// Contract of the public sandboxed unit [`LinuxServiceRenderer::PUBLIC`]
/// renders.
pub const LINUX_PUBLIC_SERVICE_CONTRACT: &str = "hypercolor-public-1";
/// Reserved for the historical direct unit, which carries no contract line.
const LINUX_DIRECT_SERVICE_CONTRACT: &str = "hypercolor-direct-1";

pub(super) const CONTRACT_LINE_PREFIX: &str = "# Hypercolor service contract: ";
const MAX_CONTRACT_BYTES: usize = 64;

/// What a renderer renders a unit from.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct LinuxServiceInput<'a> {
    /// The release directory the unit runs: `<release root>/units/<id>`.
    pub release: &'a Path,
    /// The recorded installation.
    pub location: &'a LinuxInstallLocation,
}

/// A rendered unit, without its contract line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LinuxRenderedService {
    /// The unit file text. It must be a `Type=notify` service with exactly
    /// one `ExecStart`; the installer prepends the contract line.
    pub unit: String,
    /// The argument vector the daemon runs with once the service started,
    /// which the installer's owner proof requires of the running process.
    pub daemon_arguments: Vec<String>,
}

impl LinuxRenderedService {
    /// A rendered unit and the daemon arguments it runs the daemon with.
    #[must_use]
    pub const fn new(unit: String, daemon_arguments: Vec<String>) -> Self {
        Self {
            unit,
            daemon_arguments,
        }
    }
}

type Render = fn(&LinuxServiceInput<'_>) -> Result<LinuxRenderedService, InstallPlatformError>;

/// Renders a managed installation's service unit under one contract.
#[derive(Clone, Copy)]
pub struct LinuxServiceRenderer {
    contract: &'static str,
    render: Render,
}

impl LinuxServiceRenderer {
    /// The public unit: the release's daemon, UI and effects, sandboxed so
    /// it writes only the recorded roots.
    pub const PUBLIC: Self = Self {
        contract: LINUX_PUBLIC_SERVICE_CONTRACT,
        render: render_public,
    };

    /// A renderer for another contract. A contract ID must render the same
    /// unit text for the same input in every build that carries it; change
    /// the text, change the ID.
    ///
    /// # Panics
    /// Panics on the public or the direct contract's ID, which belong to
    /// this crate's own units.
    #[must_use]
    pub const fn new(contract: &'static str, render: Render) -> Self {
        assert!(
            !same(contract, LINUX_PUBLIC_SERVICE_CONTRACT)
                && !same(contract, LINUX_DIRECT_SERVICE_CONTRACT),
            "a renderer cannot claim a contract this crate's own units use"
        );
        Self { contract, render }
    }

    /// The contract ID this renderer writes into its units.
    #[must_use]
    pub const fn contract(&self) -> &'static str {
        self.contract
    }

    pub(super) fn render(
        &self,
        input: &LinuxServiceInput<'_>,
    ) -> Result<LinuxRenderedService, InstallPlatformError> {
        if !valid_contract(self.contract) || self.contract == LINUX_DIRECT_SERVICE_CONTRACT {
            return Err(error(format!(
                "service contract {:?} is not a valid contract ID",
                self.contract
            )));
        }
        (self.render)(input)
    }
}

impl PartialEq for LinuxServiceRenderer {
    fn eq(&self, other: &Self) -> bool {
        self.contract == other.contract
    }
}

impl Eq for LinuxServiceRenderer {}

impl std::fmt::Debug for LinuxServiceRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinuxServiceRenderer")
            .field("contract", &self.contract)
            .finish_non_exhaustive()
    }
}

impl Default for LinuxServiceRenderer {
    fn default() -> Self {
        Self::PUBLIC
    }
}

/// The contract a unit's first line names; `None` for a unit without one,
/// which is the historical direct unit.
pub(super) fn unit_contract(unit: &[u8]) -> Option<&str> {
    let first = unit.split(|byte| *byte == b'\n').next()?;
    let first = first.strip_suffix(b"\r").unwrap_or(first);
    let contract = std::str::from_utf8(first)
        .ok()?
        .strip_prefix(CONTRACT_LINE_PREFIX)?;
    Some(contract)
}

const fn same(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

fn valid_contract(contract: &str) -> bool {
    !contract.is_empty()
        && contract.len() <= MAX_CONTRACT_BYTES
        && contract
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The public unit.
///
/// `ExecStart` names the release's daemon, UI and effects by the release's
/// own directory. The daemon gets the recorded roots as its XDG bases and
/// keeps its caches inside the recorded daemon state. The sandbox makes the
/// whole file system read-only except the recorded configuration, data and
/// daemon state roots, with the release root and the update state root
/// read-only beneath them, and gives the daemon a private `/tmp`.
///
/// systemd refuses to build that sandbox around a writable path that does
/// not exist, so `ExecStartPre=+mkdir` (outside the sandbox) first creates
/// the configuration root if a user deleted it to reset their settings.
/// The data and daemon state roots hold the releases and the update state,
/// so deleting either removes the installation itself. `-<update
/// state>/coordinator` is writable only when a build that uses it has
/// created it; absent, systemd ignores it and the update state stays
/// read-only.
fn render_public(
    input: &LinuxServiceInput<'_>,
) -> Result<LinuxRenderedService, InstallPlatformError> {
    let text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| error("Linux install roots must be exact UTF-8"))
    };
    let base = |root: &Path| {
        root.parent()
            .ok_or_else(|| error("a recorded root has no base directory"))
            .and_then(text)
    };
    let location = input.location;
    let release = text(input.release)?;
    let daemon = format!("{release}/bin/hypercolor-daemon");
    let ui = format!("{release}/share/hypercolor/ui");
    let effects = format!("{release}/share/hypercolor/effects/bundled");
    let config = text(location.config_root())?;
    let data = text(location.data_root())?;
    let daemon_state = text(location.daemon_state_root())?;
    let state = text(location.state_root())?;
    let releases = text(location.release_root())?;
    let config_base = base(location.config_root())?;
    let data_base = base(location.data_root())?;
    let state_base = base(location.daemon_state_root())?;
    let unit = format!(
        "[Unit]\nDescription=Hypercolor RGB Lighting Daemon\nAfter=graphical-session.target dbus.socket\nWants=graphical-session.target\n\n[Service]\nType=notify\nExecStartPre=+mkdir -p -m 0700 {config}\nExecStart={daemon} --ui-dir {ui} --effects-dir {effects}\nWatchdogSec=30\nRestart=on-failure\nRestartSec=3\nEnvironment=HYPERCOLOR_LOG=info\nEnvironment=RUST_BACKTRACE=1\nEnvironment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service\nEnvironment=XDG_CONFIG_HOME={config_base}\nEnvironment=XDG_DATA_HOME={data_base}\nEnvironment=XDG_STATE_HOME={state_base}\nEnvironment=XDG_CACHE_HOME={daemon_state}/cache\nProtectSystem=strict\nProtectHome=read-only\nPrivateTmp=true\nNoNewPrivileges=true\nReadWritePaths={config} {data} {daemon_state} -{state}/coordinator\nReadOnlyPaths={releases} {state}\n\n[Install]\nWantedBy=default.target\n"
    );
    Ok(LinuxRenderedService::new(
        unit,
        vec![
            daemon,
            "--ui-dir".to_owned(),
            ui,
            "--effects-dir".to_owned(),
            effects,
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::{LinuxServiceRenderer, unit_contract};

    #[test]
    fn the_first_line_names_the_contract() {
        assert_eq!(
            unit_contract(b"# Hypercolor service contract: a-1\n[Unit]\n"),
            Some("a-1")
        );
        assert_eq!(
            unit_contract(b"# Hypercolor service contract: a-1\r\n[Unit]\r\n"),
            Some("a-1")
        );
        assert_eq!(unit_contract(b"[Unit]\nDescription=x\n"), None);
        assert_eq!(
            unit_contract(b"# edited\n# Hypercolor service contract: a-1\n"),
            None,
            "only the first line counts"
        );
        assert_eq!(
            unit_contract(b" # Hypercolor service contract: a-1\n"),
            None
        );
        assert_eq!(unit_contract(b""), None);
    }

    #[test]
    #[should_panic(expected = "cannot claim a contract")]
    fn a_renderer_cannot_claim_the_public_contract() {
        let _ = LinuxServiceRenderer::new("hypercolor-public-1", super::render_public);
    }

    #[test]
    #[should_panic(expected = "cannot claim a contract")]
    fn a_renderer_cannot_claim_the_direct_contract() {
        let _ = LinuxServiceRenderer::new("hypercolor-direct-1", super::render_public);
    }
}
