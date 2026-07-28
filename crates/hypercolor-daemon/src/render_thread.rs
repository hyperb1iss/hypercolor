//! Frame pipeline render thread — the heartbeat of Hypercolor.
//!
//! Spawns a dedicated OS thread with its own Tokio runtime that runs the core
//! render loop:
//!
//! ```text
//! input publication pump              // samples sources at max live demand
//!     -> immutable input graph slots  // latest values + bounded events
//!
//! loop {
//!     RenderLoop::tick()                 // timing gate + FPS control
//!     read immutable input graph         // shared frame inputs
//!     render active scene groups         // Servo/native/media producers
//!     SparkleFlinger::compose_frame()    // canonical scene canvas
//!     sample LED output                  // CPU or prepared GPU sampler
//!     publish scene/display canvases     // latest-value bus/watch streams
//!     BackendManager::write_frame()      // staged hardware output
//!     RenderLoop::frame_complete()       // pressure metrics + tier adaptation
//!     sleep_until(next_deadline)         // pace to target FPS
//! }
//! ```

mod binding_eval;
mod capture_demand;
mod composition_planner;
mod display_lane;
mod frame_admission;
mod frame_composer;
mod frame_executor;
mod frame_io;
mod frame_metrics;
mod frame_policy;
mod frame_reporting;
mod frame_sampling;
mod frame_throttle;
#[cfg(feature = "wgpu")]
#[doc(hidden)]
pub mod gpu_device;
mod input_publication;
mod layer_runtime;
mod lighting_feed;
mod pipeline_driver;
mod pipeline_runtime;
mod producer_queue;
mod render_groups;
mod scene_dependency;
mod scene_snapshot;
mod scene_state;
mod screen_canvas;
#[doc(hidden)]
pub mod sparkleflinger;
mod unassigned_output;

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, watch};
use tokio_util::sync::CancellationToken;
use tracing::info;

pub use self::input_publication::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationDemandRegistration, InputPublicationStatus,
};
use self::input_publication::{InputPublicationMonitor, InputPublicationPump};
use self::pipeline_driver::run_pipeline;
pub(crate) use self::producer_queue::ProducerFrame;
pub(crate) use self::render_groups::{RenderSceneContext, ZoneFrameInputs};
pub(crate) use self::scene_dependency::SceneDependencyKey;
use crate::device_settings::DeviceSettingsStore;
use crate::discovery::DiscoveryRuntime;
use crate::interaction_routing::InteractionRoutingControl;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::scene_transactions::SceneTransactionQueue;
use crate::session::OutputPowerState;
use crate::zone_layout_preview::ZoneLayoutPreviewStore;
use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::event::ZoneColors;

pub(crate) struct InteractivePreviewZoneRuntime(render_groups::ZoneRuntime);

impl InteractivePreviewZoneRuntime {
    pub(crate) fn new(scene_width: u32, scene_height: u32) -> Result<Self> {
        Ok(Self(render_groups::ZoneRuntime::try_new_preview(
            scene_width,
            scene_height,
        )?))
    }

    pub(crate) fn with_asset_library(
        scene_width: u32,
        scene_height: u32,
        asset_library: Arc<RwLock<AssetLibrary>>,
    ) -> Result<Self> {
        Ok(Self(
            render_groups::ZoneRuntime::try_with_asset_library_preview(
                scene_width,
                scene_height,
                asset_library,
            )?,
        ))
    }

    pub(crate) fn resize_scene(&mut self, scene_width: u32, scene_height: u32) -> Result<()> {
        self.0.try_resize_scene(scene_width, scene_height)?;
        Ok(())
    }

    pub(crate) fn render_scene(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut sparkleflinger::SparkleFlinger,
        zones: &mut Vec<ZoneColors>,
    ) -> anyhow::Result<ProducerFrame> {
        self.0
            .render_scene(context, sparkleflinger, zones)
            .map(|rendered| rendered.scene_frame)
    }
}

const RENDER_RUNTIME_WORKERS: usize = 2;
const RENDER_RUNTIME_MAX_BLOCKING_THREADS: usize = 4;
/// Blocking threads outlive the gaps between display-output frame encodes.
///
/// Encodes arrive irregularly, seconds apart. A keep-alive shorter than that
/// gap retires the pool between every encode and pays a fresh thread spawn
/// for the next one.
const RENDER_RUNTIME_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(30);

pub(crate) fn producer_frame_counts() -> producer_queue::ProducerFrameCounts {
    producer_queue::producer_frame_counts()
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

/// Shared, atomically-updatable configured FPS ceiling.
///
/// The render loop can temporarily lower its runtime admission ceiling, but
/// config changes must still flow into the render thread without rebuilding
/// the pipeline runtime.
#[derive(Clone)]
pub struct ConfiguredFpsTier(Arc<AtomicU8>);

impl ConfiguredFpsTier {
    pub fn new(tier: FpsTier) -> Self {
        Self(Arc::new(AtomicU8::new(fps_tier_to_u8(tier))))
    }

    pub fn get(&self) -> FpsTier {
        u8_to_fps_tier(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, tier: FpsTier) {
        self.0.store(fps_tier_to_u8(tier), Ordering::Relaxed);
    }
}

impl From<FpsTier> for ConfiguredFpsTier {
    fn from(value: FpsTier) -> Self {
        Self::new(value)
    }
}

const fn fps_tier_to_u8(tier: FpsTier) -> u8 {
    match tier {
        FpsTier::Minimal => 0,
        FpsTier::Low => 1,
        FpsTier::Medium => 2,
        FpsTier::High => 3,
        FpsTier::Full => 4,
    }
}

const fn u8_to_fps_tier(value: u8) -> FpsTier {
    match value {
        0 => FpsTier::Minimal,
        1 => FpsTier::Low,
        2 => FpsTier::Medium,
        3 => FpsTier::High,
        _ => FpsTier::Full,
    }
}

// ── RenderThread ────────────────────────────────────────────────────────────

/// Handle to a running render thread.
///
/// Call [`shutdown`](Self::shutdown) to stop the thread gracefully.
/// The render loop must be stopped first (via `RenderLoop::stop()`) — the
/// thread will exit on the next `tick()` returning `false`.
pub struct RenderThread {
    join_handle: Option<std::thread::JoinHandle<Result<()>>>,
    cancel: CancellationToken,
    input_publication_demands: InputPublicationDemandHandle,
    input_publication_monitor: InputPublicationMonitor,
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

    /// User media asset library used by media-backed scene layers.
    pub asset_library: Arc<RwLock<AssetLibrary>>,

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

    /// Transient per-zone layout overrides driven by Studio drag previews.
    pub zone_layout_previews: Arc<ZoneLayoutPreviewStore>,

    /// Render loop — frame timing, FPS control, tier transitions.
    pub render_loop: Arc<RwLock<RenderLoop>>,

    /// Active scene stack and transition runtime.
    pub scene_manager: Arc<RwLock<SceneManager>>,

    /// Input orchestrator owned by the dedicated publication pump and demand control.
    pub input_manager: Arc<Mutex<InputManager>>,

    /// Coherent route policy and authoritative browser-source selection.
    pub interaction_routing: InteractionRoutingControl,

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

    /// Resolved render acceleration mode for the pipeline.
    pub render_acceleration_mode: RenderAccelerationMode,

    /// Resolved GPU render device from startup probing.
    #[cfg(feature = "wgpu")]
    pub render_gpu_device: Option<gpu_device::GpuRenderDevice>,

    /// Ceiling derived from user configuration before runtime admission.
    pub configured_max_fps_tier: ConfiguredFpsTier,

    /// Effective `display.face_fps_cap` for group-direct HTML faces.
    pub face_fps_cap: u32,
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
        Self::try_spawn_with_runtime_builder(state, build_render_runtime)
    }

    #[doc(hidden)]
    pub fn try_spawn_with_runtime_builder<F>(
        state: RenderThreadState,
        build_runtime: F,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<tokio::runtime::Runtime> + Send + 'static,
    {
        let input_publication_demands = InputPublicationDemandHandle::new();
        let pump_demands = input_publication_demands.clone();
        let pipeline_demands = input_publication_demands.clone();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<InputPublicationMonitor>>(1);
        let join_handle = std::thread::Builder::new()
            .name("hypercolor-render".to_owned())
            .spawn(move || -> Result<()> {
                configure_render_thread_priority();
                let runtime = match build_runtime() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return Ok(());
                    }
                };
                let mut input_pump = match runtime.block_on(InputPublicationPump::start(
                    Arc::clone(&state.input_manager),
                    pump_demands,
                )) {
                    Ok(pump) => pump,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return Ok(());
                    }
                };
                let pipeline = runtime.block_on(pipeline_runtime::PipelineRuntime::from_state(
                    &state,
                    input_pump.reader(),
                    pipeline_demands,
                ));
                match pipeline {
                    Ok(runtime_state) => {
                        let monitor = input_pump.monitor();
                        if ready_tx.send(Ok(monitor.clone())).is_err() {
                            return runtime.block_on(input_pump.shutdown());
                        }
                        let pipeline_result = runtime.block_on(async {
                            tokio::select! {
                                () = run_pipeline(state.clone(), runtime_state) => Ok(()),
                                status = monitor.wait_for_terminal() => {
                                    tracing::error!(?status, "input publication pump terminated while rendering");
                                    state.render_loop.write().await.stop();
                                    Err(anyhow!("input publication pump terminated: {status:?}"))
                                }
                                () = worker_cancel.cancelled() => {
                                    state.render_loop.write().await.stop();
                                    Ok(())
                                }
                            }
                        });
                        let shutdown_result = runtime.block_on(input_pump.shutdown());
                        pipeline_result.and(shutdown_result)
                    }
                    Err(error) => {
                        if let Err(shutdown_error) = runtime.block_on(input_pump.shutdown()) {
                            tracing::warn!(%shutdown_error, "input publication cleanup failed after render startup error");
                        }
                        let _ = ready_tx.send(Err(error));
                        Ok(())
                    }
                }
            })
            .context("failed to spawn render thread")?;
        let ready = ready_rx
            .recv()
            .context("render thread exited before startup completed")?;
        let input_publication_monitor = match ready {
            Ok(monitor) => monitor,
            Err(error) => {
                let _ = join_handle.join();
                return Err(error);
            }
        };
        info!("render thread spawned");
        Ok(Self {
            join_handle: Some(join_handle),
            cancel,
            input_publication_demands,
            input_publication_monitor,
        })
    }

    /// Clone the lock-free demand publication used by input consumers.
    pub fn input_publication_demands(&self) -> InputPublicationDemandHandle {
        self.input_publication_demands.clone()
    }

    /// Read the input-publication worker lifecycle state without blocking.
    pub fn input_publication_status(&self) -> InputPublicationStatus {
        self.input_publication_monitor.status()
    }

    /// Wait for the render thread to exit.
    ///
    /// The caller must stop the render loop first — this method
    /// just awaits the task's completion.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.cancel.cancel();
        if let Some(handle) = self.join_handle.take() {
            tokio::task::spawn_blocking(move || -> Result<()> {
                handle.join().map_err(|panic| {
                    anyhow!(
                        "render thread panicked: {}",
                        panic_payload_message(panic.as_ref())
                    )
                })?
            })
            .await
            .context("failed to join render thread")??;
            info!("render thread stopped");
        }
        Ok(())
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Build the render runtime.
///
/// Deliberately no `on_thread_start` priority hook. Tokio launches its worker
/// threads through the blocking pool, so the hook fires for both roles and
/// cannot tell them apart. It therefore raised every transient frame-encode
/// thread to the same elevated priority as the render thread, leaving
/// throughput work competing with the frame loop it was meant to protect.
/// The frame loop runs on the `hypercolor-render` thread, which sets its own
/// priority before entering `block_on`.
fn build_render_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RENDER_RUNTIME_WORKERS)
        .max_blocking_threads(RENDER_RUNTIME_MAX_BLOCKING_THREADS)
        .thread_keep_alive(RENDER_RUNTIME_THREAD_KEEP_ALIVE)
        .thread_name("hypercolor-render-rt")
        .enable_all()
        .build()
        .context("failed to initialize render thread runtime")
}

#[cfg(target_os = "windows")]
fn configure_render_thread_priority() {
    use thread_priority::{ThreadPriority, WinAPIThreadPriority, set_current_thread_priority};

    let priority = ThreadPriority::Os(WinAPIThreadPriority::AboveNormal.into());
    match set_current_thread_priority(priority) {
        Ok(()) => tracing::debug!("configured Windows render thread priority"),
        Err(error) => tracing::warn!(
            error = %error,
            "failed to configure Windows render thread priority"
        ),
    }
}

#[cfg(not(target_os = "windows"))]
fn configure_render_thread_priority() {}

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

/// Saturating conversion from `Duration` milliseconds to `u64`.
fn millis_u64(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
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
    use hypercolor_core::types::canvas::{
        Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor, SurfaceResourceError,
    };
    use hypercolor_core::types::event::ZoneColors;

    use super::frame_policy::SkipDecision;
    use super::screen_canvas::screen_data_to_surface;
    use super::{micros_u32, millis_u64};

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
    fn millis_u64_preserves_elapsed_time_beyond_u32_range() {
        let elapsed = Duration::from_millis(u64::from(u32::MAX) + 1);

        assert_eq!(millis_u64(elapsed), u64::from(u32::MAX) + 1);
    }

    #[test]
    fn screen_data_to_surface_maps_declared_row_major_colors() {
        let screen_data = ScreenData::from_zones(
            vec![
                ZoneColors {
                    zone_id: "arbitrary-a".to_owned(),
                    colors: vec![[255, 0, 0]],
                },
                ZoneColors {
                    zone_id: "arbitrary-b".to_owned(),
                    colors: vec![[0, 255, 0]],
                },
                ZoneColors {
                    zone_id: "arbitrary-c".to_owned(),
                    colors: vec![[0, 0, 255]],
                },
                ZoneColors {
                    zone_id: "arbitrary-d".to_owned(),
                    colors: vec![[255, 255, 255]],
                },
            ],
            2,
            2,
        );

        let mut sector_grid = Vec::new();
        let mut surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 2);
        let surface =
            screen_data_to_surface(&screen_data, 4, 4, &mut sector_grid, &mut surface_pool)
                .expect("screen surface conversion should succeed")
                .expect("surface should build");
        assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(surface.get_pixel(3, 0), Rgba::new(0, 255, 0, 255));
        assert_eq!(surface.get_pixel(0, 3), Rgba::new(0, 0, 255, 255));
        assert_eq!(surface.get_pixel(3, 3), Rgba::new(255, 255, 255, 255));
    }

    #[test]
    fn screen_data_to_surface_preserves_downscale_geometry() {
        let mut screen_data = ScreenData::from_zones(
            vec![ZoneColors {
                zone_id: "screen".to_owned(),
                colors: vec![[255, 0, 0]],
            }],
            1,
            1,
        );
        screen_data.canvas_downscale = Some(PublishedSurface::from_owned_canvas(
            Canvas::new(16, 9),
            1,
            16,
        ));

        let mut sector_grid = Vec::new();
        let mut surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 2);
        let surface =
            screen_data_to_surface(&screen_data, 4, 4, &mut sector_grid, &mut surface_pool)
                .expect("screen surface conversion should succeed")
                .expect("downscale should pass through");

        assert_eq!(surface.width(), 16);
        assert_eq!(surface.height(), 9);
        assert!(sector_grid.is_empty());
    }

    #[test]
    fn screen_data_to_surface_accepts_addressable_wide_extent() {
        let screen_data = ScreenData::from_zones(
            vec![ZoneColors {
                zone_id: "screen".to_owned(),
                colors: vec![[12, 34, 56]],
            }],
            1,
            1,
        );
        let mut sector_grid = Vec::new();
        let mut surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(1, 1), 2);

        let surface =
            screen_data_to_surface(&screen_data, 7_681, 1, &mut sector_grid, &mut surface_pool)
                .expect("addressable wide surface conversion should succeed")
                .expect("wide surface should build");

        assert_eq!(surface.width(), 7_681);
        assert_eq!(surface.height(), 1);
        assert_eq!(surface.get_pixel(7_680, 0), Rgba::new(12, 34, 56, 255));
    }

    #[test]
    fn screen_data_to_surface_reserves_scratch_before_grid_growth() {
        let screen_data = ScreenData::from_zones(
            (0_u8..12)
                .map(|value| ZoneColors {
                    zone_id: format!("screen:{value}"),
                    colors: vec![[value, 0, 0]],
                })
                .collect(),
            4,
            3,
        );
        let mut sector_grid = Vec::with_capacity(8);
        assert!(sector_grid.capacity() < 12);
        let mut surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 3), 2);

        let surface =
            screen_data_to_surface(&screen_data, 4, 3, &mut sector_grid, &mut surface_pool)
                .expect("growing screen grid conversion should succeed")
                .expect("screen surface should build");

        assert_eq!(sector_grid.len(), 12);
        assert!(sector_grid.capacity() >= 12);
        assert_eq!(surface.get_pixel(0, 0), Rgba::new(0, 0, 0, 255));
        assert_eq!(surface.get_pixel(3, 2), Rgba::new(11, 0, 0, 255));
    }

    #[test]
    fn screen_data_to_surface_preserves_pool_after_geometry_overflow() {
        let screen_data = ScreenData::from_zones(
            vec![ZoneColors {
                zone_id: "screen".to_owned(),
                colors: vec![[12, 34, 56]],
            }],
            1,
            1,
        );
        let mut sector_grid = Vec::new();
        let mut surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 2);

        let error = screen_data_to_surface(
            &screen_data,
            u32::MAX,
            u32::MAX,
            &mut sector_grid,
            &mut surface_pool,
        )
        .expect_err("overflowing geometry should be rejected");

        assert!(matches!(
            error.downcast_ref::<SurfaceResourceError>(),
            Some(SurfaceResourceError::ByteLengthOverflow {
                width: u32::MAX,
                height: u32::MAX,
            })
        ));
        assert_eq!(surface_pool.descriptor(), SurfaceDescriptor::rgba8888(4, 4));
    }

    #[test]
    fn screen_data_to_surface_reuses_pool_after_warmup() {
        let screen_data = ScreenData::from_zones(
            vec![ZoneColors {
                zone_id: "screen:sector_0_0".to_owned(),
                colors: vec![[255, 0, 0]],
            }],
            1,
            1,
        );
        let mut sector_grid = Vec::new();
        let mut surface_pool =
            RenderSurfacePool::with_slot_count(SurfaceDescriptor::rgba8888(4, 4), 2);

        let first = screen_data_to_surface(&screen_data, 4, 4, &mut sector_grid, &mut surface_pool)
            .expect("screen surface conversion should succeed")
            .expect("first surface should build")
            .rgba_bytes()
            .as_ptr()
            .addr();
        let second =
            screen_data_to_surface(&screen_data, 4, 4, &mut sector_grid, &mut surface_pool)
                .expect("screen surface conversion should succeed")
                .expect("second surface should build")
                .rgba_bytes()
                .as_ptr()
                .addr();
        let third = screen_data_to_surface(&screen_data, 4, 4, &mut sector_grid, &mut surface_pool)
            .expect("screen surface conversion should succeed")
            .expect("third surface should build")
            .rgba_bytes()
            .as_ptr()
            .addr();

        assert_ne!(first, second);
        assert_eq!(first, third);
    }
}
