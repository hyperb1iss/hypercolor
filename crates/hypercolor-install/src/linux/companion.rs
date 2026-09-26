//! Companion units: systemd user units a release ships as templates for the
//! installer to render with the installation's recorded paths.
//!
//! They belong to the installer contract, like the launcher. The installer
//! renders them when it publishes the launcher and keeps the rendered text
//! inside the launcher directory, so an ordinary install, whatever templates
//! its release ships, never changes the unit text a recovery runs under.
//! After a managed install settles on a release, the installer puts every
//! rendered unit that is missing into the systemd user directory and enables
//! the ones the release asked for; a unit file that differs from the
//! rendered text is left alone and reported.
//!
//! Templates name paths only through placeholders:
//!
//! | Placeholder | Value |
//! | --- | --- |
//! | `@LAUNCHER@` | `<release root>/launcher/hypercolor __launch` |
//! | `@RELEASE_ROOT@`, `@STATE_ROOT@`, `@DATA_ROOT@`, `@CONFIG_ROOT@` | the recorded roots |
//! | `@DAEMON_STATE_ROOT@` | the directory holding the update state root |
//! | `@USER_UNIT_DIR@` | `~/.config/systemd/user` |
//! | `@OPTIONAL_LAYOUT_PATHS@` | every [`linux_layout_directories`](super::linux_layout_directories) entry, each prefixed `-` |
//!
//! Recorded paths hold only letters, digits, `/`, `.`, `_` and `-`, which
//! systemd reads literally, so the rendered text needs no further escaping.
//! Any other `@NAME@` token refuses the template.

use std::path::Path;

use super::super::InstallPlatformError;
use super::LinuxInstallLocation;
use super::bootstrap::{LINUX_LAUNCH_COMMAND, linux_launcher_path};
use super::executor::LinuxInstallExecutor;
use super::model::{LinuxExactEntry, LinuxFilePublication, error};

/// The largest rendered companion unit.
pub const MAX_COMPANION_UNIT_BYTES: usize = 32 * 1024;
const COMPANION_UNIT_MODE: u32 = 0o644;

/// One rendered companion unit an installation's launcher carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCompanionUnit {
    name: String,
    text: Vec<u8>,
    enable: bool,
}

impl LinuxCompanionUnit {
    pub(super) fn new(name: String, text: Vec<u8>, enable: bool) -> Self {
        Self { name, text, enable }
    }

    /// The unit's file name in the systemd user directory.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The rendered unit text.
    #[must_use]
    pub fn text(&self) -> &[u8] {
        &self.text
    }

    /// Whether the installer enables the unit.
    #[must_use]
    pub const fn enable(&self) -> bool {
        self.enable
    }
}

/// What putting an installation's companion units in place did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxCompanionReport {
    /// Units written because they were missing.
    pub installed: Vec<String>,
    /// Units enabled as their release asked.
    pub enabled: Vec<String>,
    /// Units left alone, with why.
    pub refused: Vec<(String, String)>,
}

/// Render one companion template for `location`.
///
/// # Errors
/// Refuses a template that is not UTF-8, names an unknown placeholder, or
/// renders past [`MAX_COMPANION_UNIT_BYTES`].
pub fn render_linux_companion_unit(
    template: &[u8],
    location: &LinuxInstallLocation,
    home: &Path,
) -> Result<Vec<u8>, InstallPlatformError> {
    let template =
        std::str::from_utf8(template).map_err(|_| error("a companion template is not UTF-8"))?;
    let text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| error("installation paths must be exact UTF-8"))
    };
    let layout = super::linux_layout_directories(home)
        .iter()
        .map(|directory| text(directory).map(|directory| format!("-{directory}")))
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let values = [
        (
            "LAUNCHER",
            format!(
                "{} {LINUX_LAUNCH_COMMAND}",
                text(&linux_launcher_path(location))?
            ),
        ),
        ("RELEASE_ROOT", text(location.release_root())?),
        ("STATE_ROOT", text(location.state_root())?),
        ("DATA_ROOT", text(location.data_root())?),
        ("CONFIG_ROOT", text(location.config_root())?),
        ("DAEMON_STATE_ROOT", text(location.daemon_state_root())?),
        ("USER_UNIT_DIR", text(&home.join(".config/systemd/user"))?),
        ("OPTIONAL_LAYOUT_PATHS", layout),
    ];
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('@') {
        rendered.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let token_length = after
            .bytes()
            .take_while(|byte| byte.is_ascii_uppercase() || *byte == b'_')
            .count();
        if token_length > 0 && after.as_bytes().get(token_length) == Some(&b'@') {
            let token = &after[..token_length];
            let value = values
                .iter()
                .find(|(name, _)| *name == token)
                .map(|(_, value)| value)
                .ok_or_else(|| {
                    error(format!(
                        "a companion template names the unknown placeholder @{token}@"
                    ))
                })?;
            rendered.push_str(value);
            rest = &after[token_length + 1..];
        } else {
            rendered.push('@');
            rest = after;
        }
    }
    rendered.push_str(rest);
    if rendered.len() > MAX_COMPANION_UNIT_BYTES {
        return Err(error("a rendered companion unit exceeds its byte bound"));
    }
    Ok(rendered.into_bytes())
}

/// Put every companion unit an installation's launcher carries in place.
///
/// A missing unit is written; an identical one is kept; one that differs
/// is left alone and reported, since a person or a package made it. Units
/// asked for are enabled, and the manager reloads when anything changed.
///
/// # Errors
/// Returns an executor error; units handled before it stay handled.
pub fn apply_linux_companion_units<E: LinuxInstallExecutor>(
    executor: &mut E,
    units: &[LinuxCompanionUnit],
) -> Result<LinuxCompanionReport, InstallPlatformError> {
    let mut report = LinuxCompanionReport::default();
    for unit in units {
        let (entry, bytes) =
            executor.companion_unit_entry(unit.name(), MAX_COMPANION_UNIT_BYTES)?;
        match &entry {
            LinuxExactEntry::Absent => {
                executor.replace_companion_unit(
                    unit.name(),
                    &entry,
                    Some(&LinuxFilePublication {
                        mode: COMPANION_UNIT_MODE,
                        contents: unit.text().to_vec(),
                    }),
                )?;
                report.installed.push(unit.name().to_owned());
            }
            LinuxExactEntry::RegularFile { .. } if bytes == unit.text() => {}
            LinuxExactEntry::RegularFile { .. } | LinuxExactEntry::Symlink { .. } => {
                report.refused.push((
                    unit.name().to_owned(),
                    "differs from the unit this installation's launcher renders; left in place"
                        .to_owned(),
                ));
                continue;
            }
        }
        if unit.enable() {
            executor.enable_companion_unit(unit.name(), true)?;
            report.enabled.push(unit.name().to_owned());
        }
    }
    if !report.installed.is_empty() || !report.enabled.is_empty() {
        executor.reload_manager()?;
    }
    Ok(report)
}
