//! Frame pipeline render thread — the heartbeat of Hypercolor.
//!
//! Spawns a tokio task that runs the core render loop:
//!
//! ```text
//! loop {
//!     RenderLoop::tick()              // timing gate + FPS control
//!     EffectEngine::tick()            // render effect → Canvas
//!     SpatialEngine::sample()         // map pixels → LED colors
//!     BackendManager::write_frame()   // push to hardware
//!     HypercolorBus::publish()        // notify subscribers
//!     RenderLoop::frame_complete()    // measure + adapt FPS tier
//!     sleep(headroom)                 // pace to target FPS
//! }
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace, warn};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::BackendManager;
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::engine::{FrameStats, RenderLoop};
use hypercolor_core::input::{InputData, InputManager, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, Rgba};
use hypercolor_core::types::event::{FrameData, FrameTiming, HypercolorEvent, SpectrumData};

// ── RenderThread ────────────────────────────────────────────────────────────

/// Handle to a running render thread.
///
/// Call [`shutdown`](Self::shutdown) to stop the thread gracefully.
/// The render loop must be stopped first (via `RenderLoop::stop()`) — the
/// thread will exit on the next `tick()` returning `false`.
pub struct RenderThread {
    join_handle: Option<JoinHandle<()>>,
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

    /// System-wide event bus — frame data and timing events.
    pub event_bus: Arc<HypercolorBus>,

    /// Render loop — frame timing, FPS control, tier transitions.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Input orchestrator for audio and screen capture sampling.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Target render canvas width.
    pub canvas_width: u32,

    /// Target render canvas height.
    pub canvas_height: u32,

    /// Whether screen capture input is enabled in daemon configuration.
    pub screen_capture_enabled: bool,
}

impl RenderThread {
    /// Spawn the render thread as a tokio task.
    ///
    /// The thread runs until `RenderLoop::tick()` returns `false`
    /// (i.e., the render loop has been stopped or paused).
    pub fn spawn(state: RenderThreadState) -> Self {
        let join_handle = tokio::spawn(run_pipeline(state));
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
            handle.await?;
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
    headroom: Duration,
    next_skip_decision: SkipDecision,
}

/// Sleep duration when the pipeline is fully idle.
const IDLE_THROTTLE_SLEEP: Duration = Duration::from_millis(120);

#[derive(Clone)]
struct FrameInputs {
    audio: AudioData,
    screen_canvas: Option<Canvas>,
}

impl FrameInputs {
    fn silence() -> Self {
        Self {
            audio: AudioData::silence(),
            screen_canvas: None,
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
    info!("render pipeline started");

    let mut cached_inputs = FrameInputs::silence();
    let mut cached_canvas: Option<Canvas> = None;
    let mut skip_decision = SkipDecision::None;
    let mut last_tick = Instant::now();
    let mut idle_black_pushed = false;

    loop {
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
                // Paused — yield and retry.
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }

            debug!("render loop not running, exiting pipeline");
            break;
        }

        let frame = execute_frame(
            &state,
            skip_decision,
            &mut cached_inputs,
            &mut cached_canvas,
            &mut last_tick,
            &mut idle_black_pushed,
        )
        .await;
        skip_decision = frame.next_skip_decision;

        if !frame.headroom.is_zero() {
            tokio::time::sleep(frame.headroom).await;
        }
    }

    info!("render pipeline exited");
}

async fn execute_frame(
    state: &RenderThreadState,
    skip_decision: SkipDecision,
    cached_inputs: &mut FrameInputs,
    cached_canvas: &mut Option<Canvas>,
    last_tick: &mut Instant,
    idle_black_pushed: &mut bool,
) -> FrameExecution {
    let frame_start = Instant::now();
    let delta_secs = last_tick.elapsed().as_secs_f32();
    *last_tick = frame_start;

    let effect_running = current_effect_running(state).await;
    if let Some(frame) = maybe_idle_throttle(state, effect_running, idle_black_pushed).await {
        return frame;
    }

    // ── Stage 1: Input sampling ─────────────────────────────────
    let input_start = Instant::now();
    let inputs = match skip_decision {
        SkipDecision::None => {
            *cached_inputs = sample_inputs(state).await;
            cached_inputs.clone()
        }
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas => cached_inputs.clone(),
    };
    let input_us = micros_u32(input_start.elapsed());

    // ── Stage 2: Effect render → Canvas ─────────────────────────
    let render_start = Instant::now();
    let canvas = if let (SkipDecision::ReuseCanvas, Some(previous)) =
        (skip_decision, cached_canvas.as_ref())
    {
        previous.clone()
    } else if let Some(screen_canvas) = inputs.screen_canvas.clone() {
        *cached_canvas = Some(screen_canvas.clone());
        screen_canvas
    } else {
        let rendered = render_effect(state, delta_secs, &inputs.audio).await;
        *cached_canvas = Some(rendered.clone());
        rendered
    };
    let render_us = micros_u32(render_start.elapsed());

    // ── Stage 3: Spatial sampling → ZoneColors ──────────────────
    let sample_start = Instant::now();
    let (zone_colors, layout) = {
        let spatial = state.spatial_engine.read().await;
        let colors = spatial.sample(&canvas);
        let layout = spatial.layout().clone();
        (colors, layout)
    };
    let sample_us = micros_u32(sample_start.elapsed());

    // ── Stage 4: Device push → hardware ─────────────────────────
    let push_start = Instant::now();
    let write_stats = {
        let mut manager = state.backend_manager.lock().await;
        manager.write_frame(&zone_colors, &layout).await
    };
    let push_us = micros_u32(push_start.elapsed());

    // ── Stage 5: Publish to bus ─────────────────────────────────
    let (frame_number, elapsed_ms, budget_us) = frame_snapshot(state).await;
    let frame_num_u32 = u64_to_u32(frame_number);
    let total_us = micros_u32(frame_start.elapsed());
    let publish_us = publish_frame_updates(
        state,
        zone_colors,
        &inputs.audio,
        &canvas,
        frame_num_u32,
        elapsed_ms,
        FrameTiming {
            render_us,
            sample_us,
            push_us,
            total_us,
            budget_us,
        },
    );

    for err in &write_stats.errors {
        warn!(error = %err, "device write error");
    }

    trace!(
        frame = frame_number,
        input_us,
        render_us,
        sample_us,
        push_us,
        publish_us,
        total_us,
        ?skip_decision,
        devices = write_stats.devices_written,
        leds = write_stats.total_leds,
        "frame complete"
    );

    let (headroom, next_skip_decision) = {
        let mut rl = state.render_loop.write().await;
        match rl.frame_complete() {
            Some(frame_stats) => (
                frame_stats.headroom,
                SkipDecision::from_frame_stats(&frame_stats),
            ),
            None => (Duration::ZERO, SkipDecision::None),
        }
    };

    if !effect_running {
        *idle_black_pushed = true;
    }

    FrameExecution {
        headroom,
        next_skip_decision,
    }
}

async fn current_effect_running(state: &RenderThreadState) -> bool {
    let engine = state.effect_engine.lock().await;
    engine.is_running()
}

async fn maybe_idle_throttle(
    state: &RenderThreadState,
    effect_running: bool,
    idle_black_pushed: &mut bool,
) -> Option<FrameExecution> {
    let can_idle_throttle = should_idle_throttle(
        effect_running,
        state.screen_capture_enabled,
        state.event_bus.frame_receiver_count(),
        state.event_bus.canvas_receiver_count(),
        state.event_bus.spectrum_receiver_count(),
    );

    if effect_running {
        *idle_black_pushed = false;
        return None;
    }

    if can_idle_throttle && *idle_black_pushed {
        {
            let mut rl = state.render_loop.write().await;
            let _ = rl.frame_complete();
        }

        return Some(FrameExecution {
            headroom: IDLE_THROTTLE_SLEEP,
            next_skip_decision: SkipDecision::None,
        });
    }

    None
}

fn should_idle_throttle(
    effect_running: bool,
    screen_capture_enabled: bool,
    frame_receivers: usize,
    canvas_receivers: usize,
    spectrum_receivers: usize,
) -> bool {
    if effect_running || screen_capture_enabled {
        return false;
    }

    frame_receivers == 0 && canvas_receivers == 0 && spectrum_receivers == 0
}

/// Sample current input values for the frame.
async fn sample_inputs(state: &RenderThreadState) -> FrameInputs {
    let samples = {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.sample_all()
    };

    let mut audio = AudioData::silence();
    let mut screen_data: Option<ScreenData> = None;
    for sample in samples {
        match sample {
            InputData::Audio(snapshot) => audio = snapshot,
            InputData::Screen(snapshot) => screen_data = Some(snapshot),
            InputData::None => {}
        }
    }

    let screen_canvas = screen_data
        .as_ref()
        .and_then(|data| screen_data_to_canvas(data, state.canvas_width, state.canvas_height));

    FrameInputs {
        audio,
        screen_canvas,
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

fn publish_frame_updates(
    state: &RenderThreadState,
    zone_colors: Vec<hypercolor_core::types::event::ZoneColors>,
    audio: &AudioData,
    canvas: &Canvas,
    frame_number: u32,
    elapsed_ms: u32,
    timing: FrameTiming,
) -> u32 {
    let publish_start = Instant::now();
    let frame_data = FrameData::new(zone_colors, frame_number, elapsed_ms);
    let _ = state.event_bus.frame_sender().send(frame_data);
    let _ = state
        .event_bus
        .spectrum_sender()
        .send(spectrum_from_audio(audio, elapsed_ms));
    let _ = state
        .event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(canvas, frame_number, elapsed_ms));
    state.event_bus.publish(HypercolorEvent::FrameRendered {
        frame_number,
        timing,
    });
    micros_u32(publish_start.elapsed())
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
async fn render_effect(state: &RenderThreadState, delta_secs: f32, audio: &AudioData) -> Canvas {
    let mut engine = state.effect_engine.lock().await;

    match engine.tick(delta_secs, audio) {
        Ok(canvas) => canvas,
        Err(e) => {
            warn!(error = %e, "effect render failed, producing black canvas");
            Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hypercolor_core::engine::FpsTier;
    use hypercolor_core::input::ScreenData;
    use hypercolor_core::types::canvas::Rgba;
    use hypercolor_core::types::event::ZoneColors;

    use super::{
        SkipDecision, micros_u32, parse_sector_zone_id, screen_data_to_canvas, should_idle_throttle,
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
        assert!(should_idle_throttle(false, false, 0, 0, 0));
    }

    #[test]
    fn idle_throttle_disabled_when_effect_running() {
        assert!(!should_idle_throttle(true, false, 0, 0, 0));
    }

    #[test]
    fn idle_throttle_disabled_when_capture_enabled() {
        assert!(!should_idle_throttle(false, true, 0, 0, 0));
    }

    #[test]
    fn idle_throttle_disabled_when_stream_has_subscribers() {
        assert!(!should_idle_throttle(false, false, 1, 0, 0));
        assert!(!should_idle_throttle(false, false, 0, 1, 0));
        assert!(!should_idle_throttle(false, false, 0, 0, 1));
    }

    #[test]
    fn micros_u32_saturates_large_duration() {
        let very_large = Duration::from_secs(u64::MAX);
        assert_eq!(micros_u32(very_large), u32::MAX);
    }

    #[test]
    fn parse_sector_zone_id_parses_valid_ids() {
        assert_eq!(parse_sector_zone_id("screen:sector_0_0"), Some((0, 0)));
        assert_eq!(parse_sector_zone_id("screen:sector_12_5"), Some((12, 5)));
        assert_eq!(parse_sector_zone_id("zone_1"), None);
    }

    #[test]
    fn screen_data_to_canvas_maps_sector_colors() {
        let screen_data = ScreenData {
            zone_colors: vec![
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
        };

        let canvas = screen_data_to_canvas(&screen_data, 4, 4).expect("canvas should build");
        assert_eq!(canvas.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(canvas.get_pixel(3, 0), Rgba::new(0, 255, 0, 255));
        assert_eq!(canvas.get_pixel(0, 3), Rgba::new(0, 0, 255, 255));
        assert_eq!(canvas.get_pixel(3, 3), Rgba::new(255, 255, 255, 255));
    }
}
