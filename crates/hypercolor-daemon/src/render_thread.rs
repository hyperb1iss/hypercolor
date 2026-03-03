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

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::BackendManager;
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_core::types::event::{FrameData, FrameTiming, HypercolorEvent};

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

    let silence = AudioData::silence();
    let mut last_tick = Instant::now();

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

        let frame_start = Instant::now();
        let delta_secs = last_tick.elapsed().as_secs_f32();
        last_tick = frame_start;

        // ── Stage 1: Effect render → Canvas ─────────────────────────
        let render_start = Instant::now();
        let canvas = render_effect(&state, delta_secs, &silence).await;
        let render_us = micros_u32(render_start.elapsed());

        // ── Stage 2: Spatial sampling → ZoneColors ──────────────────
        let sample_start = Instant::now();
        let (zone_colors, layout) = {
            let spatial = state.spatial_engine.read().await;
            let colors = spatial.sample(&canvas);
            let layout = spatial.layout().clone();
            (colors, layout)
        };
        let sample_us = micros_u32(sample_start.elapsed());

        // ── Stage 3: Device push → hardware ─────────────────────────
        let push_start = Instant::now();
        let write_stats = {
            let mut manager = state.backend_manager.lock().await;
            manager.write_frame(&zone_colors, &layout).await
        };
        let push_us = micros_u32(push_start.elapsed());

        let total_us = micros_u32(frame_start.elapsed());

        // ── Stage 4: Publish to bus ─────────────────────────────────
        let (frame_number, elapsed_ms, budget_us) = {
            let rl = state.render_loop.read().await;
            (
                rl.frame_number(),
                millis_u32(rl.elapsed()),
                micros_u32(rl.target_interval()),
            )
        };

        let frame_num_u32 = u64_to_u32(frame_number);

        // Frame data via watch channel (latest-value, consumers skip stale).
        let frame_data = FrameData::new(zone_colors, frame_num_u32, elapsed_ms);
        let _ = state.event_bus.frame_sender().send(frame_data);

        // Timing event via broadcast channel.
        let timing = FrameTiming {
            render_us,
            sample_us,
            push_us,
            total_us,
            budget_us,
        };
        state.event_bus.publish(HypercolorEvent::FrameRendered {
            frame_number: frame_num_u32,
            timing,
        });

        // Log write errors.
        for err in &write_stats.errors {
            warn!(error = %err, "device write error");
        }

        trace!(
            frame = frame_number,
            render_us,
            sample_us,
            push_us,
            total_us,
            devices = write_stats.devices_written,
            leds = write_stats.total_leds,
            "frame complete"
        );

        // ── Timing: complete frame and sleep ────────────────────────
        let headroom = {
            let mut rl = state.render_loop.write().await;
            rl.frame_complete().map(|s| s.headroom).unwrap_or_default()
        };

        if !headroom.is_zero() {
            tokio::time::sleep(headroom).await;
        }
    }

    info!("render pipeline exited");
}

/// Render one frame from the effect engine, falling back to a black canvas on error.
async fn render_effect(state: &RenderThreadState, delta_secs: f32, audio: &AudioData) -> Canvas {
    let mut engine = state.effect_engine.lock().await;

    // TODO: Replace silence with real audio from the spectrum watch channel
    // once the audio input pipeline is implemented.
    match engine.tick(delta_secs, audio) {
        Ok(canvas) => canvas,
        Err(e) => {
            warn!(error = %e, "effect render failed, producing black canvas");
            Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
        }
    }
}
