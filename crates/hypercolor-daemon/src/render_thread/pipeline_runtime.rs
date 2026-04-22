use std::collections::VecDeque;
use std::time::Instant;

use anyhow::Result;
use hypercolor_core::engine::FpsTier;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};
use hypercolor_core::types::event::FrameData;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::SceneId;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;
use std::sync::Arc;

use super::RenderThreadState;
use super::composition_planner::CompositionPlanner;
use super::desired_render_surface_slots;
use super::frame_admission::FrameAdmissionController;
use super::frame_scheduler::FrameScheduler;
use super::frame_state::CachedRenderGroupDemand;
use super::producer_queue::ProducerQueue;
use super::render_groups::RenderGroupRuntime;
use super::scene_state::RenderSceneState;
use super::sparkleflinger::{PendingZoneSampling, SparkleFlinger};

pub(crate) struct FrameInputs {
    pub(crate) audio: AudioData,
    pub(crate) interaction: hypercolor_core::input::InteractionData,
    pub(crate) screen_data: Option<hypercolor_core::input::ScreenData>,
    pub(crate) sensors: Arc<SystemSnapshot>,
    pub(crate) screen_canvas: Option<Canvas>,
    pub(crate) screen_sector_grid: Vec<[u8; 3]>,
}

impl FrameInputs {
    pub(crate) fn silence() -> Self {
        Self {
            audio: AudioData::silence(),
            interaction: hypercolor_core::input::InteractionData::default(),
            screen_data: None,
            sensors: Arc::new(SystemSnapshot::empty()),
            screen_canvas: None,
            screen_sector_grid: Vec::new(),
        }
    }

    pub(crate) fn screen_canvas_for_frame(&mut self, width: u32, height: u32) -> Option<Canvas> {
        if self.screen_canvas.is_none() {
            self.screen_canvas = self.screen_data.as_ref().and_then(|data| {
                super::frame_io::screen_data_to_canvas(
                    data,
                    width,
                    height,
                    &mut self.screen_sector_grid,
                )
            });
        }

        self.screen_canvas.clone()
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
    pub(crate) last_canvas_preview_publish_ms: Option<u32>,
    pub(crate) last_screen_canvas_preview_publish_ms: Option<u32>,
    pub(crate) last_web_viewport_preview_publish_ms: Option<u32>,
    pub(crate) last_audio_capture_active: Option<bool>,
    pub(crate) last_screen_capture_active: Option<bool>,
    pub(crate) last_render_group_demand: Option<CachedRenderGroupDemand>,
    pub(crate) last_output_brightness_bits: Option<u32>,
    pub(crate) last_device_output_brightness_generation: Option<u64>,
}

pub(crate) struct RenderCaches {
    pub(crate) screen_queue: ProducerQueue,
    pub(crate) composition_planner: CompositionPlanner,
    pub(crate) sparkleflinger: SparkleFlinger,
    pub(crate) deferred_zone_sampling: Option<PendingZoneSampling>,
    pub(crate) retired_zone_sampling: VecDeque<PendingZoneSampling>,
    pub(crate) deferred_zone_sampling_scratch: Vec<hypercolor_types::event::ZoneColors>,
    pub(crate) zone_transition_planner: ZoneTransitionPlanner,
    pub(crate) render_group_runtime: RenderGroupRuntime,
    pub(crate) render_surface_pool: RenderSurfacePool,
    pub(crate) render_scene_state: RenderSceneState,
    pub(crate) static_surface_cache: Option<CachedStaticSurface>,
    pub(crate) recycled_frame: FrameData,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RenderSurfaceSnapshot {
    pub(crate) slot_count: u32,
    pub(crate) free_slots: u32,
    pub(crate) published_slots: u32,
    pub(crate) dequeued_slots: u32,
    pub(crate) canvas_receivers: u32,
    /// Monotonic counter from the render-group runtime's preview pool:
    /// how many times a dequeue had to reuse a still-shared Published
    /// slot and allocate a fresh canvas. Only fires at the pool's cap.
    pub(crate) preview_pool_saturation_reallocs: u64,
    /// Same counter summed across per-group direct-canvas pools.
    pub(crate) direct_pool_saturation_reallocs: u64,
    /// Current slot count above the preview pool's initial size. Grows
    /// once per high-water mark, then settles.
    pub(crate) preview_pool_grown_slots: u32,
    /// Same gauge summed across per-group direct-canvas pools.
    pub(crate) direct_pool_grown_slots: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SceneTransitionKey {
    pub(crate) from_scene: SceneId,
    pub(crate) to_scene: SceneId,
}

#[derive(Clone)]
pub(crate) struct RetainedZoneFrame {
    pub(crate) layout: Arc<SpatialLayout>,
    pub(crate) zones: Vec<ZoneColors>,
}

#[derive(Default)]
pub(crate) struct ZoneTransitionPlanner {
    pub(crate) active_transition: Option<SceneTransitionKey>,
    pub(crate) transition_base: Option<RetainedZoneFrame>,
    pub(crate) last_stable: Option<RetainedZoneFrame>,
}

impl ZoneTransitionPlanner {
    pub(crate) fn clear(&mut self) {
        self.active_transition = None;
        self.transition_base = None;
    }

    pub(crate) fn record_stable(&mut self, layout: Arc<SpatialLayout>, zones: &[ZoneColors]) {
        self.clear();
        self.last_stable = Some(RetainedZoneFrame {
            layout,
            zones: zones.to_vec(),
        });
    }
}

impl RenderCaches {
    /// Rebuild surface pools and clear cached canvases for a canvas resize.
    ///
    /// Called at the frame boundary when a `ResizeCanvas` transaction is drained.
    /// Existing published surfaces stay valid until their leases drop; new
    /// dequeues get the updated dimensions.
    pub(crate) fn apply_canvas_resize(&mut self, width: u32, height: u32) {
        if let Some(pending) = self.deferred_zone_sampling.take() {
            self.sparkleflinger.discard_pending_zone_sampling(pending);
        }
        while let Some(pending) = self.retired_zone_sampling.pop_front() {
            self.sparkleflinger.discard_pending_zone_sampling(pending);
        }
        self.render_surface_pool = RenderSurfacePool::with_slot_count(
            SurfaceDescriptor::rgba8888(width, height),
            desired_render_surface_slots(0),
        );
        self.render_group_runtime = RenderGroupRuntime::new(width, height);
        self.composition_planner = CompositionPlanner::new();
        self.deferred_zone_sampling_scratch.clear();
        self.zone_transition_planner = ZoneTransitionPlanner::default();
        self.static_surface_cache = None;
    }

    pub(crate) fn render_surface_snapshot(
        &mut self,
        canvas_receiver_count: usize,
    ) -> RenderSurfaceSnapshot {
        let slot_counts = self.render_surface_pool.slot_counts();
        let mut snapshot = RenderSurfaceSnapshot {
            slot_count: u32::try_from(self.render_surface_pool.slot_count()).unwrap_or(u32::MAX),
            canvas_receivers: u32::try_from(canvas_receiver_count).unwrap_or(u32::MAX),
            ..RenderSurfaceSnapshot::default()
        };
        snapshot.free_slots = u32::try_from(slot_counts.free).unwrap_or(u32::MAX);
        snapshot.published_slots = u32::try_from(slot_counts.published).unwrap_or(u32::MAX);
        snapshot.dequeued_slots = u32::try_from(slot_counts.dequeued).unwrap_or(u32::MAX);
        snapshot.preview_pool_saturation_reallocs = self
            .render_group_runtime
            .preview_surface_pool_saturation_reallocs();
        snapshot.direct_pool_saturation_reallocs = self
            .render_group_runtime
            .direct_surface_pool_saturation_reallocs();
        snapshot.preview_pool_grown_slots =
            self.render_group_runtime.preview_surface_pool_grown_slots();
        snapshot.direct_pool_grown_slots =
            self.render_group_runtime.direct_surface_pool_grown_slots();

        snapshot
    }
}

pub(crate) struct PipelineRuntime {
    pub(crate) frame_scheduler: FrameScheduler,
    pub(crate) frame_loop: FrameLoopState,
    pub(crate) render: RenderCaches,
    pub(crate) frame_admission: FrameAdmissionController,
}

impl PipelineRuntime {
    pub(crate) async fn from_state(state: &RenderThreadState) -> Result<Self> {
        let initial_spatial_engine = state.spatial_engine.read().await.clone();
        Self::new(
            state.canvas_dims.width(),
            state.canvas_dims.height(),
            initial_spatial_engine,
            state.screen_capture_configured,
            state.render_acceleration_mode,
            state.configured_max_fps_tier,
        )
    }

    pub(crate) fn new(
        canvas_width: u32,
        canvas_height: u32,
        initial_spatial_engine: SpatialEngine,
        screen_capture_configured: bool,
        render_acceleration_mode: RenderAccelerationMode,
        configured_max_fps_tier: FpsTier,
    ) -> Result<Self> {
        Ok(Self {
            frame_scheduler: FrameScheduler::new(),
            frame_loop: FrameLoopState {
                cached_inputs: FrameInputs::silence(),
                last_tick: Instant::now(),
                idle_black_pushed: false,
                sleep_black_pushed: false,
                last_audio_level_update_ms: None,
                last_canvas_preview_publish_ms: None,
                last_screen_canvas_preview_publish_ms: None,
                last_web_viewport_preview_publish_ms: None,
                last_audio_capture_active: None,
                last_screen_capture_active: None,
                last_render_group_demand: None,
                last_output_brightness_bits: None,
                last_device_output_brightness_generation: None,
            },
            render: RenderCaches {
                screen_queue: ProducerQueue::new(),
                composition_planner: CompositionPlanner::new(),
                sparkleflinger: SparkleFlinger::new(render_acceleration_mode)?,
                deferred_zone_sampling: None,
                retired_zone_sampling: VecDeque::new(),
                deferred_zone_sampling_scratch: Vec::new(),
                zone_transition_planner: ZoneTransitionPlanner::default(),
                render_group_runtime: RenderGroupRuntime::new(canvas_width, canvas_height),
                render_surface_pool: RenderSurfacePool::with_slot_count(
                    SurfaceDescriptor::rgba8888(canvas_width, canvas_height),
                    desired_render_surface_slots(0),
                ),
                render_scene_state: RenderSceneState::new(
                    initial_spatial_engine,
                    screen_capture_configured,
                ),
                static_surface_cache: None,
                recycled_frame: FrameData::empty(),
            },
            frame_admission: FrameAdmissionController::new(configured_max_fps_tier),
        })
    }
}
