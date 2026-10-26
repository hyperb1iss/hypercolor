//! One daemon run's scene revision sequence: where it starts, the opaque
//! epoch that names it, and the durable reservation that keeps every later
//! run's revisions above it.
//!
//! A revision is an optimistic-concurrency fence, so a fence a client
//! captured in a previous run must never pass in this one. A counter that
//! restarts at zero breaks that the moment it climbs back past the old
//! value.
//!
//! **The reservation.** `scene-revisions.json` in the state tier records a
//! ceiling. A run hands out only revisions strictly below a ceiling that is
//! already durable on disk, and the next run starts at or above that
//! ceiling, so no revision can repeat whatever the wall clock does and
//! across crashes. Startup persists a ceiling one
//! [`REVISION_RESERVATION_BLOCK`] past the start before the first commit.
//! Once half a block is used, the next block is persisted on the blocking
//! pool off every commit path. A commit that finds its revision uncovered
//! (that background write failed or has not landed) waits for a
//! reservation before it takes any scene lock, and the render thread never
//! waits: a layout publication it cannot cover is rejected and retried.
//!
//! **Degraded mode.** When the reservation cannot be persisted, scene
//! commits past the durable ceiling are refused with
//! [`RevisionsUnavailable`], and each later commit retries the write at
//! most once per [`RESERVATION_RETRY_BACKOFF`]. That is a deliberate
//! choice over serving revisions the disk does not cover: an uncovered
//! revision can repeat after a crash and a clock rollback, and then a fence
//! from the earlier run passes against different contents, which silently
//! overwrites newer work. A refused write is loud, retryable, and clears on
//! its own once the state tier accepts writes again; nothing else in the
//! scene degrades.
//!
//! **The clock.** The start is also never below the wall clock in
//! microseconds since the Unix epoch, clamped below 2^52, so a lost or
//! unreadable reservation still starts far above an earlier run in
//! practice, and a persisted floor at or above 2^53 is distrusted. Both
//! keep revisions exact for clients that hold JSON numbers as doubles. The
//! block is about a second of that clock, so the reservation never runs
//! far ahead of it.
//!
//! The epoch is a fresh random identifier for every run. It never orders
//! anything; clients compare it for equality to notice a new run and
//! re-anchor before trusting any revision they held from the old one.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use super::commit::SceneRevision;
use crate::scene_revision_store::{ReservationStore, read_reservation};

pub use crate::scene_revision_store::SCENE_REVISIONS_FILE;

/// How many revisions one persisted reservation covers: about one second
/// of the microsecond clock the start is drawn from, so a burst of quick
/// restarts cannot push the reservation far ahead of the clock.
pub const REVISION_RESERVATION_BLOCK: u64 = 1 << 20;

/// Upper bound for the wall-clock term of a run's starting revision, kept
/// below 2^53 so the revision survives a trip through an IEEE double.
pub const CLOCK_BASE_CEILING: u64 = 1 << 52;

/// A persisted floor at or above this is treated as corrupt. A real one
/// cannot get there: the clock term is clamped to [`CLOCK_BASE_CEILING`]
/// and each run reserves one small block past its start.
pub const MAX_TRUSTED_FLOOR: u64 = 1 << 53;

/// How long a failed reservation write is remembered before a commit
/// tries the disk again. Commits in between are refused at once.
pub const RESERVATION_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// Why a scene commit was refused a revision.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "scene revisions cannot be reserved durably ({reason}); scene writes are refused until \
     the state directory accepts writes again"
)]
pub struct RevisionsUnavailable {
    reason: String,
}

impl RevisionsUnavailable {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub(crate) fn uncovered(revision: SceneRevision, durable: SceneRevision) -> Self {
        Self::new(format!(
            "revision {revision} is not below the durable ceiling {durable}"
        ))
    }

    /// What kept the reservation from reaching disk.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The identity and starting point of one daemon run's scene revisions.
#[derive(Debug)]
pub struct SceneRun {
    epoch: Arc<str>,
    start: SceneRevision,
    reservation: Option<Arc<RevisionReservation>>,
}

impl SceneRun {
    /// A run that starts at revision zero and persists nothing, for
    /// isolated in-memory consumers that never outlive their process.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            epoch: fresh_epoch(),
            start: 0,
            reservation: None,
        }
    }

    /// Resume the revision sequence recorded under `state_dir`, starting
    /// above every revision a previous run could have handed out.
    ///
    /// Persists the first reservation before returning, which is startup
    /// I/O: call it before any scene lock exists. When that write fails the
    /// run still starts, with every commit refused until a later
    /// reservation lands.
    #[must_use]
    pub fn resume(state_dir: &Path) -> Self {
        Self::resume_at(state_dir, SystemTime::now())
    }

    /// [`resume`](Self::resume) with the wall clock read as `now`.
    ///
    /// A clock that reads earlier than a previous run's is the case the
    /// persisted reservation exists for, so the result is still above
    /// everything that run handed out.
    #[must_use]
    pub fn resume_at(state_dir: &Path, now: SystemTime) -> Self {
        Self::resume_with_block(state_dir, now, REVISION_RESERVATION_BLOCK)
    }

    pub(crate) fn resume_with_block(state_dir: &Path, now: SystemTime, block: u64) -> Self {
        let path = state_dir.join(SCENE_REVISIONS_FILE);
        let floor = match read_reservation(&path) {
            Ok(Some(floor)) if floor >= MAX_TRUSTED_FLOOR => {
                tracing::error!(
                    path = %path.display(),
                    floor,
                    "Scene revision reservation is beyond any real ceiling; starting from the clock"
                );
                None
            }
            Ok(floor) => floor,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Scene revision reservation is unreadable; starting from the clock"
                );
                None
            }
        };
        let start = floor.unwrap_or(0).max(clock_base(now));
        let store = Arc::new(ReservationStore::new(path, floor.unwrap_or(0)));
        let reservation = Arc::new(RevisionReservation::new(store, block));
        let ceiling = start.saturating_add(reservation.block);
        if let Err(error) = reservation.store.persist_at_least(ceiling) {
            tracing::error!(
                error = format!("{error:#}"),
                ceiling,
                "Failed to persist the scene revision reservation; scene writes are refused until it lands"
            );
        }
        Self {
            epoch: fresh_epoch(),
            start,
            reservation: Some(reservation),
        }
    }

    /// The opaque identifier of this run. Compare by equality only.
    #[must_use]
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// The revision the scene stands at before this run's first commit.
    #[must_use]
    pub const fn start_revision(&self) -> SceneRevision {
        self.start
    }

    /// The durable ceiling: every revision this run may hand out is
    /// strictly below it. `None` for an ephemeral run, which has no
    /// ceiling.
    #[must_use]
    pub fn reserved_until(&self) -> Option<SceneRevision> {
        self.reservation
            .as_ref()
            .map(|reservation| reservation.durable())
    }

    pub(crate) fn into_parts(self) -> (Arc<str>, SceneRevision, Option<Arc<RevisionReservation>>) {
        (self.epoch, self.start, self.reservation)
    }

    #[cfg(test)]
    pub(crate) fn reservation_for_test(&self) -> Option<Arc<RevisionReservation>> {
        self.reservation.clone()
    }
}

/// The durable bound a resumed run hands revisions out under, and the
/// machinery that raises it without any commit path doing I/O.
#[derive(Debug)]
pub(crate) struct RevisionReservation {
    store: Arc<ReservationStore>,
    block: u64,
    /// Single flight for reservation writes, remembering the last failure
    /// so a failing disk is retried at most once per backoff.
    writer: tokio::sync::Mutex<Option<ReservationFailure>>,
    /// Whether a background top-up is queued or running.
    topping_up: Arc<AtomicBool>,
}

#[derive(Debug)]
struct ReservationFailure {
    at: Instant,
    reason: String,
}

impl RevisionReservation {
    fn new(store: Arc<ReservationStore>, block: u64) -> Self {
        Self {
            store,
            block: block.max(2),
            writer: tokio::sync::Mutex::new(None),
            topping_up: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The highest ceiling known to be durable on disk.
    pub(crate) fn durable(&self) -> SceneRevision {
        self.store.persisted()
    }

    /// Whether `revision` is below the durable ceiling, so a later run is
    /// certain to start above it.
    pub(crate) fn covers(&self, revision: SceneRevision) -> bool {
        revision < self.durable()
    }

    /// Make `needed` covered, persisting a further block on the blocking
    /// pool when it is not. Never call this while holding a scene lock.
    ///
    /// # Errors
    ///
    /// [`RevisionsUnavailable`] when the write fails, or failed less than
    /// [`RESERVATION_RETRY_BACKOFF`] ago.
    pub(crate) async fn ensure(
        self: &Arc<Self>,
        needed: SceneRevision,
    ) -> Result<(), RevisionsUnavailable> {
        if self.covers(needed) {
            self.top_up_if_low(needed);
            return Ok(());
        }
        let mut failure = self.writer.lock().await;
        if self.covers(needed) {
            return Ok(());
        }
        if let Some(previous) = failure.as_ref()
            && previous.at.elapsed() < RESERVATION_RETRY_BACKOFF
        {
            return Err(RevisionsUnavailable::new(previous.reason.clone()));
        }
        let target = needed.max(self.durable()).saturating_add(self.block);
        let store = Arc::clone(&self.store);
        let written = tokio::task::spawn_blocking(move || store.persist_at_least(target)).await;
        let outcome = match written {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("{error:#}")),
            Err(error) => Err(format!("reservation write did not finish: {error}")),
        };
        match outcome {
            Ok(()) => {
                *failure = None;
                Ok(())
            }
            Err(reason) => {
                tracing::error!(
                    error = %reason,
                    ceiling = target,
                    "Failed to persist the scene revision reservation; scene writes past the durable ceiling are refused"
                );
                *failure = Some(ReservationFailure {
                    at: Instant::now(),
                    reason: reason.clone(),
                });
                Err(RevisionsUnavailable::new(reason))
            }
        }
    }

    /// Start persisting the next block in the background once less than
    /// half a block of headroom is left above `revision`. Safe to call
    /// under any lock and on the render thread: it only spawns, and without
    /// a runtime it does nothing at all rather than write inline.
    pub(crate) fn top_up_if_low(self: &Arc<Self>, revision: SceneRevision) {
        let durable = self.durable();
        if durable.saturating_sub(revision) > self.block / 2 {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.topping_up.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = Arc::clone(self);
        let clear = ClearOnDrop(Arc::clone(&self.topping_up));
        runtime.spawn(async move {
            let _clear = clear;
            // Errors are logged and remembered by `ensure`.
            let _ = this.ensure(durable).await;
        });
    }

    #[cfg(test)]
    pub(crate) fn fail_writes_for_test(&self, fail: bool) {
        self.store.fail_writes_for_test(fail);
    }
}

/// Clears the top-up flag however its task ends, cancelled included, so a
/// runtime that drops the task never wedges future top-ups.
struct ClearOnDrop(Arc<AtomicBool>);

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Microseconds since the Unix epoch, clamped to [`CLOCK_BASE_CEILING`].
/// A clock set before the epoch contributes nothing.
fn clock_base(now: SystemTime) -> SceneRevision {
    now.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
        })
        .min(CLOCK_BASE_CEILING)
}

fn fresh_epoch() -> Arc<str> {
    Arc::from(uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests;
