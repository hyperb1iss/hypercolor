//! Frame pipeline render thread — the heartbeat of Hypercolor.
//!
//! Spawns a dedicated OS thread with its own Tokio runtime that runs the core
//! render loop:
//!
//! ```text
//! loop {
//!     RenderLoop::tick()              // timing gate + FPS control
//!     EffectEngine::tick()            // render effect → Canvas
//!     SpatialEngine::sample()         // map pixels → LED colors
//!     BackendManager::write_frame()   // push to hardware
//!     HypercolorBus::publish()        // notify subscribers
//!     RenderLoop::frame_complete()    // measure + adapt FPS tier
//!     sleep_until(next_deadline)      // pace to target FPS
//! }
//! ```

mod composition_planner;
mod frame_executor;
mod frame_io;
mod frame_scheduler;
mod frame_state;
mod pipeline_runtime;
mod producer_queue;
mod render_groups;
mod scene_state;
#[doc(hidden)]
pub mod sparkleflinger;

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::{debug, info, trace, warn};

use self::frame_executor::{FrameExecution, execute_frame};
use self::frame_io::publish_frame_updates;
use self::frame_scheduler::FrameSceneSnapshot;
use self::frame_state::{reconcile_audio_capture, reconcile_screen_capture};
use self::pipeline_runtime::{CachedStaticSurface, FrameInputs, PipelineRuntime, StaticSurfaceKey};
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{DiscoveryRuntime, handle_async_write_failures};
use crate::performance::{FrameTimeline, LatestFrameMetrics, PerformanceTracker};
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::OutputPowerState;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::BackendManager;
use hypercolor_core::effect::{EffectEngine, EffectRegistry};
use hypercolor_core::engine::{FrameStats, RenderLoop};
use hypercolor_core::input::{InputManager, InteractionData, ScreenData};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_core::types::event::{FrameData, FrameTiming};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::session::OffOutputBehavior;

const RENDER_RUNTIME_WORKERS: usize = 2;
const RENDER_RUNTIME_MAX_BLOCKING_THREADS: usize = 4;
const RENDER_RUNTIME_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(2);
const MAX_RENDER_SURFACE_SLOTS: usize = 6;
const PRECISE_WAKE_GUARD: Duration = Duration::from_micros(1_000);
const PRECISE_WAKE_SPIN_THRESHOLD: Duration = Duration::from_micros(150);

// ── RenderThread ────────────────────────────────────────────────────────────

/// Handle to a running render thread.
///
/// Call [`shutdown`](Self::shutdown) to stop the thread gracefully.
/// The render loop must be stopped first (via `RenderLoop::stop()`) — the
/// thread will exit on the next `tick()` returning `false`.
pub struct RenderThread {
    join_handle: Option<std::thread::JoinHandle<()>>,
}

/// All shared state the render thread needs.
///
/// Each field is `Arc`-wrapped so it can be shared with the API server
/// and other subsystems. The render thread takes locks only for the
/// duration of each pipeline stage.
#[derive(Clone)]
pub struct RenderThreadState {
    /// Active effect lifecycle and frame production.
    pub effect_engine: Arc<Mutex<EffectEngine>>,

    /// Effect catalog used to resolve render-group assignments.
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Rolling render-performance snapshot shared with metrics endpoints.
    pub performance: Arc<RwLock<PerformanceTracker>>,

    /// Discovery/lifecycle runtime used to react to async device write failures.
    pub discovery_runtime: Option<DiscoveryRuntime>,

    /// System-wide event bus — frame data and timing events.
    pub event_bus: Arc<HypercolorBus>,

    /// Render loop — frame timing, FPS control, tier transitions.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Active scene stack and transition runtime.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// Input orchestrator for audio and screen capture sampling.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Session policy output state (brightness scale + sleep flag).
    pub power_state: watch::Receiver<OutputPowerState>,

    /// Persisted global and per-device output settings.
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

    /// Frame-boundary scene changes consumed by the render thread.
    pub scene_transactions: SceneTransactionQueue,

    /// Whether screen capture is configured for direct passthrough / effects.
    pub screen_capture_configured: bool,

    /// Target render canvas width.
    pub canvas_width: u32,

    /// Target render canvas height.
    pub canvas_height: u32,

    /// Requested render acceleration mode for the pipeline.
    pub render_acceleration_mode: RenderAccelerationMode,
}

impl RenderThread {
    /// Spawn the render thread on a dedicated OS thread.
    ///
    /// The thread runs until `RenderLoop::tick()` returns `false`
    /// (i.e., the render loop has been stopped or paused).
    pub fn spawn(state: RenderThreadState) -> Self {
        let join_handle = std::thread::Builder::new()
            .name("hypercolor-render".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(RENDER_RUNTIME_WORKERS)
                    .max_blocking_threads(RENDER_RUNTIME_MAX_BLOCKING_THREADS)
                    .thread_keep_alive(RENDER_RUNTIME_THREAD_KEEP_ALIVE)
                    .thread_name("hypercolor-render-rt")
                    .enable_all()
                    .build()
                    .expect("render thread runtime should initialize");
                runtime.block_on(run_pipeline(state));
            })
            .expect("render thread should spawn");
        info!("render thread spawned");
        Self {
            join_handle: Some(join_handle),
        }
    }

    /// Wait for the render thread to exit.
    ///
    /// The caller must stop the render loop first — this method
    /// just awaits the task's completion.
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(handle) = self.join_handle.take() {
            tokio::task::spawn_blocking(move || {
                handle.join().map_err(|panic| {
                    anyhow!(
                        "render thread panicked: {}",
                        panic_payload_message(panic.as_ref())
                    )
                })
            })
            .await
            .context("failed to join render thread")??;
            info!("render thread stopped");
        }
        Ok(())
    }
}

// ── Pipeline ────────────────────────────────────────────────────────────────

/// Saturating conversion from `Duration` microseconds to `u32`.
///
/// Frame stage timings never exceed ~16ms (16000us), so this never
/// actually saturates in practice. But clippy pedantic demands it.
fn micros_u32(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

/// Saturating conversion from `Duration` milliseconds to `u32`.
fn millis_u32(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

/// Saturating conversion from `u64` to `u32`.
fn u64_to_u32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Saturating conversion from `usize` to `u32`.
fn usize_to_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Runtime decision for which frame stages may be reused when over budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipDecision {
    /// Execute all stages.
    None,
    /// Reuse previously sampled inputs (audio/screen/etc).
    ReuseInputs,
    /// Reuse previous rendered canvas and sampled inputs.
    ReuseCanvas,
}

impl SkipDecision {
    fn from_frame_stats(stats: &FrameStats) -> Self {
        if !stats.budget_exceeded {
            return Self::None;
        }

        if stats.consecutive_misses >= 2 {
            Self::ReuseCanvas
        } else {
            Self::ReuseInputs
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RenderLoopSnapshot {
    frame_token: u64,
    elapsed_ms: u32,
    budget_us: u32,
}

/// Scheduler decision for when the next render iteration should begin.
enum NextWake {
    /// Continue on the regular render cadence using the current FPS interval.
    Interval(Duration),
    /// Hold the loop for a fixed delay before checking again.
    Delay(Duration),
}

/// Sleep duration when the pipeline is fully idle.
const IDLE_THROTTLE_SLEEP: Duration = Duration::from_millis(120);
/// Sleep duration while session policy has output fully suspended.
const SESSION_SLEEP_THROTTLE_SLEEP: Duration = Duration::from_millis(250);
/// Emit UI audio summary events at 10 Hz.
const AUDIO_LEVEL_EVENT_INTERVAL_MS: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EffectDemand {
    effect_running: bool,
    audio_capture_active: bool,
    screen_capture_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EffectSceneSnapshot {
    demand: EffectDemand,
    generation: u64,
}

/// The main render pipeline loop.
///
/// Runs continuously, producing one frame per iteration:
/// 1. Gate on `RenderLoop::tick()` (exit if stopped)
/// 2. Render effect → `Canvas`
/// 3. Spatial sample → `Vec<ZoneColors>`
/// 4. Route to device backends
/// 5. Publish frame data + timing event
/// 6. Sleep for remaining frame budget
async fn run_pipeline(state: RenderThreadState) {
    info!(
        mode = ?state.render_acceleration_mode,
        "render pipeline started"
    );

    let initial_spatial_engine = state.spatial_engine.read().await.clone();
    let mut runtime = PipelineRuntime::new(
        state.canvas_width,
        state.canvas_height,
        initial_spatial_engine,
        state.screen_capture_configured,
    );
    let mut skip_decision = SkipDecision::None;
    let mut next_frame_at = Instant::now();

    loop {
        let scheduled_start = next_frame_at;
        wait_until_frame_deadline(scheduled_start).await;

        // ── Timing gate ─────────────────────────────────────────────
        let should_render = {
            let mut rl = state.render_loop.write().await;
            rl.tick()
        };

        if !should_render {
            // Check if we're paused (should wait) vs stopped (should exit).
            let loop_state = {
                let rl = state.render_loop.read().await;
                rl.state()
            };

            if loop_state == hypercolor_core::engine::RenderLoopState::Paused {
                reconcile_audio_capture(
                    &state,
                    false,
                    &mut runtime.frame_loop.last_audio_capture_active,
                )
                .await;
                reconcile_screen_capture(
                    &state,
                    false,
                    &mut runtime.frame_loop.last_screen_capture_active,
                )
                .await;
                // Paused — yield and retry.
                next_frame_at = Instant::now();
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }

            reconcile_audio_capture(
                &state,
                false,
                &mut runtime.frame_loop.last_audio_capture_active,
            )
            .await;
            reconcile_screen_capture(
                &state,
                false,
                &mut runtime.frame_loop.last_screen_capture_active,
            )
            .await;
            debug!("render loop not running, exiting pipeline");
            break;
        }

        let frame = execute_frame(&state, &mut runtime, scheduled_start, skip_decision).await;
        skip_decision = frame.next_skip_decision;
        next_frame_at = match frame.next_wake {
            NextWake::Interval(interval) => {
                advance_deadline(scheduled_start, interval, Instant::now())
            }
            NextWake::Delay(delay) => Instant::now()
                .checked_add(delay)
                .unwrap_or_else(Instant::now),
        };
    }

    info!("render pipeline exited");
}

fn advance_deadline(previous_deadline: Instant, interval: Duration, now: Instant) -> Instant {
    previous_deadline
        .checked_add(interval)
        .unwrap_or(now)
        .max(now)
}

fn coarse_sleep_deadline(deadline: Instant, now: Instant) -> Option<Instant> {
    deadline
        .checked_sub(PRECISE_WAKE_GUARD)
        .filter(|coarse_deadline| *coarse_deadline > now)
}

async fn wait_until_frame_deadline(deadline: Instant) {
    let now = Instant::now();
    if let Some(coarse_deadline) = coarse_sleep_deadline(deadline, now) {
        tokio::time::sleep_until(tokio::time::Instant::from_std(coarse_deadline)).await;
    }

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }

        if deadline.duration_since(now) > PRECISE_WAKE_SPIN_THRESHOLD {
            std::thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
}

async fn maybe_idle_throttle(
    state: &RenderThreadState,
    effect_running: bool,
    screen_capture_active: bool,
    idle_black_pushed: &mut bool,
) -> Option<FrameExecution> {
    let can_idle_throttle = should_idle_throttle(effect_running, screen_capture_active);

    if effect_running {
        *idle_black_pushed = false;
        return None;
    }

    if can_idle_throttle && !*idle_black_pushed {
        debug!(
            "No active effect or capture input; layout changes render black until an effect or input source starts"
        );
    }

    if can_idle_throttle && *idle_black_pushed {
        {
            let mut rl = state.render_loop.write().await;
            let _ = rl.frame_complete();
        }

        return Some(FrameExecution {
            next_wake: NextWake::Delay(IDLE_THROTTLE_SLEEP),
            next_skip_decision: SkipDecision::None,
        });
    }

    None
}

#[allow(
    clippy::too_many_lines,
    reason = "sleep-throttle execution is easier to audit when frame synthesis, output, and telemetry stay in one async block"
)]
async fn maybe_sleep_throttle(
    state: &RenderThreadState,
    scene_snapshot: &FrameSceneSnapshot,
    frame_start: Instant,
    scene_snapshot_done_us: u32,
    static_surface_cache: &mut Option<CachedStaticSurface>,
    recycled_frame: &mut FrameData,
    sleep_black_pushed: &mut bool,
    last_audio_level_update_ms: &mut Option<u32>,
) -> Option<FrameExecution> {
    let power_state = scene_snapshot.output_power;
    if !power_state.sleeping {
        *sleep_black_pushed = false;
        return None;
    }
    if *sleep_black_pushed {
        {
            let mut rl = state.render_loop.write().await;
            let _ = rl.frame_complete();
        }

        return Some(FrameExecution {
            next_wake: NextWake::Delay(SESSION_SLEEP_THROTTLE_SLEEP),
            next_skip_decision: SkipDecision::None,
        });
    }

    if power_state.off_output_behavior == OffOutputBehavior::Release {
        recycled_frame.zones.clear();
        let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
        let surface = static_surface(
            static_surface_cache,
            state.canvas_width,
            state.canvas_height,
            [0, 0, 0],
        );
        let publish_stats = publish_frame_updates(
            state,
            recycled_frame,
            &AudioData::silence(),
            Canvas::from_published_surface(&surface),
            Some(surface),
            None,
            frame_num_u32,
            scene_snapshot.elapsed_ms,
            last_audio_level_update_ms,
            FrameTiming {
                producer_us: 0,
                composition_us: 0,
                render_us: 0,
                sample_us: 0,
                push_us: 0,
                total_us: 0,
                budget_us: scene_snapshot.budget_us,
            },
        );
        let publish_us = publish_stats.elapsed_us;
        {
            let mut rl = state.render_loop.write().await;
            let _ = rl.frame_complete();
        }

        trace!(
            publish_us,
            "published cleared frame/canvas for release sleep"
        );
        *sleep_black_pushed = true;
        return Some(FrameExecution {
            next_wake: NextWake::Delay(SESSION_SLEEP_THROTTLE_SLEEP),
            next_skip_decision: SkipDecision::None,
        });
    }

    let surface = static_surface(
        static_surface_cache,
        state.canvas_width,
        state.canvas_height,
        power_state.off_output_color,
    );
    let canvas = Canvas::from_published_surface(&surface);
    let sample_start = Instant::now();
    scene_snapshot
        .spatial_engine
        .sample_into(&canvas, &mut recycled_frame.zones);
    let zone_colors = &recycled_frame.zones;
    let layout = scene_snapshot.spatial_engine.layout();
    let sample_us = micros_u32(sample_start.elapsed());
    let sample_done_us = micros_u32(frame_start.elapsed());

    let push_start = Instant::now();
    let (write_stats, async_failures) = {
        let mut manager = state.backend_manager.lock().await;
        let write_stats = manager.write_frame(zone_colors, layout.as_ref()).await;
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let push_us = micros_u32(push_start.elapsed());
    let output_done_us = micros_u32(frame_start.elapsed());

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }

    let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
    let timing_total_us = sample_us.saturating_add(push_us);
    let publish_stats = publish_frame_updates(
        state,
        recycled_frame,
        &AudioData::silence(),
        canvas,
        Some(surface),
        None,
        frame_num_u32,
        scene_snapshot.elapsed_ms,
        last_audio_level_update_ms,
        FrameTiming {
            producer_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us,
            push_us,
            total_us: timing_total_us,
            budget_us: scene_snapshot.budget_us,
        },
    );
    let publish_us = publish_stats.elapsed_us;
    let publish_done_us = micros_u32(frame_start.elapsed());
    let total_us = micros_u32(frame_start.elapsed());
    let overhead_us =
        total_us.saturating_sub(sample_us.saturating_add(push_us).saturating_add(publish_us));

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(LatestFrameMetrics {
            timestamp_ms: scene_snapshot.elapsed_ms,
            input_us: 0,
            producer_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us,
            push_us,
            postprocess_us: 0,
            publish_us,
            overhead_us,
            total_us,
            wake_late_us: 0,
            jitter_us: 0,
            reused_inputs: false,
            reused_canvas: false,
            retained_effect: false,
            retained_screen: false,
            composition_bypassed: false,
            logical_layer_count: 0,
            render_group_count: scene_snapshot.scene_runtime.active_render_group_count(),
            scene_active: scene_snapshot.scene_runtime.active_scene_id.is_some(),
            scene_transition_active: scene_snapshot.scene_runtime.active_transition.is_some(),
            full_frame_copy_count: publish_stats.full_frame_copy_count,
            full_frame_copy_bytes: publish_stats.full_frame_copy_bytes,
            output_errors: u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX),
            timeline: FrameTimeline {
                frame_token: scene_snapshot.frame_token,
                budget_us: scene_snapshot.budget_us,
                scene_snapshot_done_us,
                input_done_us: scene_snapshot_done_us,
                producer_done_us: scene_snapshot_done_us,
                composition_done_us: scene_snapshot_done_us,
                sample_done_us,
                output_done_us,
                publish_done_us,
                frame_done_us: total_us,
            },
        });
    }

    {
        let mut rl = state.render_loop.write().await;
        let _ = rl.frame_complete();
    }

    *sleep_black_pushed = true;
    Some(FrameExecution {
        next_wake: NextWake::Delay(SESSION_SLEEP_THROTTLE_SLEEP),
        next_skip_decision: SkipDecision::None,
    })
}

fn static_hold_canvas(width: u32, height: u32, color: [u8; 3]) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    if color != [0, 0, 0] {
        canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
    }
    canvas
}

fn static_surface(
    cache: &mut Option<CachedStaticSurface>,
    width: u32,
    height: u32,
    color: [u8; 3],
) -> PublishedSurface {
    let key = StaticSurfaceKey {
        width,
        height,
        color,
    };

    if let Some(cached) = cache.as_ref()
        && cached.key == key
    {
        return cached.surface.clone();
    }

    let surface =
        PublishedSurface::from_owned_canvas(static_hold_canvas(width, height, color), 0, 0);
    *cache = Some(CachedStaticSurface {
        key,
        surface: surface.clone(),
    });
    surface
}

fn should_idle_throttle(effect_running: bool, screen_capture_active: bool) -> bool {
    if effect_running || screen_capture_active {
        return false;
    }

    // The bus keeps the latest black frame/spectrum snapshot for late subscribers,
    // so internal or UI watch receivers should not force the daemon to keep
    // rendering when nothing is active.
    true
}

fn panic_payload_message(panic: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

/// Render one frame from the effect engine, falling back to a black canvas on error.
async fn render_effect_into(
    state: &RenderThreadState,
    expected_generation: u64,
    delta_secs: f32,
    audio: &AudioData,
    interaction: &InteractionData,
    screen: Option<&ScreenData>,
    target: &mut Canvas,
) {
    let mut engine = state.effect_engine.lock().await;
    let actual_generation = engine.scene_generation();

    if actual_generation != expected_generation {
        debug!(
            expected_generation,
            actual_generation, "deferred effect render until next frame after scene change"
        );
        if target.width() != state.canvas_width || target.height() != state.canvas_height {
            *target = Canvas::new(state.canvas_width, state.canvas_height);
        } else {
            target.clear();
        }
        return;
    }

    match engine.tick_with_inputs_into(delta_secs, audio, interaction, screen, target) {
        Ok(()) => {}
        Err(e) => {
            warn!(error = %e, "effect render failed, producing black canvas");
            if target.width() != state.canvas_width || target.height() != state.canvas_height {
                *target = Canvas::new(state.canvas_width, state.canvas_height);
            } else {
                target.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use hypercolor_core::engine::FpsTier;
    use hypercolor_core::input::ScreenData;
    use hypercolor_core::types::canvas::Rgba;
    use hypercolor_core::types::event::ZoneColors;

    use super::frame_io::{parse_sector_zone_id, screen_data_to_canvas};
    use super::{
        PRECISE_WAKE_GUARD, SkipDecision, advance_deadline, coarse_sleep_deadline, micros_u32,
        should_idle_throttle,
    };

    fn frame_stats(
        budget_exceeded: bool,
        consecutive_misses: u32,
    ) -> hypercolor_core::engine::FrameStats {
        hypercolor_core::engine::FrameStats {
            frame_time: Duration::from_millis(20),
            headroom: Duration::ZERO,
            budget_exceeded,
            ewma_frame_time: Duration::from_millis(18),
            tier: FpsTier::Full,
            consecutive_misses,
            frames_since_tier_change: 10,
        }
    }

    #[test]
    fn skip_decision_is_none_when_frame_is_within_budget() {
        let stats = frame_stats(false, 0);
        assert_eq!(SkipDecision::from_frame_stats(&stats), SkipDecision::None);
    }

    #[test]
    fn skip_decision_reuses_inputs_after_single_budget_miss() {
        let stats = frame_stats(true, 1);
        assert_eq!(
            SkipDecision::from_frame_stats(&stats),
            SkipDecision::ReuseInputs
        );
    }

    #[test]
    fn skip_decision_reuses_canvas_after_consecutive_misses() {
        let stats = frame_stats(true, 3);
        assert_eq!(
            SkipDecision::from_frame_stats(&stats),
            SkipDecision::ReuseCanvas
        );
    }

    #[test]
    fn idle_throttle_enabled_only_when_fully_idle() {
        assert!(should_idle_throttle(false, false));
    }

    #[test]
    fn idle_throttle_disabled_when_effect_running() {
        assert!(!should_idle_throttle(true, false));
    }

    #[test]
    fn idle_throttle_disabled_when_capture_enabled() {
        assert!(!should_idle_throttle(false, true));
    }

    #[test]
    fn micros_u32_saturates_large_duration() {
        let very_large = Duration::from_secs(u64::MAX);
        assert_eq!(micros_u32(very_large), u32::MAX);
    }

    #[test]
    fn advance_deadline_preserves_phase_when_scheduler_wakes_late() {
        let start = Instant::now();
        let late_now = start + Duration::from_millis(18);

        let next = advance_deadline(start, Duration::from_millis(16), late_now);

        assert_eq!(next, late_now);
    }

    #[test]
    fn advance_deadline_keeps_regular_cadence_when_on_time() {
        let start = Instant::now();
        let now = start + Duration::from_millis(8);

        let next = advance_deadline(start, Duration::from_millis(16), now);

        assert_eq!(next, start + Duration::from_millis(16));
    }

    #[test]
    fn coarse_sleep_deadline_uses_guard_band_when_there_is_headroom() {
        let now = Instant::now();
        let deadline = now + Duration::from_millis(16);

        let coarse = coarse_sleep_deadline(deadline, now).expect("guard band should apply");

        assert_eq!(coarse, deadline - PRECISE_WAKE_GUARD);
    }

    #[test]
    fn coarse_sleep_deadline_skips_sleep_when_deadline_is_inside_guard_band() {
        let now = Instant::now();
        let deadline = now + PRECISE_WAKE_GUARD / 2;

        assert!(coarse_sleep_deadline(deadline, now).is_none());
    }

    #[test]
    fn parse_sector_zone_id_parses_valid_ids() {
        assert_eq!(parse_sector_zone_id("screen:sector_0_0"), Some((0, 0)));
        assert_eq!(parse_sector_zone_id("screen:sector_12_5"), Some((12, 5)));
        assert_eq!(parse_sector_zone_id("zone_1"), None);
    }

    #[test]
    fn screen_data_to_canvas_maps_sector_colors() {
        let screen_data = ScreenData::from_zones(
            vec![
                ZoneColors {
                    zone_id: "screen:sector_0_0".to_owned(),
                    colors: vec![[255, 0, 0]],
                },
                ZoneColors {
                    zone_id: "screen:sector_0_1".to_owned(),
                    colors: vec![[0, 255, 0]],
                },
                ZoneColors {
                    zone_id: "screen:sector_1_0".to_owned(),
                    colors: vec![[0, 0, 255]],
                },
                ZoneColors {
                    zone_id: "screen:sector_1_1".to_owned(),
                    colors: vec![[255, 255, 255]],
                },
            ],
            2,
            2,
        );

        let canvas = screen_data_to_canvas(&screen_data, 4, 4).expect("canvas should build");
        assert_eq!(canvas.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(canvas.get_pixel(3, 0), Rgba::new(0, 255, 0, 255));
        assert_eq!(canvas.get_pixel(0, 3), Rgba::new(0, 0, 255, 255));
        assert_eq!(canvas.get_pixel(3, 3), Rgba::new(255, 255, 255, 255));
    }
}
