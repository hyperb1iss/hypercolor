use std::collections::VecDeque;
use std::time::Instant;

use anyhow::Result;
use hypercolor_core::engine::FpsTier;
use hypercolor_core::input::{InputData, InteractionData, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
};
use hypercolor_core::types::event::{FrameData, HypercolorEvent};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::SceneId;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;
use std::sync::Arc;

use super::capture_demand::CaptureDemandState;
use super::composition_planner::CompositionPlanner;
use super::desired_render_surface_slots;
use super::frame_policy::FramePolicy;
use super::frame_policy::SkipDecision;
use super::producer_queue::ProducerQueue;
use super::render_groups::RenderGroupRuntime;
use super::scene_snapshot::SceneSnapshotCache;
use super::scene_state::RenderSceneState;
use super::screen_canvas::screen_data_to_canvas;
use super::sparkleflinger::{PendingZoneSampling, SparkleFlinger};
use super::{RenderThreadState, micros_u32};

const AUDIO_LEVEL_EVENT_INTERVAL_MS: u32 = 100;

pub(crate) struct FrameInputs {
    pub(crate) audio: AudioData,
    pub(crate) interaction: hypercolor_core::input::InteractionData,
    pub(crate) screen_data: Option<hypercolor_core::input::ScreenData>,
    pub(crate) sensors: Arc<SystemSnapshot>,
    pub(crate) screen_canvas: Option<Canvas>,
    pub(crate) screen_sector_grid: Vec<[u8; 3]>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameTick {
    pub(crate) frame_interval_us: u32,
    pub(crate) delta_secs: f32,
}

#[derive(Debug)]
pub(crate) struct FrameClockState {
    last_tick: Instant,
}

impl Default for FrameClockState {
    fn default() -> Self {
        Self {
            last_tick: Instant::now(),
        }
    }
}

impl FrameClockState {
    pub(crate) fn advance(&mut self, frame_start: Instant) -> FrameTick {
        let frame_interval = frame_start.saturating_duration_since(self.last_tick);
        self.last_tick = frame_start;
        FrameTick {
            frame_interval_us: micros_u32(frame_interval),
            delta_secs: frame_interval.as_secs_f32(),
        }
    }
}

pub(crate) struct InputReuseState {
    cached_inputs: FrameInputs,
}

impl Default for InputReuseState {
    fn default() -> Self {
        Self {
            cached_inputs: FrameInputs::silence(),
        }
    }
}

impl InputReuseState {
    pub(crate) async fn inputs_for_frame<'a>(
        &'a mut self,
        state: &RenderThreadState,
        skip_decision: SkipDecision,
        delta_secs: f32,
    ) -> &'a mut FrameInputs {
        if matches!(skip_decision, SkipDecision::None) {
            self.cached_inputs = FrameInputs::sample(state, delta_secs).await;
        }

        &mut self.cached_inputs
    }
}

impl FrameInputs {
    pub(crate) async fn sample(state: &RenderThreadState, delta_secs: f32) -> Self {
        let (samples, events) = {
            let mut input_manager = state.input_manager.lock().await;
            (
                input_manager.sample_all_with_delta_secs(delta_secs),
                input_manager.drain_events(),
            )
        };

        for event in events {
            state
                .event_bus
                .publish(HypercolorEvent::InputEventReceived { event });
        }

        let mut audio = AudioData::silence();
        let mut interaction = InteractionData::default();
        let mut screen_data: Option<ScreenData> = None;
        let mut sensors = Arc::new(SystemSnapshot::empty());
        for sample in samples {
            match sample {
                InputData::Audio(snapshot) => audio = snapshot,
                InputData::Interaction(snapshot) => interaction = snapshot,
                InputData::Screen(snapshot) => screen_data = Some(snapshot),
                InputData::Sensors(snapshot) => sensors = snapshot,
                InputData::None => {}
            }
        }

        Self {
            audio,
            interaction,
            screen_data,
            sensors,
            screen_canvas: None,
            screen_sector_grid: Vec::new(),
        }
    }

    pub(crate) fn silence() -> Self {
        Self {
            audio: AudioData::silence(),
            interaction: InteractionData::default(),
            screen_data: None,
            sensors: Arc::new(SystemSnapshot::empty()),
            screen_canvas: None,
            screen_sector_grid: Vec::new(),
        }
    }

    pub(crate) fn screen_canvas_for_frame(&mut self, width: u32, height: u32) -> Option<Canvas> {
        if self.screen_canvas.is_none() {
            self.screen_canvas = self.screen_data.as_ref().and_then(|data| {
                screen_data_to_canvas(data, width, height, &mut self.screen_sector_grid)
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

pub(crate) struct OutputArtifactsState {
    static_surface_cache: Option<CachedStaticSurface>,
    recycled_frame: FrameData,
}

impl Default for OutputArtifactsState {
    fn default() -> Self {
        Self {
            static_surface_cache: None,
            recycled_frame: FrameData::empty(),
        }
    }
}

impl OutputArtifactsState {
    pub(crate) fn static_surface(
        &mut self,
        width: u32,
        height: u32,
        color: [u8; 3],
    ) -> PublishedSurface {
        let key = StaticSurfaceKey {
            width,
            height,
            color,
        };

        if let Some(cached) = self.static_surface_cache.as_ref()
            && cached.key == key
        {
            return cached.surface.clone();
        }

        let mut canvas = Canvas::new(width, height);
        if color != [0, 0, 0] {
            canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
        }

        let surface = PublishedSurface::from_owned_canvas(canvas, 0, 0);
        self.static_surface_cache = Some(CachedStaticSurface {
            key,
            surface: surface.clone(),
        });
        surface
    }

    pub(crate) fn reset_for_canvas_resize(&mut self) {
        self.static_surface_cache = None;
    }

    pub(crate) fn clear_zones(&mut self) {
        self.recycled_frame.zones.clear();
    }

    pub(crate) fn zones(&self) -> &[ZoneColors] {
        &self.recycled_frame.zones
    }

    pub(crate) fn zones_mut(&mut self) -> &mut Vec<ZoneColors> {
        &mut self.recycled_frame.zones
    }

    pub(crate) fn frame_mut(&mut self) -> &mut FrameData {
        &mut self.recycled_frame
    }
}

pub(crate) enum PendingZoneSamplingStatus {
    Completed(PendingZoneSampling),
    Stale(PendingZoneSampling),
}

#[derive(Default)]
pub(crate) struct DeferredSamplingState {
    pending: Option<PendingZoneSampling>,
    retired: VecDeque<PendingZoneSampling>,
    scratch: Vec<ZoneColors>,
}

impl DeferredSamplingState {
    pub(crate) fn scratch_mut(&mut self) -> &mut Vec<ZoneColors> {
        &mut self.scratch
    }

    pub(crate) fn clone_scratch_into(&self, target: &mut Vec<ZoneColors>) {
        target.clone_from(&self.scratch);
    }

    pub(crate) fn store_pending(&mut self, pending: PendingZoneSampling) {
        self.pending = Some(pending);
    }

    pub(crate) fn take_pending_status(
        &mut self,
        sparkleflinger: &mut SparkleFlinger,
        error_message: &'static str,
    ) -> Option<PendingZoneSamplingStatus> {
        let mut pending = self.pending.take()?;
        match sparkleflinger.try_finish_pending_zone_sampling(&mut pending, &mut self.scratch) {
            Ok(true) => Some(PendingZoneSamplingStatus::Completed(pending)),
            Ok(false) => Some(PendingZoneSamplingStatus::Stale(pending)),
            Err(error) => {
                tracing::warn!(%error, "{error_message}");
                None
            }
        }
    }

    pub(crate) fn finish_retired(
        &mut self,
        sparkleflinger: &mut SparkleFlinger,
        error_message: &'static str,
    ) {
        let retired_count = self.retired.len();
        for _ in 0..retired_count {
            let Some(mut retired_sampling) = self.retired.pop_front() else {
                break;
            };
            match sparkleflinger
                .try_finish_pending_zone_sampling(&mut retired_sampling, &mut self.scratch)
            {
                Ok(true) => {}
                Ok(false) => {
                    self.retired.push_back(retired_sampling);
                }
                Err(error) => {
                    tracing::warn!(%error, "{error_message}");
                }
            }
        }
    }

    pub(crate) fn retire_or_return(
        &mut self,
        sparkleflinger: &mut SparkleFlinger,
        pending: PendingZoneSampling,
    ) -> Option<PendingZoneSampling> {
        self.finish_retired(
            sparkleflinger,
            "Retired GPU spatial sampling cleanup failed; dropping stale deferred sample result",
        );

        let retired_capacity = sparkleflinger.max_pending_zone_sampling().saturating_sub(1);
        if self.retired.len() >= retired_capacity {
            return Some(pending);
        }

        self.retired.push_back(pending);
        None
    }

    pub(crate) fn discard_backlog(&mut self, sparkleflinger: &mut SparkleFlinger) {
        if let Some(pending) = self.pending.take() {
            sparkleflinger.discard_pending_zone_sampling(pending);
        }
        while let Some(pending) = self.retired.pop_front() {
            sparkleflinger.discard_pending_zone_sampling(pending);
        }
    }

    pub(crate) fn clear_for_canvas_resize(&mut self, sparkleflinger: &mut SparkleFlinger) {
        self.discard_backlog(sparkleflinger);
        self.scratch.clear();
    }
}

#[derive(Debug, Default)]
pub(crate) struct PublicationCadenceState {
    pub(crate) last_audio_level_update_ms: Option<u32>,
    pub(crate) last_canvas_preview_publish_ms: Option<u32>,
    pub(crate) last_screen_canvas_preview_publish_ms: Option<u32>,
    pub(crate) last_web_viewport_preview_publish_ms: Option<u32>,
}

impl PublicationCadenceState {
    pub(crate) fn should_publish_audio_level(
        &self,
        elapsed_ms: u32,
        has_event_subscribers: bool,
    ) -> bool {
        has_event_subscribers
            && !self.last_audio_level_update_ms.is_some_and(|last_sent| {
                elapsed_ms.saturating_sub(last_sent) < AUDIO_LEVEL_EVENT_INTERVAL_MS
            })
    }

    pub(crate) fn record_audio_level_update(&mut self, elapsed_ms: u32) {
        self.last_audio_level_update_ms = Some(elapsed_ms);
    }

    pub(crate) fn canvas_preview_due(
        &self,
        elapsed_ms: u32,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_canvas_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_canvas_publication(&mut self, elapsed_ms: u32) {
        self.last_canvas_preview_publish_ms = Some(elapsed_ms);
    }

    pub(crate) fn screen_canvas_preview_due(
        &self,
        elapsed_ms: u32,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_screen_canvas_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_screen_canvas_publication(&mut self, elapsed_ms: u32) {
        self.last_screen_canvas_preview_publish_ms = Some(elapsed_ms);
    }

    pub(crate) fn web_viewport_preview_due(
        &self,
        elapsed_ms: u32,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_web_viewport_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_web_viewport_publication(&mut self, elapsed_ms: u32) {
        self.last_web_viewport_preview_publish_ms = Some(elapsed_ms);
    }
}

#[derive(Debug, Default)]
pub(crate) struct ThrottleState {
    pub(crate) idle_black_pushed: bool,
    pub(crate) sleep_black_pushed: bool,
}

impl ThrottleState {
    pub(crate) fn reset_for_canvas_resize(&mut self) {
        self.idle_black_pushed = false;
        self.sleep_black_pushed = false;
    }

    pub(crate) fn note_effect_running(&mut self) {
        self.idle_black_pushed = false;
    }

    pub(crate) fn idle_black_pushed(&self) -> bool {
        self.idle_black_pushed
    }

    pub(crate) fn note_idle_frame_without_effect(&mut self) {
        self.idle_black_pushed = true;
    }

    pub(crate) fn clear_sleep(&mut self) {
        self.sleep_black_pushed = false;
    }

    pub(crate) fn sleep_black_pushed(&self) -> bool {
        self.sleep_black_pushed
    }

    pub(crate) fn note_sleep_frame_published(&mut self) {
        self.sleep_black_pushed = true;
    }
}

#[derive(Debug, Default)]
pub(crate) struct OutputReuseState {
    pub(crate) last_output_brightness_bits: Option<u32>,
    pub(crate) last_device_output_brightness_generation: Option<u64>,
}

impl OutputReuseState {
    pub(crate) fn matches(
        &self,
        output_brightness_bits: u32,
        device_output_brightness_generation: u64,
    ) -> bool {
        self.last_output_brightness_bits == Some(output_brightness_bits)
            && self.last_device_output_brightness_generation
                == Some(device_output_brightness_generation)
    }

    pub(crate) fn record(
        &mut self,
        output_brightness_bits: u32,
        device_output_brightness_generation: u64,
    ) {
        self.last_output_brightness_bits = Some(output_brightness_bits);
        self.last_device_output_brightness_generation = Some(device_output_brightness_generation);
    }
}

fn preview_publish_fps_limit(
    total_receivers: usize,
    tracked_receivers: usize,
    tracked_max_fps: u32,
) -> Option<u32> {
    (total_receivers > 0 && total_receivers == tracked_receivers).then_some(tracked_max_fps.max(1))
}

fn should_publish_preview_frame(
    elapsed_ms: u32,
    last_publish_ms: Option<u32>,
    target_fps: Option<u32>,
) -> bool {
    let Some(target_fps) = target_fps else {
        return true;
    };
    let interval_ms = 1000_u32.div_ceil(target_fps.max(1));
    last_publish_ms.is_none_or(|last_sent| elapsed_ms.saturating_sub(last_sent) >= interval_ms)
}

fn preview_publication_due(
    elapsed_ms: u32,
    last_publish_ms: Option<u32>,
    total_receivers: usize,
    tracked_receivers: usize,
    tracked_max_fps: u32,
) -> bool {
    if total_receivers == 0 {
        return false;
    }

    should_publish_preview_frame(
        elapsed_ms,
        last_publish_ms,
        preview_publish_fps_limit(total_receivers, tracked_receivers, tracked_max_fps),
    )
}

pub(crate) struct FrameLoopState {
    pub(crate) clock: FrameClockState,
    pub(crate) inputs: InputReuseState,
    pub(crate) throttle: ThrottleState,
    pub(crate) publication_cadence: PublicationCadenceState,
    pub(crate) capture_demand: CaptureDemandState,
    pub(crate) output_reuse: OutputReuseState,
}

pub(crate) struct RenderCaches {
    pub(crate) screen_queue: ProducerQueue,
    pub(crate) composition_planner: CompositionPlanner,
    pub(crate) sparkleflinger: SparkleFlinger,
    pub(crate) deferred_sampling: DeferredSamplingState,
    pub(crate) zone_transition_planner: ZoneTransitionPlanner,
    pub(crate) render_group_runtime: RenderGroupRuntime,
    pub(crate) render_surface_pool: RenderSurfacePool,
    pub(crate) render_scene_state: RenderSceneState,
    pub(crate) output_artifacts: OutputArtifactsState,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RenderSurfaceSnapshot {
    pub(crate) slot_count: u32,
    pub(crate) free_slots: u32,
    pub(crate) published_slots: u32,
    pub(crate) dequeued_slots: u32,
    pub(crate) canvas_receivers: u32,
    /// Monotonic counter from the render-group runtime's scene surface pool:
    /// how many times a dequeue had to reuse a still-shared Published
    /// slot and allocate a fresh canvas. Only fires at the pool's cap.
    pub(crate) scene_pool_saturation_reallocs: u64,
    /// Same counter summed across per-group direct-canvas pools.
    pub(crate) direct_pool_saturation_reallocs: u64,
    /// Current slot count above the scene surface pool's initial size. Grows
    /// once per high-water mark, then settles.
    pub(crate) scene_pool_grown_slots: u32,
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
        self.deferred_sampling
            .clear_for_canvas_resize(&mut self.sparkleflinger);
        self.render_surface_pool = RenderSurfacePool::with_slot_count(
            SurfaceDescriptor::rgba8888(width, height),
            desired_render_surface_slots(0),
        );
        self.render_group_runtime = RenderGroupRuntime::new(width, height);
        self.composition_planner = CompositionPlanner::new();
        self.zone_transition_planner = ZoneTransitionPlanner::default();
        self.output_artifacts.reset_for_canvas_resize();
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
        snapshot.scene_pool_saturation_reallocs = self
            .render_group_runtime
            .scene_surface_pool_saturation_reallocs();
        snapshot.direct_pool_saturation_reallocs = self
            .render_group_runtime
            .direct_surface_pool_saturation_reallocs();
        snapshot.scene_pool_grown_slots =
            self.render_group_runtime.scene_surface_pool_grown_slots();
        snapshot.direct_pool_grown_slots =
            self.render_group_runtime.direct_surface_pool_grown_slots();

        snapshot
    }
}

pub(crate) struct PipelineRuntime {
    pub(crate) scene_snapshot_cache: SceneSnapshotCache,
    pub(crate) frame_loop: FrameLoopState,
    pub(crate) render: RenderCaches,
    pub(crate) frame_policy: FramePolicy,
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
            scene_snapshot_cache: SceneSnapshotCache::new(),
            frame_loop: FrameLoopState {
                clock: FrameClockState::default(),
                inputs: InputReuseState::default(),
                throttle: ThrottleState::default(),
                publication_cadence: PublicationCadenceState::default(),
                capture_demand: CaptureDemandState::default(),
                output_reuse: OutputReuseState::default(),
            },
            render: RenderCaches {
                screen_queue: ProducerQueue::new(),
                composition_planner: CompositionPlanner::new(),
                sparkleflinger: SparkleFlinger::new(render_acceleration_mode)?,
                deferred_sampling: DeferredSamplingState::default(),
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
                output_artifacts: OutputArtifactsState::default(),
            },
            frame_policy: FramePolicy::new(configured_max_fps_tier),
        })
    }
}
