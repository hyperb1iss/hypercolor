use std::time::Instant;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};
use hypercolor_core::types::event::FrameData;

use super::composition_planner::CompositionPlanner;
use super::frame_scheduler::FrameScheduler;
use super::producer_queue::ProducerQueue;
use super::render_groups::RenderGroupRuntime;
use super::scene_state::RenderSceneState;
use super::sparkleflinger::SparkleFlinger;

pub(crate) struct FrameInputs {
    pub(crate) audio: AudioData,
    pub(crate) interaction: hypercolor_core::input::InteractionData,
    pub(crate) screen_data: Option<hypercolor_core::input::ScreenData>,
    pub(crate) screen_canvas: Option<Canvas>,
    pub(crate) screen_preview_surface: Option<PublishedSurface>,
}

impl FrameInputs {
    pub(crate) fn silence() -> Self {
        Self {
            audio: AudioData::silence(),
            interaction: hypercolor_core::input::InteractionData::default(),
            screen_data: None,
            screen_canvas: None,
            screen_preview_surface: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StaticSurfaceKey {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color: [u8; 3],
}

#[derive(Clone)]
pub(crate) struct CachedStaticSurface {
    pub(crate) key: StaticSurfaceKey,
    pub(crate) surface: PublishedSurface,
}

pub(crate) struct FrameLoopState {
    pub(crate) cached_inputs: FrameInputs,
    pub(crate) last_tick: Instant,
    pub(crate) idle_black_pushed: bool,
    pub(crate) sleep_black_pushed: bool,
    pub(crate) last_audio_level_update_ms: Option<u32>,
    pub(crate) last_audio_capture_active: Option<bool>,
    pub(crate) last_screen_capture_active: Option<bool>,
}

pub(crate) struct RenderCaches {
    pub(crate) effect_target_canvas: Option<Canvas>,
    pub(crate) effect_queue: ProducerQueue,
    pub(crate) screen_queue: ProducerQueue,
    pub(crate) composition_planner: CompositionPlanner,
    pub(crate) sparkleflinger: SparkleFlinger,
    pub(crate) render_group_runtime: RenderGroupRuntime,
    pub(crate) render_surface_pool: RenderSurfacePool,
    pub(crate) render_scene_state: RenderSceneState,
    pub(crate) static_surface_cache: Option<CachedStaticSurface>,
    pub(crate) recycled_frame: FrameData,
}

pub(crate) struct PipelineRuntime {
    pub(crate) frame_scheduler: FrameScheduler,
    pub(crate) frame_loop: FrameLoopState,
    pub(crate) render: RenderCaches,
}

impl PipelineRuntime {
    pub(crate) fn new(
        canvas_width: u32,
        canvas_height: u32,
        initial_spatial_engine: SpatialEngine,
        screen_capture_configured: bool,
    ) -> Self {
        Self {
            frame_scheduler: FrameScheduler::new(),
            frame_loop: FrameLoopState {
                cached_inputs: FrameInputs::silence(),
                last_tick: Instant::now(),
                idle_black_pushed: false,
                sleep_black_pushed: false,
                last_audio_level_update_ms: None,
                last_audio_capture_active: None,
                last_screen_capture_active: None,
            },
            render: RenderCaches {
                effect_target_canvas: None,
                effect_queue: ProducerQueue::new(),
                screen_queue: ProducerQueue::new(),
                composition_planner: CompositionPlanner::new(),
                sparkleflinger: SparkleFlinger::new(),
                render_group_runtime: RenderGroupRuntime::new(canvas_width, canvas_height),
                render_surface_pool: RenderSurfacePool::new(SurfaceDescriptor::rgba8888(
                    canvas_width,
                    canvas_height,
                )),
                render_scene_state: RenderSceneState::new(
                    initial_spatial_engine,
                    screen_capture_configured,
                ),
                static_surface_cache: None,
                recycled_frame: FrameData::empty(),
            },
        }
    }
}
