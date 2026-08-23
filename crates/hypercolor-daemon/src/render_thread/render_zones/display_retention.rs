use std::collections::HashMap;

use hypercolor_core::bus::DisplayZoneOutputRoute;
use hypercolor_types::scene::{DisplayFaceTarget, Zone, ZoneId};

use super::ZoneRuntime;
use super::model::{
    DisplayZoneCanvasFrame, PendingDisplayZoneFrame, RetainedDirectZoneFrame,
    RetainedMaterializedZoneFrame,
};
use super::zone_state::zone_publishes_direct_canvas;
use crate::render_thread::scene_dependency::SceneDependencyKey;

impl ZoneRuntime {
    pub(super) fn reuse_retained_direct_zone_frame(
        &self,
        zone: &Zone,
        elapsed_ms: u64,
        display_zone_target_fps: &HashMap<ZoneId, u32>,
        dependency_key: SceneDependencyKey,
    ) -> Option<PendingDisplayZoneFrame> {
        if !zone_publishes_direct_canvas(zone) || !zone.layout.zones.is_empty() {
            return None;
        }

        let target_fps = *display_zone_target_fps.get(&zone.id)?;
        let retained = self.retained_direct_zone_frames.get(&zone.id)?;
        if retained.frame.empty_direct_shell {
            return None;
        }
        if retained.dependency_key != dependency_key {
            return None;
        }
        let frame_interval_ms = display_frame_interval_ms(target_fps);
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    pub(super) fn retain_direct_zone_frame(
        &mut self,
        zone_id: ZoneId,
        elapsed_ms: u64,
        dependency_key: SceneDependencyKey,
        frame: &PendingDisplayZoneFrame,
    ) {
        self.retained_direct_zone_frames.insert(
            zone_id,
            RetainedDirectZoneFrame {
                frame: frame.clone(),
                rendered_at_ms: elapsed_ms,
                dependency_key,
            },
        );
    }

    pub(super) fn reuse_latest_direct_zone_frame(
        &self,
        zone: &Zone,
    ) -> Option<PendingDisplayZoneFrame> {
        if !zone_publishes_direct_canvas(zone) {
            return None;
        }
        let retained = self.retained_direct_zone_frames.get(&zone.id)?;
        if retained.frame.empty_direct_shell {
            return None;
        }
        let display_target = zone.display_target.as_ref()?;
        if retained.frame.display_target != *display_target
            || retained.frame.frame.width() != zone.layout.canvas_width
            || retained.frame.frame.height() != zone.layout.canvas_height
        {
            return None;
        }

        Some(retained.frame.clone())
    }

    pub(crate) fn reuse_retained_materialized_zone_frame(
        &self,
        zone_id: ZoneId,
        elapsed_ms: u64,
        target_fps: Option<u32>,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        empty_direct_shell: bool,
    ) -> Option<DisplayZoneCanvasFrame> {
        let target_fps = target_fps?;
        if display_route.device_id != display_target.device_id {
            return None;
        }

        let retained = self.retained_materialized_zone_frames.get(&zone_id)?;
        if retained.dependency_key != dependency_key
            || retained.display_target != *display_target
            || retained.display_route != *display_route
            || retained.empty_direct_shell != empty_direct_shell
        {
            return None;
        }

        let frame_interval_ms = display_frame_interval_ms(target_fps);
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    pub(crate) fn reuse_latest_materialized_zone_frame(
        &self,
        zone_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        empty_direct_shell: bool,
    ) -> Option<DisplayZoneCanvasFrame> {
        if display_route.device_id != display_target.device_id {
            return None;
        }

        let retained = self.retained_materialized_zone_frames.get(&zone_id)?;
        if retained.display_target != *display_target
            || retained.display_route != *display_route
            || retained.empty_direct_shell != empty_direct_shell
        {
            return None;
        }

        Some(retained.frame.clone())
    }

    pub(crate) fn retain_materialized_zone_frame(
        &mut self,
        zone_id: ZoneId,
        elapsed_ms: u64,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        empty_direct_shell: bool,
        frame: &DisplayZoneCanvasFrame,
    ) {
        if display_route.device_id != display_target.device_id || !frame.display_target.finalized {
            return;
        }

        self.retained_materialized_zone_frames.insert(
            zone_id,
            RetainedMaterializedZoneFrame {
                frame: frame.clone(),
                rendered_at_ms: elapsed_ms,
                dependency_key,
                display_target: display_target.clone(),
                display_route: display_route.clone(),
                empty_direct_shell,
            },
        );
    }
}

fn display_frame_interval_ms(target_fps: u32) -> u64 {
    (1000 / u64::from(target_fps.max(1))).max(1)
}
