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
//! ceiling, and a run hands out only revisions strictly below the ceiling
//! it persisted. Startup reads the previous run's ceiling, starts at or
//! above it, and persists a new ceiling one [`REVISION_RESERVATION_BLOCK`]
//! further on before the first commit. A run that exhausts its block
//! persists the next one before handing out the revision that needs it.
//! This holds whatever the wall clock does, and across crashes, because
//! the ceiling always reaches disk before any revision it covers is used.
//!
//! **The clock.** The start is also never below the wall clock in
//! microseconds since the Unix epoch, which protects an install whose
//! reservation file was lost: the previous run started earlier and would
//! have needed more than one commit per microsecond to climb past now.
//! The clock term is clamped below 2^52 so revisions stay exact for
//! clients that hold JSON numbers as doubles.
//!
//! The epoch is a fresh random identifier for every run. It never orders
//! anything; clients compare it for equality to notice a new run and
//! re-anchor before trusting any revision they held from the old one.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::commit::SceneRevision;
use crate::persistence::{serialize_json_pretty, write_atomic};

/// The state-tier file that holds the revision reservation.
pub const SCENE_REVISIONS_FILE: &str = "scene-revisions.json";

/// How many revisions one persisted reservation covers. At thousands of
/// commits a second a run still needs weeks to exhaust one block, so the
/// commit path effectively never writes the file.
pub const REVISION_RESERVATION_BLOCK: u64 = 1 << 32;

/// Upper bound for the wall-clock term of a run's starting revision, kept
/// below 2^53 so the revision survives a trip through an IEEE double.
pub const CLOCK_BASE_CEILING: u64 = 1 << 52;

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
        let path = state_dir.join(SCENE_REVISIONS_FILE);
        let floor = match read_reservation(&path) {
            Ok(floor) => floor,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "Scene revision reservation is unreadable; starting from the clock alone"
                );
                None
            }
        };
        let start = floor.unwrap_or(0).max(clock_base(now));
        let mut ledger = RevisionLedger {
            path,
            reserved_until: start,
            block: block.max(1),
        };
        ledger.extend_past(start);
        Self {
            epoch: fresh_epoch(),
            start,
            ledger: Some(ledger),
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

    /// The persisted ceiling: every revision this run has handed out so
    /// far is strictly below it. `None` for an ephemeral run.
    #[must_use]
    pub fn reserved_until(&self) -> Option<SceneRevision> {
        self.ledger.as_ref().map(|ledger| ledger.reserved_until)
    }

    pub(crate) fn into_parts(self) -> (Arc<str>, SceneRevision, Option<RevisionLedger>) {
        (self.epoch, self.start, self.ledger)
    }
}

/// The persisted reservation a resumed run hands revisions out under.
#[derive(Debug)]
pub(crate) struct RevisionLedger {
    path: PathBuf,
    /// Every revision this run hands out stays strictly below this.
    reserved_until: SceneRevision,
    block: u64,
}

impl RevisionLedger {
    /// Make `revision` safe to hand out, persisting a further reservation
    /// first when it reaches the current ceiling.
    ///
    /// The write is synchronous and happens on the commit path, which is
    /// acceptable only because it happens once per
    /// [`REVISION_RESERVATION_BLOCK`] commits. A failed write is logged and
    /// the run carries on under the clock-derived guarantee alone.
    pub(crate) fn cover(&mut self, revision: SceneRevision) {
        if revision >= self.reserved_until {
            self.extend_past(revision);
        }
    }

    fn extend_past(&mut self, revision: SceneRevision) {
        let ceiling = revision.saturating_add(self.block);
        if let Err(error) = write_reservation(&self.path, ceiling) {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                ceiling,
                "Failed to persist the scene revision reservation; the next run relies on the clock alone"
            );
        }
        self.reserved_until = ceiling;
    }
}

/// On-disk shape of [`SCENE_REVISIONS_FILE`].
#[derive(Debug, Serialize, Deserialize)]
struct ReservationFile {
    /// Every scene revision any run has handed out is below this.
    reserved_until: SceneRevision,
}

fn read_reservation(path: &Path) -> anyhow::Result<Option<SceneRevision>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let file: ReservationFile = serde_json::from_slice(&bytes)?;
    Ok(Some(file.reserved_until))
}

fn write_reservation(path: &Path, reserved_until: SceneRevision) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serialize_json_pretty(&ReservationFile { reserved_until })?;
    write_atomic(path, &payload)?;
    Ok(())
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

    #[test]
    fn a_run_that_exhausts_its_block_persists_the_next_one_before_using_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let run = SceneRun::resume_with_block(dir.path(), now, 4);
        let start = run.start_revision();
        assert_eq!(run.reserved_until(), Some(start + 4));

        let (_, start, ledger) = run.into_parts();
        let sequencer = Arc::new(SceneCommitSequencer::resuming(start, ledger));
        let bus = Arc::new(HypercolorBus::new());
        for _ in 0..3 {
            sequencer.admit(Arc::clone(&bus)).release(Vec::new());
        }
        assert_eq!(
            read_reservation(&dir.path().join(SCENE_REVISIONS_FILE))
                .ok()
                .flatten(),
            Some(start + 4)
        );

        // The fourth revision reaches the ceiling, so the next block lands
        // on disk before the revision is handed out.
        let ticket = sequencer.admit(Arc::clone(&bus));
        assert_eq!(ticket.generation(), start + 4);
        assert_eq!(
            read_reservation(&dir.path().join(SCENE_REVISIONS_FILE))
                .ok()
                .flatten(),
            Some(start + 8)
        );
        ticket.release(Vec::new());

        let next = SceneRun::resume_with_block(dir.path(), UNIX_EPOCH, 4);
        assert!(
            next.start_revision() > start + 4,
            "a later run starts above every revision the exhausted run used"
        );
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
}
