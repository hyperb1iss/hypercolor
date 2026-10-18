//! Per-target discovery lanes.
//!
//! A scan applies each target's results, and retires that target's
//! departures, while it holds the target's lane. The lane lock guards the
//! target's observation epoch, which counts the scans that have applied an
//! observation of the target, so later work can tell whether a newer scan
//! has seen the target since. Scans of different targets never wait on each
//! other, and two scans of one target never interleave.
//!
//! The lanes also own discovery's shutdown. Lane scans and the background
//! connects scans start are tracked, and shutdown cancels them and waits
//! until every one has stopped.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Instant;

use tokio::runtime::Handle;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use hypercolor_types::device::DeviceId;

use super::{DiscoveryRuntime, DiscoveryTarget};

/// Exclusive hold on one target's lane. It dereferences to the target's
/// observation epoch; a scan that applied an observation bumps it.
pub(super) type LaneHold = OwnedMutexGuard<u64>;

/// Lane registry plus the tracked work that shutdown has to stop.
#[derive(Default)]
pub struct TargetLanes {
    lanes: StdMutex<HashMap<String, Arc<TargetLane>>>,
    shutdown: CancellationToken,
    tasks: StdMutex<TrackedTasks>,
}

#[derive(Default)]
struct TargetLane {
    observation: Arc<Mutex<u64>>,
    /// A lane scan is waiting to start. Requests that arrive meanwhile are
    /// covered by it, since its observation begins after they did.
    queued: AtomicBool,
}

#[derive(Default)]
struct TrackedTasks {
    /// Lane scans. Each stops on its own once shutdown is signalled and
    /// drains its own children before it finishes.
    workers: Vec<JoinHandle<()>>,
    /// Background lifecycle work, which has no cancellation point of its
    /// own, so shutdown aborts it and then waits for it.
    background: Vec<JoinHandle<()>>,
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

    /// Hold `target`'s lane without standing in for a scan, so a queued lane
    /// scan still runs afterwards.
    async fn acquire(&self, target: &DiscoveryTarget) -> LaneHold {
        Arc::clone(&self.lane(target).observation)
            .lock_owned()
            .await
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

    /// Run a lane scan on `handle`. It must watch
    /// [`shutdown_signal`](Self::shutdown_signal) and stop on its own.
    /// Returns `false`, without running it, once discovery has shut down.
    pub(super) fn spawn_worker<F>(&self, handle: &Handle, scan: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.track(handle, scan, |tasks| &mut tasks.workers)
    }

    /// Run background lifecycle work on `handle`. Returns `false`, without
    /// running it, once discovery has shut down.
    pub(super) fn spawn_background<F>(&self, handle: &Handle, work: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.track(handle, work, |tasks| &mut tasks.background)
    }

    /// Token that fires when discovery shuts down.
    #[must_use]
    pub fn shutdown_signal(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Whether discovery has stopped accepting work.
    #[must_use]
    pub fn is_shut_down(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Refuse new work, stop the running work, and wait until every tracked
    /// task, and every task those spawned, has finished.
    pub async fn shut_down(&self) {
        let TrackedTasks {
            workers,
            background,
        } = {
            let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            // Cancelling under the lock means no spawn can slip in after the
            // tasks are taken.
            self.shutdown.cancel();
            std::mem::take(&mut *tasks)
        };
        for worker in workers {
            await_stopped(worker, "discovery lane scan").await;
        }
        for task in &background {
            task.abort();
        }
        for task in background {
            await_stopped(task, "discovery background connect").await;
        }
    }

    fn track<F>(
        &self,
        handle: &Handle,
        future: F,
        bucket: fn(&mut TrackedTasks) -> &mut Vec<JoinHandle<()>>,
    ) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        if self.shutdown.is_cancelled() {
            return false;
        }
        let tracked = bucket(&mut tasks);
        tracked.retain(|task| !task.is_finished());
        tracked.push(handle.spawn(future));
        true
    }

    fn lane(&self, target: &DiscoveryTarget) -> Arc<TargetLane> {
        let mut lanes = self.lanes.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(lanes.entry(target.as_str().to_owned()).or_default())
    }
}

async fn await_stopped(task: JoinHandle<()>, label: &str) {
    if let Err(error) = task.await
        && !error.is_cancelled()
    {
        warn!(%error, task = label, "task did not stop cleanly at discovery shutdown");
    }
}

/// Ties a background connect to the lifecycle intent that issued it.
///
/// A scan hands connects that may outlive its lane hold to the background.
/// Before such a connect publishes its mapping it takes the device's lane,
/// so any scan of the target in progress finishes first, and confirms the
/// lifecycle still waits on this very connect. A device a newer scan
/// retired meanwhile, or one a reissued connect now owns, is never mapped
/// by the stale connect.
#[derive(Clone, Debug)]
pub(crate) struct ConnectFence {
    target: DiscoveryTarget,
    device_id: DeviceId,
    issued_at: Instant,
}

/// What a fenced connect found when it came to publish.
pub(crate) enum FenceCheck {
    /// Still wanted: publish while holding the lane.
    Publish(LaneHold),
    /// Another connect owns the device now; leave its connection alone.
    Yield,
    /// The device was retired: tear down the connection just opened.
    TearDown,
}

impl ConnectFence {
    /// Fence the connect in flight for `device_id`, or `None` when the
    /// lifecycle is not waiting on one.
    pub(crate) async fn for_connect(
        runtime: &DiscoveryRuntime,
        target: &DiscoveryTarget,
        device_id: DeviceId,
    ) -> Option<Self> {
        let issued_at = runtime
            .lifecycle_manager
            .lock()
            .await
            .connect_in_flight_since(device_id)?;
        Some(Self {
            target: target.clone(),
            device_id,
            issued_at,
        })
    }

    /// The device this fence guards.
    pub(crate) fn device_id(&self) -> DeviceId {
        self.device_id
    }

    /// Take the device's lane and decide whether the connect may publish.
    pub(crate) async fn check(&self, runtime: &DiscoveryRuntime) -> FenceCheck {
        let hold = runtime.target_lanes.acquire(&self.target).await;
        let lifecycle = runtime.lifecycle_manager.lock().await;
        match lifecycle.connect_in_flight_since(self.device_id) {
            Some(issued_at) if issued_at == self.issued_at => {
                drop(lifecycle);
                FenceCheck::Publish(hold)
            }
            Some(_) => FenceCheck::Yield,
            None if lifecycle
                .state(self.device_id)
                .is_some_and(|state| state.is_renderable()) =>
            {
                FenceCheck::Yield
            }
            None => FenceCheck::TearDown,
        }
    }
}
