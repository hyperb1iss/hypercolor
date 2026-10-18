//! Transport-sized Studio zone previews for GPU-resident zone frames
//! (Spec 86 §5.2). The CPU compositor has no GPU frames to preview.

use anyhow::Result;
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::scene::ZoneId;

use super::{SparkleFlinger, SparkleFlingerBackend};
use crate::render_thread::producer_queue::ProducerFrame;

/// What staging a zone preview did this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "wgpu"),
    allow(dead_code, reason = "only the GPU compositor stages zone previews")
)]
pub(crate) enum ZonePreviewStage {
    /// A scale and readback were submitted.
    Staged,
    /// The latched preview already shows this content at this size.
    Current,
    /// The zone's previous readback is still in flight.
    Busy,
    /// The frame is not GPU-resident or has no pixels.
    Unsupported,
}

impl SparkleFlinger {
    /// Scale a GPU zone frame to `width` x `height` on the GPU and start an
    /// asynchronous readback of just that payload.
    pub(crate) fn stage_zone_preview(
        &mut self,
        zone_id: ZoneId,
        frame: &ProducerFrame,
        width: u32,
        height: u32,
    ) -> Result<ZonePreviewStage> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => {
                let _ = (zone_id, frame, width, height);
                Ok(ZonePreviewStage::Unsupported)
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => {
                gpu.stage_zone_preview(zone_id, frame, width, height)
            }
        }
    }

    /// The newest finished preview for `zone_id`, polling its readback
    /// without blocking.
    pub(crate) fn zone_preview_surface(
        &mut self,
        zone_id: ZoneId,
    ) -> Result<Option<PublishedSurface>> {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => {
                let _ = zone_id;
                Ok(None)
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.zone_preview_surface(zone_id),
        }
    }

    /// Release the preview resources of every zone `keep` rejects.
    pub(crate) fn retain_zone_previews(&mut self, keep: impl FnMut(ZoneId) -> bool) {
        match &mut self.backend {
            SparkleFlingerBackend::Cpu(_) => {
                let _ = keep;
            }
            #[cfg(feature = "wgpu")]
            SparkleFlingerBackend::Gpu { gpu, .. } => gpu.retain_zone_previews(keep),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn zone_preview_lane_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.zone_preview_lane_count()),
        }
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn zone_preview_stage_count_for_test(&self) -> Option<usize> {
        match &self.backend {
            SparkleFlingerBackend::Cpu(_) => None,
            SparkleFlingerBackend::Gpu { gpu, .. } => Some(gpu.zone_preview_stage_count()),
        }
    }
}
