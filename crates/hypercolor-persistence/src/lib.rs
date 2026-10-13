//! Durable cross-platform file replacement for Hypercolor stores.
//!
//! Coordinators order writers inside this process. The daemon's single-instance
//! guard is the cross-process ownership contract for these files.

#[cfg(not(windows))]
use std::collections::HashMap;
#[cfg(not(windows))]
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

#[cfg(any(test, feature = "persistence-test-hooks"))]
use std::cell::Cell;
#[cfg(any(test, feature = "persistence-test-hooks"))]
use std::sync::atomic::AtomicUsize;
#[cfg(any(unix, test, feature = "persistence-test-hooks"))]
use std::sync::atomic::Ordering;

use serde::Serialize;
use sha2::{Digest as _, Sha256};

use tempfile::NamedTempFile;

mod history;

pub use history::{
    Equivalence, Generation, HistoryError, HistoryPolicy, RestoreOutcome, capture_next_writes,
    list_generations, restore_generation, set_replacement_observer,
};

#[cfg(not(windows))]
static DESTINATIONS: LazyLock<Mutex<HashMap<PathBuf, Weak<Destination>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[cfg(windows)]
static DESTINATIONS: LazyLock<Mutex<Vec<RegisteredDestination>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static RETRY_SUPERVISOR: OnceLock<Result<Arc<RetrySupervisor>, String>> = OnceLock::new();
#[cfg(any(test, feature = "persistence-test-hooks"))]
thread_local! {
    static INJECTED_SERIALIZATION_FAILURES: Cell<usize> = const { Cell::new(0) };
}
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// The stage at which an atomic persistence operation failed.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// The shared persistence retry supervisor could not be initialized.
    #[error("failed to initialize persistence retry supervisor: {detail}")]
    InitializeRetrySupervisor {
        /// Worker initialization failure.
        detail: String,
    },
    /// A complete snapshot could not be serialized before admission.
    #[error("failed to serialize {subject} snapshot: {source}")]
    SerializeSnapshot {
        /// Store or domain whose snapshot failed.
        subject: &'static str,
        /// JSON serialization failure.
        #[source]
        source: serde_json::Error,
    },
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
    /// The temporary file could not be restricted to the destination mode.
    #[cfg(unix)]
    #[error("failed to set persistence file mode for {path}: {source}")]
    SetTemporaryMode {
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

/// Stage-aware result of attempting one admitted atomic replacement.
#[derive(Debug)]
pub enum AtomicWriteCommitResult {
    /// A newer admitted generation won before this write replaced the file.
    Superseded,
    /// The replacement and its parent-directory durability barrier completed.
    DurableWritten,
    /// The destination was not replaced.
    FailedBeforeReplacement(PersistenceError),
    /// The replacement is visible, but parent-directory durability was not proven.
    ReplacementVisibleButNotDurable(PersistenceError),
}

impl AtomicWriteCommitResult {
    fn into_compatibility_result(self) -> Result<AtomicWriteOutcome, PersistenceError> {
        match self {
            Self::Superseded => Ok(AtomicWriteOutcome::Superseded),
            Self::DurableWritten => Ok(AtomicWriteOutcome::Written),
            Self::FailedBeforeReplacement(error) | Self::ReplacementVisibleButNotDurable(error) => {
                Err(error)
            }
        }
    }
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
#[derive(Debug, Clone, thiserror::Error)]
#[error("persistence retry for {path} did not converge: {last_error}")]
pub struct PersistenceFlushError {
    path: PathBuf,
    last_error: String,
}

/// Aggregate result of flushing every live persistence destination.
#[derive(Debug, Default)]
pub struct PersistenceFlushReport {
    clean: usize,
    written: usize,
    superseded: usize,
    errors: Vec<PersistenceFlushError>,
}

impl PersistenceFlushReport {
    /// Destinations that had no dirty snapshot to flush.
    #[must_use]
    pub const fn clean(&self) -> usize {
        self.clean
    }

    /// Dirty destinations whose newest snapshot committed.
    #[must_use]
    pub const fn written(&self) -> usize {
        self.written
    }

    /// Dirty destinations superseded by a newer generation.
    #[must_use]
    pub const fn superseded(&self) -> usize {
        self.superseded
    }

    /// Destinations that did not converge before the deadline.
    #[must_use]
    pub fn errors(&self) -> &[PersistenceFlushError] {
        &self.errors
    }

    /// Whether every live destination converged.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Generation coordinator for one stable destination.
///
/// While any writer keeps the destination alive, it remembers a digest of
/// the bytes it last made durable and the change stamp of the file they
/// landed as. A payload with the same digest lands as written without
/// touching the filesystem, as long as the file's change stamp shows nothing
/// has replaced, rewritten, or re-permissioned it since and it still carries
/// the requested mode.
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

/// A complete payload admitted as the newest durable intent for one destination.
#[derive(Debug)]
pub struct AdmittedAtomicWrite {
    destination: Arc<Destination>,
    generation: u64,
    payload: Arc<[u8]>,
    armed: bool,
}

#[derive(Debug)]
struct Destination {
    path: PathBuf,
    parent: PathBuf,
    state: Mutex<DestinationState>,
    state_changed: Condvar,
    history: Mutex<Option<history::HistoryTracking>>,
    #[cfg(unix)]
    file_mode: AtomicU32,
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    injected_replace_failures: AtomicUsize,
    #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
    injected_directory_sync_failures: AtomicUsize,
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    replacements: AtomicUsize,
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    directory_syncs: AtomicUsize,
}

#[cfg(windows)]
#[derive(Debug)]
struct RegisteredDestination {
    identity: hypercolor_platform_fs::DestinationIdentity,
    destination: Weak<Destination>,
}

#[derive(Debug, Default)]
struct DestinationState {
    next_generation: u64,
    latest_admitted_generation: u64,
    pending: Option<PendingRetry>,
    active_generation: Option<u64>,
    foreground_generation: Option<u64>,
    last_outcome: Option<AtomicWriteOutcome>,
    last_error: Option<String>,
    /// The latest failure since the destination last landed durably.
    /// Unlike `last_error`, a newer admission does not clear it, because
    /// the file still lags until some attempt lands.
    lag_error: Option<String>,
    /// The bytes this process last made durable here.
    ///
    /// An attempt whose payload matches them has nothing to replace, so it
    /// lands without touching the filesystem. `None` until the first durable
    /// write, and from the moment a replacement starts until its durability
    /// barrier completes, because the file's durable content is unknown in
    /// that window.
    durable: Option<DurableBytes>,
}

/// What a destination's file durably held after one of this process's
/// writes: the payload's digest, and the change stamp of the file it landed
/// as.
///
/// Other code can replace or rewrite the same path without this coordinator
/// (a directly staged replacement, a hand edit, a timestamp-preserving
/// restore), and any of those moves the change stamp, so the digest only
/// describes the file while a fresh stamp still matches. A platform with no
/// change stamp never records one, and every write there replaces the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DurableBytes {
    digest: PayloadDigest,
    stamp: hypercolor_platform_fs::FileChangeStamp,
}

type PayloadDigest = [u8; 32];

fn payload_digest(payload: &[u8]) -> PayloadDigest {
    Sha256::digest(payload).into()
}

impl DurableBytes {
    /// Whether the destination file is still exactly the one these bytes
    /// landed as, with the permissions the destination asks for.
    fn still_current(&self, destination: &Destination) -> bool {
        hypercolor_platform_fs::path_change_stamp(&destination.path)
            .ok()
            .flatten()
            == Some(self.stamp)
            && self.has_requested_mode(destination)
    }

    #[cfg(unix)]
    fn has_requested_mode(&self, destination: &Destination) -> bool {
        let requested = destination.file_mode.load(Ordering::Acquire);
        requested == 0 || self.stamp.permission_bits() == requested & 0o7777
    }

    #[cfg(not(unix))]
    #[expect(
        clippy::unused_self,
        reason = "Non-Unix platforms carry no requested file mode"
    )]
    const fn has_requested_mode(&self, _destination: &Destination) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
struct PendingRetry {
    generation: u64,
    payload: Arc<[u8]>,
}

#[derive(Debug)]
struct RetrySupervisor {
    queue: Mutex<Vec<ScheduledRetry>>,
    queue_changed: Condvar,
}

#[derive(Debug)]
struct ScheduledRetry {
    destination: Arc<Destination>,
    due: Instant,
}

impl AtomicFileWriter {
    /// Resolve `path` to a process-stable destination coordinator.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem error when the destination directory cannot
    /// be created or resolved.
    pub fn new(path: &Path) -> Result<Self, PersistenceError> {
        retry_supervisor()?;
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

        #[cfg(windows)]
        let identity =
            hypercolor_platform_fs::DestinationIdentity::resolve(&canonical_parent, file_name)
                .map_err(|source| PersistenceError::ResolveDirectory {
                    path: canonical_parent.clone(),
                    source,
                })?;
        #[cfg(not(windows))]
        let key = destination_key(&canonical_parent, file_name);

        let mut destinations = DESTINATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        #[cfg(windows)]
        {
            destinations.retain(|registered| registered.destination.strong_count() > 0);
            if let Some(destination) = destinations.iter().find_map(|registered| {
                registered
                    .identity
                    .equivalent(&identity)
                    .then(|| registered.destination.upgrade())
                    .flatten()
            }) {
                return Ok(Self { destination });
            }
        }

        #[cfg(not(windows))]
        {
            destinations.retain(|_, destination| destination.strong_count() > 0);
            if let Some(destination) = destinations.get(&key).and_then(Weak::upgrade) {
                return Ok(Self { destination });
            }
        }

        let destination = Arc::new(Destination {
            path: canonical_path,
            parent: canonical_parent,
            state: Mutex::new(DestinationState::default()),
            state_changed: Condvar::new(),
            history: Mutex::new(None),
            #[cfg(unix)]
            file_mode: AtomicU32::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            injected_replace_failures: AtomicUsize::new(0),
            #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
            injected_directory_sync_failures: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            replacements: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            directory_syncs: AtomicUsize::new(0),
        });

        #[cfg(windows)]
        destinations.push(RegisteredDestination {
            identity,
            destination: Arc::downgrade(&destination),
        });
        #[cfg(not(windows))]
        destinations.insert(key, Arc::downgrade(&destination));
        Ok(Self { destination })
    }

    /// Resolve `path` to a destination whose replacements carry `mode`.
    ///
    /// The mode is a property of the destination, so the newest explicit
    /// request wins for every writer sharing that path. Non-unix platforms
    /// ignore it and rely on inherited directory permissions.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem error when the destination directory cannot
    /// be created or resolved.
    pub fn with_file_mode(path: &Path, mode: u32) -> Result<Self, PersistenceError> {
        let writer = Self::new(path)?;
        #[cfg(unix)]
        writer.destination.file_mode.store(mode, Ordering::Release);
        #[cfg(not(unix))]
        let _ = mode;
        Ok(writer)
    }

    /// Reserve the next write generation.
    #[must_use]
    pub fn reserve(&self) -> AtomicWriteReservation {
        let mut state = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .expect("persistence generation must not exhaust u64");
        AtomicWriteReservation {
            destination: Arc::clone(&self.destination),
            generation: state.next_generation,
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
        self.destination.schedule_retry(Duration::ZERO);
        self.destination.state_changed.notify_all();
    }

    /// The latest failure holding this destination's file behind its
    /// newest admitted snapshot.
    ///
    /// `Some` from the first failed attempt until an attempt lands durably;
    /// the retry supervisor keeps trying until one does, and admitting a
    /// newer snapshot does not clear it. `None` while the file is current,
    /// including while a first attempt after a durable write is in flight.
    #[must_use]
    pub fn pending_failure(&self) -> Option<String> {
        let state = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.as_ref()?;
        state.lag_error.clone()
    }

    /// Stop trusting what this destination last made durable, after code
    /// replaced its file without this coordinator. The next write of any
    /// payload then replaces the file.
    pub fn forget_durable_bytes(&self) {
        self.destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .durable = None;
    }

    /// Whether the file still holds, untouched, the bytes this process last
    /// made durable here.
    #[must_use]
    pub fn holds_durable_bytes(&self) -> bool {
        self.destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .durable
            .is_some_and(|durable| durable.still_current(&self.destination))
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
        let mut state = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.pending.is_some() || state.active_generation.is_some() {
            let now = Instant::now();
            if now >= deadline {
                return Err(self.destination.flush_error(&state));
            }
            let wait = deadline.saturating_duration_since(now);
            let (next, result) = self
                .destination
                .state_changed
                .wait_timeout(state, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if result.timed_out() && (state.pending.is_some() || state.active_generation.is_some())
            {
                return Err(self.destination.flush_error(&state));
            }
        }

        Ok(match state.last_outcome.take() {
            Some(AtomicWriteOutcome::Written) => PersistenceFlushOutcome::Written,
            Some(AtomicWriteOutcome::Superseded) => PersistenceFlushOutcome::Superseded,
            None => PersistenceFlushOutcome::Clean,
        })
    }

    /// Inject replacement failures for deterministic persistence tests.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    pub fn set_injected_replace_failures(&self, count: usize) {
        self.destination
            .injected_replace_failures
            .store(count, Ordering::Release);
    }

    /// How many times this destination's file has been replaced in this
    /// process, for tests that pin which writes a workflow performs.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    #[must_use]
    pub fn replacement_count(&self) -> usize {
        self.destination.replacements.load(Ordering::Acquire)
    }

    /// How many directory durability barriers this destination's writes have
    /// run in this process: exactly one per replacement on Unix, none on
    /// Windows, where the replacement itself is write-through.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    #[must_use]
    pub fn directory_sync_count(&self) -> usize {
        self.destination.directory_syncs.load(Ordering::Acquire)
    }

    /// Inject post-replacement directory-sync failures for deterministic tests.
    #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
    pub fn set_injected_directory_sync_failures(&self, count: usize) {
        self.destination
            .injected_directory_sync_failures
            .store(count, Ordering::Release);
    }
}

impl Destination {
    fn admit(self: &Arc<Self>, generation: u64, payload: Arc<[u8]>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation > state.latest_admitted_generation {
            state.latest_admitted_generation = generation;
            state.pending = Some(PendingRetry {
                generation,
                payload,
            });
            state.foreground_generation = Some(generation);
            state.last_outcome = None;
            state.last_error = None;
        }
        self.state_changed.notify_all();
    }

    fn begin_foreground(&self, generation: u64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if generation != state.latest_admitted_generation {
                if state.foreground_generation == Some(generation) {
                    state.foreground_generation = None;
                }
                self.state_changed.notify_all();
                return false;
            }
            if state.active_generation.is_none() {
                state.active_generation = Some(generation);
                if state.foreground_generation == Some(generation) {
                    state.foreground_generation = None;
                }
                return true;
            }
            state = self
                .state_changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn finish_attempt(
        self: &Arc<Self>,
        generation: u64,
        result: &AtomicWriteCommitResult,
        retry_attempt: bool,
    ) {
        let retry_needed = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.active_generation == Some(generation) {
                state.active_generation = None;
            }
            match result {
                AtomicWriteCommitResult::DurableWritten => {
                    if state.pending.as_ref().map(|pending| pending.generation) == Some(generation)
                    {
                        state.pending = None;
                    }
                    state.last_outcome = retry_attempt.then_some(AtomicWriteOutcome::Written);
                    state.last_error = None;
                    state.lag_error = None;
                }
                AtomicWriteCommitResult::Superseded => {
                    state.last_outcome = retry_attempt.then_some(AtomicWriteOutcome::Superseded);
                }
                AtomicWriteCommitResult::FailedBeforeReplacement(error)
                | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                    let error = error.to_string();
                    state.lag_error = Some(error.clone());
                    state.last_error = Some(error);
                }
            }
            self.state_changed.notify_all();
            state.pending.is_some()
                && state.active_generation.is_none()
                && state.foreground_generation.is_none()
        };
        if retry_needed {
            self.schedule_retry(RETRY_DELAY);
        }
    }

    fn retry_once(self: &Arc<Self>) -> Option<Duration> {
        let pending = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.foreground_generation.is_some() || state.active_generation.is_some() {
                return state.pending.as_ref().map(|_| RETRY_DELAY);
            }
            let pending = state.pending.clone()?;
            state.active_generation = Some(pending.generation);
            pending
        };

        let result = try_write_stage_aware(self, pending.generation, &pending.payload);
        self.finish_attempt(pending.generation, &result, true);
        if matches!(
            result,
            AtomicWriteCommitResult::FailedBeforeReplacement(_)
                | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(_)
        ) {
            Some(RETRY_DELAY)
        } else {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pending.as_ref().map(|_| Duration::ZERO)
        }
    }

    fn schedule_retry(self: &Arc<Self>, delay: Duration) {
        let has_pending = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .is_some();
        if has_pending {
            retry_supervisor()
                .expect("destination exists only after retry supervisor initialization")
                .schedule(Arc::clone(self), delay);
        }
    }

    fn flush_error(&self, state: &DestinationState) -> PersistenceFlushError {
        PersistenceFlushError {
            path: self.path.clone(),
            last_error: state
                .last_error
                .clone()
                .unwrap_or_else(|| "dirty snapshot did not finish before the deadline".to_owned()),
        }
    }
}

impl AtomicWriteReservation {
    /// Admit a complete payload as this destination's newest durable intent.
    #[must_use]
    pub fn admit(self, payload: Vec<u8>) -> AdmittedAtomicWrite {
        let payload = Arc::<[u8]>::from(payload);
        self.destination
            .admit(self.generation, Arc::clone(&payload));
        AdmittedAtomicWrite {
            destination: self.destination,
            generation: self.generation,
            payload,
            armed: true,
        }
    }

    /// Admit this payload and replace the destination unless newer admitted
    /// bytes exist.
    ///
    /// # Errors
    ///
    /// Returns the typed stage error from payload preparation or replacement.
    pub fn write(self, payload: &[u8]) -> Result<AtomicWriteOutcome, PersistenceError> {
        self.admit(payload.to_vec()).commit()
    }
}

impl AdmittedAtomicWrite {
    /// Attempt the admitted replacement. Failures remain retained by the
    /// shared retry supervisor.
    ///
    /// # Errors
    ///
    /// Returns the typed preparation or replacement error from this attempt.
    pub fn commit(self) -> Result<AtomicWriteOutcome, PersistenceError> {
        self.commit_stage_aware().into_compatibility_result()
    }

    /// Attempt the replacement and preserve whether failure occurred before or after visibility.
    #[must_use]
    pub fn commit_stage_aware(mut self) -> AtomicWriteCommitResult {
        self.armed = false;
        if !self.destination.begin_foreground(self.generation) {
            return AtomicWriteCommitResult::Superseded;
        }
        let result = try_write_stage_aware(&self.destination, self.generation, &self.payload);
        self.destination
            .finish_attempt(self.generation, &result, false);
        result
    }
}

impl Drop for AdmittedAtomicWrite {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        {
            let mut state = self
                .destination
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.foreground_generation == Some(self.generation) {
                state.foreground_generation = None;
            }
            self.destination.state_changed.notify_all();
        }
        self.destination.schedule_retry(Duration::ZERO);
    }
}

impl RetrySupervisor {
    fn start() -> Result<Arc<Self>, String> {
        let supervisor = Arc::new(Self {
            queue: Mutex::new(Vec::new()),
            queue_changed: Condvar::new(),
        });
        let worker_count =
            std::thread::available_parallelism().map_or(2, |count| count.get().max(2));
        let mut running = 0;
        let mut last_error = None;
        for index in 0..worker_count {
            let worker = Arc::clone(&supervisor);
            match std::thread::Builder::new()
                .name(format!("hypercolor-persistence-{index}"))
                .spawn(move || retry_worker(worker))
            {
                Ok(_) => running += 1,
                Err(error) => last_error = Some(error),
            }
        }
        if running == 0 {
            Err(last_error.map_or_else(
                || "no persistence retry workers started".to_owned(),
                |error| error.to_string(),
            ))
        } else {
            Ok(supervisor)
        }
    }

    fn schedule(&self, destination: Arc<Destination>, delay: Duration) {
        let due = Instant::now() + delay;
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = queue
            .iter_mut()
            .find(|job| Arc::ptr_eq(&job.destination, &destination))
        {
            existing.due = existing.due.min(due);
        } else {
            queue.push(ScheduledRetry { destination, due });
        }
        self.queue_changed.notify_one();
    }

    fn next(&self) -> Arc<Destination> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some((index, job)) = queue.iter().enumerate().min_by_key(|(_, job)| job.due) {
                let now = Instant::now();
                if job.due <= now {
                    return queue.swap_remove(index).destination;
                }
                let wait = job.due.saturating_duration_since(now);
                let (next, _) = self
                    .queue_changed
                    .wait_timeout(queue, wait)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                queue = next;
            } else {
                queue = self
                    .queue_changed
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

fn retry_supervisor() -> Result<&'static Arc<RetrySupervisor>, PersistenceError> {
    RETRY_SUPERVISOR
        .get_or_init(RetrySupervisor::start)
        .as_ref()
        .map_err(|detail| PersistenceError::InitializeRetrySupervisor {
            detail: detail.clone(),
        })
}

fn retry_worker(supervisor: Arc<RetrySupervisor>) {
    loop {
        let destination = supervisor.next();
        if let Some(delay) = destination.retry_once() {
            supervisor.schedule(destination, delay);
        }
    }
}

fn try_write_stage_aware(
    destination: &Arc<Destination>,
    generation: u64,
    payload: &[u8],
) -> AtomicWriteCommitResult {
    let digest = payload_digest(payload);
    let holds_known_bytes = {
        let state = destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The newest admitted generation only grows, so a payload that is
        // already behind it would lose the check before replacement too.
        if generation != state.latest_admitted_generation {
            return AtomicWriteCommitResult::Superseded;
        }
        // Only one attempt per destination runs at a time, so nothing this
        // coordinator owns can change the file between this check and the
        // outcome; the change stamp catches anything that did from outside,
        // and a mode the file no longer carries is written again too.
        match state.durable {
            Some(durable) if durable.still_current(destination) => {
                if durable.digest == digest {
                    return AtomicWriteCommitResult::DurableWritten;
                }
                true
            }
            _ => false,
        }
    };

    let mut temporary = match NamedTempFile::new_in(&destination.parent) {
        Ok(temporary) => temporary,
        Err(source) => {
            return AtomicWriteCommitResult::FailedBeforeReplacement(
                PersistenceError::CreateTemporary {
                    path: destination.path.clone(),
                    source,
                },
            );
        }
    };
    if let Err(source) = temporary.write_all(payload) {
        return AtomicWriteCommitResult::FailedBeforeReplacement(
            PersistenceError::WriteTemporary {
                path: destination.path.clone(),
                source,
            },
        );
    }
    if let Err(source) = temporary.as_file().sync_all() {
        return AtomicWriteCommitResult::FailedBeforeReplacement(PersistenceError::SyncTemporary {
            path: destination.path.clone(),
            source,
        });
    }
    #[cfg(unix)]
    if let Err(source) = apply_file_mode(destination, temporary.as_file()) {
        return AtomicWriteCommitResult::FailedBeforeReplacement(
            PersistenceError::SetTemporaryMode {
                path: destination.path.clone(),
                source,
            },
        );
    }

    // Only one attempt per destination runs at a time (the active
    // generation), so the file cannot change between this read and the
    // replacement below. The copy into history is durable before the
    // replacement starts.
    let rotation_due = history::rotation_due(destination);
    let previous = history::previous_content(destination, holds_known_bytes, rotation_due);
    history::rotate_before_replace(destination, &previous, payload, rotation_due);

    let mut state = destination
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation != state.latest_admitted_generation {
        return AtomicWriteCommitResult::Superseded;
    }

    #[cfg(any(test, feature = "persistence-test-hooks"))]
    if destination
        .injected_replace_failures
        .try_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        return AtomicWriteCommitResult::FailedBeforeReplacement(PersistenceError::Replace {
            path: destination.path.clone(),
            source: std::io::Error::other("injected persistence replacement failure"),
        });
    }

    // Which file is ours, read from the staged file itself. A rename keeps
    // that identity but moves the change stamp, so the stamp to record is
    // read from the destination after the barrier, and only kept when it
    // still names this file.
    let staged = hypercolor_platform_fs::file_change_stamp(temporary.as_file())
        .ok()
        .flatten();
    let temporary_path = temporary.into_temp_path();
    // From here until the durability barrier completes, nothing proves what
    // the file durably holds, so no later attempt may be skipped against it.
    state.durable = None;
    // The rename alone: this function owns the one directory barrier below,
    // and the stamp it records is read after that barrier.
    if let Err(source) =
        hypercolor_platform_fs::rename_replacing(&temporary_path, &destination.path)
    {
        return AtomicWriteCommitResult::FailedBeforeReplacement(PersistenceError::Replace {
            path: destination.path.clone(),
            source,
        });
    }
    drop(state);
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    destination.replacements.fetch_add(1, Ordering::AcqRel);
    history::note_replacement(destination, &previous, payload);

    #[cfg(unix)]
    {
        #[cfg(any(test, feature = "persistence-test-hooks"))]
        if destination
            .injected_directory_sync_failures
            .try_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return AtomicWriteCommitResult::ReplacementVisibleButNotDurable(
                PersistenceError::SyncDirectory {
                    path: destination.parent.clone(),
                    source: std::io::Error::other("injected persistence directory sync failure"),
                },
            );
        }
        #[cfg(any(test, feature = "persistence-test-hooks"))]
        destination.directory_syncs.fetch_add(1, Ordering::AcqRel);
        if let Err(error) = sync_parent_directory(&destination.parent) {
            return AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error);
        }
    }
    let landed = hypercolor_platform_fs::path_change_stamp(&destination.path)
        .ok()
        .flatten()
        .filter(|landed| staged.is_some_and(|staged| staged.same_file(landed)));
    destination
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .durable = landed.map(|stamp| DurableBytes { digest, stamp });
    AtomicWriteCommitResult::DurableWritten
}

/// Write a complete file beside its destination and atomically replace it.
///
/// The temporary file lives in the destination directory so persistence never
/// crosses filesystems. Windows replacement requests both replace-existing and
/// write-through semantics; Unix replacement is followed by a parent fsync.
///
/// # Errors
///
/// Returns the typed stage error from destination resolution, payload
/// preparation, or replacement.
pub fn write_atomic(path: &Path, payload: &[u8]) -> Result<(), PersistenceError> {
    AtomicFileWriter::new(path)?.write(payload).map(|_| ())
}

/// Serialize a complete persistence snapshot before admitting its bytes.
///
/// # Errors
///
/// Returns the JSON serialization failure without changing destination
/// freshness.
pub fn serialize_json_pretty<T>(value: &T) -> Result<Vec<u8>, serde_json::Error>
where
    T: Serialize + ?Sized,
{
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    if INJECTED_SERIALIZATION_FAILURES.with(|remaining| {
        let count = remaining.get();
        remaining.set(count.saturating_sub(1));
        count > 0
    }) {
        return Err(serde_json::Error::io(std::io::Error::other(
            "injected persistence serialization failure",
        )));
    }
    serde_json::to_vec_pretty(value)
}

/// Inject serialization failures for deterministic persistence tests.
#[cfg(any(test, feature = "persistence-test-hooks"))]
pub fn set_injected_serialization_failures(count: usize) {
    INJECTED_SERIALIZATION_FAILURES.with(|remaining| remaining.set(count));
}

/// Flush every live persistence destination within one shared deadline.
///
/// Runtime retry workers remain active after this bounded observation returns.
#[must_use]
pub fn flush_all(timeout: Duration) -> PersistenceFlushReport {
    let writers = {
        let mut destinations = DESTINATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        #[cfg(windows)]
        {
            destinations.retain(|registered| registered.destination.strong_count() > 0);
            destinations
                .iter()
                .filter_map(|registered| registered.destination.upgrade())
                .map(|destination| AtomicFileWriter { destination })
                .collect::<Vec<_>>()
        }

        #[cfg(not(windows))]
        {
            destinations.retain(|_, destination| destination.strong_count() > 0);
            destinations
                .values()
                .filter_map(Weak::upgrade)
                .map(|destination| AtomicFileWriter { destination })
                .collect::<Vec<_>>()
        }
    };
    let deadline = Instant::now() + timeout;
    let mut report = PersistenceFlushReport::default();
    for writer in writers {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match writer.flush(remaining) {
            Ok(PersistenceFlushOutcome::Clean) => report.clean += 1,
            Ok(PersistenceFlushOutcome::Written) => report.written += 1,
            Ok(PersistenceFlushOutcome::Superseded) => report.superseded += 1,
            Err(error) => report.errors.push(error),
        }
    }
    report
}

#[cfg(unix)]
fn apply_file_mode(destination: &Destination, temporary: &fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = destination.file_mode.load(Ordering::Acquire);
    if mode == 0 {
        return Ok(());
    }
    temporary.set_permissions(fs::Permissions::from_mode(mode))
}

fn destination_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_durable_write_runs_exactly_one_directory_barrier() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let writer = AtomicFileWriter::new(&path).expect("writer");

        writer.write(b"first").expect("first write lands");
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(writer.directory_sync_count(), 1);

        writer.write(b"second").expect("second write lands");
        assert_eq!(writer.replacement_count(), 2);
        assert_eq!(writer.directory_sync_count(), 2);

        // Bytes already durable need neither a replacement nor a barrier.
        writer.write(b"second").expect("identical write lands");
        assert_eq!(writer.replacement_count(), 2);
        assert_eq!(writer.directory_sync_count(), 2);
    }

    #[test]
    fn a_replacement_that_never_proved_durable_is_not_trusted() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.write(b"first").expect("first write lands");

        // The rename lands but the directory barrier fails, so the file
        // visibly holds bytes nothing has proven durable.
        writer.set_injected_directory_sync_failures(1);
        assert!(writer.write(b"second").is_err());
        assert_eq!(fs::read(&path).expect("store"), b"second");

        // Asking for the first bytes again must replace the file, not trust
        // the digest recorded before the unproven replacement.
        assert_eq!(
            writer.write(b"first").expect("revert lands"),
            AtomicWriteOutcome::Written
        );
        assert_eq!(fs::read(&path).expect("store"), b"first");
        writer
            .flush(Duration::from_secs(5))
            .expect("nothing is left pending");
    }
}
