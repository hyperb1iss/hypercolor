//! Read-only observation of the recorded Linux installation.
//!
//! Anything that needs to know how Hypercolor was installed (the daemon
//! deciding whether it runs from a managed release, a status command) reads
//! the same records the installer elects authority from, without taking the
//! installation lock and without writing. An observation is a snapshot: a
//! transaction running at the same time can change it at once, so callers
//! use it to describe and classify, never to authorize a change. Changes
//! elect authority through [`elect_linux_installation`](super::elect_linux_installation).

use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

use super::election::read_hint;
use super::{LinuxInstallAuthority, LinuxInstallLocation, LinuxLocatorError, MAX_LOCATOR_BYTES};
use crate::{InstallDisposition, InstallJournalV1, MAX_MANAGED_INSTALL_JOURNAL_BYTES, UnitId};

const LEGACY_ROOT: &str = ".local/lib/hypercolor";
const JOURNAL_NAME: &str = "install-journal.json";
const IDENTITY_NAME: &str = "installation.json";
const DAEMON_RELATIVE_PATH: [&str; 2] = ["bin", "hypercolor-daemon"];

/// What is recorded for one user's Linux installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxInstallObservation {
    /// No release installer has recorded an installation for this user:
    /// no locator, no historical journal and no historical `active`
    /// pointer. Binaries placed by other means (a distribution package, a
    /// source build, or a script from before the release installer kept a
    /// journal) are not installations this observes; callers classify
    /// those by where the running executable lives.
    Absent,
    /// A raw installation under the historical `~/.local/lib/hypercolor`
    /// root that no managed-aware installer has adopted yet.
    Legacy(LinuxInstallRecords),
    /// A managed installation at its recorded roots.
    Managed {
        location: LinuxInstallLocation,
        records: LinuxInstallRecords,
    },
}

impl LinuxInstallObservation {
    /// The store records, unless nothing is installed.
    #[must_use]
    pub fn records(&self) -> Option<&LinuxInstallRecords> {
        match self {
            Self::Absent => None,
            Self::Legacy(records) | Self::Managed { records, .. } => Some(records),
        }
    }
}

/// The active unit and install journal of one store, read without its lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxInstallRecords {
    /// Root holding the `active` pointer and the immutable `units/`.
    pub release_root: PathBuf,
    /// The unit the `active` pointer names.
    pub active_unit: Option<UnitId>,
    /// The last install journal: the one in flight, or the last settled one.
    pub journal: Option<InstallJournalV1>,
}

impl LinuxInstallRecords {
    /// The journal while its transaction has not settled.
    #[must_use]
    pub fn pending_transaction(&self) -> Option<&InstallJournalV1> {
        self.journal.as_ref().filter(|journal| {
            matches!(
                journal.disposition,
                InstallDisposition::Forward | InstallDisposition::Rollback
            )
        })
    }

    /// The units the installed service may legitimately run right now: the
    /// active unit and, while a transaction has not settled, both of its
    /// units, since either side may be running at that moment.
    #[must_use]
    pub fn runnable_units(&self) -> Vec<UnitId> {
        let mut units: Vec<UnitId> = self.active_unit.iter().cloned().collect();
        if let Some(journal) = self.pending_transaction() {
            for unit in
                std::iter::once(&journal.candidate_unit).chain(journal.prior_active_unit.as_ref())
            {
                if !units.contains(unit) {
                    units.push(unit.clone());
                }
            }
        }
        units
    }

    /// The runnable unit whose daemon `executable` is.
    ///
    /// `executable` must be exactly `<release_root>/units/<unit>/bin/
    /// hypercolor-daemon` (a resolved `/proc/self/exe`, not a path through
    /// the `active` pointer) and `<unit>` one of [`Self::runnable_units`].
    #[must_use]
    pub fn daemon_unit(&self, executable: &Path) -> Option<UnitId> {
        let relative = executable.strip_prefix(&self.release_root).ok()?;
        let mut components = relative.components().map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        });
        if components.next()? != Some("units") {
            return None;
        }
        let unit = UnitId::new(components.next()??).ok()?;
        for expected in DAEMON_RELATIVE_PATH {
            if components.next()? != Some(expected) {
                return None;
            }
        }
        if components.next().is_some() {
            return None;
        }
        self.runnable_units().contains(&unit).then_some(unit)
    }
}

/// A recorded installation that could not be read consistently.
#[derive(Debug, thiserror::Error)]
pub enum LinuxObservationError {
    #[error("installation locator could not be read: {0}")]
    Locator(#[from] LinuxLocatorError),
    #[error("failed to read {path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} exceeds its byte bound", path = .0.display())]
    TooLarge(PathBuf),
    #[error("invalid install journal at {path}: {detail}", path = .path.display())]
    Journal { path: PathBuf, detail: String },
    #[error("invalid active pointer at {path}: {detail}", path = .path.display())]
    ActivePointer { path: PathBuf, detail: String },
    #[error("the managed installation is incomplete or inconsistent: {0}")]
    Unprepared(String),
}

/// Observe the Linux installation recorded for `home`.
///
/// Reads the permanent locator, then the recorded store's active pointer and
/// install journal, through read-only handles that never follow a final
/// symbolic link. Takes no lock and writes nothing, so it succeeds while an
/// installer holds the installation lock.
///
/// # Errors
/// Returns an error when a record exists but cannot be read or does not
/// parse, or when a managed installation's identity and journal are missing.
pub fn observe_linux_installation(
    home: &Path,
) -> Result<LinuxInstallObservation, LinuxObservationError> {
    match read_hint(home)? {
        LinuxInstallAuthority::Managed(location) => {
            let state = open_directory(location.state_root())?;
            let identity = read_bounded(
                &state,
                location.state_root(),
                IDENTITY_NAME,
                MAX_LOCATOR_BYTES,
            )?
            .ok_or_else(|| {
                LinuxObservationError::Unprepared("installation.json is missing".to_owned())
            })?;
            if LinuxInstallLocation::parse(&identity, home).ok().as_ref() != Some(&location) {
                return Err(LinuxObservationError::Unprepared(
                    "installation.json does not match the locator".to_owned(),
                ));
            }
            let journal = read_journal(&state, location.state_root())?.ok_or_else(|| {
                LinuxObservationError::Unprepared("the install journal is missing".to_owned())
            })?;
            let release_root = location.release_root().to_path_buf();
            let active_unit = read_active(&release_root)?;
            Ok(LinuxInstallObservation::Managed {
                location,
                records: LinuxInstallRecords {
                    release_root,
                    active_unit,
                    journal: Some(journal),
                },
            })
        }
        LinuxInstallAuthority::Legacy(journal) => {
            let release_root = home.join(LEGACY_ROOT);
            let active_unit = read_active(&release_root)?;
            if journal.is_none() && active_unit.is_none() {
                return Ok(LinuxInstallObservation::Absent);
            }
            Ok(LinuxInstallObservation::Legacy(LinuxInstallRecords {
                release_root,
                active_unit,
                journal,
            }))
        }
    }
}

fn open_directory(path: &Path) -> Result<ReadOnlyDirectoryAuthority, LinuxObservationError> {
    ReadOnlyDirectoryAuthority::open(path).map_err(|source| LinuxObservationError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_bounded(
    directory: &ReadOnlyDirectoryAuthority,
    directory_path: &Path,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, LinuxObservationError> {
    let path = directory_path.join(name);
    let mut file = match directory.open_regular_file(Path::new(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(LinuxObservationError::Io { path, source }),
    };
    let mut bytes = Vec::new();
    file.file_mut()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| LinuxObservationError::Io {
            path: path.clone(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(LinuxObservationError::TooLarge(path));
    }
    Ok(Some(bytes))
}

fn read_journal(
    state: &ReadOnlyDirectoryAuthority,
    state_root: &Path,
) -> Result<Option<InstallJournalV1>, LinuxObservationError> {
    let Some(bytes) = read_bounded(
        state,
        state_root,
        JOURNAL_NAME,
        MAX_MANAGED_INSTALL_JOURNAL_BYTES as u64,
    )?
    else {
        return Ok(None);
    };
    let path = state_root.join(JOURNAL_NAME);
    let journal: InstallJournalV1 =
        serde_json::from_slice(&bytes).map_err(|source| LinuxObservationError::Journal {
            path: path.clone(),
            detail: source.to_string(),
        })?;
    journal
        .validate()
        .map_err(|source| LinuxObservationError::Journal {
            path,
            detail: source.to_string(),
        })?;
    Ok(Some(journal))
}

fn read_active(release_root: &Path) -> Result<Option<UnitId>, LinuxObservationError> {
    let root = match ReadOnlyDirectoryAuthority::open(release_root) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LinuxObservationError::Io {
                path: release_root.to_path_buf(),
                source,
            });
        }
    };
    let path = release_root.join("active");
    let Some(target) =
        root.read_symlink(Path::new("active"))
            .map_err(|source| LinuxObservationError::Io {
                path: path.clone(),
                source,
            })?
    else {
        return Ok(None);
    };
    let mut components = target.components();
    let (Some(Component::Normal(units)), Some(Component::Normal(unit)), None) =
        (components.next(), components.next(), components.next())
    else {
        return Err(LinuxObservationError::ActivePointer {
            path,
            detail: format!("unexpected target {}", target.display()),
        });
    };
    if units != "units" {
        return Err(LinuxObservationError::ActivePointer {
            path,
            detail: format!("unexpected target {}", target.display()),
        });
    }
    let unit = unit
        .to_str()
        .and_then(|unit| UnitId::new(unit).ok())
        .ok_or_else(|| LinuxObservationError::ActivePointer {
            path,
            detail: format!("invalid unit in {}", target.display()),
        })?;
    Ok(Some(unit))
}
