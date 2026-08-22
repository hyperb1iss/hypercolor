#[cfg(test)]
use super::super::PreviewSurfaceRequest;
use super::preview::PendingPreviewReadback;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use crate::render_thread::producer_queue::MacosScreenTextureLease;

pub(super) struct FrameInFlight {
    pub(super) generation: u64,
    encoder: EncoderStage,
    readbacks: Vec<StagedReadback>,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    native_screen_leases: Vec<MacosScreenTextureLease>,
}

pub(in crate::render_thread::sparkleflinger) struct StashedFrame {
    pub(in crate::render_thread::sparkleflinger) encoder: wgpu::CommandEncoder,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(in crate::render_thread::sparkleflinger) native_screen_leases: Vec<MacosScreenTextureLease>,
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
        #[cfg(all(target_os = "macos", feature = "screen-capture"))] native_screen_leases: Vec<
            MacosScreenTextureLease,
        >,
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases: std::mem::take(&mut self.native_screen_leases),
        })
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(super) fn take_native_screen_leases(&mut self) -> Vec<MacosScreenTextureLease> {
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases: Vec::new(),
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
