use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use hypercolor_core::input::screen::{ScreenBranchPayload, ScreenBranchPublication};
use hypercolor_macos_capture::MacosCaptureFrame;
use hypercolor_macos_gpu_interop::{ImportedMacosScreenFrame, MacosGpuInteropError};

use super::MacosScreenBridge;
use super::model::PreparedMacosScreenTarget;
use super::recovery::MacosScreenCopyOutcome;
use super::reduction;
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameOrigin, MacosScreenTextureLease,
};
use crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger;

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

impl GpuSparkleFlinger {
    pub(crate) fn copy_screen_publication(
        &mut self,
        publication: &Arc<ScreenBranchPublication>,
    ) -> MacosScreenCopyOutcome {
        self.retry_macos_screen_execution();
        let result = self.try_copy_screen_publication(publication);
        self.finish_macos_screen_copy(result)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn finish_macos_screen_copy(
        &mut self,
        result: Result<Option<GpuTextureFrame>>,
    ) -> MacosScreenCopyOutcome {
        match result {
            Ok(Some(frame)) => MacosScreenCopyOutcome::Copied(frame),
            Ok(None) => MacosScreenCopyOutcome::Ignored,
            Err(error) if is_transient_copy_failure(&error) => {
                MacosScreenCopyOutcome::Deferred(error)
            }
            Err(error) if error.downcast_ref::<FencedMacosScreenTarget>().is_some() => {
                MacosScreenCopyOutcome::Invalidated(error)
            }
            Err(error) => self.recover_macos_screen_execution(error),
        }
    }

    fn try_copy_screen_publication(
        &mut self,
        publication: &Arc<ScreenBranchPublication>,
    ) -> Result<Option<GpuTextureFrame>> {
        let Some(bridge) = self.screen_bridge.clone() else {
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
            .macos_screen_recovery
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
        if std::mem::take(&mut self.fail_next_macos_screen_import) {
            anyhow::bail!("injected native macOS screen importer failure");
        }
        let import_started = Instant::now();
        let imported = bridge.import_frame(&self.device, target_owner.resource_generation, capture);
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
        let submission_lease = MacosScreenTextureLease::new(
            imported.clone(),
            capture_owner.clone(),
            target_owner.clone(),
            target_lifetime.clone(),
            shared_target_lifetime.clone(),
            capture_lifetime.clone(),
        );
        let resolved = if requires_work {
            self.flush_pending_output_submission()?;
            let reduction_started = Instant::now();
            let reduced = reduction::reduce_imported_frame(
                self,
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
            macos_screen_lease: Some(MacosScreenTextureLease::new(
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
