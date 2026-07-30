//! Durable cross-platform file replacement for daemon state stores.
//!
//! Coordinators order writers inside this process. The daemon's single-instance
//! guard is the cross-process ownership contract for these files.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use tempfile::NamedTempFile;

static DESTINATIONS: LazyLock<Mutex<HashMap<PathBuf, Weak<Destination>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The stage at which an atomic persistence operation failed.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// The destination has no file-name component.
    #[error("persistence destination has no file name: {path}")]
    InvalidDestination {
        /// Invalid destination path.
        path: PathBuf,
    },
    /// The destination directory could not be created.
    #[error("failed to create persistence directory {path}: {source}")]
    CreateDirectory {
        /// Destination directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The destination directory could not be resolved to a stable identity.
    #[error("failed to resolve persistence directory {path}: {source}")]
    ResolveDirectory {
        /// Destination directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A same-directory temporary file could not be created.
    #[error("failed to create temporary persistence file beside {path}: {source}")]
    CreateTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The payload could not be written to the temporary file.
    #[error("failed to write temporary persistence file for {path}: {source}")]
    WriteTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The temporary file contents could not be flushed.
    #[error("failed to flush temporary persistence file for {path}: {source}")]
    SyncTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The destination could not be atomically replaced.
    #[error("failed to replace persistence file {path}: {source}")]
    Replace {
        /// Destination path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The destination directory metadata could not be flushed.
    #[cfg(unix)]
    #[error("failed to flush persistence directory {path}: {source}")]
    SyncDirectory {
        /// Destination directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
}

/// Result of committing a reserved atomic write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicWriteOutcome {
    /// This reservation replaced the destination.
    Written,
    /// A newer reservation superseded this payload before replacement.
    Superseded,
}

/// Generation coordinator for one stable destination.
#[derive(Debug, Clone)]
pub struct AtomicFileWriter {
    destination: Arc<Destination>,
}

/// A write generation reserved at the owning store's snapshot boundary.
#[derive(Debug)]
pub struct AtomicWriteReservation {
    destination: Arc<Destination>,
    generation: u64,
}

#[derive(Debug)]
struct Destination {
    path: PathBuf,
    parent: PathBuf,
    state: Mutex<DestinationState>,
}

#[derive(Debug, Default)]
struct DestinationState {
    latest_generation: u64,
}

impl AtomicFileWriter {
    /// Resolve `path` to a process-stable destination coordinator.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem error when the destination directory cannot
    /// be created or resolved.
    pub fn new(path: &Path) -> Result<Self, PersistenceError> {
        let file_name = path
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| PersistenceError::InvalidDestination {
                path: path.to_path_buf(),
            })?;
        let parent = destination_parent(path);
        fs::create_dir_all(parent).map_err(|source| PersistenceError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
        let canonical_parent =
            fs::canonicalize(parent).map_err(|source| PersistenceError::ResolveDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        let canonical_path = canonical_parent.join(file_name);
        let key = destination_key(&canonical_parent, file_name);

        let mut destinations = DESTINATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        destinations.retain(|_, destination| destination.strong_count() > 0);
        if let Some(destination) = destinations.get(&key).and_then(Weak::upgrade) {
            return Ok(Self { destination });
        }

        let destination = Arc::new(Destination {
            path: canonical_path,
            parent: canonical_parent,
            state: Mutex::new(DestinationState::default()),
        });
        destinations.insert(key, Arc::downgrade(&destination));
        Ok(Self { destination })
    }

    /// Reserve the next write generation.
    #[must_use]
    pub fn reserve(&self) -> AtomicWriteReservation {
        let mut state = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.latest_generation = state
            .latest_generation
            .checked_add(1)
            .expect("persistence generation must not exhaust u64");
        AtomicWriteReservation {
            destination: Arc::clone(&self.destination),
            generation: state.latest_generation,
        }
    }

    /// Reserve and write one complete payload.
    ///
    /// # Errors
    ///
    /// Returns the typed stage error from payload preparation or replacement.
    pub fn write(&self, payload: &[u8]) -> Result<AtomicWriteOutcome, PersistenceError> {
        self.reserve().write(payload)
    }
}

impl AtomicWriteReservation {
    /// Prepare this payload and replace the destination unless a newer
    /// reservation exists.
    ///
    /// # Errors
    ///
    /// Returns the typed stage error from payload preparation or replacement.
    pub fn write(self, payload: &[u8]) -> Result<AtomicWriteOutcome, PersistenceError> {
        if !self.is_current() {
            return Ok(AtomicWriteOutcome::Superseded);
        }

        let mut temporary = NamedTempFile::new_in(&self.destination.parent).map_err(|source| {
            PersistenceError::CreateTemporary {
                path: self.destination.path.clone(),
                source,
            }
        })?;
        temporary
            .write_all(payload)
            .map_err(|source| PersistenceError::WriteTemporary {
                path: self.destination.path.clone(),
                source,
            })?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| PersistenceError::SyncTemporary {
                path: self.destination.path.clone(),
                source,
            })?;

        let state = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.generation != state.latest_generation {
            return Ok(AtomicWriteOutcome::Superseded);
        }

        temporary
            .persist(&self.destination.path)
            .map_err(|error| PersistenceError::Replace {
                path: self.destination.path.clone(),
                source: error.error,
            })?;

        #[cfg(unix)]
        sync_parent_directory(&self.destination.parent)?;
        Ok(AtomicWriteOutcome::Written)
    }

    fn is_current(&self) -> bool {
        let state = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.generation == state.latest_generation
    }
}

/// Write a complete file beside its destination and atomically replace it.
///
/// The temporary file lives in the destination directory so persistence never
/// crosses filesystems. `tempfile` maps replacement to `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING` on Windows and `rename` on Unix.
///
/// # Errors
///
/// Returns the typed stage error from destination resolution, payload
/// preparation, or replacement.
pub fn write_atomic(path: &Path, payload: &[u8]) -> Result<(), PersistenceError> {
    AtomicFileWriter::new(path)?.write(payload).map(|_| ())
}

fn destination_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(windows)]
fn destination_key(parent: &Path, file_name: &OsStr) -> PathBuf {
    let normalized = file_name
        .encode_wide()
        .map(|unit| match unit {
            0x41..=0x5a => unit + 0x20,
            _ => unit,
        })
        .collect::<Vec<_>>();
    parent.join(OsString::from_wide(&normalized))
}

#[cfg(not(windows))]
fn destination_key(parent: &Path, file_name: &OsStr) -> PathBuf {
    parent.join(file_name)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<(), PersistenceError> {
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| PersistenceError::SyncDirectory {
            path: parent.to_path_buf(),
            source,
        })
}
