//! One daemon run's scene revision sequence: the opaque epoch that names
//! it and the revision it starts at.
//!
//! A revision is an optimistic-concurrency fence, and it is comparable
//! only within one run. The epoch is what keeps runs apart: the scene
//! fence a client sends back is `"<run_epoch>:<revision>"`, and one that
//! names another run is always stale (see
//! [`SceneFence`](hypercolor_types::api::scene::SceneFence)). Nothing is
//! persisted for this, and nothing near a commit does I/O.
//!
//! Revisions still start at the wall clock in microseconds since the Unix
//! epoch, clamped to at most 2^52, so a bare numeric fence from an earlier run
//! is practically unreachable too: that run started earlier and would need
//! more than one commit per microsecond to climb past now. Only a clock
//! that jumped backwards breaks that, which is why the epoch-qualified
//! fence is the safe one. The clamp keeps revisions exact for clients that
//! hold JSON numbers as doubles.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::commit::SceneRevision;

/// Upper bound for a run's starting revision. It leaves 2^52 commits of
/// headroom below 2^53, so revisions survive a trip through an IEEE double.
pub const CLOCK_BASE_CEILING: u64 = 1 << 52;

/// The identity and starting point of one daemon run's scene revisions.
#[derive(Debug)]
pub struct SceneRun {
    epoch: Arc<str>,
    start: SceneRevision,
}

impl SceneRun {
    /// A run that starts at revision zero, for isolated in-memory consumers
    /// that never outlive their process.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            epoch: fresh_epoch(),
            start: 0,
        }
    }

    /// A run whose revisions start at the wall clock, under a fresh epoch.
    #[must_use]
    pub fn from_clock() -> Self {
        Self::from_clock_at(SystemTime::now())
    }

    /// [`from_clock`](Self::from_clock) with the wall clock read as `now`.
    #[must_use]
    pub fn from_clock_at(now: SystemTime) -> Self {
        Self {
            epoch: fresh_epoch(),
            start: clock_base(now),
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

    pub(crate) fn into_parts(self) -> (Arc<str>, SceneRevision) {
        (self.epoch, self.start)
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
