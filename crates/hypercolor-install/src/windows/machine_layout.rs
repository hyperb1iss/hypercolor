//! Where one machine-wide installation keeps what lives outside its units.
//!
//! The launcher and the two recovery bootstrap slots are the only programs
//! outside a unit, so each finds the installation from its own path:
//!
//! ```text
//! %ProgramFiles%\Hypercolor\
//!   Hypercolor.exe            stable launcher
//!   update\slot-a.exe         recovery bootstrap, slot A
//!   update\slot-b.exe         recovery bootstrap, slot B
//!   units\<digest>\           immutable release units
//! %ProgramData%\Hypercolor\update\
//!   install-journal.json, active.json, installation.json, activator\
//! ```

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The desktop app, at the root of every unit.
pub const WINDOWS_APP_PROGRAM: &str = "hypercolor-app.exe";
/// The daemon, at the root of every unit.
pub const WINDOWS_DAEMON_PROGRAM: &str = "hypercolor-daemon.exe";
/// The SMBus broker, as a unit member path.
pub const WINDOWS_BROKER_PROGRAM: &str = "tools/hypercolor-smbus-service.exe";
/// The recovery bootstrap a unit carries for the installation's slots, as a
/// unit member path.
pub const WINDOWS_BOOTSTRAP_PROGRAM: &str = "tools/hypercolor-update-bootstrap.exe";
/// The CLI, at the root of every unit. Its `update activator` command is
/// the activator.
pub const WINDOWS_CLI_PROGRAM: &str = "hypercolor.exe";
pub(super) const BOOTSTRAP_DIRECTORY: &str = "update";

/// One of the two fixed places a recovery bootstrap lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapSlot {
    A,
    B,
}

impl BootstrapSlot {
    /// The slot's file under `update\`.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::A => "slot-a.exe",
            Self::B => "slot-b.exe",
        }
    }

    /// The slot this one retains while it is the active one.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    /// The slot a file name names, in any case, as NTFS compares names.
    #[must_use]
    pub fn from_file_name(name: &OsStr) -> Option<Self> {
        [Self::A, Self::B]
            .into_iter()
            .find(|slot| name.eq_ignore_ascii_case(slot.file_name()))
    }

    /// The slot a service's image path names, whether or not the path is
    /// quoted.
    #[must_use]
    pub fn from_image_path(image_path: &OsStr) -> Option<Self> {
        let text = image_path.to_str()?;
        let path = text.trim().trim_matches('"');
        let name = path.rsplit(['\\', '/']).next()?;
        Self::from_file_name(OsStr::new(name))
    }
}

/// The two roots of one installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsInstallLayout {
    release_root: PathBuf,
    state_root: PathBuf,
}

impl WindowsInstallLayout {
    fn new(release_root: &Path, program_data: &Path) -> Self {
        Self {
            release_root: release_root.to_path_buf(),
            state_root: program_data.join("Hypercolor").join("update"),
        }
    }

    /// The installation at exactly these two roots.
    #[cfg(windows)]
    #[must_use]
    pub(super) fn at(release_root: &Path, state_root: &Path) -> Self {
        Self {
            release_root: release_root.to_path_buf(),
            state_root: state_root.to_path_buf(),
        }
    }

    /// The installation whose launcher is the program at `launcher`.
    #[must_use]
    pub fn of_launcher(launcher: &Path, program_data: &Path) -> Option<Self> {
        Some(Self::new(launcher.parent()?, program_data))
    }

    /// The installation and slot of the bootstrap at `image`, or `None` for
    /// a path that is not a slot.
    #[must_use]
    pub fn of_bootstrap(image: &Path, program_data: &Path) -> Option<(Self, BootstrapSlot)> {
        let slot = BootstrapSlot::from_file_name(image.file_name()?)?;
        let directory = image.parent()?;
        if !directory
            .file_name()?
            .eq_ignore_ascii_case(BOOTSTRAP_DIRECTORY)
        {
            return None;
        }
        Some((Self::new(directory.parent()?, program_data), slot))
    }

    /// Directory holding the launcher, the slots and the units.
    #[must_use]
    pub fn release_root(&self) -> &Path {
        &self.release_root
    }

    /// Directory holding the journal, the pointer and the service's records.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// The directory the slots live in.
    #[must_use]
    pub fn bootstrap_directory(&self) -> PathBuf {
        self.release_root.join(BOOTSTRAP_DIRECTORY)
    }

    /// The bootstrap image of `slot`.
    #[must_use]
    pub fn slot_image(&self, slot: BootstrapSlot) -> PathBuf {
        self.bootstrap_directory().join(slot.file_name())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::{BootstrapSlot, WindowsInstallLayout};

    #[test]
    fn slots_name_each_other_and_their_files() {
        assert_eq!(BootstrapSlot::A.other(), BootstrapSlot::B);
        assert_eq!(BootstrapSlot::B.other(), BootstrapSlot::A);
        assert_eq!(
            BootstrapSlot::from_file_name(OsStr::new("SLOT-B.EXE")),
            Some(BootstrapSlot::B)
        );
        assert_eq!(
            BootstrapSlot::from_file_name(OsStr::new("slot-c.exe")),
            None
        );
    }

    #[test]
    fn a_quoted_or_bare_image_path_names_its_slot() {
        for (path, slot) in [
            (
                r#""C:\Program Files\Hypercolor\update\slot-a.exe""#,
                Some(BootstrapSlot::A),
            ),
            (r"C:\Hypercolor\update\slot-b.exe", Some(BootstrapSlot::B)),
            (r"C:\Hypercolor\update\other.exe", None),
        ] {
            assert_eq!(BootstrapSlot::from_image_path(OsStr::new(path)), slot);
        }
    }

    #[test]
    fn a_launcher_finds_its_installation_beside_itself() {
        let layout = WindowsInstallLayout::of_launcher(
            Path::new("/programs/Hypercolor/Hypercolor.exe"),
            Path::new("/data"),
        )
        .expect("layout");

        assert_eq!(layout.release_root(), Path::new("/programs/Hypercolor"));
        assert_eq!(layout.state_root(), Path::new("/data/Hypercolor/update"));
    }

    #[test]
    fn a_bootstrap_finds_its_installation_above_its_slot() {
        let (layout, slot) = WindowsInstallLayout::of_bootstrap(
            Path::new("/programs/Hypercolor/update/slot-b.exe"),
            Path::new("/data"),
        )
        .expect("layout");

        assert_eq!(slot, BootstrapSlot::B);
        assert_eq!(layout.release_root(), Path::new("/programs/Hypercolor"));
        assert_eq!(
            layout.slot_image(slot.other()),
            Path::new("/programs/Hypercolor/update/slot-a.exe")
        );
    }

    #[test]
    fn only_a_slot_under_the_update_directory_is_a_bootstrap() {
        for image in [
            "/programs/Hypercolor/update/hypercolor.exe",
            "/programs/Hypercolor/slot-a.exe",
            "/programs/Hypercolor/tools/slot-a.exe",
        ] {
            assert_eq!(
                WindowsInstallLayout::of_bootstrap(Path::new(image), Path::new("/data")),
                None,
                "{image}"
            );
        }
    }
}
