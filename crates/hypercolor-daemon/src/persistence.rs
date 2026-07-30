//! Durable cross-platform file replacement for daemon state stores.
//!
//! Coordinators order writers inside this process. The daemon's single-instance
//! guard is the cross-process ownership contract for these files.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

#[cfg(feature = "persistence-test-hooks")]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use tempfile::NamedTempFile;

static DESTINATIONS: LazyLock<Mutex<HashMap<PathBuf, Weak<Destination>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const RETRY_DELAY: Duration = Duration::from_millis(100);

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

/// Result of waiting for one destination's dirty snapshot to converge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceFlushOutcome {
    /// No dirty snapshot was pending.
    Clean,
    /// The latest dirty snapshot committed during the flush.
    Written,
    /// A newer generation superseded the dirty snapshot.
    Superseded,
}

/// A dirty snapshot could not converge before its flush deadline.
#[derive(Debug, thiserror::Error)]
#[error("persistence retry for {path} did not converge: {last_error}")]
pub struct PersistenceFlushError {
    path: PathBuf,
    last_error: String,
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
    retry: Mutex<RetryState>,
    retry_changed: Condvar,
    #[cfg(feature = "persistence-test-hooks")]
    injected_replace_failures: AtomicUsize,
}

#[derive(Debug, Default)]
struct DestinationState {
    latest_generation: u64,
}

#[derive(Debug, Default)]
struct RetryState {
    pending: Option<PendingRetry>,
    worker_running: bool,
    last_outcome: Option<AtomicWriteOutcome>,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingRetry {
    generation: u64,
    payload: Arc<[u8]>,
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
            retry: Mutex::new(RetryState::default()),
            retry_changed: Condvar::new(),
            #[cfg(feature = "persistence-test-hooks")]
            injected_replace_failures: AtomicUsize::new(0),
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

    /// Wake this destination's retry worker when a semantic no-op observes
    /// previously dirty state.
    pub fn kick(&self) {
        self.destination.start_retry_worker();
        self.destination.retry_changed.notify_all();
    }

    /// Wait for this destination's newest dirty snapshot to converge.
    ///
    /// # Errors
    ///
    /// Returns the latest retry failure when the deadline expires.
    pub fn flush(
        &self,
        timeout: Duration,
    ) -> Result<PersistenceFlushOutcome, PersistenceFlushError> {
        self.kick();
        let deadline = Instant::now() + timeout;
        let mut retry = self
            .destination
            .retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while retry.pending.is_some() || retry.worker_running {
            let now = Instant::now();
            if now >= deadline {
                return Err(self.destination.flush_error(&retry));
            }
            let wait = deadline.saturating_duration_since(now);
            let (next, result) = self
                .destination
                .retry_changed
                .wait_timeout(retry, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            retry = next;
            if result.timed_out() && (retry.pending.is_some() || retry.worker_running) {
                return Err(self.destination.flush_error(&retry));
            }
        }

        Ok(match retry.last_outcome.take() {
            Some(AtomicWriteOutcome::Written) => PersistenceFlushOutcome::Written,
            Some(AtomicWriteOutcome::Superseded) => PersistenceFlushOutcome::Superseded,
            None => PersistenceFlushOutcome::Clean,
        })
    }

    /// Inject replacement failures for deterministic persistence tests.
    #[cfg(feature = "persistence-test-hooks")]
    pub fn set_injected_replace_failures(&self, count: usize) {
        self.destination
            .injected_replace_failures
            .store(count, Ordering::Release);
    }
}

impl Destination {
    fn queue_retry(self: &Arc<Self>, generation: u64, payload: Arc<[u8]>, error: String) {
        let should_spawn = {
            let mut retry = self
                .retry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let accepts_generation = retry
                .pending
                .as_ref()
                .is_none_or(|pending| generation >= pending.generation);
            if accepts_generation {
                retry.pending = Some(PendingRetry {
                    generation,
                    payload,
                });
                retry.last_outcome = None;
                retry.last_error = Some(error);
            }
            if retry.pending.is_some() && !retry.worker_running {
                retry.worker_running = true;
                true
            } else {
                false
            }
        };
        self.retry_changed.notify_all();
        if should_spawn {
            self.spawn_retry_worker();
        }
    }

    fn complete_generation(&self, generation: u64, outcome: AtomicWriteOutcome) {
        let mut retry = self
            .retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pending) = retry.pending.as_ref() {
            if pending.generation > generation {
                return;
            }
            retry.pending = None;
        }
        retry.last_outcome = Some(outcome);
        retry.last_error = None;
        self.retry_changed.notify_all();
    }

    fn start_retry_worker(self: &Arc<Self>) {
        let should_spawn = {
            let mut retry = self
                .retry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if retry.pending.is_some() && !retry.worker_running {
                retry.worker_running = true;
                true
            } else {
                false
            }
        };
        if should_spawn {
            self.spawn_retry_worker();
        }
    }

    fn spawn_retry_worker(self: &Arc<Self>) {
        let destination = Arc::clone(self);
        if let Err(error) = std::thread::Builder::new()
            .name("hypercolor-persistence-retry".to_owned())
            .spawn(move || retry_loop(destination))
        {
            let mut retry = self
                .retry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            retry.worker_running = false;
            retry.last_error = Some(format!("failed to start persistence retry worker: {error}"));
            self.retry_changed.notify_all();
        }
    }

    fn flush_error(&self, retry: &RetryState) -> PersistenceFlushError {
        PersistenceFlushError {
            path: self.path.clone(),
            last_error: retry
                .last_error
                .clone()
                .unwrap_or_else(|| "retry worker did not finish before the deadline".to_owned()),
        }
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
        let result = try_write(&self.destination, self.generation, payload);
        match &result {
            Ok(outcome) => self
                .destination
                .complete_generation(self.generation, *outcome),
            Err(error) => self.destination.queue_retry(
                self.generation,
                Arc::<[u8]>::from(payload),
                error.to_string(),
            ),
        }
        result
    }
}

fn retry_loop(destination: Arc<Destination>) {
    loop {
        let pending = {
            let mut retry = destination
                .retry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(pending) = retry.pending.clone() else {
                retry.worker_running = false;
                destination.retry_changed.notify_all();
                return;
            };
            pending
        };

        let result = try_write(&destination, pending.generation, &pending.payload);
        let mut retry = destination
            .retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if retry.pending.as_ref().map(|current| current.generation) != Some(pending.generation) {
            continue;
        }
        match result {
            Ok(outcome) => {
                retry.pending = None;
                retry.last_outcome = Some(outcome);
                retry.last_error = None;
                destination.retry_changed.notify_all();
            }
            Err(error) => {
                retry.last_error = Some(error.to_string());
                destination.retry_changed.notify_all();
                let (next, _) = destination
                    .retry_changed
                    .wait_timeout(retry, RETRY_DELAY)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drop(next);
            }
        }
    }
}

fn try_write(
    destination: &Destination,
    generation: u64,
    payload: &[u8],
) -> Result<AtomicWriteOutcome, PersistenceError> {
    {
        let state = destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation != state.latest_generation {
            return Ok(AtomicWriteOutcome::Superseded);
        }
    }

    let mut temporary = NamedTempFile::new_in(&destination.parent).map_err(|source| {
        PersistenceError::CreateTemporary {
            path: destination.path.clone(),
            source,
        }
    })?;
    temporary
        .write_all(payload)
        .map_err(|source| PersistenceError::WriteTemporary {
            path: destination.path.clone(),
            source,
        })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| PersistenceError::SyncTemporary {
            path: destination.path.clone(),
            source,
        })?;

    let state = destination
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation != state.latest_generation {
        return Ok(AtomicWriteOutcome::Superseded);
    }

    #[cfg(feature = "persistence-test-hooks")]
    if destination
        .injected_replace_failures
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        return Err(PersistenceError::Replace {
            path: destination.path.clone(),
            source: std::io::Error::other("injected persistence replacement failure"),
        });
    }

    temporary
        .persist(&destination.path)
        .map_err(|error| PersistenceError::Replace {
            path: destination.path.clone(),
            source: error.error,
        })?;

    #[cfg(unix)]
    sync_parent_directory(&destination.parent)?;
    drop(state);
    Ok(AtomicWriteOutcome::Written)
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
