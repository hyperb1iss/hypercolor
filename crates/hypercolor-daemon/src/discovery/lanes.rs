//! Per-target discovery lanes.
//!
//! A scan applies each target's results, and reconciles that target's
//! departures, while it holds the target's lane. Scans of different targets
//! never wait on each other, and two scans of one target never interleave,
//! so an older observation of a target cannot land after a newer one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use tokio::sync::{Mutex, OwnedMutexGuard};

use super::DiscoveryTarget;

/// One lane per discovery target.
#[derive(Default)]
pub struct TargetLanes {
    lanes: StdMutex<HashMap<String, Arc<TargetLane>>>,
}

#[derive(Default)]
struct TargetLane {
    scan: Arc<Mutex<()>>,
}

impl TargetLanes {
    /// Hold `target`'s lane, waiting only for a scan of the same target.
    pub(super) async fn hold(&self, target: &DiscoveryTarget) -> OwnedMutexGuard<()> {
        let lane = self.lane(target);
        Arc::clone(&lane.scan).lock_owned().await
    }

    fn lane(&self, target: &DiscoveryTarget) -> Arc<TargetLane> {
        let mut lanes = self.lanes.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(lanes.entry(target.as_str().to_owned()).or_default())
    }
}
