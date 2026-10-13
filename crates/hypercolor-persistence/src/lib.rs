//! Durable cross-platform file replacement for Hypercolor stores.
//!
//! Coordinators order writers inside this process. The daemon's single-instance
//! guard is the cross-process ownership contract for these files.

#[cfg(not(windows))]
use std::collections::HashMap;
#[cfg(not(windows))]
use std::ffi::OsStr;
use std::fs;
use std::io::{Read as _, Write};
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
/// The stored file mode when none was requested. Requested modes keep only
/// their permission bits, so every one of them, `0o000` included, differs.
#[cfg(unix)]
const NO_FILE_MODE: u32 = u32::MAX;

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
    /// The destination durably holds this payload: this reservation replaced
    /// it, or it already held exactly these bytes, verified and synced. See
    /// [`AtomicFileWriter`] for what the guarantee covers.
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
/// # Durability contract
///
/// The destination is daemon-owned. A write that reports
/// [`AtomicWriteOutcome::Written`] (or [`AtomicWriteCommitResult::DurableWritten`])
/// leaves the destination durably holding its payload, with the requested
/// mode. That covers every outside edit that completed before the call,
/// including replacements, renames, and inode reuse.
///
/// The guarantee covers the destination's own directory entry in its
/// parent directory, which is resolved once when the destination is first
/// opened. Durability of ancestor directories, including an outside rename
/// or symlink swap of the state directory itself, belongs to whoever
/// changed them; no write here syncs ancestors up to the mount.
///
/// A payload identical to the last one this process made durable can land
/// without being rewritten. While any writer keeps the destination alive, it
/// remembers that payload's digest and the change stamp of the file it
/// landed as. A different stamp means the write goes ahead. An equal stamp
/// proves nothing on its own, so the skip opens the destination itself
/// (never through a final symlink or reparse point), checks it is the
/// regular file this process wrote and carries the requested mode, compares its
/// bytes, and syncs the file and, on Unix, its parent directory. That
/// re-establishes whichever directory entry names the file, so the result
/// does not depend on the entry being the one this process created.
///
/// A writer active during the call is outside the contract. It can leave
/// other bytes at the destination after the call reports success, as it can
/// for any rename-based write, and no file API prevents that without an
/// exclusion protocol the stores do not have. Once that writer has
/// finished, the next write verifies the file again and repairs it.
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
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    injected_stale_stamps: std::sync::atomic::AtomicBool,
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    verified_skips: AtomicUsize,
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    verification_reads: AtomicUsize,
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    file_syncs: AtomicUsize,
    #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
    synced_staged_mode: AtomicU32,
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
/// writes, so an identical payload can be confirmed instead of rewritten.
///
/// Nothing here is proof on its own. The change stamp is a fast reject: a
/// different stamp or file identity shows the file changed, and the write
/// goes ahead without reading anything. An equal stamp establishes nothing,
/// since timestamp precision varies by filesystem and Windows lets callers
/// set `ChangeTime`. A skip is reported only after opening the destination
/// itself, never through a final symlink or reparse point, and confirming
/// from that same handle that it is a regular file, the file this process staged
/// and synced and that it carries the requested mode, reading its bytes and
/// comparing them with the payload (or the digest), syncing that handle so
/// the bytes it holds are durable even if something rewrote identical bytes
/// without syncing, and then syncing the parent directory so whichever entry
/// names the file is durable too. The identity match carries no durability
/// weight: a rename or a reused inode number cannot leave the skip trusting
/// an entry nothing synced. Any mismatch or error falls through to a normal
/// write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DurableBytes {
    digest: PayloadDigest,
    /// The staged file as read from its own handle before the rename. Only
    /// its identity is compared, which a rename keeps.
    staged: hypercolor_platform_fs::FileChangeStamp,
    /// The destination's stamp once the barrier completed: a fast reject.
    stamp: hypercolor_platform_fs::FileChangeStamp,
}

type PayloadDigest = [u8; 32];

fn payload_digest(payload: &[u8]) -> PayloadDigest {
    Sha256::digest(payload).into()
}

/// What a verification compares the destination's bytes against.
#[derive(Clone, Copy)]
enum ExpectedBytes<'a> {
    /// The caller holds the payload, so the comparison is byte for byte.
    Payload(&'a [u8]),
    /// Only the recorded digest is at hand.
    Digest(&'a PayloadDigest),
}

impl DurableBytes {
    /// Whether the destination's stamp still matches. A mismatch proves the
    /// file changed; a match proves nothing.
    fn stamp_unchanged(&self, destination: &Destination) -> bool {
        #[cfg(any(test, feature = "persistence-test-hooks"))]
        if destination.injected_stale_stamps.load(Ordering::Acquire) {
            return true;
        }
        hypercolor_platform_fs::path_change_stamp(&destination.path)
            .ok()
            .flatten()
            == Some(self.stamp)
    }

    /// Whether the destination durably holds `expected` in the file this
    /// process wrote. Blocks on file I/O: a read and a file sync.
    fn verify(&self, destination: &Destination, expected: ExpectedBytes<'_>) -> bool {
        self.stamp_unchanged(destination)
            && verify_destination_file(destination, &self.staged, expected).unwrap_or(false)
    }
}

/// Open the destination, confirm from that handle that it is the staged
/// file with the requested mode, compare its bytes, and sync it.
fn verify_destination_file(
    destination: &Destination,
    staged: &hypercolor_platform_fs::FileChangeStamp,
    expected: ExpectedBytes<'_>,
) -> std::io::Result<bool> {
    let mut file = open_for_verification(&destination.path)?;
    let Some(opened) = hypercolor_platform_fs::file_change_stamp(&file)? else {
        return Ok(false);
    };
    if !opened.same_file(staged) || !has_requested_mode(destination, &opened) {
        return Ok(false);
    }
    if let ExpectedBytes::Payload(payload) = expected
        && u64::try_from(payload.len()).ok() != Some(opened.byte_len())
    {
        return Ok(false);
    }
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    destination
        .verification_reads
        .fetch_add(1, Ordering::AcqRel);
    let mut bytes = Vec::with_capacity(usize::try_from(opened.byte_len()).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    let matches = match expected {
        ExpectedBytes::Payload(payload) => bytes == payload,
        ExpectedBytes::Digest(digest) => payload_digest(&bytes) == *digest,
    };
    if !matches {
        return Ok(false);
    }
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    destination.file_syncs.fetch_add(1, Ordering::AcqRel);
    file.sync_all()?;
    // The entry naming the file may be one nothing synced: an outside rename
    // that moved this file away and back, or a new file that reused its
    // inode number. Syncing the directory makes whichever entry it is
    // durable, so the identity match above carries no durability weight.
    #[cfg(unix)]
    {
        #[cfg(any(test, feature = "persistence-test-hooks"))]
        destination.directory_syncs.fetch_add(1, Ordering::AcqRel);
        sync_parent_directory(&destination.parent).map_err(std::io::Error::other)?;
    }
    Ok(true)
}

/// Open the destination itself for verification. A final symlink or
/// reparse point, or anything but a regular file, is refused: syncing the
/// file it leads to would leave the link itself unsynced, so the write goes
/// ahead and replaces it.
fn open_for_verification(path: &Path) -> std::io::Result<fs::File> {
    hypercolor_platform_fs::open_regular_no_follow_for_sync(path)
}

#[cfg(unix)]
fn has_requested_mode(
    destination: &Destination,
    stamp: &hypercolor_platform_fs::FileChangeStamp,
) -> bool {
    let requested = destination.file_mode.load(Ordering::Acquire);
    requested == NO_FILE_MODE || stamp.permission_bits() == requested
}

#[cfg(not(unix))]
const fn has_requested_mode(
    _destination: &Destination,
    _stamp: &hypercolor_platform_fs::FileChangeStamp,
) -> bool {
    true
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
            file_mode: AtomicU32::new(NO_FILE_MODE),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            injected_replace_failures: AtomicUsize::new(0),
            #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
            injected_directory_sync_failures: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            replacements: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            directory_syncs: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            injected_stale_stamps: std::sync::atomic::AtomicBool::new(false),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            verified_skips: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            verification_reads: AtomicUsize::new(0),
            #[cfg(any(test, feature = "persistence-test-hooks"))]
            file_syncs: AtomicUsize::new(0),
            #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
            synced_staged_mode: AtomicU32::new(0),
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
    /// request wins for every writer sharing that path. Only its permission
    /// bits are kept, and every value is a real request, `0o000` included.
    /// Non-unix platforms ignore it and rely on inherited directory
    /// permissions.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem error when the destination directory cannot
    /// be created or resolved.
    pub fn with_file_mode(path: &Path, mode: u32) -> Result<Self, PersistenceError> {
        let writer = Self::new(path)?;
        #[cfg(unix)]
        writer
            .destination
            .file_mode
            .store(mode & 0o7777, Ordering::Release);
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

    /// Whether the file durably holds the bytes this process last made
    /// durable here, in the file it wrote, with the requested mode.
    ///
    /// This blocks on file I/O: it reads the file, compares it with the
    /// recorded digest, and syncs the file and its directory entry, as the
    /// identical-bytes skip does. A `true` answer covers outside edits that
    /// completed before the call, under the same daemon-owned contract as a
    /// write (see [`AtomicFileWriter`]); a writer active during the call can
    /// leave other bytes behind.
    #[must_use]
    pub fn holds_durable_bytes(&self) -> bool {
        let durable = self
            .destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .durable;
        durable.is_some_and(|durable| {
            durable.verify(&self.destination, ExpectedBytes::Digest(&durable.digest))
        })
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

    /// How many directory durability barriers this destination has run in
    /// this process. On Unix that is exactly one per replacement and one per
    /// successful verification; Windows runs none, since its replacement is
    /// write-through.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    #[must_use]
    pub fn directory_sync_count(&self) -> usize {
        self.destination.directory_syncs.load(Ordering::Acquire)
    }

    /// How many writes this destination confirmed instead of rewriting,
    /// after verifying the bytes on disk.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    #[must_use]
    pub fn verified_skip_count(&self) -> usize {
        self.destination.verified_skips.load(Ordering::Acquire)
    }

    /// How many times this destination's file was opened and read to
    /// verify its bytes.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    #[must_use]
    pub fn verification_read_count(&self) -> usize {
        self.destination.verification_reads.load(Ordering::Acquire)
    }

    /// How many file syncs this destination's writes and verifications ran:
    /// the staged file's before each replacement, and the destination's at
    /// each successful verification. Directory barriers are counted apart.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    #[must_use]
    pub fn file_sync_count(&self) -> usize {
        self.destination.file_syncs.load(Ordering::Acquire)
    }

    /// The permission bits the last staged file carried when it was synced,
    /// so tests can check the requested mode is part of what became durable.
    #[cfg(all(unix, any(test, feature = "persistence-test-hooks")))]
    #[must_use]
    pub fn synced_staged_mode(&self) -> u32 {
        self.destination.synced_staged_mode.load(Ordering::Acquire)
    }

    /// Make the change-stamp fast reject report no change, as on a
    /// filesystem whose timestamps miss a same-length rewrite, so tests can
    /// reach the content verification behind it.
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    pub fn set_injected_stale_stamps(&self, stale: bool) {
        self.destination
            .injected_stale_stamps
            .store(stale, Ordering::Release);
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
    let durable = {
        let state = destination
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The newest admitted generation only grows, so a payload that is
        // already behind it would lose the check before replacement too.
        if generation != state.latest_admitted_generation {
            return AtomicWriteCommitResult::Superseded;
        }
        state.durable
    };
    // Verification reads and syncs a file, so it runs outside the state
    // lock. Only one attempt per destination runs at a time, so nothing this
    // coordinator owns changes the file meanwhile.
    let holds_known_bytes = match durable {
        Some(durable) if durable.digest == digest => {
            if durable.verify(destination, ExpectedBytes::Payload(payload)) {
                #[cfg(any(test, feature = "persistence-test-hooks"))]
                destination.verified_skips.fetch_add(1, Ordering::AcqRel);
                return AtomicWriteCommitResult::DurableWritten;
            }
            false
        }
        // Only spares the observer and the history clock a read of bytes the
        // digest already says differ; it never skips a write.
        Some(durable) => durable.stamp_unchanged(destination),
        None => false,
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
    // The mode is part of what the sync makes durable: applied after it, a
    // crash could recover the staged file's default mode under a result
    // that promised the requested one.
    #[cfg(unix)]
    if let Err(source) = apply_file_mode(destination, temporary.as_file()) {
        return AtomicWriteCommitResult::FailedBeforeReplacement(
            PersistenceError::SetTemporaryMode {
                path: destination.path.clone(),
                source,
            },
        );
    }
    #[cfg(any(test, feature = "persistence-test-hooks"))]
    {
        destination.file_syncs.fetch_add(1, Ordering::AcqRel);
        #[cfg(unix)]
        if let Ok(metadata) = temporary.as_file().metadata() {
            use std::os::unix::fs::PermissionsExt as _;
            destination
                .synced_staged_mode
                .store(metadata.permissions().mode() & 0o7777, Ordering::Release);
        }
    }
    if let Err(source) = temporary.as_file().sync_all() {
        return AtomicWriteCommitResult::FailedBeforeReplacement(PersistenceError::SyncTemporary {
            path: destination.path.clone(),
            source,
        });
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

    // Which file is ours, read from the staged file's own handle. A rename
    // keeps that identity but moves the change stamp, so the fast-reject
    // stamp is read from the destination after the barrier, and only kept
    // when it still names this file. Neither is trusted as content.
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
        .flatten();
    destination
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .durable = staged
        .zip(landed)
        .filter(|(staged, landed)| staged.same_file(landed))
        .map(|(staged, stamp)| DurableBytes {
            digest,
            staged,
            stamp,
        });
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
    if mode == NO_FILE_MODE {
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

        // Bytes already durable need no replacement, only the verification
        // that they are still there: one file sync and one directory sync.
        let file_syncs = writer.file_sync_count();
        writer.write(b"second").expect("identical write lands");
        assert_eq!(writer.replacement_count(), 2);
        assert_eq!(writer.verified_skip_count(), 1);
        assert_eq!(writer.verification_read_count(), 1);
        assert_eq!(writer.file_sync_count(), file_syncs + 1);
        assert_eq!(writer.directory_sync_count(), 3);
    }

    #[test]
    fn the_requested_mode_is_applied_before_the_staged_file_syncs() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let writer = AtomicFileWriter::with_file_mode(&path, 0o640).expect("writer");
        writer.write(b"payload").expect("first write lands");
        assert_eq!(writer.synced_staged_mode(), 0o640);

        // An outside chmod, finished before the save: the identical save
        // must replace the file, and the mode must be in what it synced.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
        writer.write(b"payload").expect("save lands");
        assert_eq!(writer.replacement_count(), 2);
        assert_eq!(writer.synced_staged_mode(), 0o640);
        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o7777,
            0o640
        );
    }

    #[test]
    fn an_explicit_0000_mode_is_applied_and_verified() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let mode =
            |path: &Path| fs::metadata(path).expect("metadata").permissions().mode() & 0o7777;
        let writer = AtomicFileWriter::with_file_mode(&path, 0o000).expect("writer");
        writer.write(b"payload").expect("first write lands");
        assert_eq!(mode(&path), 0o000, "0000 is a mode, not the absence of one");

        // A readable mode no longer satisfies the request, so an identical
        // save replaces the file instead of trusting it.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
        writer.set_injected_stale_stamps(true);
        writer.write(b"payload").expect("save lands");
        assert_eq!(writer.verified_skip_count(), 0);
        assert_eq!(writer.replacement_count(), 2);
        assert_eq!(mode(&path), 0o000);
    }

    #[test]
    fn a_fifo_at_the_destination_is_replaced_without_waiting() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.write(b"payload").expect("first write lands");

        fs::remove_file(&path).expect("remove the payload");
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo should create the FIFO");
        writer.set_injected_stale_stamps(true);

        let (done, finished) = std::sync::mpsc::channel();
        let saving = writer.clone();
        std::thread::spawn(move || {
            let _ = done.send(saving.write(b"payload").map_err(|error| error.to_string()));
        });
        let outcome = finished
            .recv_timeout(Duration::from_secs(5))
            .expect("verification must not wait on a FIFO");
        assert_eq!(outcome, Ok(AtomicWriteOutcome::Written));
        assert_eq!(writer.verified_skip_count(), 0);
        assert!(
            fs::symlink_metadata(&path)
                .expect("destination metadata")
                .file_type()
                .is_file(),
            "the FIFO was replaced by a regular file"
        );
        assert_eq!(fs::read(&path).expect("store"), b"payload");
    }

    #[test]
    fn a_symlink_to_the_identical_payload_is_replaced_not_trusted() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let aside = directory.path().join("store.json.aside");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.write(b"payload").expect("first write lands");

        // The durable inode moved aside, and a symlink to it put in its
        // place: identity, mode, and bytes all match through the link.
        fs::rename(&path, &aside).expect("move the payload aside");
        std::os::unix::fs::symlink(&aside, &path).expect("link to it");
        writer.set_injected_stale_stamps(true);
        assert!(!writer.holds_durable_bytes());

        assert_eq!(
            writer.write(b"payload").expect("save lands"),
            AtomicWriteOutcome::Written
        );
        assert_eq!(writer.verified_skip_count(), 0);
        assert_eq!(writer.replacement_count(), 2, "the link was replaced");
        let landed = fs::symlink_metadata(&path).expect("destination metadata");
        assert!(landed.file_type().is_file(), "a regular file again");
        assert_eq!(fs::read(&path).expect("store"), b"payload");
    }

    #[test]
    fn a_skip_syncs_the_directory_entry_an_outside_rename_left() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let aside = directory.path().join("store.json.aside");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.write(b"payload").expect("first write lands");

        // Moved away and back by something that never synced the directory:
        // same inode, same bytes, an entry this process did not create.
        fs::rename(&path, &aside).expect("move away");
        fs::rename(&aside, &path).expect("move back");
        writer.set_injected_stale_stamps(true);
        let barriers = writer.directory_sync_count();

        assert_eq!(
            writer.write(b"payload").expect("identical write lands"),
            AtomicWriteOutcome::Written
        );
        assert_eq!(writer.replacement_count(), 1, "the bytes were kept");
        assert_eq!(writer.verified_skip_count(), 1);
        assert_eq!(
            writer.directory_sync_count(),
            barriers + 1,
            "the skip made the current entry durable"
        );
    }

    #[test]
    fn an_equal_stamp_over_different_bytes_is_still_written() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.write(b"original").expect("first write lands");
        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .expect("landed mtime");

        // A same-length rewrite of the same inode with its mtime restored,
        // on a filesystem whose stamps miss it.
        fs::write(&path, b"restored").expect("overwrite in place");
        fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(modified))
            .expect("restore mtime");
        writer.set_injected_stale_stamps(true);
        assert!(!writer.holds_durable_bytes());

        assert_eq!(
            writer.write(b"original").expect("save lands"),
            AtomicWriteOutcome::Written
        );
        assert_eq!(fs::read(&path).expect("store"), b"original");
        assert_eq!(writer.replacement_count(), 2, "a real write ran");
        assert_eq!(writer.verified_skip_count(), 0);
        assert!(writer.holds_durable_bytes());
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
