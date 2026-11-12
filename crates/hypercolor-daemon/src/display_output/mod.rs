//! Automatic display output pipeline for LCD-capable devices.
//!
//! This task delivers each screen's frames, a face or the backdrop of the
//! zone that places the screen, without disturbing the LED frame routing
//! path. The render thread decides what each screen shows; this task says
//! which screens exist and carries their frames to the panels.

mod encode;
mod placement;
mod render;
mod worker;

use std::any::Any;
use std::collections::{
    HashMap, HashSet,
    hash_map::{DefaultHasher, Entry},
};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, oneshot, watch};
use tracing::{debug, info, warn};

use hypercolor_core::bus::{
    CanvasFrame, DisplayZoneFrame, DisplayZoneOutputRoute, DisplayZoneViewport, HypercolorBus,
    OutputDemandHold, ScreenBackdropRoute, screen_backdrop_lane_id,
};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::device::{DeviceId, DeviceTopologyHint, DisplayFrameFormat};
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::{DisplayFaceTarget, ZoneId};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

pub(crate) use self::encode::PanelCropper;
pub use self::placement::{ScreenPlacement, screen_placement, screen_viewport_in_layout};
use self::render::display_viewport_signature;
use crate::display_frames::DisplayFrameRuntime;
use crate::logical_devices::LogicalDevice;
use crate::output_power::OutputPowerState;
use worker::DisplayWorkerHandle;

const DISPLAY_ERROR_WARN_INTERVAL: Duration = Duration::from_secs(5);
const DISPLAY_OUTPUT_MAX_FPS: u32 = 15;
const RAW_DISPLAY_OUTPUT_MAX_FPS: u32 = 30;
pub(crate) const DISPLAY_FACE_DEFAULT_FPS: u32 = 30;
const DISPLAY_OUTPUT_DISPATCH_INTERVAL: Duration = Duration::from_millis(16);
pub const DEFAULT_STATIC_HOLD_REFRESH_INTERVAL: Duration = Duration::from_secs(20);
const DISPLAY_RUNTIME_WORKERS: usize = 2;
const DISPLAY_RUNTIME_MAX_BLOCKING_THREADS: usize = 4;
const DISPLAY_RUNTIME_THREAD_KEEP_ALIVE: Duration = Duration::from_secs(2);

/// Handle to the automatic display output task.
pub struct DisplayOutputThread {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

/// Shared state for the automatic display output task.
#[derive(Clone)]
pub struct DisplayOutputState {
    /// Direct device writer used for JPEG frame delivery.
    pub backend_manager: Arc<Mutex<BackendManager>>,
    /// Live registry used to discover currently renderable display devices.
    pub device_registry: DeviceRegistry,
    /// Logical-device mappings that name each physical device's layout ids.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    /// Event bus carrying the display frames the render thread produces.
    pub event_bus: Arc<HypercolorBus>,
    /// Session power policy used to decide whether static hold refresh is active.
    pub power_state: watch::Receiver<OutputPowerState>,
    /// How often unchanged display frames should be reasserted during static hold.
    pub static_hold_refresh_interval: Duration,
    /// Latest composited JPEG frames published per device for preview surfaces.
    pub display_frames: Arc<RwLock<DisplayFrameRuntime>>,
    /// Effective `display.face_fps_cap` for zone-direct HTML faces.
    pub face_fps_cap: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DisplayGeometry {
    width: u32,
    height: u32,
    circular: bool,
}

#[derive(Clone, Debug)]
#[allow(
    clippy::struct_field_names,
    reason = "display routing vocabulary mirrors the scene model and keeps the mappings explicit"
)]
struct DisplayTarget {
    worker_key: DisplayWorkerKey,
    backend_id: String,
    device_id: DeviceId,
    name: String,
    target_fps: u32,
    brightness: f32,
    geometry: DisplayGeometry,
    frame_format: DisplayFrameFormat,
    /// The device's encoded-frame wire cap, when it declares one.
    max_frame_len: Option<usize>,
    canvas_source: DisplayCanvasSource,
    /// The display lane this screen's frames arrive on.
    lane_sender: watch::Sender<DisplayZoneFrame>,
    /// The screen's backdrop lane, for a face the worker finishes itself:
    /// it carries the backdrop to blend over and the viewport to turn with.
    backdrop_sender: Option<watch::Sender<DisplayZoneFrame>>,
    display_target: Option<DisplayFaceTarget>,
    /// Whether the face lane's frames arrive finalized by the render thread.
    finalized: bool,
    /// The whole canvas, turned the way the panel is mounted: what a face
    /// turns with on a screen no zone places.
    mount_viewport: DisplayViewport,
}

type DisplayWorkerKey = (String, DeviceId);

#[derive(Clone, Debug, PartialEq)]
struct DisplayFaceTargetBinding {
    zone_id: ZoneId,
    target: DisplayFaceTarget,
    finalized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DisplayWorkerConfigSignature {
    target_fps: u32,
    brightness_bits: u32,
    geometry: DisplayGeometry,
    frame_format: DisplayFrameFormat,
    max_frame_len: Option<usize>,
    canvas_source: DisplayCanvasSourceSignature,
    face_blend_mode: BlendMode,
    face_opacity_bits: u32,
    mount_viewport: DisplayViewportSignature,
}

#[derive(Default)]
struct DisplayTargetCache {
    version: u64,
    cache_key: Option<DisplayTargetCacheKey>,
    targets: Arc<[Arc<DisplayTarget>]>,
    awaiting_lanes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayTargetDependencyKey {
    registry_generation: u64,
    display_zone_targets_revision: u64,
    screen_lane_plan_revision: u64,
}

impl DisplayTargetDependencyKey {
    const fn new(
        registry_generation: u64,
        display_zone_targets_revision: u64,
        screen_lane_plan_revision: u64,
    ) -> Self {
        Self {
            registry_generation,
            display_zone_targets_revision,
            screen_lane_plan_revision,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayTargetCacheKey {
    dependency_key: DisplayTargetDependencyKey,
    logical_signature: u64,
    display_preview_signature: u64,
}

impl DisplayTargetCacheKey {
    const fn new(
        dependency_key: DisplayTargetDependencyKey,
        logical_signature: u64,
        display_preview_signature: u64,
    ) -> Self {
        Self {
            dependency_key,
            logical_signature,
            display_preview_signature,
        }
    }
}

/// What a screen shows.
#[derive(Clone, Debug)]
enum DisplayCanvasSource {
    /// The backdrop of the zone that places the screen, alone.
    Backdrop,
    /// The face a display zone renders for the screen.
    ZoneDirect { zone_id: ZoneId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DisplayCanvasSourceSignature {
    Backdrop,
    ZoneDirect { zone_id: ZoneId },
}

type DisplayViewport = DisplayZoneViewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayViewportSignature {
    position_x_bits: u32,
    position_y_bits: u32,
    size_x_bits: u32,
    size_y_bits: u32,
    rotation_bits: u32,
    scale_bits: u32,
    edge_behavior: u8,
    fade_falloff_bits: u32,
}

#[derive(Clone)]
struct DisplayTargetsSnapshot {
    version: u64,
    targets: Arc<[Arc<DisplayTarget>]>,
    /// Whether a previewed simulator is routed, has no lane yet, and may
    /// still get one. The render thread makes its lane, and a parked render
    /// thread makes nothing: nobody but the preview is asking for frames.
    /// Once the render thread has placed the screens it was handed and made
    /// the simulator no lane, none is coming and nothing is awaited.
    awaiting_lanes: bool,
}

#[derive(Clone, Debug)]
pub(super) enum DisplayWorkerFrameSource {
    /// The owning zone's frame, for the worker to crop to the screen
    /// through the viewport it arrived with, or the crop itself when the
    /// render thread already made it.
    Backdrop {
        frame: Arc<CanvasFrame>,
        viewport: DisplayViewport,
        cropped: bool,
    },
    /// A frame the render thread finalized for the screen.
    Direct(Arc<DisplayZoneFrame>),
    /// A face for the worker to blend over the screen's backdrop.
    /// `viewport` turns the face, and crops the backdrop unless it arrived
    /// cropped.
    Face {
        backdrop_frame: Option<Arc<CanvasFrame>>,
        backdrop_cropped: bool,
        face_frame: Arc<CanvasFrame>,
        viewport: DisplayViewport,
        blend_mode: BlendMode,
        opacity: f32,
    },
}

#[derive(Clone, Debug)]
pub(super) struct DisplayWorkerFrameSet {
    pub source: DisplayWorkerFrameSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplaySourceIdentity {
    Rgba {
        generation: u64,
        storage: PublishedSurfaceStorageIdentity,
        width: u32,
        height: u32,
    },
    Yuv420 {
        storage: hypercolor_core::bus::DisplayYuv420StorageIdentity,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StableDisplayFrameSetIdentity {
    source: StableDisplayFrameSourceIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StableDisplayFrameSourceIdentity {
    Backdrop {
        frame: DisplaySourceIdentity,
        viewport: DisplayViewportSignature,
        cropped: bool,
    },
    Direct(DisplaySourceIdentity),
    Face {
        backdrop_frame: Option<DisplaySourceIdentity>,
        backdrop_cropped: bool,
        face_frame: DisplaySourceIdentity,
        viewport: DisplayViewportSignature,
        blend_mode: BlendMode,
        opacity_bits: u32,
    },
}

impl DisplayTarget {
    pub(super) fn worker_config_signature(&self) -> DisplayWorkerConfigSignature {
        DisplayWorkerConfigSignature {
            target_fps: self.target_fps,
            brightness_bits: self.brightness.to_bits(),
            geometry: self.geometry,
            frame_format: self.frame_format,
            max_frame_len: self.max_frame_len,
            canvas_source: self.canvas_source.signature(),
            face_blend_mode: self.face_blend_mode(),
            face_opacity_bits: self.face_opacity().to_bits(),
            mount_viewport: display_viewport_signature(&self.mount_viewport),
        }
    }

    fn face_blend_mode(&self) -> BlendMode {
        self.display_target
            .as_ref()
            .map_or(BlendMode::Replace, |target| target.blend_mode)
    }

    fn blends_with_effect(&self) -> bool {
        self.face_blend_mode().blends_with_base()
    }

    fn face_opacity(&self) -> f32 {
        if !self.blends_with_effect() {
            return 1.0;
        }
        self.display_target
            .as_ref()
            .map_or(1.0, |target| target.opacity.clamp(0.0, 1.0))
    }
}

impl DisplayCanvasSource {
    fn signature(&self) -> DisplayCanvasSourceSignature {
        match self {
            Self::Backdrop => DisplayCanvasSourceSignature::Backdrop,
            Self::ZoneDirect { zone_id } => {
                DisplayCanvasSourceSignature::ZoneDirect { zone_id: *zone_id }
            }
        }
    }
}

impl DisplayOutputThread {
    /// Spawn the automatic display output task.
    #[must_use]
    pub fn spawn(state: DisplayOutputState) -> Self {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let join_handle = std::thread::Builder::new()
            .name("hypercolor-display".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(DISPLAY_RUNTIME_WORKERS)
                    .max_blocking_threads(DISPLAY_RUNTIME_MAX_BLOCKING_THREADS)
                    .thread_keep_alive(DISPLAY_RUNTIME_THREAD_KEEP_ALIVE)
                    .thread_name("hypercolor-display-rt")
                    .enable_all()
                    .build()
                    .expect("display output runtime should initialize");
                runtime.block_on(run_display_output(state, shutdown_rx));
            })
            .expect("display output thread should spawn");
        info!("display output thread spawned");
        Self {
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(join_handle),
        }
    }

    /// Stop the automatic display output task.
    ///
    /// The task waits on canvas updates indefinitely, so shutdown aborts the
    /// task directly after the render thread has stopped producing frames.
    ///
    /// # Errors
    ///
    /// Returns an error if task shutdown fails unexpectedly.
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        let Some(handle) = self.join_handle.take() else {
            return Ok(());
        };

        tokio::task::spawn_blocking(move || {
            handle.join().map_err(|panic| {
                anyhow!(
                    "display output thread panicked: {}",
                    panic_payload_message(panic.as_ref())
                )
            })
        })
        .await
        .context("failed to join display output thread")??;
        info!("display output thread stopped");
        Ok(())
    }
}

async fn run_display_output(state: DisplayOutputState, mut shutdown_rx: oneshot::Receiver<()>) {
    let mut workers = HashMap::<DisplayWorkerKey, DisplayWorkerHandle>::new();
    let mut targets_cache = DisplayTargetCache::default();
    let initial_targets = display_targets(
        &state.device_registry,
        &state.logical_devices,
        &state.event_bus,
        &state.display_frames,
        &mut targets_cache,
        state.face_fps_cap,
    )
    .await;
    let mut output_demand_hold = None;
    sync_output_demand_hold(
        &mut output_demand_hold,
        &state.event_bus,
        !initial_targets.targets.is_empty() || initial_targets.awaiting_lanes,
    );
    reconcile_display_workers(&state, &mut workers, initial_targets.targets.as_ref()).await;
    let mut last_reconciled_target_version = Some(initial_targets.version);
    let mut last_dispatched_sources =
        HashMap::<DisplayWorkerKey, StableDisplayFrameSetIdentity>::new();
    let mut dispatch_tick = tokio::time::interval(DISPLAY_OUTPUT_DISPATCH_INTERVAL);
    dispatch_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                debug!("display output task shutting down");
                break;
            }
            _ = dispatch_tick.tick() => {
            }
        }

        let targets = display_targets(
            &state.device_registry,
            &state.logical_devices,
            &state.event_bus,
            &state.display_frames,
            &mut targets_cache,
            state.face_fps_cap,
        )
        .await;
        let backend_generation_changed = workers.values().any(|worker| !worker.lane_is_active());
        if last_reconciled_target_version != Some(targets.version) || backend_generation_changed {
            reconcile_display_workers(&state, &mut workers, targets.targets.as_ref()).await;
            last_reconciled_target_version = Some(targets.version);
            last_dispatched_sources.clear();
        }
        sync_output_demand_hold(
            &mut output_demand_hold,
            &state.event_bus,
            !targets.targets.is_empty() || targets.awaiting_lanes,
        );
        if targets.targets.is_empty() {
            last_dispatched_sources.clear();
            continue;
        }

        for target in targets.targets.iter() {
            let lane_frame = stable_lane_frame(&target.lane_sender);
            let backdrop_frame = target.backdrop_sender.as_ref().and_then(stable_lane_frame);
            let Some((frames, dispatch_identity)) = build_display_worker_frame_set(
                target.as_ref(),
                lane_frame.as_ref(),
                backdrop_frame.as_ref(),
            ) else {
                continue;
            };
            if last_dispatched_sources.get(&target.worker_key) == Some(&dispatch_identity) {
                continue;
            }
            if let Some(worker) = workers.get(&target.worker_key) {
                worker.push(frames);
                last_dispatched_sources.insert(target.worker_key.clone(), dispatch_identity);
            }
        }
    }

    for (_, worker) in workers {
        worker.shutdown().await;
    }
}

/// The lane's latest frame, unless the lane has published nothing yet.
fn stable_lane_frame(sender: &watch::Sender<DisplayZoneFrame>) -> Option<Arc<DisplayZoneFrame>> {
    let frame = sender.borrow();
    stable_display_zone_source_identity(&frame).map(|_| Arc::new(frame.clone()))
}

fn stable_display_source_identity(frame: &CanvasFrame) -> Option<DisplaySourceIdentity> {
    let surface = frame.surface();
    (frame.width > 0 && frame.height > 0).then_some(DisplaySourceIdentity::Rgba {
        generation: surface.generation(),
        storage: surface.storage_identity(),
        width: frame.width,
        height: frame.height,
    })
}

fn stable_display_zone_source_identity(frame: &DisplayZoneFrame) -> Option<DisplaySourceIdentity> {
    match frame {
        DisplayZoneFrame::Canvas(frame) => stable_display_source_identity(frame),
        DisplayZoneFrame::Backdrop(backdrop) => stable_display_source_identity(&backdrop.frame),
        DisplayZoneFrame::Yuv420(frame) => {
            (frame.width > 0 && frame.height > 0).then_some(DisplaySourceIdentity::Yuv420 {
                storage: frame.storage_identity(),
                width: frame.width,
                height: frame.height,
            })
        }
    }
}

fn build_display_worker_frame_set(
    target: &DisplayTarget,
    lane_frame: Option<&Arc<DisplayZoneFrame>>,
    backdrop_frame: Option<&Arc<DisplayZoneFrame>>,
) -> Option<(DisplayWorkerFrameSet, StableDisplayFrameSetIdentity)> {
    let lane_frame = lane_frame?;
    let lane_identity = stable_display_zone_source_identity(lane_frame.as_ref())?;
    match &target.canvas_source {
        // A backdrop lane's frame says what it is: a zone canvas with its
        // viewport for the worker to crop, or a finished picture.
        DisplayCanvasSource::Backdrop => Some(match lane_frame.as_ref() {
            DisplayZoneFrame::Backdrop(backdrop) => (
                DisplayWorkerFrameSet {
                    source: DisplayWorkerFrameSource::Backdrop {
                        frame: Arc::new(backdrop.frame.clone()),
                        viewport: backdrop.viewport,
                        cropped: backdrop.cropped,
                    },
                },
                StableDisplayFrameSetIdentity {
                    source: StableDisplayFrameSourceIdentity::Backdrop {
                        frame: lane_identity,
                        viewport: display_viewport_signature(&backdrop.viewport),
                        cropped: backdrop.cropped,
                    },
                },
            ),
            DisplayZoneFrame::Canvas(_) | DisplayZoneFrame::Yuv420(_) => (
                DisplayWorkerFrameSet {
                    source: DisplayWorkerFrameSource::Direct(Arc::clone(lane_frame)),
                },
                StableDisplayFrameSetIdentity {
                    source: StableDisplayFrameSourceIdentity::Direct(lane_identity),
                },
            ),
        }),
        DisplayCanvasSource::ZoneDirect { .. } => {
            if target.finalized {
                return Some((
                    DisplayWorkerFrameSet {
                        source: DisplayWorkerFrameSource::Direct(Arc::clone(lane_frame)),
                    },
                    StableDisplayFrameSetIdentity {
                        source: StableDisplayFrameSourceIdentity::Direct(lane_identity),
                    },
                ));
            }
            // A face the render thread did not finalize arrives at panel
            // size for the worker to blend over the screen's backdrop.
            let DisplayZoneFrame::Canvas(face_canvas_frame) = lane_frame.as_ref() else {
                return None;
            };
            let backdrop = backdrop_frame.and_then(|frame| match frame.as_ref() {
                DisplayZoneFrame::Backdrop(backdrop) => Some(backdrop),
                DisplayZoneFrame::Canvas(_) | DisplayZoneFrame::Yuv420(_) => None,
            });
            let viewport = backdrop.map_or(target.mount_viewport, |backdrop| backdrop.viewport);
            let backdrop_cropped = backdrop.is_some_and(|backdrop| backdrop.cropped);
            let blended = target
                .blends_with_effect()
                .then_some(backdrop)
                .flatten()
                .and_then(|backdrop| {
                    let identity = stable_display_source_identity(&backdrop.frame)?;
                    Some((identity, Arc::new(backdrop.frame.clone())))
                });
            let (backdrop_identity, backdrop_frame) = blended
                .map(|(identity, frame)| (Some(identity), Some(frame)))
                .unwrap_or((None, None));
            Some((
                DisplayWorkerFrameSet {
                    source: DisplayWorkerFrameSource::Face {
                        backdrop_frame,
                        backdrop_cropped,
                        face_frame: Arc::new(face_canvas_frame.clone()),
                        viewport,
                        blend_mode: target.face_blend_mode(),
                        opacity: target.face_opacity(),
                    },
                },
                StableDisplayFrameSetIdentity {
                    source: StableDisplayFrameSourceIdentity::Face {
                        backdrop_frame: backdrop_identity,
                        backdrop_cropped,
                        face_frame: lane_identity,
                        viewport: display_viewport_signature(&viewport),
                        blend_mode: target.face_blend_mode(),
                        opacity_bits: target.face_opacity().to_bits(),
                    },
                },
            ))
        }
    }
}

/// Hold render output demand while any display target exists, or a
/// previewed simulator waits for the lane that would make it one.
///
/// A screen's frames are read from its lane's sender, not through a
/// receiver, so the render loop would not count the screen as a consumer.
/// Physical displays are already counted as devices; this hold is what
/// keeps a previewed simulated display rendering.
fn sync_output_demand_hold(
    hold: &mut Option<OutputDemandHold>,
    event_bus: &HypercolorBus,
    has_targets: bool,
) {
    if has_targets {
        if hold.is_none() {
            *hold = Some(event_bus.hold_output_demand());
        }
    } else {
        let _ = hold.take();
    }
}

async fn reconcile_display_workers(
    state: &DisplayOutputState,
    workers: &mut HashMap<DisplayWorkerKey, DisplayWorkerHandle>,
    targets: &[Arc<DisplayTarget>],
) {
    let expected_keys = targets
        .iter()
        .map(|target| target.worker_key.clone())
        .collect::<HashSet<_>>();
    let stale_keys = workers
        .keys()
        .filter(|key| !expected_keys.contains(*key))
        .cloned()
        .collect::<Vec<_>>();

    for key in stale_keys {
        if let Some(worker) = workers.remove(&key) {
            retire_display_worker(worker, Arc::clone(&state.display_frames), key.1, true);
        }
    }

    for target in targets {
        let key = target.worker_key.clone();
        let needs_restart = workers.get(&key).is_some_and(|worker| {
            !worker.lane_is_active() || worker.config_signature != target.worker_config_signature()
        });
        if needs_restart && let Some(worker) = workers.remove(&key) {
            retire_display_worker(worker, Arc::clone(&state.display_frames), key.1, false);
        }

        if workers.contains_key(&key) {
            continue;
        }

        let output_lane = {
            let mut manager = state.backend_manager.lock().await;
            manager.display_output_lane(&target.backend_id, target.device_id)
        };

        match output_lane {
            Some(output_lane) => {
                workers.insert(
                    key,
                    DisplayWorkerHandle::spawn(
                        Arc::clone(target),
                        output_lane,
                        state.power_state.clone(),
                        state.static_hold_refresh_interval,
                        Arc::clone(&state.display_frames),
                    ),
                );
            }
            None => {
                warn!(
                    device = %target.name,
                    backend_id = %target.backend_id,
                    device_id = %target.device_id,
                    "display target backend is not registered"
                );
            }
        }
    }
}

fn retire_display_worker(
    worker: DisplayWorkerHandle,
    display_frames: Arc<RwLock<DisplayFrameRuntime>>,
    device_id: DeviceId,
    clear_preview: bool,
) {
    std::mem::drop(tokio::spawn(async move {
        worker.shutdown().await;
        if clear_preview {
            display_frames.write().await.remove(device_id);
        }
    }));
}

async fn display_targets(
    registry: &DeviceRegistry,
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
    event_bus: &Arc<HypercolorBus>,
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    cache: &mut DisplayTargetCache,
    face_fps_cap: u32,
) -> DisplayTargetsSnapshot {
    let (screen_lane_plan_revision, screen_lane_plan) = event_bus.screen_lane_plan_snapshot();
    let (display_zone_targets_revision, published_display_zone_targets) =
        event_bus.display_zone_targets_snapshot();
    let logical_store = logical_devices.read().await;
    let display_preview_subscribers = display_frames.read().await.subscribed_device_ids();
    let registry_generation = registry.generation();
    let logical_signature = logical_device_store_signature(&logical_store);
    let display_preview_signature = device_id_set_signature(&display_preview_subscribers);
    let dependency_key = DisplayTargetDependencyKey::new(
        registry_generation,
        display_zone_targets_revision,
        screen_lane_plan_revision,
    );
    let cache_key =
        DisplayTargetCacheKey::new(dependency_key, logical_signature, display_preview_signature);

    if cache.cache_key == Some(cache_key) {
        return DisplayTargetsSnapshot {
            version: cache.version,
            targets: Arc::clone(&cache.targets),
            awaiting_lanes: cache.awaiting_lanes,
        };
    }

    let lane_targets = display_lane_targets_by_device(published_display_zone_targets);
    let mut targets = Vec::new();
    let mut screen_routes = HashMap::new();
    let mut simulators_without_lane = Vec::new();
    for tracked in registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| tracked.state.is_renderable())
    {
        let metadata = registry.metadata_for_id(&tracked.info.id).await;
        let is_simulator = metadata.as_ref().is_some_and(|metadata| {
            metadata
                .get("simulator")
                .is_some_and(|value| value == "true")
        });
        if is_simulator && !display_preview_subscribers.contains(&tracked.info.id) {
            continue;
        }
        let Some(surface) = tracked.info.display_surface() else {
            continue;
        };
        let device_id = tracked.info.id;
        let geometry = DisplayGeometry {
            width: surface.width,
            height: surface.height,
            circular: surface.circular,
        };
        let frame_format = surface.format;
        // How the panel is mounted is the device's own setting. It turns
        // the screen's viewport wherever a zone places it, and it is all
        // the viewport a face has on a screen no zone places.
        let mount_rotation = tracked.user_settings.display_rotation;
        let mut mount_viewport = default_display_viewport();
        mount_viewport.rotation += mount_rotation.radians();
        let mut layout_device_ids = logical_store
            .iter()
            .filter(|(_, entry)| entry.physical_device_id == device_id)
            .map(|(layout_id, _)| layout_id.clone())
            .collect::<Vec<_>>();
        layout_device_ids.sort_unstable();
        let backdrop_source = DisplayCanvasSource::Backdrop;
        screen_routes.insert(
            device_id,
            ScreenBackdropRoute {
                layout_device_ids,
                has_led_segments: tracked.info.segments.iter().any(|segment| {
                    segment.led_count > 0
                        && !matches!(segment.topology, DeviceTopologyHint::Display { .. })
                }),
                output: display_zone_output_route(
                    device_id,
                    geometry,
                    tracked.user_settings.brightness,
                    frame_format,
                    mount_viewport,
                ),
                target_fps: capped_display_target_fps(
                    tracked.info.capabilities.max_fps,
                    &backdrop_source,
                    frame_format,
                    face_fps_cap,
                ),
            },
        );

        // The render thread decides what a screen shows and publishes it on
        // a lane: a face, or the backdrop of the zone that places the
        // screen. A screen with neither is not driven.
        let backdrop_lane = screen_backdrop_lane_id(device_id);
        let face = lane_targets.faces.get(&device_id);
        let has_backdrop_lane = lane_targets.backdrops.contains(&device_id);
        let (canvas_source, lane_id) = match face {
            Some(face) => (
                DisplayCanvasSource::ZoneDirect {
                    zone_id: face.zone_id,
                },
                face.zone_id,
            ),
            None if has_backdrop_lane => (backdrop_source, backdrop_lane),
            None => {
                if is_simulator {
                    simulators_without_lane.push(device_id);
                }
                continue;
            }
        };
        debug!(
            %device_id,
            face_zone = ?face.map(|binding| binding.zone_id),
            has_backdrop_lane,
            ?mount_rotation,
            face_bindings = lane_targets.faces.len(),
            "display target resolved"
        );

        let backend_id = tracked.info.output_backend_id().to_owned();
        targets.push(Arc::new(DisplayTarget {
            worker_key: (backend_id.clone(), device_id),
            backend_id,
            device_id,
            name: tracked.info.name,
            target_fps: capped_display_target_fps(
                tracked.info.capabilities.max_fps,
                &canvas_source,
                frame_format,
                face_fps_cap,
            ),
            brightness: tracked.user_settings.brightness,
            geometry,
            frame_format,
            max_frame_len: tracked.info.capabilities.features.max_display_frame_len,
            lane_sender: event_bus.zone_canvas_sender(lane_id),
            backdrop_sender: (face.is_some() && has_backdrop_lane)
                .then(|| event_bus.zone_canvas_sender(backdrop_lane)),
            canvas_source,
            display_target: face.map(|binding| binding.target.clone()),
            finalized: face.is_some_and(|binding| binding.finalized),
            mount_viewport,
        }));
    }

    targets.sort_by(|left, right| {
        left.backend_id
            .cmp(&right.backend_id)
            .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
    });
    publish_display_zone_output_routes(event_bus, &targets);
    event_bus.replace_screen_backdrop_routes(screen_routes);
    // A lane is awaited while the render thread has yet to place this set
    // of screens, or has placed it and is making the simulator a lane that
    // has not arrived. A set it placed with no lane for the simulator has
    // nothing coming, until the set or the render thread's answer changes.
    let routes_revision = event_bus.screen_backdrop_routes_revision();
    let awaiting_lanes = simulators_without_lane.iter().any(|device_id| {
        screen_lane_plan.as_ref().is_none_or(|plan| {
            plan.routes_revision != routes_revision || plan.screens.contains(device_id)
        })
    });
    cache.version = cache.version.saturating_add(1);
    cache.cache_key = Some(cache_key);
    cache.targets = Arc::from(targets);
    cache.awaiting_lanes = awaiting_lanes;
    DisplayTargetsSnapshot {
        version: cache.version,
        targets: Arc::clone(&cache.targets),
        awaiting_lanes,
    }
}

fn display_zone_output_route(
    device_id: DeviceId,
    geometry: DisplayGeometry,
    brightness: f32,
    frame_format: DisplayFrameFormat,
    viewport: DisplayViewport,
) -> DisplayZoneOutputRoute {
    DisplayZoneOutputRoute {
        device_id,
        width: geometry.width,
        height: geometry.height,
        circular: geometry.circular,
        brightness,
        frame_format,
        viewport,
    }
}

fn publish_display_zone_output_routes(event_bus: &HypercolorBus, targets: &[Arc<DisplayTarget>]) {
    let mut active_zone_ids = Vec::new();
    for target in targets {
        let DisplayCanvasSource::ZoneDirect { zone_id } = target.canvas_source else {
            continue;
        };
        active_zone_ids.push(zone_id);
        event_bus.upsert_display_zone_output_route(
            zone_id,
            display_zone_output_route(
                target.device_id,
                target.geometry,
                target.brightness,
                target.frame_format,
                target.mount_viewport,
            ),
        );
    }
    event_bus.retain_display_zone_output_routes(&active_zone_ids);
}

/// The display lanes the render thread publishes, by the screen they feed.
#[derive(Default)]
struct DisplayLaneTargets {
    /// The face lane each screen shows.
    faces: HashMap<DeviceId, DisplayFaceTargetBinding>,
    /// Screens with a backdrop lane.
    backdrops: HashSet<DeviceId>,
}

/// Sort the published lanes into faces and backdrops, and pick one face per
/// device: a finalized zone beats a plain one, and among equals the newer
/// zone id wins.
fn display_lane_targets_by_device(
    targets: HashMap<ZoneId, hypercolor_core::bus::DisplayZoneTarget>,
) -> DisplayLaneTargets {
    let mut lanes = DisplayLaneTargets::default();
    for (zone_id, target) in targets {
        if zone_id == screen_backdrop_lane_id(target.device_id) {
            lanes.backdrops.insert(target.device_id);
            continue;
        }
        let binding = DisplayFaceTargetBinding {
            zone_id,
            target: DisplayFaceTarget {
                device_id: target.device_id,
                blend_mode: target.blend_mode,
                opacity: target.opacity,
            },
            finalized: target.finalized,
        };

        match lanes.faces.entry(target.device_id) {
            Entry::Vacant(entry) => {
                entry.insert(binding);
            }
            Entry::Occupied(mut entry) => {
                let current = entry.get();
                let replace = display_face_target_binding_preferred(&binding, current);
                debug!(
                    device_id = %target.device_id,
                    current_zone_id = %current.zone_id,
                    candidate_zone_id = %binding.zone_id,
                    current_finalized = current.finalized,
                    candidate_finalized = binding.finalized,
                    selected_zone_id = %if replace { binding.zone_id } else { current.zone_id },
                    "resolved duplicate display face targets for one device"
                );
                if replace {
                    entry.insert(binding);
                }
            }
        }
    }
    lanes
}

fn display_face_target_binding_preferred(
    candidate: &DisplayFaceTargetBinding,
    current: &DisplayFaceTargetBinding,
) -> bool {
    (candidate.finalized && !current.finalized)
        || (candidate.finalized == current.finalized && candidate.zone_id.0 > current.zone_id.0)
}

fn default_display_viewport() -> DisplayViewport {
    DisplayViewport {
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        edge_behavior: EdgeBehavior::Clamp,
    }
}

/// The frame rate a screen is driven at.
///
/// A face takes the face cadence. A screen showing its backdrop alone keeps
/// the cap a mirrored canvas has always had: an encoded panel takes fewer
/// frames than a raw one.
fn capped_display_target_fps(
    device_max_fps: u32,
    canvas_source: &DisplayCanvasSource,
    frame_format: DisplayFrameFormat,
    face_fps_cap: u32,
) -> u32 {
    match canvas_source {
        DisplayCanvasSource::ZoneDirect { .. } => {
            capped_zone_direct_display_target_fps(device_max_fps, face_fps_cap)
        }
        DisplayCanvasSource::Backdrop => {
            let max_fps = if frame_format == DisplayFrameFormat::Rgb {
                RAW_DISPLAY_OUTPUT_MAX_FPS
            } else {
                DISPLAY_OUTPUT_MAX_FPS
            };
            let device_limit = if device_max_fps == 0 {
                max_fps
            } else {
                device_max_fps
            };

            device_limit.clamp(1, max_fps)
        }
    }
}

pub(crate) fn capped_zone_direct_display_target_fps(device_max_fps: u32, face_fps_cap: u32) -> u32 {
    let device_limit = if device_max_fps == 0 {
        DISPLAY_FACE_DEFAULT_FPS
    } else {
        device_max_fps
    };

    // The cap defaults to the conservative 30 fps face baseline; faster
    // panels only upshift when the user raises display.face_fps_cap, so the
    // render/composite/JPEG churn of a blind 60 fps inherit stays opt-in.
    device_limit.clamp(1, face_fps_cap)
}

fn logical_device_store_signature(store: &HashMap<String, LogicalDevice>) -> u64 {
    let mut combined = u64::try_from(store.len()).unwrap_or(u64::MAX);
    for entry in store.values() {
        let mut hasher = DefaultHasher::new();
        entry.id.hash(&mut hasher);
        entry.physical_device_id.hash(&mut hasher);
        combined ^= hasher.finish().rotate_left(1);
    }

    combined
}

fn device_id_set_signature(device_ids: &HashSet<DeviceId>) -> u64 {
    let mut combined = u64::try_from(device_ids.len()).unwrap_or(u64::MAX);
    for device_id in device_ids {
        let mut hasher = DefaultHasher::new();
        device_id.hash(&mut hasher);
        combined ^= hasher.finish().rotate_left(1);
    }

    combined
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
    use hypercolor_core::bus::{CanvasFrame, DisplayZoneFrame, DisplayZoneTarget};
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::device::DeviceId;
    use uuid::Uuid;

    use super::*;

    fn zone_canvas_frame(frame_number: u32) -> Arc<DisplayZoneFrame> {
        Arc::new(DisplayZoneFrame::Canvas(CanvasFrame::from_owned_canvas(
            Canvas::new(2, 2),
            frame_number,
            frame_number,
        )))
    }

    #[test]
    fn display_output_holds_render_demand_exactly_while_it_has_targets() {
        let event_bus = HypercolorBus::new();
        let mut hold = None;

        sync_output_demand_hold(&mut hold, &event_bus, true);
        assert_eq!(event_bus.output_demand().hold_count(), 1);
        sync_output_demand_hold(&mut hold, &event_bus, true);
        assert_eq!(
            event_bus.output_demand().hold_count(),
            1,
            "a held demand is not taken twice"
        );

        sync_output_demand_hold(&mut hold, &event_bus, false);
        assert_eq!(event_bus.output_demand().hold_count(), 0);
        assert!(hold.is_none());
    }

    fn display_target(blend_mode: BlendMode) -> DisplayTarget {
        let device_id = DeviceId::new();
        let (lane_sender, _) = watch::channel(DisplayZoneFrame::empty());
        DisplayTarget {
            worker_key: ("test".into(), device_id),
            backend_id: "test".into(),
            device_id,
            name: "Display".into(),
            target_fps: DISPLAY_FACE_DEFAULT_FPS,
            brightness: 1.0,
            geometry: DisplayGeometry {
                width: 2,
                height: 2,
                circular: false,
            },
            frame_format: DisplayFrameFormat::Jpeg,
            max_frame_len: None,
            canvas_source: DisplayCanvasSource::ZoneDirect {
                zone_id: ZoneId::new(),
            },
            lane_sender,
            backdrop_sender: None,
            display_target: Some(DisplayFaceTarget {
                device_id,
                blend_mode,
                opacity: 0.5,
            }),
            finalized: false,
            mount_viewport: default_display_viewport(),
        }
    }

    /// A zone canvas the way a compositor that leaves the crop to the
    /// worker publishes it: with the owning zone's viewport for the screen.
    fn zone_backdrop_frame(frame_number: u32, viewport: DisplayViewport) -> Arc<DisplayZoneFrame> {
        Arc::new(DisplayZoneFrame::Backdrop(
            hypercolor_core::bus::DisplayBackdropFrame {
                frame: CanvasFrame::from_owned_canvas(
                    Canvas::new(2, 2),
                    frame_number,
                    frame_number,
                ),
                viewport,
                cropped: false,
            },
        ))
    }

    /// A screen's box somewhere other than the whole canvas.
    fn placed_viewport() -> DisplayViewport {
        DisplayViewport {
            position: hypercolor_types::spatial::NormalizedPosition::new(0.25, 0.75),
            size: hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.25),
            rotation: 0.5,
            ..default_display_viewport()
        }
    }

    fn backdrop_target() -> DisplayTarget {
        let mut target = display_target(BlendMode::Replace);
        target.canvas_source = DisplayCanvasSource::Backdrop;
        target.display_target = None;
        target
    }

    fn fixed_device_id(value: u128) -> DeviceId {
        DeviceId::from_uuid(Uuid::from_u128(value))
    }

    fn fixed_zone_id(value: u128) -> ZoneId {
        ZoneId(Uuid::from_u128(value))
    }

    fn display_zone_target(device_id: DeviceId, finalized: bool) -> DisplayZoneTarget {
        DisplayZoneTarget {
            device_id,
            blend_mode: BlendMode::Replace,
            opacity: 1.0,
            finalized,
        }
    }

    #[test]
    fn face_fps_cap_default_keeps_zone_direct_at_thirty() {
        // Default cap, fast panel: today's 30 fps baseline is unchanged.
        assert_eq!(capped_zone_direct_display_target_fps(60, 30), 30);
        // Default cap, device with no declared limit.
        assert_eq!(capped_zone_direct_display_target_fps(0, 30), 30);
        // Default cap, slow panel: device limit still wins below the cap.
        assert_eq!(capped_zone_direct_display_target_fps(15, 30), 15);
    }

    #[test]
    fn face_fps_cap_raised_to_sixty_is_honored_when_device_allows() {
        assert_eq!(capped_zone_direct_display_target_fps(60, 60), 60);
        // Device that never declared a limit stays on the 30 fps default.
        assert_eq!(capped_zone_direct_display_target_fps(0, 60), 30);
        // Device limit below the cap still wins.
        assert_eq!(capped_zone_direct_display_target_fps(24, 60), 24);
    }

    #[test]
    fn capped_display_target_fps_zone_direct_consults_face_cap() {
        let zone_direct = DisplayCanvasSource::ZoneDirect {
            zone_id: fixed_zone_id(1),
        };

        assert_eq!(
            capped_display_target_fps(60, &zone_direct, DisplayFrameFormat::Jpeg, 30),
            30
        );
        assert_eq!(
            capped_display_target_fps(60, &zone_direct, DisplayFrameFormat::Jpeg, 60),
            60
        );
    }

    #[test]
    fn capped_display_target_fps_backdrop_keeps_its_cap_whatever_the_face_cap() {
        let backdrop = DisplayCanvasSource::Backdrop;

        assert_eq!(
            capped_display_target_fps(60, &backdrop, DisplayFrameFormat::Jpeg, 60),
            DISPLAY_OUTPUT_MAX_FPS
        );
        assert_eq!(
            capped_display_target_fps(60, &backdrop, DisplayFrameFormat::Rgb, 60),
            RAW_DISPLAY_OUTPUT_MAX_FPS
        );
        assert_eq!(
            capped_display_target_fps(0, &backdrop, DisplayFrameFormat::Rgb, 60),
            RAW_DISPLAY_OUTPUT_MAX_FPS
        );
        assert_eq!(
            capped_display_target_fps(10, &backdrop, DisplayFrameFormat::Jpeg, 60),
            10,
            "a slower panel still limits its own backdrop"
        );
    }

    #[test]
    fn finalized_backdrop_goes_to_the_worker_as_it_is() {
        let target = backdrop_target();
        let frame = zone_canvas_frame(1);

        let (frames, identity) = build_display_worker_frame_set(&target, Some(&frame), None)
            .expect("a finalized backdrop should build a frame set");

        let DisplayWorkerFrameSource::Direct(published) = frames.source else {
            panic!("a finalized backdrop should use the direct frame source");
        };
        assert_eq!(
            stable_display_zone_source_identity(published.as_ref()),
            stable_display_zone_source_identity(frame.as_ref())
        );
        assert!(matches!(
            identity.source,
            StableDisplayFrameSourceIdentity::Direct(_)
        ));
    }

    /// A zone canvas is cropped through the viewport it arrived with, and a
    /// viewport that moves is a new picture even over the same canvas.
    #[test]
    fn zone_canvas_backdrop_is_cropped_by_the_worker_through_its_own_viewport() {
        let target = backdrop_target();
        let frame = zone_backdrop_frame(1, placed_viewport());

        let (frames, identity) = build_display_worker_frame_set(&target, Some(&frame), None)
            .expect("a zone canvas backdrop should build a frame set");

        let DisplayWorkerFrameSource::Backdrop {
            frame: published,
            viewport,
            cropped,
        } = frames.source
        else {
            panic!("a zone canvas backdrop should use the backdrop frame source");
        };
        assert_eq!(
            stable_display_source_identity(published.as_ref()),
            stable_display_zone_source_identity(frame.as_ref())
        );
        assert_eq!(viewport, placed_viewport());
        assert!(!cropped);

        let DisplayZoneFrame::Backdrop(backdrop) = frame.as_ref() else {
            unreachable!("the helper builds a backdrop frame");
        };
        let moved = Arc::new(DisplayZoneFrame::Backdrop(
            hypercolor_core::bus::DisplayBackdropFrame {
                frame: backdrop.frame.clone(),
                viewport: default_display_viewport(),
                cropped: false,
            },
        ));
        let (_, moved_identity) = build_display_worker_frame_set(&target, Some(&moved), None)
            .expect("the moved backdrop should build a frame set");
        assert_ne!(identity, moved_identity);
    }

    #[test]
    fn a_screen_waits_for_its_lane_to_publish() {
        assert!(build_display_worker_frame_set(&backdrop_target(), None, None).is_none());
        assert!(
            build_display_worker_frame_set(&display_target(BlendMode::Alpha), None, None).is_none()
        );
    }

    #[test]
    fn direct_display_face_uses_unified_face_frame_source_without_backdrop() {
        let target = display_target(BlendMode::Replace);
        let face_frame = zone_canvas_frame(1);
        let backdrop_frame = zone_canvas_frame(2);

        let (frames, identity) =
            build_display_worker_frame_set(&target, Some(&face_frame), Some(&backdrop_frame))
                .expect("direct face should not require a backdrop frame");

        let DisplayWorkerFrameSource::Face {
            backdrop_frame,
            face_frame: published_face,
            blend_mode,
            opacity,
            ..
        } = frames.source
        else {
            panic!("display face should use the unified face frame source");
        };
        let StableDisplayFrameSourceIdentity::Face {
            backdrop_frame: backdrop_identity,
            blend_mode: identity_blend_mode,
            opacity_bits,
            ..
        } = identity.source
        else {
            panic!("display face identity should use the unified face identity");
        };

        assert!(
            backdrop_frame.is_none(),
            "a face that replaces its backdrop should not carry one"
        );
        assert!(backdrop_identity.is_none());
        assert_eq!(
            stable_display_source_identity(published_face.as_ref()),
            stable_display_zone_source_identity(face_frame.as_ref())
        );
        assert_eq!(blend_mode, BlendMode::Replace);
        assert_eq!(identity_blend_mode, BlendMode::Replace);
        assert_eq!(opacity, 1.0);
        assert_eq!(opacity_bits, 1.0_f32.to_bits());
    }

    #[test]
    fn blended_display_face_uses_same_face_frame_source_with_its_backdrop() {
        let target = display_target(BlendMode::Alpha);
        let backdrop_frame = zone_backdrop_frame(1, placed_viewport());
        let backdrop_identity = stable_display_zone_source_identity(backdrop_frame.as_ref())
            .expect("backdrop should be stable");
        let face_frame = zone_canvas_frame(2);

        let (frames, identity) =
            build_display_worker_frame_set(&target, Some(&face_frame), Some(&backdrop_frame))
                .expect("blended face should build when backdrop and face are available");

        let DisplayWorkerFrameSource::Face {
            backdrop_frame,
            face_frame: published_face,
            viewport,
            blend_mode,
            opacity,
            ..
        } = frames.source
        else {
            panic!("blended display face should use the unified face frame source");
        };
        assert_eq!(
            viewport,
            placed_viewport(),
            "a face turns with the viewport its screen's backdrop arrived with"
        );
        let StableDisplayFrameSourceIdentity::Face {
            backdrop_frame: identity_backdrop,
            blend_mode: identity_blend_mode,
            opacity_bits,
            ..
        } = identity.source
        else {
            panic!("blended display face identity should use the unified face identity");
        };

        assert!(backdrop_frame.is_some());
        assert_eq!(
            stable_display_source_identity(published_face.as_ref()),
            stable_display_zone_source_identity(face_frame.as_ref())
        );
        assert_eq!(identity_backdrop, Some(backdrop_identity));
        assert_eq!(blend_mode, BlendMode::Alpha);
        assert_eq!(identity_blend_mode, BlendMode::Alpha);
        assert_eq!(opacity, 0.5);
        assert_eq!(opacity_bits, 0.5_f32.to_bits());
    }

    #[test]
    fn blended_display_face_composes_against_black_without_backdrop_frame() {
        let target = display_target(BlendMode::Alpha);
        let face_frame = zone_canvas_frame(1);

        let (frames, identity) = build_display_worker_frame_set(&target, Some(&face_frame), None)
            .expect("blended face should not wait for a backdrop frame");

        let DisplayWorkerFrameSource::Face {
            backdrop_frame,
            face_frame: published_face,
            blend_mode,
            opacity,
            ..
        } = frames.source
        else {
            panic!("blended display face should use the unified face frame source");
        };
        let StableDisplayFrameSourceIdentity::Face {
            backdrop_frame: identity_backdrop,
            blend_mode: identity_blend_mode,
            opacity_bits,
            ..
        } = identity.source
        else {
            panic!("blended display face identity should use the unified face identity");
        };

        assert!(backdrop_frame.is_none());
        assert!(identity_backdrop.is_none());
        assert_eq!(
            stable_display_source_identity(published_face.as_ref()),
            stable_display_zone_source_identity(face_frame.as_ref())
        );
        assert_eq!(blend_mode, BlendMode::Alpha);
        assert_eq!(identity_blend_mode, BlendMode::Alpha);
        assert_eq!(opacity, 0.5);
        assert_eq!(opacity_bits, 0.5_f32.to_bits());
    }

    #[test]
    fn finalized_display_face_uses_direct_frame_without_backdrop() {
        let mut target = display_target(BlendMode::Alpha);
        target.finalized = true;
        let face_frame = zone_canvas_frame(1);

        let (frames, identity) = build_display_worker_frame_set(&target, Some(&face_frame), None)
            .expect("finalized face should not require a backdrop frame");

        let DisplayWorkerFrameSource::Direct(published_face) = frames.source else {
            panic!("finalized display face should use the direct frame source");
        };
        let StableDisplayFrameSourceIdentity::Direct(source_identity) = identity.source else {
            panic!("finalized display face identity should use the direct identity");
        };

        assert_eq!(
            stable_display_zone_source_identity(published_face.as_ref()),
            stable_display_zone_source_identity(face_frame.as_ref())
        );
        assert_eq!(
            source_identity,
            stable_display_zone_source_identity(face_frame.as_ref())
                .expect("face should be stable")
        );
    }

    #[test]
    fn finalized_display_face_does_not_change_worker_config_signature() {
        let mut target = display_target(BlendMode::Alpha);
        let unfinalized = target.worker_config_signature();

        target.finalized = true;

        assert_eq!(target.worker_config_signature(), unfinalized);
    }

    #[test]
    fn display_face_target_resolution_prefers_finalized_binding() {
        let device_id = fixed_device_id(1);
        let old_finalized = fixed_zone_id(1);
        let new_pending = fixed_zone_id(2);
        let targets = std::collections::HashMap::from([
            (new_pending, display_zone_target(device_id, false)),
            (old_finalized, display_zone_target(device_id, true)),
        ]);

        let resolved = display_lane_targets_by_device(targets);

        let binding = resolved
            .faces
            .get(&device_id)
            .expect("device target should resolve");
        assert_eq!(binding.zone_id, old_finalized);
        assert!(binding.finalized);
    }

    #[test]
    fn display_face_target_resolution_prefers_newest_equivalent_binding() {
        let device_id = fixed_device_id(2);
        let older = fixed_zone_id(1);
        let newer = fixed_zone_id(2);
        let targets = std::collections::HashMap::from([
            (older, display_zone_target(device_id, true)),
            (newer, display_zone_target(device_id, true)),
        ]);

        let resolved = display_lane_targets_by_device(targets);

        let binding = resolved
            .faces
            .get(&device_id)
            .expect("device target should resolve");
        assert_eq!(binding.zone_id, newer);
        assert!(binding.finalized);
    }

    #[test]
    fn a_backdrop_lane_never_stands_in_for_a_face() {
        let with_face = fixed_device_id(3);
        let backdrop_only = fixed_device_id(4);
        let face_zone = fixed_zone_id(7);
        let targets = std::collections::HashMap::from([
            (face_zone, display_zone_target(with_face, false)),
            (
                screen_backdrop_lane_id(with_face),
                display_zone_target(with_face, false),
            ),
            (
                screen_backdrop_lane_id(backdrop_only),
                display_zone_target(backdrop_only, true),
            ),
        ]);

        let resolved = display_lane_targets_by_device(targets);

        assert_eq!(
            resolved.faces.get(&with_face).map(|face| face.zone_id),
            Some(face_zone)
        );
        assert!(!resolved.faces.contains_key(&backdrop_only));
        assert!(resolved.backdrops.contains(&backdrop_only));
        assert!(
            resolved.backdrops.contains(&with_face),
            "a face's screen keeps its backdrop lane beside the face"
        );
    }
}
