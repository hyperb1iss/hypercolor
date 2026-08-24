use std::sync::Arc;

use hypercolor_core::input::screen::planner::{
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
};

use super::preparation::create_screen_bridge;
use super::{MacosScreenBridge, MacosScreenHost};
use crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger;
use crate::render_thread::sparkleflinger::gpu::native_screen::NativeScreenCopyOutcome;

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

pub(super) fn clear_capture_caches(bridge: &MacosScreenBridge) {
    bridge.interop.clear_capture_caches();
    bridge.cache.clear_all();
}

impl MacosScreenHost {
    pub(super) fn recover_execution(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        error: anyhow::Error,
    ) -> NativeScreenCopyOutcome {
        let failed_target_id = self.recovery.ready_target_id().or_else(|| {
            gpu.screen_target
                .as_ref()
                .map(ScreenNativeExecutionTarget::id)
        });
        let Some(failed_target_id) = failed_target_id else {
            self.recovery = MacosScreenGpuRecoveryState::Unavailable {
                failed_target_id: None,
                error: bounded_error(&error),
            };
            return NativeScreenCopyOutcome::Unavailable(error);
        };
        self.recovery = MacosScreenGpuRecoveryState::Invalidating {
            failed_target_id,
            error: bounded_error(&error),
        };
        self.clear_execution(gpu);
        self.rebuild_execution(gpu, Some(failed_target_id), error)
    }

    pub(super) fn retry_execution(&mut self, gpu: &mut GpuSparkleFlinger) {
        if !self.recovery.is_unavailable() {
            return;
        }
        let failed_target_id = self.recovery.failed_target_id();
        let prior_error = self.recovery.error();
        self.recovery = MacosScreenGpuRecoveryState::Rebuilding {
            failed_target_id,
            error: Arc::clone(&prior_error),
        };
        match self.build_execution(gpu) {
            Ok((bridge, target)) => self.commit_execution(gpu, bridge, target),
            Err(error) => {
                self.recovery = MacosScreenGpuRecoveryState::Unavailable {
                    failed_target_id,
                    error: bounded_error(&error),
                };
            }
        }
    }

    pub(super) const fn ready_target_id(&self) -> Option<ScreenNativeExecutionTargetId> {
        self.recovery.ready_target_id()
    }

    fn rebuild_execution(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        failed_target_id: Option<ScreenNativeExecutionTargetId>,
        structural_error: anyhow::Error,
    ) -> NativeScreenCopyOutcome {
        self.recovery = MacosScreenGpuRecoveryState::Rebuilding {
            failed_target_id,
            error: bounded_error(&structural_error),
        };
        match self.build_execution(gpu) {
            Ok((bridge, target)) => {
                self.commit_execution(gpu, bridge, target);
                NativeScreenCopyOutcome::Invalidated(structural_error)
            }
            Err(rebuild_error) => {
                let error = structural_error.context(format!(
                    "native screen reconstruction failed: {rebuild_error}"
                ));
                self.recovery = MacosScreenGpuRecoveryState::Unavailable {
                    failed_target_id,
                    error: bounded_error(&error),
                };
                NativeScreenCopyOutcome::Unavailable(error)
            }
        }
    }

    fn build_execution(
        &mut self,
        gpu: &GpuSparkleFlinger,
    ) -> anyhow::Result<(Arc<MacosScreenBridge>, ScreenNativeExecutionTarget)> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_rebuild) {
            anyhow::bail!("injected macOS screen reconstruction failure");
        }
        create_screen_bridge(&gpu.device, gpu.probe.max_texture_dimension_2d)
    }

    fn commit_execution(
        &mut self,
        gpu: &mut GpuSparkleFlinger,
        bridge: Arc<MacosScreenBridge>,
        target: ScreenNativeExecutionTarget,
    ) {
        let target_id = target.id();
        debug_assert_eq!(bridge.target_id(), target_id);
        self.bridge = Some(bridge);
        gpu.screen_target = Some(target);
        self.recovery = MacosScreenGpuRecoveryState::ready(target_id);
    }

    fn clear_execution(&mut self, gpu: &mut GpuSparkleFlinger) {
        drop(gpu.supersede_frame_in_flight("macOS screen execution invalidated"));
        gpu.discard_pending_uploads();
        gpu.discard_pending_preview_map();
        gpu.clear_sampling_readback_latch();
        gpu.current_output = None;
        gpu.cached_composition_key = None;
        gpu.cached_readback_surface = None;
        gpu.cached_preview_surfaces.clear();
        gpu.ready_preview_surface = None;
        gpu.cached_sample_result = None;
        gpu.output_generation = gpu.output_generation.saturating_add(1);
        gpu.release_native_screen_bind_groups();
        if let Some(bridge) = &self.bridge {
            bridge.clear_capture_caches();
        }
        gpu.release_completed_native_screen_leases();
        gpu.screen_target = None;
        self.bridge = None;
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
