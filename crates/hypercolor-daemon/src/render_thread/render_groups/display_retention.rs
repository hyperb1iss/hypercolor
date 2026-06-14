use std::collections::HashMap;

use hypercolor_core::bus::DisplayGroupOutputRoute;
use hypercolor_types::scene::{DisplayFaceTarget, Zone, ZoneId};

use super::ZoneRuntime;
use super::group_state::{group_publishes_direct_canvas, group_publishes_empty_direct_canvas};
use super::model::{
    GroupCanvasFrame, PendingGroupCanvasFrame, RetainedDirectGroupFrame,
    RetainedMaterializedGroupFrame,
};
use crate::render_thread::scene_dependency::SceneDependencyKey;

impl ZoneRuntime {
    pub(super) fn reuse_retained_direct_group_frame(
        &self,
        group: &Zone,
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<ZoneId, u32>,
        dependency_key: SceneDependencyKey,
    ) -> Option<PendingGroupCanvasFrame> {
        if !group_publishes_direct_canvas(group) || !group.layout.zones.is_empty() {
            return None;
        }

        let target_fps = *display_group_target_fps.get(&group.id)?;
        let retained = self.retained_direct_group_frames.get(&group.id)?;
        if retained.frame.empty_direct_shell != group_publishes_empty_direct_canvas(group) {
            return None;
        }
        if retained.dependency_key != dependency_key {
            return None;
        }
        let frame_interval_ms = display_frame_interval_ms(target_fps);
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    pub(super) fn retain_direct_group_frame(
        &mut self,
        group_id: ZoneId,
        elapsed_ms: u32,
        dependency_key: SceneDependencyKey,
        frame: &PendingGroupCanvasFrame,
    ) {
        self.retained_direct_group_frames.insert(
            group_id,
            RetainedDirectGroupFrame {
                frame: frame.clone(),
                rendered_at_ms: elapsed_ms,
                dependency_key,
            },
        );
    }

    pub(super) fn reuse_latest_direct_group_frame(
        &self,
        group: &Zone,
    ) -> Option<PendingGroupCanvasFrame> {
        if !group_publishes_direct_canvas(group) {
            return None;
        }
        let retained = self.retained_direct_group_frames.get(&group.id)?;
        if retained.frame.empty_direct_shell != group_publishes_empty_direct_canvas(group) {
            return None;
        }
        let display_target = group.display_target.as_ref()?;
        if retained.frame.display_target != *display_target
            || retained.frame.frame.width() != group.layout.canvas_width
            || retained.frame.frame.height() != group.layout.canvas_height
        {
            return None;
        }

        Some(retained.frame.clone())
    }

    pub(crate) fn reuse_retained_materialized_group_frame(
        &self,
        group_id: ZoneId,
        elapsed_ms: u32,
        target_fps: Option<u32>,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        empty_direct_shell: bool,
    ) -> Option<GroupCanvasFrame> {
        let target_fps = target_fps?;
        if display_route.device_id != display_target.device_id {
            return None;
        }

        let retained = self.retained_materialized_group_frames.get(&group_id)?;
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

    pub(crate) fn reuse_latest_materialized_group_frame(
        &self,
        group_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        empty_direct_shell: bool,
    ) -> Option<GroupCanvasFrame> {
        if display_route.device_id != display_target.device_id {
            return None;
        }

        let retained = self.retained_materialized_group_frames.get(&group_id)?;
        if retained.display_target != *display_target
            || retained.display_route != *display_route
            || retained.empty_direct_shell != empty_direct_shell
        {
            return None;
        }

        Some(retained.frame.clone())
    }

    pub(crate) fn retain_materialized_group_frame(
        &mut self,
        group_id: ZoneId,
        elapsed_ms: u32,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        empty_direct_shell: bool,
        frame: &GroupCanvasFrame,
    ) {
        if display_route.device_id != display_target.device_id || !frame.display_target.finalized {
            return;
        }

        self.retained_materialized_group_frames.insert(
            group_id,
            RetainedMaterializedGroupFrame {
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

fn display_frame_interval_ms(target_fps: u32) -> u32 {
    (1000 / target_fps.max(1)).max(1)
}
