use hypercolor_types::canvas::{RenderSurfacePool, SurfaceStateCounts};
use hypercolor_types::scene::ZoneId;

use super::ZoneRuntime;
use super::frame_helpers::surface_backed_frame;
use crate::performance::FullFrameCopyMetrics;
use crate::render_thread::producer_queue::ProducerFrame;

impl ZoneRuntime {
    #[cfg(test)]
    pub(super) fn scene_cpu_backing_bytes(&self) -> u64 {
        let scene_bytes = self.scene_surface_pool.as_ref().map_or(0, |pool| {
            let bytes = pool.descriptor().checked_byte_len().unwrap_or(0);
            u64::try_from(bytes)
                .unwrap_or(u64::MAX)
                .saturating_mul(u64::try_from(pool.materialized_slot_count()).unwrap_or(u64::MAX))
        });
        self.target_canvases
            .values()
            .fold(scene_bytes, |total, canvas| {
                total.saturating_add(u64::try_from(canvas.rgba_len()).unwrap_or(u64::MAX))
            })
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn projected_scene_layer_capacity(&self) -> usize {
        self.projected_scene_layers.capacity()
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(super) const fn projected_scene_layer_allocation_count(&self) -> usize {
        self.projected_scene_layer_allocation_count
    }

    pub(super) fn surface_backed_scene_frame(
        &mut self,
        frame: ProducerFrame,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> anyhow::Result<Option<ProducerFrame>> {
        let Some(surface_pool) = &mut self.scene_surface_pool else {
            return Ok(Some(frame));
        };
        surface_backed_frame(surface_pool, frame, full_frame_copy)
    }

    pub(super) fn surface_backed_direct_frame(
        &mut self,
        group_id: ZoneId,
        frame: ProducerFrame,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> anyhow::Result<Option<ProducerFrame>> {
        let Some(surface_pool) = self.direct_surface_pools.get_mut(&group_id) else {
            return Ok(None);
        };
        surface_backed_frame(surface_pool, frame, full_frame_copy)
    }

    /// Total count of times the backing scene-surface pool had to reuse a
    /// still-shared Published slot (and therefore allocate a fresh canvas).
    /// Monotonically increasing; non-zero growth means the pool is
    /// undersized for current downstream fan-out.
    #[must_use]
    pub(crate) fn scene_surface_pool_saturation_reallocs(&self) -> u64 {
        self.scene_surface_pool
            .as_ref()
            .map_or(0, RenderSurfacePool::saturation_reallocs)
    }

    /// Same as `scene_surface_pool_saturation_reallocs` but summed across
    /// every direct-canvas group pool (one per HTML-face zone).
    #[must_use]
    pub(crate) fn direct_surface_pool_saturation_reallocs(&self) -> u64 {
        self.direct_surface_pools
            .values()
            .map(RenderSurfacePool::saturation_reallocs)
            .sum()
    }

    /// Count of slots the backing scene-surface pool has appended above its
    /// initial capacity since construction. Non-zero values are benign and
    /// reflect the pool settling at its working-set size.
    #[must_use]
    pub(crate) fn scene_surface_pool_grown_slots(&self) -> u32 {
        self.scene_surface_pool
            .as_ref()
            .map_or(0, RenderSurfacePool::grown_slots)
    }

    /// Total grown slots across every direct-canvas group pool.
    #[must_use]
    pub(crate) fn direct_surface_pool_grown_slots(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(RenderSurfacePool::grown_slots)
            .sum()
    }

    #[must_use]
    pub(crate) fn scene_surface_pool_max_slots(&self) -> u32 {
        self.scene_surface_pool.as_ref().map_or(0, |pool| {
            u32::try_from(pool.max_slots()).unwrap_or(u32::MAX)
        })
    }

    #[must_use]
    pub(crate) fn direct_surface_pool_slot_count(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(|pool| u32::try_from(pool.slot_count()).unwrap_or(u32::MAX))
            .fold(0_u32, u32::saturating_add)
    }

    #[must_use]
    pub(crate) fn direct_surface_pool_max_slots(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(|pool| u32::try_from(pool.max_slots()).unwrap_or(u32::MAX))
            .fold(0_u32, u32::saturating_add)
    }

    pub(crate) fn scene_surface_pool_state_counts(&mut self) -> SurfaceStateCounts {
        self.scene_surface_pool
            .as_mut()
            .map_or_else(SurfaceStateCounts::default, RenderSurfacePool::slot_counts)
    }

    pub(crate) fn direct_surface_pool_state_counts(&mut self) -> SurfaceStateCounts {
        self.direct_surface_pools
            .values_mut()
            .map(RenderSurfacePool::slot_counts)
            .fold(SurfaceStateCounts::default(), merge_surface_state_counts)
    }

    pub(crate) fn scene_surface_pool_shared_published_slots(&mut self) -> u32 {
        let Some(pool) = &mut self.scene_surface_pool else {
            return 0;
        };
        let counts = pool.sharing_counts();
        u32::try_from(counts.shared_published).unwrap_or(u32::MAX)
    }

    pub(crate) fn scene_surface_pool_max_ref_count(&mut self) -> u32 {
        let Some(pool) = &mut self.scene_surface_pool else {
            return 0;
        };
        let counts = pool.sharing_counts();
        u32::try_from(counts.max_ref_count).unwrap_or(u32::MAX)
    }

    pub(crate) fn direct_surface_pool_shared_published_slots(&mut self) -> u32 {
        self.direct_surface_pools
            .values_mut()
            .map(|pool| u32::try_from(pool.sharing_counts().shared_published).unwrap_or(u32::MAX))
            .fold(0_u32, u32::saturating_add)
    }

    pub(crate) fn direct_surface_pool_max_ref_count(&mut self) -> u32 {
        self.direct_surface_pools
            .values_mut()
            .map(|pool| u32::try_from(pool.sharing_counts().max_ref_count).unwrap_or(u32::MAX))
            .max()
            .unwrap_or_default()
    }
}

fn merge_surface_state_counts(
    mut total: SurfaceStateCounts,
    counts: SurfaceStateCounts,
) -> SurfaceStateCounts {
    total.free = total.free.saturating_add(counts.free);
    total.dequeued = total.dequeued.saturating_add(counts.dequeued);
    total.published = total.published.saturating_add(counts.published);
    total
}
