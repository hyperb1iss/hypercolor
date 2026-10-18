//! Studio zone previews for zones whose frames stay on the GPU (Spec 86 §5.2).
//!
//! CPU-backed zones hand their canvases straight to frame publication. A GPU
//! zone's frame is scaled on the GPU to the zone-preview demand and read back
//! at that transport size. Staging happens only on frames where publication
//! will admit a zone preview, so a scene nobody previews does no preview work.

use std::borrow::Cow;

use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::scene::{SceneId, ZoneId};
use tracing::debug;

use super::RenderThreadState;
use super::pipeline_runtime::PublicationCadenceState;
use super::producer_queue::ProducerFrame;
use super::sparkleflinger::SparkleFlinger;
use crate::preview_runtime::PreviewDemandSummary;

/// This frame's zone-preview demand, read once so staging and collection
/// agree with frame publication's own cadence gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ZonePreviewDemand {
    active: bool,
    due: bool,
    full_resolution: bool,
    max_width: u32,
    max_height: u32,
}

impl ZonePreviewDemand {
    pub(crate) fn for_frame(
        state: &RenderThreadState,
        cadence: &PublicationCadenceState,
        scene_id: Option<SceneId>,
        elapsed_ms: u64,
    ) -> Self {
        let receivers = state.event_bus.zone_preview_receiver_count();
        let tracked_receivers = state.preview_runtime.zone_preview_receiver_count();
        let demand = state.preview_runtime.zone_preview_demand();
        let active = scene_id.is_some() && receivers > 0;
        let due = active
            && cadence.zone_preview_due(elapsed_ms, receivers, tracked_receivers, demand.max_fps);
        Self::from_summary(active, due, receivers > tracked_receivers, demand)
    }

    /// `active` means someone watches zone previews of a live scene; `due`
    /// means publication admits one this frame. Receivers that registered no
    /// demand get full-size previews, as they do for the scene preview.
    pub(crate) fn from_summary(
        active: bool,
        due: bool,
        untracked_receivers: bool,
        demand: PreviewDemandSummary,
    ) -> Self {
        Self {
            active,
            due: active && due,
            full_resolution: untracked_receivers
                || demand.any_full_resolution
                || demand.max_width == 0
                || demand.max_height == 0,
            max_width: demand.max_width,
            max_height: demand.max_height,
        }
    }

    /// The preview extent for a `width` x `height` zone frame, sized the way
    /// the scene preview sizes its surface: full size on a full-resolution
    /// ask, otherwise the demanded size clamped per axis to the zone's.
    pub(crate) fn extent(&self, width: u32, height: u32) -> (u32, u32) {
        if self.full_resolution || width == 0 || height == 0 {
            return (width, height);
        }
        (
            self.max_width.clamp(1, width),
            self.max_height.clamp(1, height),
        )
    }
}

/// Stage a transport-sized preview of every GPU zone frame when a zone
/// preview is due, and release preview resources once nobody watches.
pub(crate) fn stage_gpu_zone_previews(
    sparkleflinger: &mut SparkleFlinger,
    demand: ZonePreviewDemand,
    zone_canvases: &[(ZoneId, ProducerFrame)],
    display_zone_ids: &[ZoneId],
) {
    if !demand.active {
        sparkleflinger.retain_zone_previews(|_| false);
        return;
    }
    sparkleflinger.retain_zone_previews(|zone_id| {
        gpu_preview_zones(zone_canvases, display_zone_ids).any(|(id, _)| *id == zone_id)
    });
    if !demand.due {
        return;
    }
    for (zone_id, frame) in gpu_preview_zones(zone_canvases, display_zone_ids) {
        let (width, height) = demand.extent(frame.width(), frame.height());
        if let Err(error) = sparkleflinger.stage_zone_preview(*zone_id, frame, width, height) {
            debug!(%error, %zone_id, "GPU zone preview staging failed; keeping the latched preview");
        }
    }
}

/// The newest finished preview of each GPU zone frame, polled without
/// blocking. Empty unless a zone preview is due this frame.
pub(crate) fn collect_gpu_zone_previews(
    sparkleflinger: &mut SparkleFlinger,
    demand: ZonePreviewDemand,
    zone_canvases: &[(ZoneId, ProducerFrame)],
    display_zone_ids: &[ZoneId],
) -> Vec<(ZoneId, PublishedSurface)> {
    if !demand.due {
        return Vec::new();
    }
    let mut previews = Vec::new();
    for (zone_id, _) in gpu_preview_zones(zone_canvases, display_zone_ids) {
        match sparkleflinger.zone_preview_surface(*zone_id) {
            Ok(Some(surface)) => previews.push((*zone_id, surface)),
            Ok(None) => {}
            Err(error) => {
                debug!(%error, %zone_id, "GPU zone preview readback failed; keeping the latched preview");
            }
        }
    }
    previews
}

/// The frames zone-preview publication reads: each GPU zone frame is
/// replaced by its transport-sized preview, everything else passes through.
pub(crate) fn zone_preview_sources<'a>(
    zone_canvases: &'a [(ZoneId, ProducerFrame)],
    gpu_previews: &[(ZoneId, PublishedSurface)],
) -> Cow<'a, [(ZoneId, ProducerFrame)]> {
    if gpu_previews.is_empty() {
        return Cow::Borrowed(zone_canvases);
    }
    Cow::Owned(
        zone_canvases
            .iter()
            .map(|(zone_id, frame)| {
                let preview = frame
                    .is_gpu_resident()
                    .then(|| gpu_previews.iter().find(|(id, _)| id == zone_id))
                    .flatten();
                match preview {
                    Some((_, surface)) => (*zone_id, ProducerFrame::Surface(surface.clone())),
                    None => (*zone_id, frame.clone()),
                }
            })
            .collect(),
    )
}

/// Scene zones whose frames live on the GPU. Display zones publish their
/// finalized faces instead.
fn gpu_preview_zones<'a>(
    zone_canvases: &'a [(ZoneId, ProducerFrame)],
    display_zone_ids: &'a [ZoneId],
) -> impl Iterator<Item = &'a (ZoneId, ProducerFrame)> {
    zone_canvases
        .iter()
        .filter(|(zone_id, frame)| frame.is_gpu_resident() && !display_zone_ids.contains(zone_id))
}

#[cfg(test)]
mod tests {
    use hypercolor_types::canvas::{Canvas, PublishedSurface};
    use hypercolor_types::scene::ZoneId;

    use super::{ZonePreviewDemand, zone_preview_sources};
    use crate::preview_runtime::PreviewDemandSummary;
    use crate::render_thread::producer_queue::ProducerFrame;

    fn sized_demand(max_width: u32, max_height: u32) -> PreviewDemandSummary {
        PreviewDemandSummary {
            subscribers: 1,
            max_fps: 15,
            max_width,
            max_height,
            ..PreviewDemandSummary::default()
        }
    }

    #[test]
    fn sized_demand_clamps_each_axis_to_the_zone() {
        let demand = ZonePreviewDemand::from_summary(true, true, false, sized_demand(320, 900));

        assert_eq!(demand.extent(640, 630), (320, 630));
        assert_eq!(demand.extent(160, 100), (160, 100));
    }

    #[test]
    fn full_resolution_and_unsized_asks_keep_the_zone_size() {
        let full = PreviewDemandSummary {
            any_full_resolution: true,
            ..sized_demand(320, 200)
        };
        for demand in [
            ZonePreviewDemand::from_summary(true, true, false, full),
            ZonePreviewDemand::from_summary(true, true, false, sized_demand(320, 0)),
            ZonePreviewDemand::from_summary(true, true, true, sized_demand(320, 200)),
        ] {
            assert_eq!(demand.extent(640, 630), (640, 630));
        }
    }

    #[test]
    fn inactive_demand_is_never_due() {
        let demand = ZonePreviewDemand::from_summary(false, true, false, sized_demand(32, 32));

        assert_eq!(
            demand,
            ZonePreviewDemand::from_summary(false, false, false, sized_demand(32, 32))
        );
    }

    #[test]
    fn preview_sources_borrow_when_no_gpu_previews_landed() {
        let zone_canvases = vec![(ZoneId::new(), ProducerFrame::Canvas(Canvas::new(2, 2)))];

        let sources = zone_preview_sources(&zone_canvases, &[]);

        assert!(matches!(sources, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn preview_sources_leave_cpu_frames_alone() {
        let zone_id = ZoneId::new();
        let canvas = Canvas::new(2, 2);
        let storage = canvas.storage_identity();
        let zone_canvases = vec![(zone_id, ProducerFrame::Canvas(canvas))];
        let previews = vec![(
            zone_id,
            PublishedSurface::from_owned_canvas(Canvas::new(1, 1), 1, 1),
        )];

        let sources = zone_preview_sources(&zone_canvases, &previews);

        let ProducerFrame::Canvas(canvas) = &sources[0].1 else {
            panic!("a CPU zone keeps publishing its own canvas");
        };
        assert_eq!(canvas.storage_identity(), storage);
    }
}
