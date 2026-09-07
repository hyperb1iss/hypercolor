use super::super::sparkleflinger::PreviewSurfaceRequest;
use crate::preview_runtime::PreviewDemandSummary;

pub(super) fn requires_cpu_sampling_canvas(can_gpu_sample: bool) -> bool {
    !can_gpu_sample
}

#[allow(
    clippy::fn_params_excessive_bools,
    reason = "preview publication depends on a small fixed matrix of boolean runtime states"
)]
pub(super) fn requires_published_surface(
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    effect_running: bool,
    screen_capture_active: bool,
    scene_canvas_receivers: usize,
) -> bool {
    scene_canvas_receivers > 0
        || publish_canvas_preview
        || (publish_screen_canvas_preview && !effect_running && screen_capture_active)
}

#[derive(Clone, Copy, Default)]
pub(super) struct PreviewSurfaceDemandLane {
    pub(super) receivers: usize,
    pub(super) tracked_receivers: usize,
    pub(super) demand: PreviewDemandSummary,
}

#[derive(Clone, Copy, Default)]
pub(super) struct PreviewSurfaceRequestContext {
    pub(super) canvas_width: u32,
    pub(super) canvas_height: u32,
    pub(super) publish_canvas_preview: bool,
    pub(super) publish_screen_canvas_preview: bool,
    pub(super) effect_running: bool,
    pub(super) screen_capture_active: bool,
    pub(super) scene_canvas: PreviewSurfaceDemandLane,
    pub(super) canvas: PreviewSurfaceDemandLane,
    pub(super) screen_canvas: PreviewSurfaceDemandLane,
}

pub(super) fn preview_surface_request(
    context: PreviewSurfaceRequestContext,
) -> Option<PreviewSurfaceRequest> {
    let wants_screen_passthrough = context.publish_screen_canvas_preview
        && !context.effect_running
        && context.screen_capture_active;
    if !requires_published_surface(
        context.publish_canvas_preview,
        context.publish_screen_canvas_preview,
        context.effect_running,
        context.screen_capture_active,
        context.scene_canvas.receivers,
    ) {
        return None;
    }

    if context.scene_canvas.receivers > context.scene_canvas.tracked_receivers
        || (context.publish_canvas_preview
            && context.canvas.receivers > context.canvas.tracked_receivers)
        || (wants_screen_passthrough
            && context.screen_canvas.receivers > context.screen_canvas.tracked_receivers)
    {
        return Some(PreviewSurfaceRequest {
            width: context.canvas_width,
            height: context.canvas_height,
        });
    }

    let mut max_width = 0;
    let mut max_height = 0;
    let mut any_full_resolution = false;
    if context.publish_canvas_preview {
        max_width = max_width.max(context.canvas.demand.max_width);
        max_height = max_height.max(context.canvas.demand.max_height);
        any_full_resolution |= context.canvas.demand.any_full_resolution;
    }
    if context.scene_canvas.receivers > 0 {
        max_width = max_width.max(context.scene_canvas.demand.max_width);
        max_height = max_height.max(context.scene_canvas.demand.max_height);
        any_full_resolution |= context.scene_canvas.demand.any_full_resolution;
    }
    if wants_screen_passthrough {
        max_width = max_width.max(context.screen_canvas.demand.max_width);
        max_height = max_height.max(context.screen_canvas.demand.max_height);
        any_full_resolution |= context.screen_canvas.demand.any_full_resolution;
    }

    if any_full_resolution
        || context.canvas_width == 0
        || context.canvas_height == 0
        || max_width == 0
        || max_height == 0
    {
        return Some(PreviewSurfaceRequest {
            width: context.canvas_width,
            height: context.canvas_height,
        });
    }

    Some(PreviewSurfaceRequest {
        width: max_width.clamp(1, context.canvas_width),
        height: max_height.clamp(1, context.canvas_height),
    })
}

pub(super) fn scene_canvas_forces_full_surface(
    canvas_width: u32,
    canvas_height: u32,
    scene_canvas_receivers: usize,
    tracked_scene_canvas_receivers: usize,
    scene_canvas_demand: PreviewDemandSummary,
) -> bool {
    if scene_canvas_receivers == 0 {
        return false;
    }

    if scene_canvas_receivers > tracked_scene_canvas_receivers {
        return true;
    }

    scene_canvas_demand.any_full_resolution
        || scene_canvas_demand.max_width == 0
        || scene_canvas_demand.max_height == 0
        || (scene_canvas_demand.max_width >= canvas_width
            && scene_canvas_demand.max_height >= canvas_height)
}
