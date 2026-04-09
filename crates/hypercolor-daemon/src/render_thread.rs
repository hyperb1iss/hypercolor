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

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::{debug, info, trace, warn};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::BackendManager;
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::engine::{FrameStats, RenderLoop};
use hypercolor_core::input::{InputData, InputManager, InteractionData, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, Rgba};
use hypercolor_core::types::event::{FrameData, FrameTiming, HypercolorEvent, SpectrumData};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::session::OffOutputBehavior;

use crate::device_settings::DeviceSettingsStore;
use crate::discovery::{DiscoveryRuntime, handle_async_write_failures};
use crate::performance::{LatestFrameMetrics, PerformanceTracker};
use crate::session::OutputPowerState;

const RENDER_RUNTIME_WORKERS: usize = 2;
const RENDER_RUNTIME_MAX_BLOCKING_THREADS: usize = 4;
const RENDER_RUNTIME_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(2);

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

    /// Input orchestrator for audio and screen capture sampling.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Session policy output state (brightness scale + sleep flag).
    pub power_state: watch::Receiver<OutputPowerState>,

    /// Persisted global and per-device output settings.
    pub device_settings: Arc<RwLock<DeviceSettingsStore>>,

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

/// Result of executing one frame through the pipeline stages.
struct FrameExecution {
    next_wake: NextWake,
    next_skip_decision: SkipDecision,
}

struct PublishFrameStats {
    elapsed_us: u32,
    full_frame_copy_count: u32,
    full_frame_copy_bytes: u32,
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

struct FrameInputs {
    audio: AudioData,
    interaction: InteractionData,
    screen_data: Option<ScreenData>,
    screen_canvas: Option<Canvas>,
    screen_preview_canvas: Option<Canvas>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EffectDemand {
    effect_running: bool,
    audio_capture_active: bool,
    screen_capture_active: bool,
}

impl FrameInputs {
    fn silence() -> Self {
        Self {
            audio: AudioData::silence(),
            interaction: InteractionData::default(),
            screen_data: None,
            screen_canvas: None,
            screen_preview_canvas: None,
        }
    }
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

    let mut cached_inputs = FrameInputs::silence();
    let mut cached_canvas: Option<Canvas> = None;
    let mut recycled_frame = FrameData::empty();
    let mut skip_decision = SkipDecision::None;
    let mut last_tick = Instant::now();
    let mut idle_black_pushed = false;
    let mut sleep_black_pushed = false;
    let mut next_frame_at = Instant::now();
    let mut last_audio_level_update_ms = None;
    let mut last_audio_capture_active = None;
    let mut last_screen_capture_active = None;

    loop {
        let scheduled_start = next_frame_at;
        if scheduled_start > Instant::now() {
            tokio::time::sleep_until(tokio::time::Instant::from_std(scheduled_start)).await;
        }

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
                reconcile_audio_capture(&state, false, &mut last_audio_capture_active).await;
                reconcile_screen_capture(&state, false, &mut last_screen_capture_active).await;
                // Paused — yield and retry.
                next_frame_at = Instant::now();
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }

            reconcile_audio_capture(&state, false, &mut last_audio_capture_active).await;
            reconcile_screen_capture(&state, false, &mut last_screen_capture_active).await;
            debug!("render loop not running, exiting pipeline");
            break;
        }

        let frame = execute_frame(
            &state,
            scheduled_start,
            skip_decision,
            &mut cached_inputs,
            &mut cached_canvas,
            &mut recycled_frame,
            &mut last_tick,
            &mut idle_black_pushed,
            &mut sleep_black_pushed,
            &mut last_audio_level_update_ms,
            &mut last_audio_capture_active,
            &mut last_screen_capture_active,
        )
        .await;
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

#[expect(
    clippy::too_many_lines,
    reason = "the frame executor keeps the render pipeline stages in one place so timing and ordering stay obvious"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "frame execution needs the live render state, caches, and throttle flags together"
)]
async fn execute_frame(
    state: &RenderThreadState,
    scheduled_start: Instant,
    skip_decision: SkipDecision,
    cached_inputs: &mut FrameInputs,
    cached_canvas: &mut Option<Canvas>,
    recycled_frame: &mut FrameData,
    last_tick: &mut Instant,
    idle_black_pushed: &mut bool,
    sleep_black_pushed: &mut bool,
    last_audio_level_update_ms: &mut Option<u32>,
    last_audio_capture_active: &mut Option<bool>,
    last_screen_capture_active: &mut Option<bool>,
) -> FrameExecution {
    let frame_start = Instant::now();
    let frame_interval = frame_start.saturating_duration_since(*last_tick);
    let delta_secs = frame_interval.as_secs_f32();
    *last_tick = frame_start;
    let frame_interval_us = micros_u32(frame_interval);
    let wake_late_us = micros_u32(frame_start.saturating_duration_since(scheduled_start));
    let reused_inputs = matches!(
        skip_decision,
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas
    );
    let reused_canvas = matches!(skip_decision, SkipDecision::ReuseCanvas);

    let output_power = *state.power_state.borrow();
    let effect_demand = current_effect_demand(state).await;
    reconcile_audio_capture(
        state,
        !output_power.sleeping && effect_demand.audio_capture_active,
        last_audio_capture_active,
    )
    .await;
    reconcile_screen_capture(
        state,
        !output_power.sleeping && effect_demand.screen_capture_active,
        last_screen_capture_active,
    )
    .await;
    if let Some(frame) = maybe_sleep_throttle(
        state,
        output_power,
        recycled_frame,
        sleep_black_pushed,
        last_audio_level_update_ms,
    )
    .await
    {
        return frame;
    }

    if let Some(frame) = maybe_idle_throttle(
        state,
        effect_demand.effect_running,
        effect_demand.screen_capture_active,
        idle_black_pushed,
    )
    .await
    {
        return frame;
    }

    // ── Stage 1: Input sampling ─────────────────────────────────
    let input_start = Instant::now();
    let inputs = match skip_decision {
        SkipDecision::None => {
            *cached_inputs = sample_inputs(state, delta_secs).await;
            &*cached_inputs
        }
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas => &*cached_inputs,
    };
    let input_us = micros_u32(input_start.elapsed());

    // ── Stage 2: Effect render → Canvas ─────────────────────────
    let (canvas, render_us) = resolve_frame_canvas(
        state,
        skip_decision,
        effect_demand.effect_running,
        inputs,
        cached_canvas,
        delta_secs,
    )
    .await;

    // ── Stage 3: Spatial sampling → ZoneColors ──────────────────
    let sample_start = Instant::now();
    let (zone_colors, layout) = {
        let spatial = state.spatial_engine.read().await;
        spatial.sample_into(&canvas, &mut recycled_frame.zones);
        let layout = spatial.layout();
        (&recycled_frame.zones, layout)
    };
    let sample_us = micros_u32(sample_start.elapsed());

    // ── Stage 4: Device push → hardware ─────────────────────────
    let push_start = Instant::now();
    let (write_stats, async_failures) = {
        let mut manager = state.backend_manager.lock().await;
        let write_stats = manager
            .write_frame_with_brightness(
                zone_colors,
                layout.as_ref(),
                output_power.effective_brightness(),
                None,
            )
            .await;
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let push_us = micros_u32(push_start.elapsed());

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }

    let postprocess_us = 0;
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;

    // ── Stage 5: Publish to bus ─────────────────────────────────
    let (frame_number, elapsed_ms, budget_us) = frame_snapshot(state).await;
    let frame_num_u32 = u64_to_u32(frame_number);
    let timing_total_us = micros_u32(frame_start.elapsed());
    let publish_stats = publish_frame_updates(
        state,
        recycled_frame,
        &inputs.audio,
        canvas,
        inputs.screen_preview_canvas.clone(),
        frame_num_u32,
        elapsed_ms,
        last_audio_level_update_ms,
        FrameTiming {
            render_us,
            sample_us,
            push_us,
            total_us: timing_total_us,
            budget_us,
        },
    );
    let publish_us = publish_stats.elapsed_us;
    full_frame_copy_count =
        full_frame_copy_count.saturating_add(publish_stats.full_frame_copy_count);
    full_frame_copy_bytes =
        full_frame_copy_bytes.saturating_add(publish_stats.full_frame_copy_bytes);
    let total_us = micros_u32(frame_start.elapsed());
    let known_stage_us = input_us
        .saturating_add(render_us)
        .saturating_add(sample_us)
        .saturating_add(push_us)
        .saturating_add(postprocess_us)
        .saturating_add(publish_us);
    let overhead_us = total_us.saturating_sub(known_stage_us);
    let jitter_us = frame_interval_us.abs_diff(budget_us);

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(LatestFrameMetrics {
            timestamp_ms: elapsed_ms,
            input_us,
            render_us,
            sample_us,
            push_us,
            postprocess_us,
            publish_us,
            overhead_us,
            total_us,
            wake_late_us,
            jitter_us,
            reused_inputs,
            reused_canvas,
            full_frame_copy_count,
            full_frame_copy_bytes,
            output_errors: u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX),
        });
    }

    for err in &write_stats.errors {
        warn!(error = %err, "device write error");
    }

    trace!(
        frame = frame_number,
        frame_interval_us,
        wake_late_us,
        jitter_us,
        input_us,
        render_us,
        sample_us,
        push_us,
        postprocess_us,
        publish_us,
        overhead_us,
        total_us,
        reused_inputs,
        reused_canvas,
        full_frame_copy_count,
        full_frame_copy_bytes,
        devices = write_stats.devices_written,
        leds = write_stats.total_leds,
        "frame complete"
    );

    let (next_wake, next_skip_decision) = {
        let mut rl = state.render_loop.write().await;
        match rl.frame_complete() {
            Some(frame_stats) => (
                NextWake::Interval(rl.target_interval()),
                SkipDecision::from_frame_stats(&frame_stats),
            ),
            None => (NextWake::Delay(Duration::ZERO), SkipDecision::None),
        }
    };

    if !effect_demand.effect_running {
        *idle_black_pushed = true;
    }

    FrameExecution {
        next_wake,
        next_skip_decision,
    }
}

async fn resolve_frame_canvas(
    state: &RenderThreadState,
    skip_decision: SkipDecision,
    effect_running: bool,
    inputs: &FrameInputs,
    cached_canvas: &mut Option<Canvas>,
    delta_secs: f32,
) -> (Canvas, u32) {
    let render_start = Instant::now();
    let canvas = if let (SkipDecision::ReuseCanvas, Some(previous)) =
        (skip_decision, cached_canvas.as_ref())
    {
        previous.clone()
    } else if !effect_running && let Some(screen_canvas) = inputs.screen_canvas.clone() {
        *cached_canvas = Some(screen_canvas.clone());
        screen_canvas
    } else {
        let mut rendered = cached_canvas
            .take()
            .filter(|canvas| {
                canvas.width() == state.canvas_width && canvas.height() == state.canvas_height
            })
            .unwrap_or_else(|| Canvas::new(state.canvas_width, state.canvas_height));
        render_effect_into(
            state,
            delta_secs,
            &inputs.audio,
            &inputs.interaction,
            inputs.screen_data.as_ref(),
            &mut rendered,
        )
        .await;
        *cached_canvas = Some(rendered.clone());
        rendered
    };
    (canvas, micros_u32(render_start.elapsed()))
}

async fn current_effect_demand(state: &RenderThreadState) -> EffectDemand {
    let engine = state.effect_engine.lock().await;
    let effect_running = engine.is_running();
    let audio_capture_active = effect_running
        && engine
            .active_metadata()
            .is_some_and(|meta| meta.audio_reactive);
    let screen_capture_active = (effect_running
        && engine
            .active_metadata()
            .is_some_and(|meta| meta.screen_reactive))
        || (!effect_running && state.screen_capture_configured);
    EffectDemand {
        effect_running,
        audio_capture_active,
        screen_capture_active,
    }
}

async fn reconcile_audio_capture(
    state: &RenderThreadState,
    desired_active: bool,
    last_audio_capture_active: &mut Option<bool>,
) {
    if last_audio_capture_active.is_some_and(|previous| previous == desired_active) {
        return;
    }

    let result = {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.set_audio_capture_active(desired_active)
    };

    match result {
        Ok(()) => {
            *last_audio_capture_active = Some(desired_active);
        }
        Err(error) => {
            warn!(
                desired_active,
                %error,
                "Failed to update audio capture demand"
            );
        }
    }
}

async fn reconcile_screen_capture(
    state: &RenderThreadState,
    desired_active: bool,
    last_screen_capture_active: &mut Option<bool>,
) {
    if last_screen_capture_active.is_some_and(|previous| previous == desired_active) {
        return;
    }

    let result = {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.set_screen_capture_active(desired_active)
    };

    match result {
        Ok(()) => {
            *last_screen_capture_active = Some(desired_active);
        }
        Err(error) => {
            warn!(
                desired_active,
                %error,
                "Failed to update screen capture demand"
            );
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
    power_state: OutputPowerState,
    recycled_frame: &mut FrameData,
    sleep_black_pushed: &mut bool,
    last_audio_level_update_ms: &mut Option<u32>,
) -> Option<FrameExecution> {
    if !power_state.sleeping {
        *sleep_black_pushed = false;
        return None;
    }

    let frame_start = Instant::now();

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
        let (frame_number, elapsed_ms, budget_us) = frame_snapshot(state).await;
        let frame_num_u32 = u64_to_u32(frame_number);
        let publish_stats = publish_frame_updates(
            state,
            recycled_frame,
            &AudioData::silence(),
            Canvas::new(state.canvas_width, state.canvas_height),
            None,
            frame_num_u32,
            elapsed_ms,
            last_audio_level_update_ms,
            FrameTiming {
                render_us: 0,
                sample_us: 0,
                push_us: 0,
                total_us: 0,
                budget_us,
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

    let canvas = static_hold_canvas(
        state.canvas_width,
        state.canvas_height,
        power_state.off_output_color,
    );
    let sample_start = Instant::now();
    let (zone_colors, layout) = {
        let spatial = state.spatial_engine.read().await;
        spatial.sample_into(&canvas, &mut recycled_frame.zones);
        let layout = spatial.layout();
        (&recycled_frame.zones, layout)
    };
    let sample_us = micros_u32(sample_start.elapsed());

    let push_start = Instant::now();
    let (write_stats, async_failures) = {
        let mut manager = state.backend_manager.lock().await;
        let write_stats = manager.write_frame(zone_colors, layout.as_ref()).await;
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let push_us = micros_u32(push_start.elapsed());

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }

    let (frame_number, elapsed_ms, budget_us) = frame_snapshot(state).await;
    let frame_num_u32 = u64_to_u32(frame_number);
    let timing_total_us = sample_us.saturating_add(push_us);
    let publish_stats = publish_frame_updates(
        state,
        recycled_frame,
        &AudioData::silence(),
        canvas,
        None,
        frame_num_u32,
        elapsed_ms,
        last_audio_level_update_ms,
        FrameTiming {
            render_us: 0,
            sample_us,
            push_us,
            total_us: timing_total_us,
            budget_us,
        },
    );
    let publish_us = publish_stats.elapsed_us;
    let total_us = micros_u32(frame_start.elapsed());
    let overhead_us =
        total_us.saturating_sub(sample_us.saturating_add(push_us).saturating_add(publish_us));

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(LatestFrameMetrics {
            timestamp_ms: elapsed_ms,
            input_us: 0,
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
            full_frame_copy_count: publish_stats.full_frame_copy_count,
            full_frame_copy_bytes: publish_stats.full_frame_copy_bytes,
            output_errors: u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX),
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

/// Sample current input values for the frame.
async fn sample_inputs(state: &RenderThreadState, delta_secs: f32) -> FrameInputs {
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
    for sample in samples {
        match sample {
            InputData::Audio(snapshot) => audio = snapshot,
            InputData::Interaction(snapshot) => interaction = snapshot,
            InputData::Screen(snapshot) => screen_data = Some(snapshot),
            InputData::None => {}
        }
    }

    let screen_canvas = screen_data
        .as_ref()
        .and_then(|data| screen_data_to_canvas(data, state.canvas_width, state.canvas_height));
    let screen_preview_canvas = screen_data
        .as_ref()
        .and_then(|data| data.canvas_downscale.clone());

    FrameInputs {
        audio,
        interaction,
        screen_data,
        screen_canvas,
        screen_preview_canvas,
    }
}

async fn frame_snapshot(state: &RenderThreadState) -> (u64, u32, u32) {
    let render_loop = state.render_loop.read().await;
    (
        render_loop.frame_number(),
        millis_u32(render_loop.elapsed()),
        micros_u32(render_loop.target_interval()),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "frame publishing needs state + all frame components"
)]
fn publish_frame_updates(
    state: &RenderThreadState,
    recycled_frame: &mut FrameData,
    audio: &AudioData,
    canvas: Canvas,
    screen_preview_canvas: Option<Canvas>,
    frame_number: u32,
    elapsed_ms: u32,
    last_audio_level_update_ms: &mut Option<u32>,
    timing: FrameTiming,
) -> PublishFrameStats {
    let publish_start = Instant::now();
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;
    recycled_frame.frame_number = frame_number;
    recycled_frame.timestamp_ms = elapsed_ms;
    let published_frame = FrameData::new(
        std::mem::take(&mut recycled_frame.zones),
        frame_number,
        elapsed_ms,
    );
    *recycled_frame = state.event_bus.frame_sender().send_replace(published_frame);
    let _ = state
        .event_bus
        .spectrum_sender()
        .send(spectrum_from_audio(audio, elapsed_ms));
    maybe_publish_audio_level_event(state, audio, elapsed_ms, last_audio_level_update_ms);
    let canvas_rgba_len = usize_to_u32(canvas.rgba_len());
    let (canvas_frame, canvas_copied) =
        CanvasFrame::from_owned_canvas_with_copy_info(canvas, frame_number, elapsed_ms);
    if canvas_copied {
        full_frame_copy_count = full_frame_copy_count.saturating_add(1);
        full_frame_copy_bytes = full_frame_copy_bytes.saturating_add(canvas_rgba_len);
    }
    let _ = state.event_bus.canvas_sender().send(canvas_frame);
    let screen_frame = if let Some(canvas) = screen_preview_canvas {
        let screen_rgba_len = usize_to_u32(canvas.rgba_len());
        let (frame, copied) =
            CanvasFrame::from_owned_canvas_with_copy_info(canvas, frame_number, elapsed_ms);
        if copied {
            full_frame_copy_count = full_frame_copy_count.saturating_add(1);
            full_frame_copy_bytes = full_frame_copy_bytes.saturating_add(screen_rgba_len);
        }
        frame
    } else {
        CanvasFrame::empty()
    };
    let _ = state.event_bus.screen_canvas_sender().send(screen_frame);
    state.event_bus.publish(HypercolorEvent::FrameRendered {
        frame_number,
        timing,
    });
    PublishFrameStats {
        elapsed_us: micros_u32(publish_start.elapsed()),
        full_frame_copy_count,
        full_frame_copy_bytes,
    }
}

fn maybe_publish_audio_level_event(
    state: &RenderThreadState,
    audio: &AudioData,
    elapsed_ms: u32,
    last_audio_level_update_ms: &mut Option<u32>,
) {
    if last_audio_level_update_ms.is_some_and(|last_sent| {
        elapsed_ms.saturating_sub(last_sent) < AUDIO_LEVEL_EVENT_INTERVAL_MS
    }) {
        return;
    }

    *last_audio_level_update_ms = Some(elapsed_ms);
    state.event_bus.publish(HypercolorEvent::AudioLevelUpdate {
        level: audio.rms_level,
        bass: audio.bass(),
        mid: audio.mid(),
        treble: audio.treble(),
        beat: audio.beat_detected,
    });
}

fn spectrum_from_audio(audio: &AudioData, timestamp_ms: u32) -> SpectrumData {
    SpectrumData {
        timestamp_ms,
        level: audio.rms_level,
        bass: audio.bass(),
        mid: audio.mid(),
        treble: audio.treble(),
        beat: audio.beat_detected,
        beat_confidence: audio.beat_confidence,
        bpm: if audio.bpm > 0.0 {
            Some(audio.bpm)
        } else {
            None
        },
        bins: audio.spectrum.clone(),
    }
}

fn screen_data_to_canvas(
    screen_data: &ScreenData,
    canvas_width: u32,
    canvas_height: u32,
) -> Option<Canvas> {
    if let Some(canvas) = &screen_data.canvas_downscale
        && canvas.width() == canvas_width
        && canvas.height() == canvas_height
    {
        return Some(canvas.clone());
    }

    if canvas_width == 0 || canvas_height == 0 {
        return None;
    }

    let mut sectors: Vec<(u32, u32, [u8; 3])> = Vec::new();
    let mut max_row = 0_u32;
    let mut max_col = 0_u32;

    for zone in &screen_data.zone_colors {
        let Some((row, col)) = parse_sector_zone_id(&zone.zone_id) else {
            continue;
        };
        let color = zone.colors.first().copied().unwrap_or([0, 0, 0]);
        max_row = max_row.max(row);
        max_col = max_col.max(col);
        sectors.push((row, col, color));
    }

    if sectors.is_empty() {
        return None;
    }

    let rows = max_row.saturating_add(1);
    let cols = max_col.saturating_add(1);
    let cell_count = usize::try_from(rows).ok().and_then(|row_count| {
        usize::try_from(cols)
            .ok()
            .and_then(|col_count| row_count.checked_mul(col_count))
    })?;

    let mut grid = vec![[0, 0, 0]; cell_count];
    for (row, col, color) in sectors {
        let idx_u64 = u64::from(row)
            .checked_mul(u64::from(cols))
            .and_then(|base| base.checked_add(u64::from(col)))?;
        let idx = usize::try_from(idx_u64).ok()?;
        if let Some(cell) = grid.get_mut(idx) {
            *cell = color;
        }
    }

    let mut canvas = Canvas::new(canvas_width, canvas_height);
    let width_u64 = u64::from(canvas_width);
    let height_u64 = u64::from(canvas_height);
    let grid_cols_u64 = u64::from(cols);
    let grid_rows_u64 = u64::from(rows);

    for y in 0..canvas_height {
        let mapped_row_u64 = (u64::from(y) * grid_rows_u64) / height_u64;
        let row = u32::try_from(mapped_row_u64)
            .unwrap_or_default()
            .min(rows.saturating_sub(1));

        for x in 0..canvas_width {
            let mapped_col_u64 = (u64::from(x) * grid_cols_u64) / width_u64;
            let col = u32::try_from(mapped_col_u64)
                .unwrap_or_default()
                .min(cols.saturating_sub(1));

            let idx_u64 = u64::from(row)
                .checked_mul(grid_cols_u64)
                .and_then(|base| base.checked_add(u64::from(col)))
                .unwrap_or_default();
            let idx = usize::try_from(idx_u64).unwrap_or_default();
            let [r, g, b] = grid.get(idx).copied().unwrap_or([0, 0, 0]);
            canvas.set_pixel(x, y, Rgba::new(r, g, b, 255));
        }
    }

    Some(canvas)
}

fn parse_sector_zone_id(zone_id: &str) -> Option<(u32, u32)> {
    let coords = zone_id.strip_prefix("screen:sector_")?;
    let (row_raw, col_raw) = coords.split_once('_')?;
    let row = row_raw.parse().ok()?;
    let col = col_raw.parse().ok()?;
    Some((row, col))
}

/// Render one frame from the effect engine, falling back to a black canvas on error.
async fn render_effect_into(
    state: &RenderThreadState,
    delta_secs: f32,
    audio: &AudioData,
    interaction: &InteractionData,
    screen: Option<&ScreenData>,
    target: &mut Canvas,
) {
    let mut engine = state.effect_engine.lock().await;

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

    use super::{
        SkipDecision, advance_deadline, micros_u32, parse_sector_zone_id, screen_data_to_canvas,
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
