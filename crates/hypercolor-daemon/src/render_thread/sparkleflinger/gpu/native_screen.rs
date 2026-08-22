#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use anyhow::{Context, Result};
use hypercolor_core::input::screen::ScreenNativeExecutionTarget;
#[cfg(target_os = "windows")]
use hypercolor_core::input::screen::{ScreenBranchPayload, ScreenBranchPublication};
#[cfg(target_os = "windows")]
use hypercolor_windows_capture::GpuSurfacePublication;

#[cfg(target_os = "windows")]
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameOrigin, WindowsScreenTextureLease,
};

use super::GpuSparkleFlinger;
#[cfg(target_os = "windows")]
use super::windows_screen::{
    NativeScreenCopyFailurePolicy, PreparedWindowsScreenTarget, create_screen_target,
    native_screen_copy_failure_policy, screen_storage_requires_cache_turnover,
};

impl GpuSparkleFlinger {
    pub(crate) fn screen_native_execution_target(
        &mut self,
    ) -> Option<&ScreenNativeExecutionTarget> {
        if !self.canvas_gpu_admitted {
            return None;
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        self.retry_macos_screen_execution();
        self.screen_target.as_ref()
    }

    pub(crate) fn release_native_screen_caches(&mut self) {
        if let Some(surfaces) = &mut self.surfaces {
            surfaces
                .compose_source_bind_groups
                .release_native_screen_entries();
            surfaces
                .source_copy_bind_groups
                .release_native_screen_entries();
        }
        for surfaces in self.compositor_surface_cache.values_mut().flatten() {
            surfaces
                .compose_source_bind_groups
                .release_native_screen_entries();
            surfaces
                .source_copy_bind_groups
                .release_native_screen_entries();
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        {
            if let Some(bridge) = &self.screen_bridge {
                bridge.clear_capture_caches();
            }
        }
        #[cfg(target_os = "windows")]
        {
            self.screen_storage_id = None;
        }
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        self.release_completed_native_screen_leases();
    }

    #[cfg(target_os = "windows")]
    fn reprepare_native_screen_target(&mut self) {
        self.release_native_screen_caches();
        self.screen_target = self
            .screen_bridge
            .as_ref()
            .and_then(|bridge| create_screen_target(bridge, self.probe.max_texture_dimension_2d));
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn copy_screen_publication(
        &mut self,
        publication: &Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        let Some(bridge) = self.screen_bridge.clone() else {
            return Ok(None);
        };
        let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
            return Ok(None);
        };
        let Some(native) = payload.surface().owner::<GpuSurfacePublication>() else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has an unknown platform owner");
        };
        let Some(prepared) = payload
            .surface()
            .retained_owner::<PreparedWindowsScreenTarget>()
        else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has no prepared renderer target");
        };
        let Some(target_lifetime) = payload.surface().resource_lifetime().cloned() else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has no renderer allocation lifetime");
        };
        let Some(capture_lifetime) = payload.surface().capture_resource_lifetime().cloned() else {
            self.reprepare_native_screen_target();
            anyhow::bail!("native screen publication has no capture allocation lifetime");
        };
        let copy = match bridge.interop.copy_publication(&prepared.interop, &native) {
            Ok(copy) => copy,
            Err(error) => {
                return match native_screen_copy_failure_policy(&error) {
                    NativeScreenCopyFailurePolicy::Retain => {
                        Err(error).context("native screen publication is not ready")
                    }
                    NativeScreenCopyFailurePolicy::Reprepare => {
                        self.reprepare_native_screen_target();
                        Err(error).context("failed to copy the native screen publication")
                    }
                    NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare => {
                        self.reprepare_native_screen_target();
                        Err(error).context("native screen target contents became uncertain")
                    }
                };
            }
        };
        if screen_storage_requires_cache_turnover(self.screen_storage_id, prepared.storage_id) {
            self.release_native_screen_caches();
            self.screen_storage_id = Some(prepared.storage_id);
        }
        let width = copy.width;
        let height = copy.height;
        let content_generation = copy.content_generation;
        let texture = copy.texture.as_ref().clone();
        let view = copy.view.as_ref().clone();
        Ok(Some(GpuTextureFrame {
            width,
            height,
            storage_id: prepared.storage_id,
            content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture,
            view,
            immutable_lease: None,
            windows_screen_lease: Some(WindowsScreenTextureLease::new(
                copy,
                target_lifetime,
                capture_lifetime,
            )),
        }))
    }
}
