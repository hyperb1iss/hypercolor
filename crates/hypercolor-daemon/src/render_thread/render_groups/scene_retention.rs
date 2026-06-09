use hypercolor_types::event::ZoneColors;

use super::ZoneRuntime;
use super::model::{RetainedRenderGroupFrame, ZoneResult};
use crate::performance::FullFrameCopyMetrics;
use crate::render_thread::frame_sampling::LedSamplingStrategy;
use crate::render_thread::scene_dependency::SceneDependencyKey;

impl ZoneRuntime {
    pub(crate) fn reuse_scene(&self, dependency_key: SceneDependencyKey) -> Option<ZoneResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.dependency_key != dependency_key {
            return None;
        }

        Some(ZoneResult {
            scene_frame: retained.scene_frame.clone(),
            group_canvases: retained.group_canvases.clone(),
            zone_canvases: retained.zone_canvases.clone(),
            active_group_canvas_ids: retained.active_group_canvas_ids.clone(),
            led_sampling_strategy: LedSamplingStrategy::from_retained(
                &retained.led_sampling_strategy,
            ),
            producer_full_frame_copy: FullFrameCopyMetrics::default(),
            render_us: 0,
            sample_us: 0,
            scene_compose_us: 0,
            logical_layer_count: retained.logical_layer_count,
        })
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
        self.retained_frame = Some(RetainedRenderGroupFrame {
            dependency_key,
            scene_frame: result.scene_frame.clone(),
            group_canvases: result.group_canvases.clone(),
            active_group_canvas_ids: result.active_group_canvas_ids.clone(),
            zone_canvases: result.zone_canvases.clone(),
            led_sampling_strategy: result.led_sampling_strategy.retain(zones, recycled),
            logical_layer_count: result.logical_layer_count,
        });
    }
}
