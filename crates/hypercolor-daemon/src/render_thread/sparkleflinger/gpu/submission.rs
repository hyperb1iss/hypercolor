#[cfg(test)]
use super::super::PreviewSurfaceRequest;
use super::GpuSparkleFlinger;
use super::preview::PendingPreviewReadback;
use crate::render_thread::producer_queue::NativeScreenTextureLease;
use anyhow::Result;
use std::time::Duration;

pub(super) struct FrameInFlight {
    pub(super) generation: u64,
    encoder: EncoderStage,
    readbacks: Vec<StagedReadback>,
    native_screen_leases: Vec<NativeScreenTextureLease>,
}

pub(in crate::render_thread::sparkleflinger) struct StashedFrame {
    pub(in crate::render_thread::sparkleflinger) encoder: wgpu::CommandEncoder,
    pub(in crate::render_thread::sparkleflinger) native_screen_leases:
        Vec<NativeScreenTextureLease>,
}

enum EncoderStage {
    Building(Option<wgpu::CommandEncoder>),
    Submitted(wgpu::SubmissionIndex),
    Superseded,
}

enum StagedReadback {
    Preview {
        readback: PendingPreviewReadback,
        stage: ReadbackStage,
    },
}

enum ReadbackStage {
    Encoded,
    Submitted(wgpu::SubmissionIndex),
}

impl FrameInFlight {
    pub(super) fn building(
        generation: u64,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
        native_screen_leases: Vec<NativeScreenTextureLease>,
    ) -> Self {
        let readbacks = preview_readback.map_or_else(Vec::new, |readback| {
            vec![StagedReadback::Preview {
                readback,
                stage: ReadbackStage::Encoded,
            }]
        });
        Self {
            generation,
            encoder: EncoderStage::Building(Some(encoder)),
            readbacks,
            native_screen_leases,
        }
    }

    pub(super) fn submitted(
        generation: u64,
        submission_index: wgpu::SubmissionIndex,
        preview_readback: PendingPreviewReadback,
    ) -> Self {
        Self {
            generation,
            encoder: EncoderStage::Submitted(submission_index.clone()),
            readbacks: vec![StagedReadback::Preview {
                readback: preview_readback,
                stage: ReadbackStage::Submitted(submission_index),
            }],
            native_screen_leases: Vec::new(),
        }
    }

    pub(super) fn preview_readback(&self) -> Option<&PendingPreviewReadback> {
        self.readbacks.first().map(|readback| match readback {
            StagedReadback::Preview { readback, .. } => readback,
        })
    }

    pub(super) fn preview_submission_index(&self) -> Option<wgpu::SubmissionIndex> {
        self.readbacks.iter().find_map(|readback| match readback {
            StagedReadback::Preview {
                stage: ReadbackStage::Submitted(submission_index),
                ..
            } => Some(submission_index.clone()),
            StagedReadback::Preview {
                stage: ReadbackStage::Encoded,
                ..
            } => None,
        })
    }

    pub(super) fn take_preview_readback(&mut self) -> Option<PendingPreviewReadback> {
        let index = self
            .readbacks
            .iter()
            .position(|readback| matches!(readback, StagedReadback::Preview { .. }))?;
        match self.readbacks.remove(index) {
            StagedReadback::Preview { readback, .. } => Some(readback),
        }
    }

    pub(super) fn submission_index(&self) -> Option<wgpu::SubmissionIndex> {
        match &self.encoder {
            EncoderStage::Submitted(submission_index) => Some(submission_index.clone()),
            EncoderStage::Building(_) | EncoderStage::Superseded => None,
        }
    }

    pub(super) fn is_building(&self) -> bool {
        matches!(self.encoder, EncoderStage::Building(_))
    }

    pub(super) fn take_encoder_for_chaining(&mut self) -> Option<wgpu::CommandEncoder> {
        match &mut self.encoder {
            EncoderStage::Building(encoder) => encoder.take(),
            EncoderStage::Submitted(_) | EncoderStage::Superseded => None,
        }
    }

    fn mark_submitted(&mut self, submission_index: wgpu::SubmissionIndex) {
        debug_assert!(
            matches!(self.encoder, EncoderStage::Building(None)),
            "only a consumed building encoder can advance to submitted"
        );
        self.encoder = EncoderStage::Submitted(submission_index.clone());
        for readback in &mut self.readbacks {
            match readback {
                StagedReadback::Preview { stage, .. } => {
                    *stage = ReadbackStage::Submitted(submission_index.clone());
                }
            }
        }
    }

    pub(super) fn submit(&mut self, queue: &wgpu::Queue) -> Option<wgpu::SubmissionIndex> {
        if let Some(submission_index) = self.submission_index() {
            return Some(submission_index);
        }
        let encoder = self.take_encoder_for_chaining()?;
        let submission_index = queue.submit(Some(encoder.finish()));
        self.mark_submitted(submission_index.clone());
        Some(submission_index)
    }

    pub(super) fn supersede(mut self, reason: &'static str) -> Option<StashedFrame> {
        let encoder = self.take_encoder_for_chaining();
        self.encoder = EncoderStage::Superseded;
        self.readbacks.clear();
        tracing::trace!(
            generation = self.generation,
            reason,
            "superseding deferred GPU frame"
        );
        encoder.map(|encoder| StashedFrame {
            encoder,
            native_screen_leases: std::mem::take(&mut self.native_screen_leases),
        })
    }

    pub(super) fn take_native_screen_leases(&mut self) -> Vec<NativeScreenTextureLease> {
        std::mem::take(&mut self.native_screen_leases)
    }

    #[cfg(test)]
    pub(super) fn encoded_preview_for_test() -> Self {
        Self {
            generation: 7,
            encoder: EncoderStage::Building(None),
            readbacks: vec![StagedReadback::Preview {
                readback: PendingPreviewReadback::PreviewBuffer {
                    request: PreviewSurfaceRequest {
                        width: 2,
                        height: 2,
                    },
                    readback_key: None,
                    cache_as_full_size: false,
                    slot: 0,
                },
                stage: ReadbackStage::Encoded,
            }],
            native_screen_leases: Vec::new(),
        }
    }
}

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn flush_pending_output_submission(
        &mut self,
    ) -> Result<()> {
        if self.pending_preview_readback().is_some() {
            return self.submit_pending_preview_work();
        }
        if let Some(mut frame) = self.frame_in_flight.take() {
            debug_assert_eq!(frame.generation, self.output_generation);
            let submission_index = frame.submit(&self.queue);
            debug_assert!(submission_index.is_some());
            if let Some(submission_index) = submission_index {
                self.finish_pending_uploads(submission_index.clone());
                self.retire_native_screen_leases(
                    submission_index,
                    frame.take_native_screen_leases(),
                );
            }
            self.release_retired_uniform_slots();
        }
        Ok(())
    }

    pub(in crate::render_thread::sparkleflinger) fn supersede_frame_in_flight(
        &mut self,
        reason: &'static str,
    ) -> Option<StashedFrame> {
        let frame = self.frame_in_flight.take()?;
        let encoder = frame.supersede(reason);
        #[cfg(test)]
        {
            self.superseded_frame_count = self.superseded_frame_count.saturating_add(1);
        }
        encoder
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn stage_frame_in_flight(
        &mut self,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
    ) {
        self.stage_frame_in_flight_with_native_screen_leases(encoder, preview_readback, Vec::new());
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn pending_preview_readback(
        &self,
    ) -> Option<&PendingPreviewReadback> {
        self.frame_in_flight
            .as_ref()
            .and_then(FrameInFlight::preview_readback)
    }

    pub(in crate::render_thread::sparkleflinger) fn pending_preview_submission(
        &self,
    ) -> Option<wgpu::SubmissionIndex> {
        self.frame_in_flight
            .as_ref()
            .and_then(FrameInFlight::preview_submission_index)
    }

    pub(in crate::render_thread::sparkleflinger) fn has_pending_output_submission(&self) -> bool {
        self.frame_in_flight
            .as_ref()
            .is_some_and(FrameInFlight::is_building)
    }

    pub(in crate::render_thread::sparkleflinger) fn finish_pending_uploads(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
    ) {
        if let Some(surfaces) = self.surfaces.as_mut() {
            surfaces.finish_pending_uploads(submission_index);
        }
    }

    pub(in crate::render_thread::sparkleflinger) fn discard_pending_uploads(&mut self) {
        if let Some(surfaces) = self.surfaces.as_mut() {
            surfaces.discard_pending_uploads();
        }
    }

    /// Advances the uniform ring watermarks so retired slots can be reused.
    ///
    /// Invariant: a ring slot must never be rewritten while a not-yet-
    /// submitted encoder references it. Call sites guarantee no local encoder
    /// is being built; the guard covers the stashed compositor encoder.
    pub(in crate::render_thread::sparkleflinger) fn release_retired_uniform_slots(&mut self) {
        if !self.has_pending_output_submission() {
            self.pipeline.release_retired_uniform_slots();
        }
    }
}

impl Drop for FrameInFlight {
    fn drop(&mut self) {
        if cfg!(debug_assertions) && !std::thread::panicking() {
            debug_assert!(
                !self.is_building() || self.readbacks.is_empty(),
                "generation {} dropped with encoded GPU readbacks before submit or supersede",
                self.generation
            );
        }
    }
}

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn stage_frame_in_flight_with_native_screen_leases(
        &mut self,
        encoder: wgpu::CommandEncoder,
        preview_readback: Option<PendingPreviewReadback>,
        native_screen_leases: Vec<NativeScreenTextureLease>,
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
        leases: Vec<NativeScreenTextureLease>,
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
