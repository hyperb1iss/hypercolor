//! One daemon run's scene revision sequence: where it starts, the opaque
//! epoch that names it, and the persisted reservation that keeps every
//! later run's revisions above it.
//!
//! A revision is an optimistic-concurrency fence, so a fence a client
//! captured in a previous run must never pass in this one. A counter that
//! restarts at zero breaks that the moment it climbs back past the old
//! value. Two mechanisms close it, each covering the other's blind spot:
//!
//! **The reservation.** `scene-revisions.json` in the state tier records a
//! ceiling, and a run hands out only revisions strictly below a ceiling it
//! made durable. Startup reads the previous run's ceiling, starts at or
//! above it, and persists a new ceiling one [`REVISION_RESERVATION_BLOCK`]
//! further on before the first commit. Halfway through a block the next
//! one is reserved on the blocking pool, off the commit path; only a run
//! that reaches its ceiling before that write lands persists inline. This
//! holds whatever the wall clock does, and across crashes, because a
//! ceiling always reaches disk before any revision it covers is used.
//!
//! **The clock.** The start is also never below the wall clock in
//! microseconds since the Unix epoch, which protects an install whose
//! reservation could not be read or written: the previous run started
//! earlier and would have needed more than one commit per microsecond to
//! climb past now. The block is about a second of that clock, so a
//! reservation never runs far ahead of it and the clock term stays the
//! one that decides the start in practice. The clock term is clamped
//! below 2^52, and a persisted floor at or above 2^53 is distrusted, so
//! revisions stay exact for clients that hold JSON numbers as doubles.
//!
//! The epoch is a fresh random identifier for every run. It never orders
//! anything; clients compare it for equality to notice a new run and
//! re-anchor before trusting any revision they held from the old one.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// The identity and starting point of one daemon run's scene revisions.
#[derive(Debug)]
pub struct SceneRun {
    epoch: Arc<str>,
    start: SceneRevision,
    ledger: Option<RevisionLedger>,
}

impl SceneRun {
    /// A run that starts at revision zero and persists nothing, for
    /// isolated in-memory consumers that never outlive their process.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            epoch: fresh_epoch(),
            start: 0,
            ledger: None,
        }
    }

    /// Resume the revision sequence recorded under `state_dir`, starting
    /// above every revision a previous run could have handed out.
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
        let store = Arc::new(ReservationStore::new(state_dir.join(SCENE_REVISIONS_FILE)));
        let floor = match read_reservation(store.path()) {
            Ok(Some(floor)) if floor >= MAX_TRUSTED_FLOOR => {
                tracing::error!(
                    path = %store.path().display(),
                    floor,
                    "Scene revision reservation is beyond any real ceiling; starting from the clock alone"
                );
                None
            }
            Ok(floor) => floor,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Scene revision reservation is unreadable; starting from the clock alone"
                );
                None
            }
        };
        let start = floor.unwrap_or(0).max(clock_base(now));
        Self {
            epoch: fresh_epoch(),
            start,
            ledger: Some(RevisionLedger::reserve(store, start, block)),
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

    /// The ceiling this run hands revisions out under: every revision it
    /// has handed out so far is strictly below it. `None` for an
    /// ephemeral run.
    #[must_use]
    pub fn reserved_until(&self) -> Option<SceneRevision> {
        self.ledger.as_ref().map(|ledger| ledger.ceiling)
    }

    pub(crate) fn into_parts(self) -> (Arc<str>, SceneRevision, Option<RevisionLedger>) {
        (self.epoch, self.start, self.ledger)
    }
}

/// The persisted reservation a resumed run hands revisions out under.
#[derive(Debug)]
pub(crate) struct RevisionLedger {
    store: Arc<ReservationStore>,
    block: u64,
    /// Every revision this run hands out stays strictly below this. It is
    /// a ceiling the store made durable, or one accepted in memory after
    /// a write failed, when only the clock still protects the next run.
    ceiling: SceneRevision,
    /// The revision that starts the background reservation of the next
    /// block.
    refresh_at: SceneRevision,
    /// Whether a background reservation is in flight.
    refreshing: Arc<AtomicBool>,
}

impl RevisionLedger {
    fn reserve(store: Arc<ReservationStore>, start: SceneRevision, block: u64) -> Self {
        let block = block.max(2);
        let ceiling = start.saturating_add(block);
        if let Err(error) = store.persist_at_least(ceiling) {
            tracing::warn!(
                %error,
                ceiling,
                "Failed to persist the scene revision reservation; the next run relies on the clock alone"
            );
        }
        Self {
            store,
            block,
            ceiling,
            refresh_at: ceiling.saturating_sub(block / 2),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Make `revision` safe to hand out.
    ///
    /// Called under the commit sequencer's lock. Past the middle of a
    /// block it starts reserving the next block on the blocking pool and
    /// returns at once; it writes inline only when `revision` reaches a
    /// ceiling that write has not yet raised, which a run never does
    /// unless it commits half a block faster than one small file write.
    pub(crate) fn cover(&mut self, revision: SceneRevision) {
        let persisted = self.store.persisted();
        if persisted > self.ceiling {
            self.ceiling = persisted;
            self.refresh_at = persisted.saturating_sub(self.block / 2);
        }
        if revision >= self.ceiling {
            let ceiling = revision.saturating_add(self.block);
            if let Err(error) = self.store.persist_at_least(ceiling) {
                tracing::warn!(
                    %error,
                    ceiling,
                    "Failed to persist the scene revision reservation; the next run relies on the clock alone"
                );
            }
            self.ceiling = ceiling;
            self.refresh_at = ceiling.saturating_sub(self.block / 2);
            return;
        }
        if revision < self.refresh_at || self.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }
        // One background attempt per ceiling. If it fails, the inline
        // write at the ceiling is the retry.
        self.refresh_at = self.ceiling;
        let target = self.ceiling.saturating_add(self.block);
        let store = Arc::clone(&self.store);
        let refreshing = Arc::clone(&self.refreshing);
        let extend = move || {
            if let Err(error) = store.persist_at_least(target) {
                tracing::warn!(
                    %error,
                    ceiling = target,
                    "Failed to reserve the next scene revision block ahead of time"
                );
            }
            refreshing.store(false, Ordering::Release);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(extend)),
            Err(_) => extend(),
        }
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
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::domain::commit::SceneCommitSequencer;
    use hypercolor_core::bus::HypercolorBus;

    fn on_disk(dir: &Path) -> u64 {
        read_reservation(&dir.join(SCENE_REVISIONS_FILE))
            .ok()
            .flatten()
            .expect("the reservation is on disk")
    }

    /// Every revision handed out is below the ceiling on disk at the
    /// moment it is handed out, whether the next block was reserved inline
    /// or ahead of time.
    fn assert_every_revision_is_covered(dir: &Path, commits: u64) {
        let run = SceneRun::resume_with_block(dir, UNIX_EPOCH + Duration::from_secs(1_000), 8);
        let (_, start, ledger) = run.into_parts();
        let sequencer = Arc::new(SceneCommitSequencer::resuming(start, ledger));
        let bus = Arc::new(HypercolorBus::new());
        let mut last = start;
        for _ in 0..commits {
            let ticket = sequencer.admit(Arc::clone(&bus));
            last = ticket.generation();
            assert!(
                on_disk(dir) > last,
                "revision {last} was handed out at or above the persisted ceiling"
            );
            ticket.release(Vec::new());
        }
        let next = SceneRun::resume_with_block(dir, UNIX_EPOCH, 8);
        assert!(next.start_revision() > last);
    }

    #[test]
    fn a_run_without_a_runtime_reserves_each_block_before_using_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_every_revision_is_covered(dir.path(), 100);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_reserves_its_next_block_off_the_commit_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        // On the blocking pool, so the ledger sees a runtime and reserves
        // ahead of time through it.
        tokio::task::spawn_blocking(move || assert_every_revision_is_covered(&path, 100))
            .await
            .expect("the commit loop finishes");
    }

    #[test]
    fn an_unwritable_reservation_falls_back_to_the_clock() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A directory where the file should be makes every read and write
        // fail.
        std::fs::create_dir_all(dir.path().join(SCENE_REVISIONS_FILE)).expect("blocker");
        let now = UNIX_EPOCH + Duration::from_secs(42);
        let run = SceneRun::resume_with_block(dir.path(), now, 4);
        assert_eq!(run.start_revision(), 42_000_000);
    }

    #[test]
    fn the_store_never_lowers_the_ceiling_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ReservationStore::new(dir.path().join(SCENE_REVISIONS_FILE));
        store.persist_at_least(10).expect("write");
        store
            .persist_at_least(5)
            .expect("a lower ceiling is a no-op");
        assert_eq!(on_disk(dir.path()), 10);
        assert_eq!(store.persisted(), 10);
    }
}
