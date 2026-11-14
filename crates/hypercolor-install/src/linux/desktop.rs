//! The desktop entry a managed installation renders for each unit.
//!
//! A release names its launcher by command (`Exec=hypercolor-open`), which
//! system packages put on `PATH`. A managed installation links the entry
//! into `~/.local/share/applications`, and a session whose `PATH` lacks
//! `~/.local/bin` hides an entry whose command it cannot find. The unit is
//! immutable and content addressed, and a regular file in the public layout
//! reads as a legacy install, so the installer renders a copy of the unit's
//! entry beside the units, with every published command spelled as the
//! installation's own absolute path, and links the public entry to that
//! copy. The copy lives and goes with its unit.

use std::borrow::Cow;
use std::io::{self, Read as _};
use std::path::{Component, Path};

use super::super::store::DESKTOP_ENTRIES_DIRECTORY;
use super::super::{InstallPlatformError, UnitId, UnitRecord};
use super::LinuxInstallPlatform;
use super::executor::{LinuxInstallExecutor, public_name, public_parent};
use super::model::{LINUX_LAYOUT_ITEMS, LinuxDirectoryItem, LinuxLayoutItem, error};

/// The rendered entry's file name, the public entry's own, so anything that
/// resolves the link still sees the same desktop file ID.
pub(super) const DESKTOP_ENTRY_NAME: &str = "hypercolor.desktop";
/// Bound for the unit's desktop entry; a real one is a few hundred bytes.
const MAX_DESKTOP_ENTRY_BYTES: u64 = 64 * 1024;
/// Keys whose value starts with the program to run.
const PROGRAM_KEYS: [&[u8]; 2] = [b"Exec=", b"TryExec="];

/// The public link target of `unit`'s rendered desktop entry in the managed
/// installation whose release root is `release_root`.
pub(super) fn rendered_target(release_root: &Path, unit: &UnitId) -> String {
    release_root
        .join(DESKTOP_ENTRIES_DIRECTORY)
        .join(unit.as_str())
        .join(DESKTOP_ENTRY_NAME)
        .to_str()
        .expect("Linux install roots were validated as exact UTF-8")
        .to_owned()
}

/// The unit whose rendered desktop entry `target` is beneath
/// `release_root`, whether or not that unit is still installed.
pub(super) fn rendered_unit(release_root: &Path, target: &str) -> Option<UnitId> {
    let relative = Path::new(target)
        .strip_prefix(release_root.join(DESKTOP_ENTRIES_DIRECTORY))
        .ok()?;
    let mut components = relative.components();
    let (Some(Component::Normal(unit)), Some(Component::Normal(name)), None) =
        (components.next(), components.next(), components.next())
    else {
        return None;
    };
    if name != DESKTOP_ENTRY_NAME {
        return None;
    }
    let unit = UnitId::new(unit.to_str()?).ok()?;
    (rendered_target(release_root, &unit) == target).then_some(unit)
}

/// `source` with the program of every `Exec` and `TryExec` line that names
/// a command the installation publishes, bare or by any path, replaced by
/// that command beneath `commands`. Every other byte stays as it was.
pub(super) fn render(source: &[u8], commands: &str) -> Vec<u8> {
    let mut rendered = Vec::with_capacity(source.len() + 256);
    for (index, line) in source.split(|byte| *byte == b'\n').enumerate() {
        if index > 0 {
            rendered.push(b'\n');
        }
        rendered.extend_from_slice(&render_line(line, commands));
    }
    rendered
}

fn render_line<'a>(line: &'a [u8], commands: &str) -> Cow<'a, [u8]> {
    let Some((key, value)) = PROGRAM_KEYS
        .iter()
        .find_map(|key| line.strip_prefix(*key).map(|value| (*key, value)))
    else {
        return Cow::Borrowed(line);
    };
    let end = value
        .iter()
        .position(|byte| *byte == b' ')
        .unwrap_or(value.len());
    let (program, arguments) = value.split_at(end);
    let name = program
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or(program);
    if !published_commands().any(|command| command.as_bytes() == name) {
        return Cow::Borrowed(line);
    }
    let mut rendered = Vec::with_capacity(line.len() + commands.len() + 1);
    rendered.extend_from_slice(key);
    rendered.extend_from_slice(commands.as_bytes());
    rendered.push(b'/');
    rendered.extend_from_slice(name);
    rendered.extend_from_slice(arguments);
    Cow::Owned(rendered)
}

/// The commands the layout links into `~/.local/bin`.
fn published_commands() -> impl Iterator<Item = &'static str> {
    LINUX_LAYOUT_ITEMS
        .into_iter()
        .filter(|item| public_parent(*item) == LinuxDirectoryItem::LocalBin)
        .map(public_name)
}

/// `home`'s command directory as desktop entry text. Recorded homes are
/// limited to bytes neither systemd nor a desktop entry's `Exec` grammar
/// reinterprets, so the path needs no quoting.
fn command_directory(home: &Path) -> Result<String, InstallPlatformError> {
    let commands = home.join(".local/bin");
    commands
        .to_str()
        .filter(|text| {
            commands.is_absolute()
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
        })
        .map(str::to_owned)
        .ok_or_else(|| {
            error("the home directory cannot be named in a desktop entry without quoting")
        })
}

/// The unit's own desktop entry, or `None` when the release ships none.
fn read_source(unit: &UnitRecord) -> Result<Option<Vec<u8>>, InstallPlatformError> {
    let path = LinuxLayoutItem::DesktopEntry.unit_path();
    let (directories, name) = path
        .rsplit_once('/')
        .expect("the desktop entry lives in a directory");
    fn found<T>(result: io::Result<T>) -> Result<Option<T>, InstallPlatformError> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(error(format!(
                "the unit's desktop entry cannot be read: {source}"
            ))),
        }
    }
    let mut components = directories.split('/');
    let first = components.next().unwrap_or(directories);
    let Some(mut directory) = found(unit.directory().open_child_directory(Path::new(first)))?
    else {
        return Ok(None);
    };
    for component in components {
        let Some(child) = found(directory.open_child_directory(Path::new(component)))? else {
            return Ok(None);
        };
        directory = child;
    }
    let Some(mut opened) = found(directory.open_regular_file(Path::new(name)))? else {
        return Ok(None);
    };
    let size = opened.metadata().size();
    if size > MAX_DESKTOP_ENTRY_BYTES {
        return Err(error("the unit's desktop entry exceeds its byte bound"));
    }
    let mut bytes = Vec::new();
    opened
        .file_mut()
        .take(MAX_DESKTOP_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| error(format!("the unit's desktop entry cannot be read: {source}")))?;
    if bytes.len() as u64 != size {
        return Err(error("the unit's desktop entry changed size while reading"));
    }
    Ok(Some(bytes))
}

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    /// Render `unit`'s desktop entry for this installation and publish it
    /// where the unit's public desktop entry link points. A release without
    /// a desktop entry gets none, and its link dangles as it always did.
    pub(super) fn publish_desktop_entry(
        &mut self,
        unit: &UnitRecord,
    ) -> Result<(), InstallPlatformError> {
        let Some(source) = read_source(unit)? else {
            return Ok(());
        };
        let rendered = render(&source, &command_directory(&self.config.home)?);
        self.executor.publish_desktop_entry(unit.id(), &rendered)
    }
}

#[cfg(test)]
#[path = "desktop_tests.rs"]
mod tests;
