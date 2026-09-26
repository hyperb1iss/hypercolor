//! Companion units: systemd user units a release ships as templates for the
//! installer to render with the installation's recorded paths.
//!
//! They belong to the installer contract, like the launcher. The installer
//! renders them when it publishes the launcher and keeps the rendered text
//! inside the launcher directory, so an ordinary install, whatever templates
//! its release ships, never changes the unit text a recovery runs under.
//! After a managed install commits, the installer puts every rendered unit
//! that is missing into the systemd user directory and enables the ones the
//! release asked for; a unit file that differs from the rendered text, or
//! cannot be read, is left alone and reported. A placement that fails is
//! retried by the next install that commits.
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
    let rendered = expand(template, |token| {
        values
            .iter()
            .find(|(name, _)| *name == token)
            .map(|(_, value)| value.clone())
    })?;
    if rendered.len() > MAX_COMPANION_UNIT_BYTES {
        return Err(error("a rendered companion unit exceeds its byte bound"));
    }
    Ok(rendered.into_bytes())
}

/// The placeholders a companion template may name.
const PLACEHOLDERS: [&str; 8] = [
    "LAUNCHER",
    "RELEASE_ROOT",
    "STATE_ROOT",
    "DATA_ROOT",
    "CONFIG_ROOT",
    "DAEMON_STATE_ROOT",
    "USER_UNIT_DIR",
    "OPTIONAL_LAYOUT_PATHS",
];

/// Check a companion template the way rendering reads it, without an
/// installation: it must be UTF-8 and name only known placeholders.
///
/// # Errors
/// Refuses a template that is not UTF-8 or names an unknown placeholder.
pub fn validate_linux_companion_template(template: &[u8]) -> Result<(), InstallPlatformError> {
    let template =
        std::str::from_utf8(template).map_err(|_| error("a companion template is not UTF-8"))?;
    expand(template, |token| {
        PLACEHOLDERS.contains(&token).then(String::new)
    })
    .map(drop)
}

/// Replace every `@NAME@` token (uppercase letters and `_`) with its value,
/// keeping any other `@` literally.
fn expand(
    template: &str,
    value: impl Fn(&str) -> Option<String>,
) -> Result<String, InstallPlatformError> {
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
            let replacement = value(token).ok_or_else(|| {
                error(format!(
                    "a companion template names the unknown placeholder @{token}@"
                ))
            })?;
            rendered.push_str(&replacement);
            rest = &after[token_length + 1..];
        } else {
            rendered.push('@');
            rest = after;
        }
    }
    rendered.push_str(rest);
    Ok(rendered)
}

/// Put every companion unit an installation's launcher carries in place.
///
/// A missing unit is written; an identical one is kept; one that differs,
/// or cannot be read, is left alone and reported, since a person or a
/// package made it. Units asked for are enabled every time, so a committed
/// install re-enables one a user disabled, and the manager reloads when
/// anything changed.
///
/// # Errors
/// Returns an error writing or enabling a unit; units handled before it
/// stay handled.
pub fn apply_linux_companion_units<E: LinuxInstallExecutor>(
    executor: &mut E,
    units: &[LinuxCompanionUnit],
) -> Result<LinuxCompanionReport, InstallPlatformError> {
    let mut report = LinuxCompanionReport::default();
    for unit in units {
        // A file that cannot be read as a unit (too large, or not a plain
        // file) is someone else's; it is reported, and the rest go on.
        let (entry, bytes) =
            match executor.companion_unit_entry(unit.name(), MAX_COMPANION_UNIT_BYTES) {
                Ok(observed) => observed,
                Err(error) => {
                    report
                        .refused
                        .push((unit.name().to_owned(), format!("cannot be read: {error}")));
                    continue;
                }
            };
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
