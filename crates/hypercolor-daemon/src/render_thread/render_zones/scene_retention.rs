use hypercolor_types::event::ZoneColors;
#[cfg(feature = "wgpu")]
use hypercolor_types::scene::ZoneId;

use super::ZoneRuntime;
use super::model::{RetainedRenderZoneFrame, ZoneResult};
use crate::performance::FullFrameCopyMetrics;
use crate::render_thread::frame_sampling::LedSamplingStrategy;
#[cfg(feature = "wgpu")]
use crate::render_thread::producer_queue::ProducerFrame;
use crate::render_thread::scene_dependency::SceneDependencyKey;

impl ZoneRuntime {
    pub(crate) fn release_retained_scene_frame(&mut self) {
        self.retained_frame = None;
    }

    pub(crate) fn reuse_scene(&self, dependency_key: SceneDependencyKey) -> Option<ZoneResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.dependency_key != dependency_key {
            return None;
        }

        Some(zone_result_from_retained(retained))
    }

    /// The zone frames of the last scene pass that rendered, while they are
    /// still retained: what each zone showed in the frame before this one.
    #[cfg(feature = "wgpu")]
    pub(crate) fn retained_zone_frames(&self) -> Option<&[(ZoneId, ProducerFrame)]> {
        self.retained_frame
            .as_ref()
            .map(|retained| retained.zone_canvases.as_slice())
    }

    pub(super) fn reuse_last_good_scene(&self) -> Option<ZoneResult> {
        self.retained_frame.as_ref().map(zone_result_from_retained)
    }

    pub(super) fn retain_frame(
        &mut self,
        dependency_key: SceneDependencyKey,
        result: &ZoneResult,
        zones: &[ZoneColors],
    ) {
        let recycled = self
            .retained_frame
            .take()
            .map(|frame| frame.led_sampling_strategy);
        self.retained_frame = Some(RetainedRenderZoneFrame {
            dependency_key,
            scene_frame: result.scene_frame.clone(),
            scene_frame_zone: result.scene_frame_zone,
            display_zone_frames: result.display_zone_frames.clone(),
            active_display_zone_ids: result.active_display_zone_ids.clone(),
            zone_canvases: result.zone_canvases.clone(),
            zone_revisions: result.zone_revisions.clone(),
            led_sampling_strategy: result.led_sampling_strategy.retain(zones, recycled),
            logical_layer_count: result.logical_layer_count,
        });
    }
}

fn zone_result_from_retained(retained: &RetainedRenderZoneFrame) -> ZoneResult {
    ZoneResult {
        scene_frame: retained.scene_frame.clone(),
        scene_frame_zone: retained.scene_frame_zone,
        display_zone_frames: retained.display_zone_frames.clone(),
        zone_canvases: retained.zone_canvases.clone(),
        zone_revisions: retained.zone_revisions.clone(),
        active_display_zone_ids: retained.active_display_zone_ids.clone(),
        led_sampling_strategy: LedSamplingStrategy::from_retained(&retained.led_sampling_strategy),
        producer_full_frame_copy: FullFrameCopyMetrics::default(),
        zone_composition_skips: 0,
        render_us: 0,
        sample_us: 0,
        scene_compose_us: 0,
        logical_layer_count: retained.logical_layer_count,
    }
}
