//! Per-target discovery lanes.
//!
//! A scan applies each target's results, and reconciles that target's
//! departures, while it holds the target's lane. Scans of different targets
//! never wait on each other, and two scans of one target never interleave,
//! so a driver-scoped scan can run beside a long scan of unrelated drivers
//! without an older observation of its target landing after a newer one.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use tokio::runtime::Handle;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::DiscoveryTarget;

/// Lane registry plus the workers that run driver-scoped scans.
#[derive(Default)]
pub struct TargetLanes {
    lanes: StdMutex<HashMap<String, Arc<TargetLane>>>,
    shutdown: CancellationToken,
    workers: StdMutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct TargetLane {
    scan: Arc<Mutex<()>>,
    /// A lane scan is waiting to start. Requests that arrive meanwhile are
    /// covered by it, since its observation begins after they did.
    queued: AtomicBool,
}

impl TargetLanes {
    /// Hold `target`'s lane, waiting only for a scan of the same target.
    pub(super) async fn hold(&self, target: &DiscoveryTarget) -> OwnedMutexGuard<()> {
        let lane = self.lane(target);
        let guard = Arc::clone(&lane.scan).lock_owned().await;
        // Whatever this scan observes postdates every request queued so far.
        lane.queued.store(false, Ordering::Release);
        guard
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
