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
use hypercolor_core::input::screen::planner::{
    ResolvedScreenPublicationDescriptor, ScreenNativeExecutionTarget,
    ScreenNativeExecutionTargetId, ScreenPlanGeneration,
};
use hypercolor_macos_capture::MacosCaptureFrame;
use hypercolor_macos_gpu_interop::{
    ImportedMacosScreenFrame, MacosNativeReducer, MacosScreenBridge as MacosInteropScreenBridge,
};

use self::cache::MacosScreenCache;
#[cfg(test)]
pub(in crate::render_thread::sparkleflinger::gpu) use self::import::macos_screen_lease;
pub(crate) use self::model::PreparedMacosScreenTarget;
pub(in crate::render_thread::sparkleflinger::gpu) use self::recovery::MacosScreenGpuRecoveryState;
use super::super::GpuSparkleFlinger;
use super::{InstalledNativeScreen, NativeScreenBridge, NativeScreenCopyOutcome};
use hypercolor_core::input::screen::implementer::ScreenBranchPublication;
use hypercolor_macos_gpu_interop::probe_macos_metal4_capabilities;

/// ScreenCaptureKit frames imported as IOSurface-backed Metal textures.
pub(in crate::render_thread::sparkleflinger::gpu) struct MacosScreenHost {
    bridge: Option<Arc<MacosScreenBridge>>,
    recovery: MacosScreenGpuRecoveryState,
    metal4_capable: bool,
    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fail_next_rebuild: bool,
    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fail_next_import: bool,
}

pub(super) fn install(
    device: &wgpu::Device,
    max_texture_dimension: u32,
) -> Result<InstalledNativeScreen> {
    let metal4_capable = probe_macos_metal4_capabilities(device)?.all_required_facilities();
    let (bridge, target, recovery) =
        match preparation::create_screen_bridge(device, max_texture_dimension) {
            Ok((bridge, target)) => {
                let recovery = MacosScreenGpuRecoveryState::ready(target.id());
                (Some(bridge), Some(target), recovery)
            }
            Err(error) => {
                tracing::debug!(%error, "renderer does not expose native Metal screen execution");
                (None, None, MacosScreenGpuRecoveryState::unavailable(&error))
            }
        };
    Ok(InstalledNativeScreen {
        bridge: Some(Box::new(MacosScreenHost {
            bridge,
            recovery,
            metal4_capable,
            #[cfg(test)]
            fail_next_rebuild: false,
            #[cfg(test)]
            fail_next_import: false,
        })),
        target,
    })
}

impl MacosScreenHost {
    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn bridge(
        &self,
    ) -> Option<&Arc<MacosScreenBridge>> {
        self.bridge.as_ref()
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) const fn recovery(
        &self,
    ) -> &MacosScreenGpuRecoveryState {
        &self.recovery
    }
}

impl NativeScreenBridge for MacosScreenHost {
    fn refresh(&mut self, gpu: &mut GpuSparkleFlinger) {
        self.retry_execution(gpu);
    }

    fn copy_screen_publication(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        publication: &Arc<ScreenBranchPublication>,
    ) -> NativeScreenCopyOutcome {
        self.retry_execution(gpu);
        let result = self.try_copy_screen_publication(gpu, publication);
        self.finish_copy(gpu, result)
    }

    fn release_caches(&mut self) {
        if let Some(bridge) = &self.bridge {
            bridge.clear_capture_caches();
        }
    }

    fn source_capability_features(&self) -> Vec<(&'static str, bool)> {
        vec![("metal4", self.metal4_capable)]
    }

    #[cfg(test)]
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

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
