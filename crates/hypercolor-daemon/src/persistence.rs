//! Durable cross-platform file replacement for daemon state stores.
//!
//! Coordinators order writers inside this process. The daemon's single-instance
//! guard is the cross-process ownership contract for these files.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

#[cfg(feature = "persistence-test-hooks")]
use std::cell::Cell;
#[cfg(feature = "persistence-test-hooks")]
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use tempfile::NamedTempFile;

static DESTINATIONS: LazyLock<Mutex<HashMap<PathBuf, Weak<Destination>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static RETRY_SUPERVISOR: OnceLock<Result<Arc<RetrySupervisor>, String>> = OnceLock::new();
#[cfg(feature = "persistence-test-hooks")]
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
    #[cfg(feature = "persistence-test-hooks")]
    injected_replace_failures: AtomicUsize,
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
            state_changed: Condvar::new(),
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
    #[cfg(feature = "persistence-test-hooks")]
    pub fn set_injected_replace_failures(&self, count: usize) {
        self.destination
            .injected_replace_failures
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
        result: &Result<AtomicWriteOutcome, PersistenceError>,
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
                Ok(AtomicWriteOutcome::Written) => {
                    if state.pending.as_ref().map(|pending| pending.generation) == Some(generation)
                    {
                        state.pending = None;
                    }
                    state.last_outcome = retry_attempt.then_some(AtomicWriteOutcome::Written);
                    state.last_error = None;
                }
                Ok(AtomicWriteOutcome::Superseded) => {
                    state.last_outcome = retry_attempt.then_some(AtomicWriteOutcome::Superseded);
                }
                Err(error) => {
                    state.last_error = Some(error.to_string());
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

        let result = try_write(self, pending.generation, &pending.payload);
        self.finish_attempt(pending.generation, &result, true);
        if result.is_err() {
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
    pub fn commit(mut self) -> Result<AtomicWriteOutcome, PersistenceError> {
        self.armed = false;
        if !self.destination.begin_foreground(self.generation) {
            return Ok(AtomicWriteOutcome::Superseded);
        }
        let result = try_write(&self.destination, self.generation, &self.payload);
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

fn try_write(
    destination: &Arc<Destination>,
    generation: u64,
    payload: &[u8],
) -> Result<AtomicWriteOutcome, PersistenceError> {
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
    if generation != state.latest_admitted_generation {
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

    let temporary_path = temporary.into_temp_path();
    hypercolor_platform_fs::replace_file(&temporary_path, &destination.path).map_err(|source| {
        PersistenceError::Replace {
            path: destination.path.clone(),
            source,
        }
    })?;

    #[cfg(unix)]
    sync_parent_directory(&destination.parent)?;
    drop(state);
    Ok(AtomicWriteOutcome::Written)
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
    #[cfg(feature = "persistence-test-hooks")]
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
#[cfg(feature = "persistence-test-hooks")]
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
        destinations.retain(|_, destination| destination.strong_count() > 0);
        destinations
            .values()
            .filter_map(Weak::upgrade)
            .map(|destination| AtomicFileWriter { destination })
            .collect::<Vec<_>>()
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
