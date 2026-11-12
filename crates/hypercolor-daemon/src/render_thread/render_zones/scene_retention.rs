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

/// What an output showed in the last scene pass that rendered.
#[cfg(feature = "wgpu")]
pub(crate) enum RetainedOutputFrame<'a> {
    /// Nobody kept that pass, so what it showed is not known.
    NotKept,
    /// The output's zone rendered nothing.
    Nothing,
    Frame(&'a ProducerFrame),
}

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

    /// What the output at `output_index` of `zone_id` showed in the last
    /// scene pass that rendered, while its frames are still retained: the
    /// frame of the output's scope class, or of the zone's whole canvas for
    /// `None`.
    #[cfg(feature = "wgpu")]
    pub(crate) fn retained_output_frame(
        &self,
        zone_id: ZoneId,
        output_index: Option<usize>,
    ) -> RetainedOutputFrame<'_> {
        let Some(retained) = self.retained_frame.as_ref() else {
            return RetainedOutputFrame::NotKept;
        };
        let class = retained.class_frames.class_of(zone_id, output_index);
        let frame = if class.is_base() {
            retained
                .zone_canvases
                .iter()
                .find_map(|(id, frame)| (*id == zone_id).then_some(frame))
        } else {
            retained
                .class_frames
                .frame(super::scope_classes::ZoneClassKey::new(zone_id, class))
        };
        frame.map_or(RetainedOutputFrame::Nothing, RetainedOutputFrame::Frame)
    }

    /// Stand in for a scene pass that rendered `zone_frames`, for a test of
    /// what the frame after it reads back.
    #[cfg(all(test, feature = "wgpu"))]
    pub(crate) fn retain_zone_frames_for_test(&mut self, zone_frames: &[(ZoneId, ProducerFrame)]) {
        use crate::render_thread::frame_sampling::RetainedLedSamplingStrategy;

        self.retained_frame = Some(RetainedRenderZoneFrame {
            dependency_key: SceneDependencyKey::default(),
            scene_frame: ProducerFrame::Canvas(hypercolor_types::canvas::Canvas::new(1, 1)),
            scene_frame_zone: None,
            display_zone_frames: Vec::new(),
            active_display_zone_ids: Vec::new(),
            zone_canvases: zone_frames.to_vec(),
            zone_revisions: Vec::new(),
            class_frames: super::model::ClassFrames::default(),
            scope_class_count: 0,
            led_sampling_strategy: RetainedLedSamplingStrategy::PreSampled {
                layout: std::sync::Arc::clone(&self.combined_led_layout),
                zones: std::sync::Arc::from(Vec::new()),
            },
            logical_layer_count: 0,
        });
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
            class_frames: result.class_frames.clone(),
            scope_class_count: result.scope_class_count,
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
        class_frames: retained.class_frames.clone(),
        scope_class_count: retained.scope_class_count,
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
