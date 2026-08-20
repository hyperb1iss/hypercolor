use std::time::Duration;

use super::MacosScreenBridge;
use crate::render_thread::producer_queue::MacosScreenTextureLease;
use crate::render_thread::sparkleflinger::gpu::{
    FrameInFlight, GpuSparkleFlinger, PendingPreviewReadback,
};

pub(super) fn clear_capture_caches(bridge: &MacosScreenBridge) {
    bridge.interop.clear_capture_caches();
    bridge.cache.clear_imports();
}

pub(crate) const fn native_screen_copy_error_invalidates_frame(_error: &anyhow::Error) -> bool {
    true
}

pub(crate) const fn is_retryable_native_screen_copy_error(_error: &anyhow::Error) -> bool {
    false
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
