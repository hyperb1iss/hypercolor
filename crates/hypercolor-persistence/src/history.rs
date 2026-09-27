//! Rolling previous-good generations for durable stores.
//!
//! A destination with a [`HistoryPolicy`] keeps copies of the content it
//! replaces in its own history directory, so a bad write can be undone. The
//! copy is taken inside the normal write path, before the atomic replacement,
//! and is itself durable before the replacement starts: a crash at any point
//! leaves either the old file with its copy already in history, or the new
//! file with the old one in history. It never leaves the old content only in
//! memory.
//!
//! Generations are named `{id:06}-{replaced_at}{extension}`, where `id` only
//! grows and `replaced_at` is the UTC instant the content stopped being
//! current (`20260926T221014.123Z`). Each file's modification time is set to
//! the replaced file's, so a listing can say when that content was written as
//! well as when it was replaced.
//!
//! Rotation is cheap and bounded:
//!
//! - Content that is byte-identical to the new payload is never rotated.
//! - Content already held by the newest generation is never copied again.
//! - Content this process wrote less than `min_interval` ago is transient and
//!   is not kept. Because the outgoing content must have been current for
//!   `min_interval`, two rotations of one destination are always at least
//!   `min_interval` apart, whatever the write rate. The first replacement in a
//!   process always qualifies, since the file predates it.
//! - [`capture_next_writes`] waives the interval once per destination, which
//!   the daemon uses on shutdown; a restore waives it for its own write.
//! - At most `generations` files are kept; the oldest are removed after each
//!   rotation.
//!
//! History is best effort. A rotation that fails is logged and the write
//! proceeds, because refusing to save live state is worse than missing one
//! backup.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::{AtomicFileWriter, AtomicWriteOutcome, Destination, PersistenceError};

const TIMESTAMP_FORMAT: &str = "%Y%m%dT%H%M%S%.3fZ";
const PARTIAL_PREFIX: &str = ".partial-";

/// Destinations with a history policy, held for the life of the process so
/// the policy outlives every short-lived writer of the same path.
static HISTORY_DESTINATIONS: LazyLock<Mutex<Vec<Arc<Destination>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static REPLACEMENT_OBSERVER: OnceLock<fn(&Path)> = OnceLock::new();

/// How many previous generations a destination keeps, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPolicy {
    directory: PathBuf,
    generations: usize,
    min_interval: Duration,
}

impl HistoryPolicy {
    /// Keep up to `generations` previous versions in `directory`, skipping
    /// content this process replaced less than `min_interval` after writing
    /// it. `generations == 0` keeps nothing.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>, generations: usize, min_interval: Duration) -> Self {
        Self {
            directory: directory.into(),
            generations,
            min_interval,
        }
    }

    /// Directory that holds this destination's generations.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Maximum number of generations kept.
    #[must_use]
    pub const fn generations(&self) -> usize {
        self.generations
    }

    /// Minimum time content must have been current before it is kept.
    #[must_use]
    pub const fn min_interval(&self) -> Duration {
        self.min_interval
    }
}

/// One kept previous version of a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// Monotonic identifier, unique within the history directory.
    pub id: u64,
    /// Path of the generation file.
    pub path: PathBuf,
    /// When this content stopped being the destination's content.
    pub replaced_at: DateTime<Utc>,
    /// When this content was written, when the filesystem recorded it.
    pub saved_at: Option<DateTime<Utc>>,
    /// Size in bytes.
    pub size: u64,
}

/// Result of restoring one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreOutcome {
    /// Whether the restore write replaced the destination.
    pub outcome: AtomicWriteOutcome,
    /// Generation that now holds the content the restore replaced, when that
    /// content was not already kept.
    pub previous_generation: Option<u64>,
}

/// A history operation that could not complete.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// The destination has no history policy in this process.
    #[error("no history is kept for {path}")]
    NotEnabled {
        /// Destination path.
        path: PathBuf,
    },
    /// No generation with this identifier exists.
    #[error("generation {id} does not exist in {directory}")]
    UnknownGeneration {
        /// Requested generation.
        id: u64,
        /// History directory searched.
        directory: PathBuf,
    },
    /// A history file or directory could not be read or written.
    #[error("history I/O failed for {path}: {source}")]
    Io {
        /// File or directory involved.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The restore write itself failed.
    #[error(transparent)]
    Persist(#[from] PersistenceError),
}

#[derive(Debug)]
pub(crate) struct HistoryTracking {
    policy: HistoryPolicy,
    last_replaced_at: Option<Instant>,
    capture_next: bool,
}

/// What the destination held immediately before a replacement.
#[derive(Debug)]
pub(crate) enum PreviousContent {
    /// Nobody needed the old bytes, so they were not read.
    NotRead,
    /// The destination did not exist.
    Absent,
    /// The destination's bytes.
    Present(Vec<u8>),
    /// The destination existed but could not be read.
    Unreadable,
}

impl PreviousContent {
    fn differs_from(&self, payload: &[u8]) -> bool {
        !matches!(self, Self::Present(bytes) if bytes == payload)
    }
}

impl AtomicFileWriter {
    /// Keep previous generations of this destination under `policy`.
    ///
    /// The policy belongs to the destination, so it applies to every writer
    /// of the same path for the rest of the process, and the newest call
    /// wins. A policy that keeps zero generations turns history off.
    pub fn enable_history(&self, policy: HistoryPolicy) {
        let mut destinations = HISTORY_DESTINATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut history = self
            .destination
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pinned = destinations
            .iter()
            .position(|destination| Arc::ptr_eq(destination, &self.destination));
        if policy.generations == 0 {
            *history = None;
            if let Some(index) = pinned {
                destinations.swap_remove(index);
            }
            return;
        }
        let last_replaced_at = history.as_ref().and_then(|state| state.last_replaced_at);
        *history = Some(HistoryTracking {
            policy,
            last_replaced_at,
            capture_next: false,
        });
        if pinned.is_none() {
            destinations.push(Arc::clone(&self.destination));
        }
    }

    /// This destination's history policy, when one is set.
    #[must_use]
    pub fn history_policy(&self) -> Option<HistoryPolicy> {
        self.destination
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|state| state.policy.clone())
    }
}

/// Keep the content each history-enabled destination replaces on its next
/// write, however recently it was written.
///
/// Identical content is still skipped. The daemon calls this before its
/// shutdown saves so the last state of the session is always kept.
pub fn capture_next_writes() {
    let destinations = HISTORY_DESTINATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for destination in destinations {
        if let Some(state) = destination
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            state.capture_next = true;
        }
    }
}

/// Install the process-wide callback told about every replacement that
/// changed a destination's bytes.
///
/// The callback runs on the thread that performed the replacement, after it
/// is visible, with the destination's canonical path. Background retries run
/// it too. Returns `false` when an observer was already installed.
pub fn set_replacement_observer(observer: fn(&Path)) -> bool {
    REPLACEMENT_OBSERVER.set(observer).is_ok()
}

/// List the generations in `directory`, oldest first.
///
/// A directory that does not exist has no generations. Files that do not
/// follow the generation naming scheme, such as partial copies a crash left
/// behind, are ignored.
///
/// # Errors
///
/// Returns an error when the directory exists but cannot be read.
pub fn list_generations(directory: &Path) -> Result<Vec<Generation>, HistoryError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(HistoryError::Io {
                path: directory.to_path_buf(),
                source,
            });
        }
    };
    let mut generations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| HistoryError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((id, replaced_at)) = parse_generation_name(&name) else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        generations.push(Generation {
            id,
            path: entry.path(),
            replaced_at,
            saved_at: metadata.modified().ok().map(DateTime::<Utc>::from),
            size: metadata.len(),
        });
    }
    generations.sort_by_key(|generation| generation.id);
    Ok(generations)
}

/// Replace the destination at `path` with generation `id` of its history.
///
/// The restore is an ordinary write through the destination's coordinator,
/// so it is atomic, and the content it replaces becomes a new generation,
/// which makes the restore itself undoable. The destination must have a
/// history policy in this process.
///
/// # Errors
///
/// Returns an error when history is not enabled for `path`, the generation
/// does not exist or cannot be read, or the write fails.
pub fn restore_generation(path: &Path, id: u64) -> Result<RestoreOutcome, HistoryError> {
    let writer = AtomicFileWriter::new(path)?;
    let policy = writer
        .history_policy()
        .ok_or_else(|| HistoryError::NotEnabled {
            path: path.to_path_buf(),
        })?;
    let before = list_generations(policy.directory())?;
    let generation = before
        .iter()
        .find(|generation| generation.id == id)
        .ok_or_else(|| HistoryError::UnknownGeneration {
            id,
            directory: policy.directory().to_path_buf(),
        })?;
    let payload = fs::read(&generation.path).map_err(|source| HistoryError::Io {
        path: generation.path.clone(),
        source,
    })?;
    let newest_before = before.last().map(|generation| generation.id);
    if let Some(state) = writer
        .destination
        .history
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        state.capture_next = true;
    }
    let outcome = writer.write(&payload)?;
    let newest_after = list_generations(policy.directory())?
        .last()
        .map(|generation| generation.id);
    Ok(RestoreOutcome {
        outcome,
        previous_generation: newest_after.filter(|after| Some(*after) != newest_before),
    })
}

/// Read what the destination holds, when rotation or the observer needs it.
pub(crate) fn previous_content(destination: &Destination) -> PreviousContent {
    if !rotation_due(destination) && REPLACEMENT_OBSERVER.get().is_none() {
        return PreviousContent::NotRead;
    }
    match fs::read(&destination.path) {
        Ok(bytes) => PreviousContent::Present(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PreviousContent::Absent,
        Err(error) => {
            tracing::warn!(
                path = %destination.path.display(),
                %error,
                "Could not read the file about to be replaced"
            );
            PreviousContent::Unreadable
        }
    }
}

/// Copy the outgoing content into history before it is replaced.
pub(crate) fn rotate_before_replace(
    destination: &Destination,
    previous: &PreviousContent,
    payload: &[u8],
) {
    let PreviousContent::Present(previous) = previous else {
        return;
    };
    if previous.as_slice() == payload || !rotation_due(destination) {
        return;
    }
    let Some(policy) = destination
        .history
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|state| state.policy.clone())
    else {
        return;
    };
    match write_generation(destination, &policy, previous) {
        Ok(Some(id)) => tracing::debug!(
            path = %destination.path.display(),
            generation = id,
            "Kept previous generation"
        ),
        Ok(None) => {}
        Err(error) => tracing::warn!(
            path = %destination.path.display(),
            %error,
            "Could not keep the previous generation; writing without it"
        ),
    }
}

/// Record a visible replacement: restart the transient window and tell the
/// observer when the bytes changed.
pub(crate) fn note_replacement(
    destination: &Destination,
    previous: &PreviousContent,
    payload: &[u8],
) {
    if let Some(state) = destination
        .history
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        state.last_replaced_at = Some(Instant::now());
        state.capture_next = false;
    }
    if let Some(observer) = REPLACEMENT_OBSERVER.get()
        && !matches!(previous, PreviousContent::NotRead)
        && previous.differs_from(payload)
    {
        observer(&destination.path);
    }
}

fn rotation_due(destination: &Destination) -> bool {
    destination
        .history
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|state| {
            state.capture_next
                || state
                    .last_replaced_at
                    .is_none_or(|replaced| replaced.elapsed() >= state.policy.min_interval)
        })
}

fn write_generation(
    destination: &Destination,
    policy: &HistoryPolicy,
    previous: &[u8],
) -> Result<Option<u64>, HistoryError> {
    let directory = policy.directory();
    fs::create_dir_all(directory).map_err(|source| HistoryError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let existing = list_generations(directory)?;
    if let Some(newest) = existing.last() {
        let newest_bytes = fs::read(&newest.path).map_err(|source| HistoryError::Io {
            path: newest.path.clone(),
            source,
        })?;
        if newest_bytes == previous {
            return Ok(None);
        }
    }

    let saved_at = fs::metadata(&destination.path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let extension = destination
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let mut id = existing.last().map_or(1, |newest| newest.id + 1);
    let replaced_at = Utc::now().format(TIMESTAMP_FORMAT).to_string();
    let mut target = directory.join(format!("{id:06}-{replaced_at}{extension}"));
    while target.exists() {
        id += 1;
        target = directory.join(format!("{id:06}-{replaced_at}{extension}"));
    }

    let io_error = |source| HistoryError::Io {
        path: target.clone(),
        source,
    };
    let mut temporary = tempfile::Builder::new()
        .prefix(PARTIAL_PREFIX)
        .tempfile_in(directory)
        .map_err(io_error)?;
    temporary.write_all(previous).map_err(io_error)?;
    temporary.as_file().sync_all().map_err(io_error)?;
    if let Some(saved_at) = saved_at {
        // The listing reads this back as when the content was written; a
        // filesystem that refuses it only loses that detail.
        let _ = temporary.as_file().set_modified(saved_at);
    }
    #[cfg(unix)]
    crate::apply_file_mode(destination, temporary.as_file()).map_err(io_error)?;
    let temporary = temporary.into_temp_path();
    hypercolor_platform_fs::replace_file(&temporary, &target).map_err(io_error)?;
    // Only the rename consumed the temporary; nothing is left to delete.
    let _ = temporary.keep();
    #[cfg(unix)]
    sync_directory(directory)?;

    prune(directory, policy.generations)?;
    Ok(Some(id))
}

fn prune(directory: &Path, keep: usize) -> Result<(), HistoryError> {
    let generations = list_generations(directory)?;
    let excess = generations.len().saturating_sub(keep);
    for generation in generations.into_iter().take(excess) {
        remove_if_present(&generation.path)?;
    }
    // Partial copies are written and renamed under the destination's
    // single-writer slot, and the daemon's instance guard keeps other
    // processes out, so any that exist were left by a crash.
    let entries = fs::read_dir(directory).map_err(|source| HistoryError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(PARTIAL_PREFIX))
        {
            remove_if_present(&entry.path())?;
        }
    }
    #[cfg(unix)]
    sync_directory(directory)?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), HistoryError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(HistoryError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), HistoryError> {
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .map_err(|source| HistoryError::Io {
            path: directory.to_path_buf(),
            source,
        })
}

fn parse_generation_name(name: &str) -> Option<(u64, DateTime<Utc>)> {
    let (id, rest) = name.split_once('-')?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = id.parse().ok()?;
    let stamp_end = rest.find('Z')? + 1;
    let replaced_at = NaiveDateTime::parse_from_str(&rest[..stamp_end], TIMESTAMP_FORMAT)
        .ok()?
        .and_utc();
    Some((id, replaced_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_names_round_trip() {
        let name = format!("000042-{}.json", Utc::now().format(TIMESTAMP_FORMAT));
        let (id, _) = parse_generation_name(&name).expect("generation name parses");
        assert_eq!(id, 42);
        assert!(parse_generation_name(".partial-abc123").is_none());
        assert!(parse_generation_name("notes.txt").is_none());
    }

    #[test]
    fn a_failed_replacement_after_rotation_leaves_one_durable_copy() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("store.json");
        let history = directory.path().join("history");
        fs::write(&path, b"stable").expect("seed store");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.enable_history(HistoryPolicy::new(&history, 10, Duration::from_hours(1)));

        // The crash window: the old content is already in history, and the
        // replacement never happens. Background retries keep failing until
        // the injector is disarmed, so the window stays open while we look.
        writer.set_injected_replace_failures(usize::MAX);
        assert!(writer.write(b"next").is_err());
        assert_eq!(fs::read(&path).expect("store"), b"stable");
        let kept = list_generations(&history).expect("history");
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read(&kept[0].path).expect("generation"), b"stable");

        // Recovery: the retried write replaces the file without copying the
        // same content into history twice.
        writer.set_injected_replace_failures(0);
        writer
            .flush(Duration::from_secs(5))
            .expect("retry converges");
        assert_eq!(fs::read(&path).expect("store"), b"next");
        assert_eq!(list_generations(&history).expect("history").len(), 1);
    }
}
