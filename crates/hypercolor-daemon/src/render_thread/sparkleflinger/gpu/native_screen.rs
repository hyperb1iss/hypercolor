//! Native screen execution seam between the compositor and platform bridges.
//!
//! The compositor never names a platform. It owns one optional
//! [`NativeScreenBridge`] plus the neutral execution target that bridge
//! published, and every native screen operation goes through the trait.
//! Each platform module installs its own bridge at construction time and
//! keeps its interop handles, recovery state, and cache identities private.

#[cfg(all(target_os = "macos", feature = "screen-capture"))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::input::screen::ScreenNativeExecutionTarget;
use hypercolor_core::input::screen::implementer::ScreenBranchPublication;

use super::GpuSparkleFlinger;
use crate::render_thread::producer_queue::GpuTextureFrame;

/// What one native screen copy attempt did to the latched screen frame.
///
/// Targets without a native screen bridge still carry the full vocabulary
/// so the compositor's fold stays target-neutral; only `Ignored` is produced
/// there.
#[cfg_attr(
    not(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    )),
    allow(dead_code, reason = "only bridges produce the native arms")
)]
#[derive(Debug)]
pub(crate) enum NativeScreenCopyOutcome {
    /// A fresh native frame is ready for the screen queue.
    Copied(GpuTextureFrame),
    /// The publication is not a native payload; the CPU path owns it.
    Ignored,
    /// A transient native stall; the last frame stays latched.
    Deferred(anyhow::Error),
    /// The copy failed but the last frame's contents are still valid.
    #[allow(
        dead_code,
        reason = "produced by bridges whose copy targets survive a failed copy"
    )]
    Failed(anyhow::Error),
    /// The last frame's contents are uncertain and must be dropped.
    Invalidated(anyhow::Error),
    /// Native execution is gone until a rebuild succeeds; drop the frame.
    #[cfg_attr(
        target_os = "windows",
        allow(dead_code, reason = "only the macOS bridge has a rebuild state")
    )]
    Unavailable(anyhow::Error),
}

impl NativeScreenCopyOutcome {
    pub(crate) fn into_result(self) -> Result<Option<GpuTextureFrame>> {
        match self {
            Self::Copied(frame) => Ok(Some(frame)),
            Self::Ignored => Ok(None),
            Self::Deferred(error)
            | Self::Failed(error)
            | Self::Invalidated(error)
            | Self::Unavailable(error) => Err(error),
        }
    }
}

/// One platform's native screen execution path.
///
/// The bridge is taken out of the compositor for the duration of each call
/// so it can drive compositor state (submissions, caches, the published
/// execution target) without aliasing it.
pub(super) trait NativeScreenBridge: Send {
    /// Give the bridge a chance to rebuild a failed execution path before
    /// the compositor reads [`GpuSparkleFlinger::screen_target`].
    fn refresh(&mut self, _gpu: &mut GpuSparkleFlinger) {}

    /// Copy one native screen publication into a compositor texture.
    fn copy_screen_publication(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        publication: &Arc<ScreenBranchPublication>,
    ) -> NativeScreenCopyOutcome;

    /// Drop platform caches keyed on retired native identities.
    fn release_caches(&mut self) {}

    /// Capability features the screen source should learn from this bridge.
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    fn source_capability_features(&self) -> Vec<(&'static str, bool)> {
        Vec::new()
    }

    #[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// The bridge a freshly built compositor installs, if this platform has one.
pub(super) struct InstalledNativeScreen {
    pub(super) bridge: Option<Box<dyn NativeScreenBridge>>,
    pub(super) target: Option<ScreenNativeExecutionTarget>,
}

impl InstalledNativeScreen {
    #[allow(
        dead_code,
        reason = "the platform arms that lack a bridge or fail to open one use it"
    )]
    pub(super) const fn none() -> Self {
        Self {
            bridge: None,
            target: None,
        }
    }
}

/// Install the platform's native screen bridge for one compositor device.
#[allow(
    unused_variables,
    reason = "each platform arm consumes the subset of device handles it needs"
)]
#[cfg_attr(
    not(all(target_os = "macos", feature = "screen-capture")),
    allow(
        clippy::unnecessary_wraps,
        reason = "macOS bridge installation is fallible"
    )
)]
pub(super) fn install(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    max_texture_dimension: u32,
) -> Result<InstalledNativeScreen> {
    #[cfg(target_os = "windows")]
    {
        Ok(windows::install(device, queue, max_texture_dimension))
    }
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    {
        macos::install(device, max_texture_dimension)
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    )))]
    {
        Ok(InstalledNativeScreen::none())
    }
}

impl GpuSparkleFlinger {
    /// Run one bridge operation with the bridge lifted out of `self`.
    fn with_screen_bridge<R>(
        &mut self,
        operation: impl FnOnce(&mut dyn NativeScreenBridge, &mut Self) -> R,
    ) -> Option<R> {
        let mut bridge = self.screen_bridge.take()?;
        let result = operation(bridge.as_mut(), self);
        self.screen_bridge = Some(bridge);
        Some(result)
    }

    pub(crate) fn screen_native_execution_target(
        &mut self,
    ) -> Option<&ScreenNativeExecutionTarget> {
        if !self.canvas_gpu_admitted {
            return None;
        }
        self.with_screen_bridge(|bridge, gpu| bridge.refresh(gpu));
        self.screen_target.as_ref()
    }

    pub(crate) fn copy_screen_publication(
        &mut self,
        publication: &Arc<ScreenBranchPublication>,
    ) -> NativeScreenCopyOutcome {
        self.with_screen_bridge(|bridge, gpu| bridge.copy_screen_publication(gpu, publication))
            .unwrap_or(NativeScreenCopyOutcome::Ignored)
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(crate) fn screen_source_capability_features(&self) -> Vec<(&'static str, bool)> {
        self.screen_bridge
            .as_ref()
            .map(|bridge| bridge.source_capability_features())
            .unwrap_or_default()
    }

    pub(crate) fn release_native_screen_caches(&mut self) {
        self.release_native_screen_bind_groups();
        if let Some(bridge) = self.screen_bridge.as_mut() {
            bridge.release_caches();
        }
        self.release_completed_native_screen_leases();
    }

    /// Drop every cached bind group that borrows a native screen lease.
    pub(super) fn release_native_screen_bind_groups(&mut self) {
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
    }

    #[cfg(all(test, target_os = "macos", feature = "screen-capture"))]
    pub(super) fn screen_bridge_mut<B: NativeScreenBridge + 'static>(&mut self) -> Option<&mut B> {
        self.screen_bridge
            .as_mut()
            .and_then(|bridge| bridge.as_any_mut().downcast_mut::<B>())
    }
}
