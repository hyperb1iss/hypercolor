use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::input::screen::{ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId};

use super::MacosScreenBridge;
use super::preparation::create_screen_bridge;
use crate::render_thread::producer_queue::MacosScreenTextureLease;
use crate::render_thread::sparkleflinger::gpu::{
    FrameInFlight, GpuSparkleFlinger, GpuTextureFrame, PendingPreviewReadback,
};

const MAX_RECOVERY_ERROR_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MacosScreenGpuRecoveryState {
    Ready {
        target_id: ScreenNativeExecutionTargetId,
    },
    Invalidating {
        failed_target_id: ScreenNativeExecutionTargetId,
        error: Arc<str>,
    },
    Rebuilding {
        failed_target_id: Option<ScreenNativeExecutionTargetId>,
        error: Arc<str>,
    },
    Unavailable {
        failed_target_id: Option<ScreenNativeExecutionTargetId>,
        error: Arc<str>,
    },
}

impl MacosScreenGpuRecoveryState {
    pub(in crate::render_thread::sparkleflinger::gpu) fn ready(
        target_id: ScreenNativeExecutionTargetId,
    ) -> Self {
        Self::Ready { target_id }
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn unavailable(
        error: &anyhow::Error,
    ) -> Self {
        Self::Unavailable {
            failed_target_id: None,
            error: bounded_error(error),
        }
    }

    pub(super) const fn ready_target_id(&self) -> Option<ScreenNativeExecutionTargetId> {
        match self {
            Self::Ready { target_id } => Some(*target_id),
            Self::Invalidating { .. } | Self::Rebuilding { .. } | Self::Unavailable { .. } => None,
        }
    }

    pub(super) const fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    fn failed_target_id(&self) -> Option<ScreenNativeExecutionTargetId> {
        match self {
            Self::Ready { target_id }
            | Self::Invalidating {
                failed_target_id: target_id,
                ..
            } => Some(*target_id),
            Self::Rebuilding {
                failed_target_id, ..
            }
            | Self::Unavailable {
                failed_target_id, ..
            } => *failed_target_id,
        }
    }

    fn error(&self) -> Arc<str> {
        match self {
            Self::Ready { .. } => Arc::from("native screen execution is ready"),
            Self::Invalidating { error, .. }
            | Self::Rebuilding { error, .. }
            | Self::Unavailable { error, .. } => Arc::clone(error),
        }
    }
}

#[derive(Debug)]
pub(crate) enum MacosScreenCopyOutcome {
    Copied(GpuTextureFrame),
    Ignored,
    Deferred(anyhow::Error),
    Invalidated(anyhow::Error),
    Unavailable(anyhow::Error),
}

impl MacosScreenCopyOutcome {
    pub(crate) fn into_result(self) -> anyhow::Result<Option<GpuTextureFrame>> {
        match self {
            Self::Copied(frame) => Ok(Some(frame)),
            Self::Ignored => Ok(None),
            Self::Deferred(error) | Self::Invalidated(error) | Self::Unavailable(error) => {
                Err(error)
            }
        }
    }
}

pub(super) fn clear_capture_caches(bridge: &MacosScreenBridge) {
    bridge.interop.clear_capture_caches();
    bridge.cache.clear_all();
}

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn recover_macos_screen_execution(
        &mut self,
        error: anyhow::Error,
    ) -> MacosScreenCopyOutcome {
        let failed_target_id = self.macos_screen_recovery.ready_target_id().or_else(|| {
            self.screen_target
                .as_ref()
                .map(ScreenNativeExecutionTarget::id)
        });
        let Some(failed_target_id) = failed_target_id else {
            self.macos_screen_recovery = MacosScreenGpuRecoveryState::Unavailable {
                failed_target_id: None,
                error: bounded_error(&error),
            };
            return MacosScreenCopyOutcome::Unavailable(error);
        };
        self.macos_screen_recovery = MacosScreenGpuRecoveryState::Invalidating {
            failed_target_id,
            error: bounded_error(&error),
        };
        self.clear_macos_screen_execution();
        self.rebuild_macos_screen_execution(Some(failed_target_id), error)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn retry_macos_screen_execution(&mut self) {
        if !self.macos_screen_recovery.is_unavailable() {
            return;
        }
        let failed_target_id = self.macos_screen_recovery.failed_target_id();
        let prior_error = self.macos_screen_recovery.error();
        self.macos_screen_recovery = MacosScreenGpuRecoveryState::Rebuilding {
            failed_target_id,
            error: Arc::clone(&prior_error),
        };
        match self.build_macos_screen_execution() {
            Ok((bridge, target)) => self.commit_macos_screen_execution(bridge, target),
            Err(error) => {
                self.macos_screen_recovery = MacosScreenGpuRecoveryState::Unavailable {
                    failed_target_id,
                    error: bounded_error(&error),
                };
            }
        }
    }

    fn rebuild_macos_screen_execution(
        &mut self,
        failed_target_id: Option<ScreenNativeExecutionTargetId>,
        structural_error: anyhow::Error,
    ) -> MacosScreenCopyOutcome {
        self.macos_screen_recovery = MacosScreenGpuRecoveryState::Rebuilding {
            failed_target_id,
            error: bounded_error(&structural_error),
        };
        match self.build_macos_screen_execution() {
            Ok((bridge, target)) => {
                self.commit_macos_screen_execution(bridge, target);
                MacosScreenCopyOutcome::Invalidated(structural_error)
            }
            Err(rebuild_error) => {
                let error = structural_error.context(format!(
                    "native screen reconstruction failed: {rebuild_error}"
                ));
                self.macos_screen_recovery = MacosScreenGpuRecoveryState::Unavailable {
                    failed_target_id,
                    error: bounded_error(&error),
                };
                MacosScreenCopyOutcome::Unavailable(error)
            }
        }
    }

    fn build_macos_screen_execution(
        &mut self,
    ) -> anyhow::Result<(Arc<MacosScreenBridge>, ScreenNativeExecutionTarget)> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_macos_screen_rebuild) {
            anyhow::bail!("injected macOS screen reconstruction failure");
        }
        create_screen_bridge(&self.device, self.probe.max_texture_dimension_2d)
    }

    fn commit_macos_screen_execution(
        &mut self,
        bridge: Arc<MacosScreenBridge>,
        target: ScreenNativeExecutionTarget,
    ) {
        let target_id = target.id();
        debug_assert_eq!(bridge.target_id(), target_id);
        self.screen_bridge = Some(bridge);
        self.screen_target = Some(target);
        self.macos_screen_recovery = MacosScreenGpuRecoveryState::ready(target_id);
    }

    fn clear_macos_screen_execution(&mut self) {
        drop(self.supersede_frame_in_flight("macOS screen execution invalidated"));
        self.discard_pending_uploads();
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surfaces.clear();
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.output_generation = self.output_generation.saturating_add(1);
        self.release_native_screen_caches();
        self.screen_target = None;
        self.screen_bridge = None;
    }
}

fn bounded_error(error: &anyhow::Error) -> Arc<str> {
    let mut message = error.to_string();
    if message.len() > MAX_RECOVERY_ERROR_BYTES {
        let mut boundary = MAX_RECOVERY_ERROR_BYTES;
        while !message.is_char_boundary(boundary) {
            boundary -= 1;
        }
        message.truncate(boundary);
    }
    Arc::from(message)
}

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn stage_frame_in_flight_with_native_screen_leases(
        &mut self,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
        native_screen_leases: Vec<MacosScreenTextureLease>,
    ) {
        debug_assert!(
            self.frame_in_flight.is_none(),
            "deferred GPU frame must be submitted or superseded before replacement"
        );
        self.frame_in_flight = Some(FrameInFlight::building(
            self.output_generation,
            encoder,
            preview_readback,
            native_screen_leases,
        ));
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn retire_native_screen_leases(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
        leases: Vec<MacosScreenTextureLease>,
    ) {
        self.native_screen_lease_retirements
            .retire(submission_index, leases);
        self.release_completed_native_screen_leases();
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn release_completed_native_screen_leases(
        &mut self,
    ) {
        let device = &self.device;
        self.native_screen_lease_retirements
            .release_completed(|submission_index| {
                match device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission_index.clone()),
                    timeout: Some(Duration::ZERO),
                }) {
                    Ok(_) => true,
                    Err(wgpu::PollError::Timeout) => false,
                    Err(error) => {
                        tracing::debug!(%error, "GPU native screen lease retirement poll failed");
                        false
                    }
                }
            });
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn wait_for_native_screen_lease_retirements(
        &mut self,
    ) {
        while let Some(submission_index) = self
            .native_screen_lease_retirements
            .front_submission()
            .cloned()
        {
            match self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(submission_index),
                timeout: None,
            }) {
                Ok(_) => self.native_screen_lease_retirements.release_front(),
                Err(error) => {
                    tracing::debug!(%error, "GPU stopped before native screen lease retirement");
                    self.native_screen_lease_retirements.release_front();
                }
            }
        }
    }
}
