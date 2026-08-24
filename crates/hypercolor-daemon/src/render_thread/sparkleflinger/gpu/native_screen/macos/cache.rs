use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};

use anyhow::Result;
use hypercolor_core::input::screen::planner::{
    ScreenPhysicalReductionDescriptor, ScreenPlanGeneration,
};
use hypercolor_macos_gpu_interop::MacosScreenStorageIdentity;

use super::model::PreparedMacosPhysicalTarget;
use crate::render_thread::sparkleflinger::gpu::NEXT_GPU_TEXTURE_STORAGE_ID;

pub(super) struct MacosScreenCache {
    storage_ids: Mutex<HashMap<MacosScreenStorageIdentity, u64>>,
    physical_targets: Mutex<
        Vec<(
            ScreenPlanGeneration,
            ScreenPhysicalReductionDescriptor,
            Weak<PreparedMacosPhysicalTarget>,
        )>,
    >,
}

impl MacosScreenCache {
    pub(super) fn new() -> Self {
        Self {
            storage_ids: Mutex::new(HashMap::new()),
            physical_targets: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn storage_id(&self, identity: MacosScreenStorageIdentity) -> Result<u64> {
        let mut storage_ids = self
            .storage_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match storage_ids.entry(identity) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(*entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let storage_id = next_texture_storage_id()?;
                entry.insert(storage_id);
                Ok(storage_id)
            }
        }
    }

    pub(super) fn physical_target(
        &self,
        plan_generation: ScreenPlanGeneration,
        descriptor: &ScreenPhysicalReductionDescriptor,
        create: impl FnOnce() -> Result<PreparedMacosPhysicalTarget>,
    ) -> Result<Arc<PreparedMacosPhysicalTarget>> {
        let mut targets = self
            .physical_targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        targets.retain(|(_, _, target)| target.strong_count() > 0);
        if let Some(target) = targets.iter().find_map(|(plan, candidate, target)| {
            (*plan == plan_generation && candidate == descriptor)
                .then(|| target.upgrade())
                .flatten()
        }) {
            return Ok(target);
        }
        let target = Arc::new(create()?);
        targets.push((plan_generation, descriptor.clone(), Arc::downgrade(&target)));
        Ok(target)
    }

    pub(super) fn clear_all(&self) {
        self.storage_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.physical_targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.storage_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
            && self
                .physical_targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
    }
}

pub(super) fn next_texture_storage_id() -> Result<u64> {
    NEXT_GPU_TEXTURE_STORAGE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| anyhow::anyhow!("GPU texture storage identity space is exhausted"))
}
