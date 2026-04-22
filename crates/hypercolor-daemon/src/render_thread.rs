//! Frame pipeline render thread — the heartbeat of Hypercolor.
//!
//! Spawns a dedicated OS thread with its own Tokio runtime that runs the core
//! render loop:
//!
//! ```text
//! loop {
//!     RenderLoop::tick()              // timing gate + FPS control
//!     compose active scene groups     // render groups → composed canvas
//!     SpatialEngine::sample()         // map pixels → LED colors
//!     BackendManager::write_frame()   // push to hardware
//!     HypercolorBus::publish()        // notify subscribers
//!     RenderLoop::frame_complete()    // measure + adapt FPS tier
//!     sleep_until(next_deadline)      // pace to target FPS
//! }
//! ```

mod composition_planner;
mod capture_demand;
mod frame_admission;
mod frame_composer;
mod frame_executor;
mod frame_io;
mod frame_policy;
mod frame_sampling;
mod frame_sources;
mod frame_throttle;
mod pipeline_driver;
mod pipeline_runtime;
mod producer_queue;
mod render_groups;
mod scene_dependency;
mod scene_snapshot;
mod scene_state;
#[doc(hidden)]
pub mod sparkleflinger;

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::info;

use self::pipeline_driver::run_pipeline;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::DiscoveryRuntime;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::OutputPowerState;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::config::RenderAccelerationMode;

const RENDER_RUNTIME_WORKERS: usize = 2;
const RENDER_RUNTIME_MAX_BLOCKING_THREADS: usize = 4;
const RENDER_RUNTIME_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(2);
const DEFAULT_RENDER_SURFACE_SLOTS: usize = 8;
const MAX_RENDER_SURFACE_SLOTS: usize = 12;

fn desired_render_surface_slots(canvas_receiver_count: usize) -> usize {
    DEFAULT_RENDER_SURFACE_SLOTS
        .saturating_add(canvas_receiver_count.saturating_mul(2))
        .min(MAX_RENDER_SURFACE_SLOTS)
}

/// Shared, atomically-updatable canvas dimensions.
///
/// Cloning shares the same underlying atomics so the render thread and
/// API handler see the same live values. Reads use `Relaxed` ordering —
/// the `SceneTransactionQueue` provides the actual synchronisation boundary.
#[derive(Clone)]
pub struct CanvasDims(Arc<(AtomicU32, AtomicU32)>);

impl CanvasDims {
    pub fn new(width: u32, height: u32) -> Self {
        Self(Arc::new((AtomicU32::new(width), AtomicU32::new(height))))
    }

    pub fn width(&self) -> u32 {
        self.0.0.load(Ordering::Relaxed)
    }

    pub fn height(&self) -> u32 {
        self.0.1.load(Ordering::Relaxed)
    }

    pub fn set(&self, width: u32, height: u32) {
        self.0.0.store(width, Ordering::Relaxed);
        self.0.1.store(height, Ordering::Relaxed);
    }
}

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
    /// Effect catalog used to resolve render-group assignments.
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,

    /// Device backend router — pushes colors to hardware.
    pub backend_manager: Arc<Mutex<BackendManager>>,

    /// Device registry — used for per-device render cadence decisions.
    pub device_registry: DeviceRegistry,

    /// Rolling render-performance snapshot shared with metrics endpoints.
    pub performance: Arc<RwLock<PerformanceTracker>>,

    /// Discovery/lifecycle runtime used to react to async device write failures.
    pub discovery_runtime: Option<DiscoveryRuntime>,

    /// System-wide event bus — frame data and timing events.
    pub event_bus: Arc<HypercolorBus>,

    /// Dedicated preview fanout for browser-facing canvas consumers.
    pub preview_runtime: Arc<PreviewRuntime>,

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

    /// Live render canvas dimensions (atomically updated on resize).
    pub canvas_dims: CanvasDims,

    /// Requested render acceleration mode for the pipeline.
    pub render_acceleration_mode: RenderAccelerationMode,

    /// Ceiling derived from user configuration before runtime admission.
    pub configured_max_fps_tier: FpsTier,
}

impl RenderThreadState {
    pub(crate) fn preview_canvas_receiver_count(&self) -> usize {
        self.event_bus.canvas_receiver_count()
    }

    pub(crate) fn scene_canvas_receiver_count(&self) -> usize {
        self.event_bus.scene_canvas_receiver_count()
    }

    pub(crate) fn published_canvas_receiver_count(&self) -> usize {
        self.preview_canvas_receiver_count()
            .saturating_add(self.scene_canvas_receiver_count())
    }
}

impl RenderThread {
    /// Spawn the render thread on a dedicated OS thread.
    ///
    /// The thread runs until `RenderLoop::tick()` returns `false`
    /// (i.e., the render loop has been stopped or paused).
    pub fn spawn(state: RenderThreadState) -> Self {
        Self::try_spawn(state).expect("render thread should spawn")
    }

    pub fn try_spawn(state: RenderThreadState) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
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
                let pipeline =
                    runtime.block_on(pipeline_runtime::PipelineRuntime::from_state(&state));
                match pipeline {
                    Ok(runtime_state) => {
                        let _ = ready_tx.send(Ok(()));
                        runtime.block_on(run_pipeline(state, runtime_state));
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .expect("render thread should spawn");
        ready_rx
            .recv()
            .context("render thread exited before startup completed")??;
        info!("render thread spawned");
        Ok(Self {
            join_handle: Some(join_handle),
        })
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

/// Saturating conversion between two monotonic instants expressed in microseconds.
fn micros_between(start: Instant, end: Instant) -> u32 {
    micros_u32(end.saturating_duration_since(start))
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

fn panic_payload_message(panic: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hypercolor_core::engine::FpsTier;
    use hypercolor_core::input::ScreenData;
    use hypercolor_core::types::canvas::Rgba;
    use hypercolor_core::types::event::ZoneColors;

    use super::frame_io::{parse_sector_zone_id, screen_data_to_canvas};
    use super::frame_policy::SkipDecision;
    use super::micros_u32;

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

        let mut sector_grid = Vec::new();
        let canvas = screen_data_to_canvas(&screen_data, 4, 4, &mut sector_grid)
            .expect("canvas should build");
        assert_eq!(canvas.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(canvas.get_pixel(3, 0), Rgba::new(0, 255, 0, 255));
        assert_eq!(canvas.get_pixel(0, 3), Rgba::new(0, 0, 255, 255));
        assert_eq!(canvas.get_pixel(3, 3), Rgba::new(255, 255, 255, 255));
    }
}
