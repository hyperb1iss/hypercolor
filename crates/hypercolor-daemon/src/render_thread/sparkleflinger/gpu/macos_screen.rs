mod cache;
mod color;
mod contract;
mod import;
mod model;
mod preparation;
mod recovery;
mod reduction;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::input::screen::{
    ResolvedScreenPublicationDescriptor, ScreenNativeExecutionTarget,
    ScreenNativeExecutionTargetId, ScreenPlanGeneration,
};
use hypercolor_macos_capture::MacosCaptureFrame;
use hypercolor_macos_gpu_interop::{
    ImportedMacosScreenFrame, MacosNativeReducer, MacosScreenBridge as MacosInteropScreenBridge,
};

use self::cache::MacosScreenCache;
pub(crate) use self::model::PreparedMacosScreenTarget;
pub(in crate::render_thread::sparkleflinger::gpu) use self::preparation::create_screen_bridge;

pub(in crate::render_thread::sparkleflinger::gpu) struct MacosScreenBridge {
    pub(in crate::render_thread::sparkleflinger::gpu) device: wgpu::Device,
    pub(in crate::render_thread::sparkleflinger::gpu) interop: MacosInteropScreenBridge,
    pub(in crate::render_thread::sparkleflinger::gpu) reducer: MacosNativeReducer,
    cache: MacosScreenCache,
    target_id: ScreenNativeExecutionTargetId,
}

impl MacosScreenBridge {
    pub(in crate::render_thread::sparkleflinger::gpu) fn import_frame(
        &self,
        device: &wgpu::Device,
        resource_generation: u64,
        frame: Arc<MacosCaptureFrame>,
    ) -> Result<(ImportedMacosScreenFrame, u64)> {
        import::import_frame(self, device, resource_generation, frame)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn prepare_target(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        plan_generation: ScreenPlanGeneration,
    ) -> Result<PreparedMacosScreenTarget> {
        preparation::prepare_target(self, descriptor, plan_generation)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn clear_capture_caches(&self) {
        recovery::clear_capture_caches(self);
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn capture_caches_are_empty(&self) -> bool {
        self.interop.cached_wrap_count() == 0 && self.cache.is_empty()
    }

    fn interop_device(&self) -> &wgpu::Device {
        &self.device
    }

    fn target_id(&self) -> ScreenNativeExecutionTargetId {
        self.target_id
    }

    fn execution_target(
        self: &Arc<Self>,
        max_texture_dimension: u32,
    ) -> ScreenNativeExecutionTarget {
        preparation::create_screen_target(self, max_texture_dimension)
    }
}

#[cfg(test)]
pub(in crate::render_thread::sparkleflinger::gpu) use self::preparation::{
    prepared_macos_screen_target_exclusive_bytes, prepared_macos_screen_target_retention,
};
pub(crate) use self::recovery::{MacosScreenCopyOutcome, MacosScreenGpuRecoveryState};
