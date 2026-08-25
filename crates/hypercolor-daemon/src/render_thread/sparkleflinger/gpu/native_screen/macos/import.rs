use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use hypercolor_core::input::screen::implementer::{ScreenBranchPayload, ScreenBranchPublication};
use hypercolor_core::input::screen::{PlatformGpuSurfaceOwner, ScreenResourceLifetime};
use hypercolor_macos_capture::MacosCaptureFrame;
use hypercolor_macos_gpu_interop::{ImportedMacosScreenFrame, MacosGpuInteropError};

use super::model::PreparedMacosScreenTarget;
use super::reduction;
use super::{MacosScreenBridge, MacosScreenHost};
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameOrigin, NativeScreenCacheRetention, NativeScreenTextureLease,
};
use crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger;
use crate::render_thread::sparkleflinger::gpu::native_screen::NativeScreenCopyOutcome;

/// Everything an imported IOSurface texture aliases and must outlive.
struct MacosScreenLeaseOwner {
    _imported: ImportedMacosScreenFrame,
    _capture_owner: PlatformGpuSurfaceOwner<MacosCaptureFrame>,
    _target_owner: PlatformGpuSurfaceOwner<PreparedMacosScreenTarget>,
    _shared_target_lifetime: Option<ScreenResourceLifetime>,
    _capture_lifetime: ScreenResourceLifetime,
}

/// Build the lease for one imported macOS screen texture.
///
/// The texture aliases the IOSurface directly, so cached bind groups keep
/// the whole lease alive rather than just the target allocation.
pub(crate) fn macos_screen_lease(
    imported: ImportedMacosScreenFrame,
    capture_owner: PlatformGpuSurfaceOwner<MacosCaptureFrame>,
    target_owner: PlatformGpuSurfaceOwner<PreparedMacosScreenTarget>,
    target_lifetime: ScreenResourceLifetime,
    shared_target_lifetime: Option<ScreenResourceLifetime>,
    capture_lifetime: ScreenResourceLifetime,
) -> NativeScreenTextureLease {
    NativeScreenTextureLease::new(
        MacosScreenLeaseOwner {
            _imported: imported,
            _capture_owner: capture_owner,
            _target_owner: target_owner,
            _shared_target_lifetime: shared_target_lifetime,
            _capture_lifetime: capture_lifetime,
        },
        target_lifetime,
        NativeScreenCacheRetention::FullLease,
    )
}

pub(super) fn import_frame(
    bridge: &MacosScreenBridge,
    device: &wgpu::Device,
    resource_generation: u64,
    frame: Arc<MacosCaptureFrame>,
) -> Result<(ImportedMacosScreenFrame, u64)> {
    let imported = bridge
        .interop
        .import_frame(device, resource_generation, frame)
        .context("failed to import the native macOS screen publication")?;
    let storage_id = bridge.cache.storage_id(imported.storage_identity())?;
    Ok((imported, storage_id))
}

impl MacosScreenHost {
    pub(super) fn finish_copy(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        result: Result<Option<GpuTextureFrame>>,
    ) -> NativeScreenCopyOutcome {
        match result {
            Ok(Some(frame)) => NativeScreenCopyOutcome::Copied(frame),
            Ok(None) => NativeScreenCopyOutcome::Ignored,
            Err(error) if is_transient_copy_failure(&error) => {
                NativeScreenCopyOutcome::Deferred(error)
            }
            Err(error) if error.downcast_ref::<FencedMacosScreenTarget>().is_some() => {
                NativeScreenCopyOutcome::Invalidated(error)
            }
            Err(error) => self.recover_execution(gpu, error),
        }
    }

    pub(super) fn try_copy_screen_publication(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        publication: &Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        let Some(bridge) = self.bridge.clone() else {
            anyhow::bail!("native macOS screen execution is unavailable");
        };
        let (surface, requires_work) = match publication.payload() {
            ScreenBranchPayload::GpuSurface(payload) => (payload.surface(), false),
            ScreenBranchPayload::NativeWork(payload) => (payload.source(), true),
            ScreenBranchPayload::Surface(_) | ScreenBranchPayload::Zones(_) => return Ok(None),
        };
        let capture_owner = surface
            .owner::<MacosCaptureFrame>()
            .context("native macOS screen publication has an unknown capture owner")?;
        let target_owner = surface
            .retained_owner::<PreparedMacosScreenTarget>()
            .context("native macOS screen publication has no prepared renderer target")?;
        let current_target_id = self
            .ready_target_id()
            .context("native macOS screen execution is not ready")?;
        validate_target_id(target_owner.target_id, current_target_id)?;
        let target_lifetime = surface
            .resource_lifetime()
            .cloned()
            .context("native macOS screen publication has no renderer allocation lifetime")?;
        let shared_target_lifetime = surface.shared_resource_lifetime().cloned();
        let capture_lifetime = surface
            .capture_resource_lifetime()
            .cloned()
            .context("native macOS screen publication has no capture allocation lifetime")?;
        let capture = capture_owner
            .downgrade()
            .upgrade()
            .context("native macOS capture owner retired before import")?;
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_import) {
            anyhow::bail!("injected native macOS screen importer failure");
        }
        let import_started = Instant::now();
        let imported = bridge.import_frame(&gpu.device, target_owner.resource_generation, capture);
        if let Some(timing_sink) = surface.timing_sink() {
            timing_sink.record_import(import_started.elapsed());
        }
        let (imported, storage_id) = imported?;
        anyhow::ensure!(
            imported.capture().storage_extent.width == surface.extent().width()
                && imported.capture().storage_extent.height == surface.extent().height(),
            "native macOS imported extent does not match the published surface"
        );
        let content_generation = imported.content_sequence();
        let submission_lease = macos_screen_lease(
            imported.clone(),
            capture_owner.clone(),
            target_owner.clone(),
            target_lifetime.clone(),
            shared_target_lifetime.clone(),
            capture_lifetime.clone(),
        );
        let resolved = if requires_work {
            gpu.flush_pending_output_submission()?;
            let reduction_started = Instant::now();
            let reduced = reduction::reduce_imported_frame(
                gpu,
                &bridge,
                &imported,
                &target_owner,
                content_generation,
                &submission_lease,
            )?;
            if reduced.submitted
                && let Some(timing_sink) = surface.timing_sink()
            {
                timing_sink.record_native_reduction_submission(reduction_started.elapsed());
            }
            reduced.frame
        } else {
            reduction::identity_frame(
                &imported,
                &target_owner,
                storage_id,
                surface.extent().width(),
                surface.extent().height(),
            )?
        };
        Ok(Some(GpuTextureFrame {
            width: resolved.width,
            height: resolved.height,
            storage_id: resolved.storage_id,
            content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: resolved.texture,
            view: resolved.view,
            immutable_lease: None,
            native_screen_lease: Some(macos_screen_lease(
                imported,
                capture_owner,
                target_owner,
                target_lifetime,
                shared_target_lifetime,
                capture_lifetime,
            )),
        }))
    }
}

pub(super) fn is_transient_copy_failure(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<MacosGpuInteropError>()
            .is_some_and(|error| matches!(error, MacosGpuInteropError::IosurfaceFenceTimeout))
    })
}

pub(super) fn validate_target_id(
    published: hypercolor_core::input::screen::ScreenNativeExecutionTargetId,
    current: hypercolor_core::input::screen::ScreenNativeExecutionTargetId,
) -> Result<()> {
    if published == current {
        return Ok(());
    }
    Err(FencedMacosScreenTarget { published, current }.into())
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
#[error("native macOS screen publication target {published:?} is fenced by {current:?}")]
struct FencedMacosScreenTarget {
    published: hypercolor_core::input::screen::ScreenNativeExecutionTargetId,
    current: hypercolor_core::input::screen::ScreenNativeExecutionTargetId,
}
