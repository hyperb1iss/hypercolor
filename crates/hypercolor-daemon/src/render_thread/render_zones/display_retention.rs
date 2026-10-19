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
        retained
            .cadence
            .is_some_and(|cadence| cadence.holds_at(elapsed_ms, target_fps))
            .then(|| retained.frame.clone())
    }

    pub(super) fn retain_direct_zone_frame(
        &mut self,
        zone_id: ZoneId,
        elapsed_ms: u64,
        target_fps: Option<u32>,
        dependency_key: SceneDependencyKey,
        frame: &PendingDisplayZoneFrame,
    ) {
        let previous = self
            .retained_direct_zone_frames
            .get(&zone_id)
            .and_then(|retained| retained.cadence);
        self.retained_direct_zone_frames.insert(
            zone_id,
            RetainedDirectZoneFrame {
                frame: frame.clone(),
                cadence: DisplayFrameCadence::after_render(previous, elapsed_ms, target_fps),
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

        retained
            .cadence
            .is_some_and(|cadence| cadence.holds_at(elapsed_ms, target_fps))
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
        target_fps: Option<u32>,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        empty_direct_shell: bool,
        frame: &DisplayZoneCanvasFrame,
    ) {
        if display_route.device_id != display_target.device_id || !frame.display_target.finalized {
            return;
        }

        let previous = self
            .retained_materialized_zone_frames
            .get(&zone_id)
            .and_then(|retained| retained.cadence);
        self.retained_materialized_zone_frames.insert(
            zone_id,
            RetainedMaterializedZoneFrame {
                frame: frame.clone(),
                cadence: DisplayFrameCadence::after_render(previous, elapsed_ms, target_fps),
                dependency_key,
                display_target: display_target.clone(),
                display_route: display_route.clone(),
                empty_direct_shell,
            },
        );
    }
}

/// One frame interval in [`DisplayFrameCadence`] time units.
const FRAME_INTERVAL_UNITS: u128 = 1000;

/// How far ahead of its deadline a tick may land and still render.
///
/// Ticks and cadence anchors are both whole milliseconds, floored from the
/// render clock, so one millisecond is what it takes for the 33 ms tick of a
/// 30 fps loop to count against a 33.33 ms deadline. The second millisecond
/// absorbs wake jitter between the tick that anchored the cadence and the
/// ticks after it. Without it a tick that woke a hair earlier than the anchor
/// slips a whole render tick, and the face judders.
const DEADLINE_TOLERANCE_MS: u64 = 2;

/// Due-time pacing for a retained display face frame.
///
/// The cadence remembers the deadline the frame was rendered for, and the
/// next frame is due one exact interval later, however late the tick that
/// rendered this one landed. Gating on the actual render time instead
/// quantizes to the render loop: a 30 fps face on a 45 fps loop could only
/// render on every second 22 ms tick, which is 22.5 fps. Slots of the uptime
/// clock fix the average but not the spacing: on a 60 fps loop whole
/// millisecond ticks straddle slot edges, and the face renders at 50, 33,
/// and 17 ms gaps that a physical panel shows as judder. A deadline renders
/// that face on exactly every second tick and still averages 30 fps on the
/// 45 fps loop.
///
/// Times are kept in units of 1/`fps` milliseconds, so one interval is
/// exactly [`FRAME_INTERVAL_UNITS`] and the schedule never drifts with
/// uptime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DisplayFrameCadence {
    fps: u32,
    scheduled: u128,
}

impl DisplayFrameCadence {
    /// Schedules the frame rendered at `elapsed_ms`.
    ///
    /// A render that answers the deadline after `previous` takes over that
    /// deadline. Any other render resyncs the cadence to `elapsed_ms`: the
    /// first frame, a target rate change, a dependency change ahead of the
    /// deadline, or a tick so late that it reached the following deadline
    /// too. Resyncing after a stall keeps the face from bursting through the
    /// deadlines it missed.
    fn after_render(
        previous: Option<Self>,
        elapsed_ms: u64,
        target_fps: Option<u32>,
    ) -> Option<Self> {
        let fps = target_fps?.max(1);
        let horizon = tolerant_time(elapsed_ms, fps);
        let scheduled = previous
            .filter(|previous| previous.fps == fps)
            .map(|previous| previous.scheduled + FRAME_INTERVAL_UNITS)
            .filter(|&due| horizon >= due && horizon < due + FRAME_INTERVAL_UNITS)
            .unwrap_or_else(|| scaled_time(elapsed_ms, fps));
        Some(Self { fps, scheduled })
    }

    /// Whether the frame still stands in for the tick at `elapsed_ms`.
    fn holds_at(self, elapsed_ms: u64, target_fps: u32) -> bool {
        let fps = target_fps.max(1);
        self.fps == fps && tolerant_time(elapsed_ms, fps) < self.scheduled + FRAME_INTERVAL_UNITS
    }
}

fn scaled_time(elapsed_ms: u64, fps: u32) -> u128 {
    u128::from(elapsed_ms) * u128::from(fps)
}

fn tolerant_time(elapsed_ms: u64, fps: u32) -> u128 {
    (u128::from(elapsed_ms) + u128::from(DEADLINE_TOLERANCE_MS)) * u128::from(fps)
}
