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
//!     render active scene zones          // Servo/native/media producers
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
mod failure_log;
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
pub mod gpu_errors;
mod input_publication;
mod layer_runtime;
mod lighting_feed;
mod output_gate;
mod pipeline_driver;
mod pipeline_runtime;
mod producer_queue;
mod render_zones;
mod scene_dependency;
mod scene_snapshot;
mod scene_state;
mod screen_canvas;
#[cfg(feature = "wgpu")]
mod screen_parity_diagnostics;
#[doc(hidden)]
pub mod sparkleflinger;
pub mod supervision;
mod supervisor;
mod unassigned_output;
mod zone_previews;

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use arc_swap::ArcSwapOption;
use tokio::sync::{Mutex, RwLock, watch};
use tokio_util::sync::CancellationToken;
use tracing::info;

use self::input_publication::InputPublicationMonitor;
pub(crate) use self::input_publication::InputScreenBranchRequest;
pub use self::input_publication::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationDemandRegistration, InputPublicationStatus, InputScreenBranchDemand,
};
pub(crate) use self::producer_queue::ProducerFrame;
pub(crate) use self::render_zones::{RenderSceneContext, ZoneFrameInputs};
pub(crate) use self::scene_dependency::SceneDependencyKey;
#[cfg(feature = "wgpu")]
#[allow(
    unused_imports,
    reason = "the snapshot types are consumed by platform diagnostic endpoints only"
)]
pub(crate) use self::screen_parity_diagnostics::{
    ScreenParityDiagnosticHandle, ScreenParityLiveSnapshot, ScreenParitySnapshotError,
};
pub use self::supervision::{
    RenderFailureKind, RenderFailureRecord, RenderRecoveryAction, RenderRestartPolicy,
    RenderSupervision, RenderSupervisionSnapshot, RenderThreadPhase,
};
use crate::discovery::DiscoveryRuntime;
use crate::domain::scene::{ScenePlanReader, SceneService};
use crate::domain::spatial::SpatialService;
use crate::interaction_routing::InteractionRoutingControl;
use crate::output_power::OutputPowerState;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::scene_transactions::SceneTransactionQueue;
use crate::startup::StartupProgress;
use crate::zone_layout_preview::ZoneLayoutPreviewStore;
use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::event::ZoneColors;

pub(crate) struct InteractivePreviewZoneRuntime(render_zones::ZoneRuntime);

impl InteractivePreviewZoneRuntime {
    pub(crate) fn new(scene_width: u32, scene_height: u32) -> Result<Self> {
        Ok(Self(render_zones::ZoneRuntime::try_new_preview(
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
            render_zones::ZoneRuntime::try_with_asset_library_preview(
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
        self.0.admit_reconcile(
            context.zones,
            context.active_scene_id,
            context.dependency_key,
            context.registry,
            context.display_zone_descriptors,
            context.authoritative_spatial_engine,
            sparkleflinger,
        )?;
        self.0
            .render_scene(context, sparkleflinger, zones)
            .map(|rendered| rendered.scene_frame)
    }

    /// Render `context.zones`, which holds the one zone an interactive
    /// preview follows, and hand back that zone's own frame: the texture
    /// its LEDs sample and its zone previews show, not the scene
    /// projection. A zone that renders nothing previews dark, the way
    /// its zone previews do.
    pub(crate) fn render_zone(
        &mut self,
        context: RenderSceneContext<'_>,
        zone_id: hypercolor_types::scene::ZoneId,
        sparkleflinger: &mut sparkleflinger::SparkleFlinger,
        zones: &mut Vec<ZoneColors>,
    ) -> anyhow::Result<InteractivePreviewRender> {
        let renders_nothing = context.zones.iter().any(|zone| {
            zone.id == zone_id
                && zone.display_target.is_none()
                && !render_zones::zone_is_active(zone)
        });
        self.0.admit_reconcile(
            context.zones,
            context.active_scene_id,
            context.dependency_key,
            context.registry,
            context.display_zone_descriptors,
            context.authoritative_spatial_engine,
            sparkleflinger,
        )?;
        let rendered = self.0.render_scene(context, sparkleflinger, zones)?;
        let frame = rendered
            .zone_canvases
            .into_iter()
            .find_map(|(id, frame)| (id == zone_id).then_some(frame));
        Ok(match frame {
            Some(frame) => InteractivePreviewRender::Frame(frame),
            None if renders_nothing => InteractivePreviewRender::Dark,
            None => InteractivePreviewRender::Pending,
        })
    }
}

/// What one interactive preview render pass leaves to publish.
pub(crate) enum InteractivePreviewRender<F = ProducerFrame> {
    /// A frame to publish.
    Frame(F),
    /// The followed zone renders nothing, because it is paused or has no
    /// enabled layers, so its preview goes dark.
    Dark,
    /// Nothing new: the followed zone's layers have not produced a frame.
    Pending,
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
///
/// The thread supervises its own pipeline: a panic or error exit is logged,
/// recorded in the [`RenderSupervision`] it was spawned with, and followed
/// by an in-process restart, until the restart budget runs out and the
/// supervision asks the daemon to exit.
pub struct RenderThread {
    join_handle: Option<std::thread::JoinHandle<Result<()>>>,
    cancel: CancellationToken,
    input_publication_demands: InputPublicationDemandHandle,
    /// The current input pump's monitor, replaced when a failed pump is
    /// restarted.
    input_publication_monitor: Arc<ArcSwapOption<InputPublicationMonitor>>,
    supervision: RenderSupervision,
    _panic_observer: Option<crate::panic_hook::PanicObserverRegistration>,
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    screen_parity_diagnostics: ScreenParityDiagnosticHandle,
}

/// All shared state the render thread needs.
///
/// Each field is `Arc`-wrapped so it can be shared with the API server
/// and other subsystems. The render thread takes locks only for the
/// duration of each pipeline stage.
#[derive(Clone)]
pub struct RenderThreadState {
    /// Effect catalog used to resolve render-zone assignments.
    pub effect_registry: Arc<RwLock<EffectRegistry>>,

    /// User media asset library used by media-backed scene layers.
    pub asset_library: Arc<RwLock<AssetLibrary>>,

    /// Spatial sampling engine — maps canvas pixels to LED positions.
    pub spatial_engine: SpatialService,

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
    pub scene_manager: SceneService,

    /// Lock-free scene plan projection read once per frame.
    pub scene_plan: ScenePlanReader,

    /// Input orchestrator owned by the dedicated publication pump and demand control.
    pub input_manager: InputManager,

    /// Coherent route policy and authoritative browser-source selection.
    pub interaction_routing: InteractionRoutingControl,

    /// Session policy output state (brightness scale + sleep flag).
    pub power_state: watch::Receiver<OutputPowerState>,

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

    /// Effective `display.face_fps_cap` for zone-direct HTML faces.
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
        Self::try_spawn_supervised(state, RenderSupervision::detached())
    }

    /// Spawn the render thread under `supervision`, which carries the
    /// restart policy and receives its health and failures.
    pub fn try_spawn_supervised(
        state: RenderThreadState,
        supervision: RenderSupervision,
    ) -> Result<Self> {
        Self::spawn_inner(state, build_render_runtime, None, supervision)
    }

    /// Spawn the render thread while reporting each completed startup step
    /// (the render runtime, input publication, every compositor pipeline
    /// compile) to `progress`.
    ///
    /// The render thread drops its handle once the pipeline is built, so
    /// nothing on the frame path touches startup progress.
    ///
    /// # Errors
    ///
    /// Returns an error when the thread cannot be spawned or the render
    /// pipeline fails to build.
    pub fn try_spawn_reporting(
        state: RenderThreadState,
        progress: StartupProgress,
        supervision: RenderSupervision,
    ) -> Result<Self> {
        Self::spawn_inner(state, build_render_runtime, Some(progress), supervision)
    }

    #[doc(hidden)]
    pub fn try_spawn_with_runtime_builder<F>(
        state: RenderThreadState,
        build_runtime: F,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<tokio::runtime::Runtime> + Send + 'static,
    {
        Self::spawn_inner(state, build_runtime, None, RenderSupervision::detached())
    }

    fn spawn_inner<F>(
        state: RenderThreadState,
        build_runtime: F,
        progress: Option<StartupProgress>,
        supervision: RenderSupervision,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<tokio::runtime::Runtime> + Send + 'static,
    {
        let input_publication_demands =
            InputPublicationDemandHandle::new(state.input_manager.screen_native_execution_policy());
        let cancel = CancellationToken::new();
        let input_publication_monitor = Arc::new(ArcSwapOption::empty());
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
        #[cfg(feature = "wgpu")]
        let (screen_parity_diagnostics, screen_parity_mailbox) =
            screen_parity_diagnostics::screen_parity_diagnostic_channel();
        #[cfg(all(
            feature = "wgpu",
            not(all(target_os = "macos", feature = "screen-capture"))
        ))]
        let _ = screen_parity_diagnostics;
        let panic_observer = supervision
            .panic_observer()
            .map(crate::panic_hook::add_observer);
        supervision.set_phase(RenderThreadPhase::Starting);
        let launch = supervisor::RenderThreadLaunch {
            state,
            build_runtime,
            progress,
            ready: ready_tx,
            cancel: cancel.clone(),
            input_demands: input_publication_demands.clone(),
            input_monitor: Arc::clone(&input_publication_monitor),
            supervision: supervision.clone(),
            #[cfg(feature = "wgpu")]
            screen_parity_mailbox,
        };
        let spawned = std::thread::Builder::new()
            .name(supervision::RENDER_THREAD_NAME.to_owned())
            .spawn(move || supervisor::run(launch))
            .context("failed to spawn render thread");
        let join_handle = match spawned {
            Ok(join_handle) => join_handle,
            Err(error) => {
                supervision.set_phase(RenderThreadPhase::Stopped);
                return Err(error);
            }
        };
        let startup = ready_rx
            .recv()
            .context("render thread exited before startup completed")
            .and_then(|ready| ready);
        if let Err(error) = startup {
            let _ = join_handle.join();
            return Err(match supervision.termination() {
                Some(record) => error.context(format!(
                    "render thread failed during startup ({}): {}",
                    record.kind.as_str(),
                    record.message
                )),
                None => error,
            });
        }
        info!("render thread spawned");
        Ok(Self {
            join_handle: Some(join_handle),
            cancel,
            input_publication_demands,
            input_publication_monitor,
            supervision,
            _panic_observer: panic_observer,
            #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
            screen_parity_diagnostics,
        })
    }

    /// The supervision this thread reports its health and failures to.
    #[must_use]
    pub fn supervision(&self) -> RenderSupervision {
        self.supervision.clone()
    }

    /// Clone the lock-free demand publication used by input consumers.
    pub fn input_publication_demands(&self) -> InputPublicationDemandHandle {
        self.input_publication_demands.clone()
    }

    /// Read the input-publication worker lifecycle state without blocking.
    pub fn input_publication_status(&self) -> InputPublicationStatus {
        self.input_publication_monitor
            .load()
            .as_ref()
            .map_or(InputPublicationStatus::Starting, |monitor| monitor.status())
    }

    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    pub(crate) fn screen_parity_diagnostics(&self) -> ScreenParityDiagnosticHandle {
        self.screen_parity_diagnostics.clone()
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
        .thread_name(supervision::RENDER_RUNTIME_THREAD_NAME)
        .enable_all()
        .build()
        .context("failed to initialize render thread runtime")
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

    use super::frame_policy::SkipDecision;
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
}
