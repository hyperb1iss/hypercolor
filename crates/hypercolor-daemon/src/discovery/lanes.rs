//! Per-target discovery lanes.
//!
//! A scan applies each target's results, and retires that target's
//! departures, while it holds the target's lane. The lane lock guards the
//! target's observation epoch, which counts the scans that have applied an
//! observation of the target, so later work can tell whether a newer scan
//! has seen the target since. Scans of different targets never wait on each
//! other, and two scans of one target never interleave.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use tokio::runtime::Handle;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::DiscoveryTarget;

/// Exclusive hold on one target's lane. It dereferences to the target's
/// observation epoch; a scan that applied an observation bumps it.
pub(super) type LaneHold = OwnedMutexGuard<u64>;

/// Lane registry plus the workers that run driver-scoped scans.
#[derive(Default)]
pub struct TargetLanes {
    lanes: StdMutex<HashMap<String, Arc<TargetLane>>>,
    shutdown: CancellationToken,
    workers: StdMutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct TargetLane {
    observation: Arc<Mutex<u64>>,
    /// A lane scan is waiting to start. Requests that arrive meanwhile are
    /// covered by it, since its observation begins after they did.
    queued: AtomicBool,
}

impl TargetLanes {
    /// Hold `target`'s lane, waiting only for a scan of the same target.
    pub(super) async fn hold(&self, target: &DiscoveryTarget) -> LaneHold {
        let lane = self.lane(target);
        let hold = Arc::clone(&lane.observation).lock_owned().await;
        // Whatever this scan observes postdates every request queued so far.
        lane.queued.store(false, Ordering::Release);
        hold
    }

    /// Hold `target`'s lane only if no scan has observed the target since
    /// `epoch`.
    ///
    /// A lane held right now counts as newer, since its holder is mid-way
    /// through a fresher observation, so this never waits on another scan.
    pub(super) fn hold_if_current(&self, target: &DiscoveryTarget, epoch: u64) -> Option<LaneHold> {
        let hold = Arc::clone(&self.lane(target).observation)
            .try_lock_owned()
            .ok()?;
        (*hold == epoch).then_some(hold)
    }

    /// Mark a lane scan queued for `target`. Returns `false` when one is
    /// already waiting to start and covers this request.
    pub(super) fn queue(&self, target: &DiscoveryTarget) -> bool {
        !self.lane(target).queued.swap(true, Ordering::AcqRel)
    }

    /// Run `scan` as a lane worker on `handle`. Returns `false`, without
    /// running it, once discovery has shut down.
    pub(super) fn spawn<F>(&self, handle: &Handle, scan: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.shutdown.is_cancelled() {
            return false;
        }
        let shutdown = self.shutdown.clone();
        let worker = handle.spawn(async move {
            // Biased, so a worker spawned as shutdown began never starts.
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {}
                () = scan => {}
            }
        });
        let mut workers = self.workers.lock().unwrap_or_else(PoisonError::into_inner);
        workers.retain(|worker| !worker.is_finished());
        workers.push(worker);
        true
    }

    /// Whether discovery has stopped accepting lane scans.
    #[must_use]
    pub fn is_shut_down(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Refuse new lane scans, cancel the running ones, and wait for every
    /// worker to stop.
    pub async fn shut_down(&self) {
        self.shutdown.cancel();
        let workers =
            std::mem::take(&mut *self.workers.lock().unwrap_or_else(PoisonError::into_inner));
        for worker in workers {
            if let Err(error) = worker.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "discovery lane worker did not stop cleanly");
            }
        }
    }

    fn lane(&self, target: &DiscoveryTarget) -> Arc<TargetLane> {
        let mut lanes = self.lanes.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(lanes.entry(target.as_str().to_owned()).or_default())
    }
}
