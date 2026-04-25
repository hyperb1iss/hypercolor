//! Automatic display output pipeline for LCD-capable devices.
//!
//! This task renders the latest canvas into layout-mapped display zones without
//! disturbing the existing LED frame routing path.

mod encode;
mod render;
mod worker;

use std::any::Any;
use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, oneshot, watch};
use tracing::{debug, info, trace, warn};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::device::{DeviceId, DeviceTopologyHint, DisplayFrameFormat};
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, RenderGroupId};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SpatialLayout};

use self::render::display_viewport_signature;
use crate::discovery::backend_id_for_device;
use crate::display_frames::DisplayFrameRuntime;
use crate::logical_devices::LogicalDevice;
use crate::preview_runtime::PreviewRuntime;
use crate::session::OutputPowerState;
use worker::DisplayWorkerHandle;

const DISPLAY_ERROR_WARN_INTERVAL: Duration = Duration::from_secs(5);
const DISPLAY_OUTPUT_MAX_FPS: u32 = 15;
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
    /// Active spatial layout used to decide which LCDs should render and how.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,
    /// Logical-device mappings used to match physical devices to layout zones.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    /// Event bus canvas stream produced by the render thread.
    pub event_bus: Arc<HypercolorBus>,
    /// Internal preview-demand accounting shared with the render thread.
    pub preview_runtime: Arc<PreviewRuntime>,
    /// Session power policy used to decide whether static hold refresh is active.
    pub power_state: watch::Receiver<OutputPowerState>,
    /// How often unchanged display frames should be reasserted during static hold.
    pub static_hold_refresh_interval: Duration,
    /// Latest composited JPEG frames published per device for preview surfaces.
    pub display_frames: Arc<RwLock<DisplayFrameRuntime>>,
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
    preview_subscribed: bool,
    canvas_source: DisplayCanvasSource,
    group_canvas_sender: Option<watch::Sender<CanvasFrame>>,
    display_target: Option<DisplayFaceTarget>,
    viewport: DisplayViewport,
}

type DisplayWorkerKey = (String, DeviceId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DisplayWorkerConfigSignature {
    target_fps: u32,
    brightness_bits: u32,
    geometry: DisplayGeometry,
    frame_format: DisplayFrameFormat,
    preview_subscribed: bool,
    canvas_source: DisplayCanvasSourceSignature,
    face_blend_mode: DisplayFaceBlendMode,
    face_opacity_bits: u32,
    viewport: DisplayViewportSignature,
}

#[derive(Default)]
struct DisplayTargetCache {
    version: u64,
    cache_key: Option<DisplayTargetCacheKey>,
    targets: Arc<[Arc<DisplayTarget>]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayTargetDependencyKey {
    registry_generation: u64,
    display_group_targets_revision: u64,
}

impl DisplayTargetDependencyKey {
    const fn new(registry_generation: u64, display_group_targets_revision: u64) -> Self {
        Self {
            registry_generation,
            display_group_targets_revision,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayTargetCacheKey {
    dependency_key: DisplayTargetDependencyKey,
    layout_ptr: usize,
    logical_signature: u64,
    display_preview_signature: u64,
}

impl DisplayTargetCacheKey {
    const fn new(
        dependency_key: DisplayTargetDependencyKey,
        layout_ptr: usize,
        logical_signature: u64,
        display_preview_signature: u64,
    ) -> Self {
        Self {
            dependency_key,
            layout_ptr,
            logical_signature,
            display_preview_signature,
        }
    }
}

#[derive(Clone, Debug)]
enum DisplayCanvasSource {
    Scene,
    GroupDirect { group_id: RenderGroupId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DisplayCanvasSourceSignature {
    Scene,
    GroupDirect { group_id: RenderGroupId },
}

#[derive(Clone, Copy, Debug)]
struct DisplayViewport {
    position: NormalizedPosition,
    size: NormalizedPosition,
    rotation: f32,
    scale: f32,
    edge_behavior: EdgeBehavior,
}

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
}

#[derive(Clone, Debug)]
pub(super) enum DisplayWorkerFrameSource {
    Scene(Arc<CanvasFrame>),
    Face {
        scene_frame: Option<Arc<CanvasFrame>>,
        face_frame: Arc<CanvasFrame>,
        blend_mode: DisplayFaceBlendMode,
        opacity: f32,
    },
}

#[derive(Clone, Debug)]
pub(super) struct DisplayWorkerFrameSet {
    pub source: DisplayWorkerFrameSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplaySourceIdentity {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StableDisplayFrameSetIdentity {
    source: StableDisplayFrameSourceIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StableDisplayFrameSourceIdentity {
    Scene(DisplaySourceIdentity),
    Face {
        scene_frame: Option<DisplaySourceIdentity>,
        face_frame: DisplaySourceIdentity,
        blend_mode: DisplayFaceBlendMode,
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
            preview_subscribed: self.preview_subscribed,
            canvas_source: self.canvas_source.signature(),
            face_blend_mode: self.face_blend_mode(),
            face_opacity_bits: self.face_opacity().to_bits(),
            viewport: display_viewport_signature(&self.viewport),
        }
    }

    fn face_blend_mode(&self) -> DisplayFaceBlendMode {
        self.display_target
            .as_ref()
            .map_or(DisplayFaceBlendMode::Replace, |target| target.blend_mode)
    }

    fn blends_with_effect(&self) -> bool {
        self.face_blend_mode().blends_with_effect()
    }

    fn face_opacity(&self) -> f32 {
        if !self.blends_with_effect() {
            return 1.0;
        }
        self.display_target
            .as_ref()
            .map_or(1.0, |target| target.opacity.clamp(0.0, 1.0))
    }

    fn requires_scene_canvas(&self) -> bool {
        matches!(self.canvas_source, DisplayCanvasSource::Scene) || self.blends_with_effect()
    }
}

impl DisplayCanvasSource {
    fn signature(&self) -> DisplayCanvasSourceSignature {
        match self {
            Self::Scene => DisplayCanvasSourceSignature::Scene,
            Self::GroupDirect { group_id } => DisplayCanvasSourceSignature::GroupDirect {
                group_id: *group_id,
            },
        }
    }

    fn is_group_direct(&self) -> bool {
        matches!(self, Self::GroupDirect { .. })
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
        &state.spatial_engine,
        &state.logical_devices,
        &state.event_bus,
        &state.display_frames,
        &mut targets_cache,
    )
    .await;
    let mut scene_canvas_rx = display_requires_scene_canvas(initial_targets.targets.as_ref())
        .then(|| state.event_bus.scene_canvas_receiver());
    reconcile_display_workers(&state, &mut workers, initial_targets.targets.as_ref()).await;
    let mut last_reconciled_target_version = Some(initial_targets.version);
    let mut last_dispatched_sources =
        HashMap::<DisplayWorkerKey, StableDisplayFrameSetIdentity>::new();
    let mut dispatch_tick = tokio::time::interval(DISPLAY_OUTPUT_DISPATCH_INTERVAL);
    dispatch_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if let Some(scene_canvas_rx) = scene_canvas_rx.as_mut() {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("display output task shutting down");
                    break;
                }
                changed = scene_canvas_rx.changed() => {
                    if changed.is_err() {
                        debug!("display output task exiting because canvas stream closed");
                        break;
                    }
                    let _ = scene_canvas_rx.borrow_and_update();
                }
                _ = dispatch_tick.tick() => {
                }
            }
        } else {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("display output task shutting down");
                    break;
                }
                _ = dispatch_tick.tick() => {
                }
            }
        }

        let targets = display_targets(
            &state.device_registry,
            &state.spatial_engine,
            &state.logical_devices,
            &state.event_bus,
            &state.display_frames,
            &mut targets_cache,
        )
        .await;
        if last_reconciled_target_version != Some(targets.version) {
            reconcile_display_workers(&state, &mut workers, targets.targets.as_ref()).await;
            last_reconciled_target_version = Some(targets.version);
            last_dispatched_sources.clear();
        }
        sync_display_canvas_receiver(
            &mut scene_canvas_rx,
            state.event_bus.as_ref(),
            display_requires_scene_canvas(targets.targets.as_ref()),
        );
        if targets.targets.is_empty() {
            last_dispatched_sources.clear();
            continue;
        }

        let scene_frame = scene_canvas_rx.as_ref().and_then(|scene_canvas_rx| {
            let frame = scene_canvas_rx.borrow();
            stable_display_source_identity(&frame)
                .map(|identity| (identity, Arc::new(frame.clone())))
        });

        for target in targets.targets.iter() {
            let face_frame = match &target.canvas_source {
                DisplayCanvasSource::Scene => None,
                DisplayCanvasSource::GroupDirect { .. } => {
                    let Some(sender) = target.group_canvas_sender.as_ref() else {
                        continue;
                    };
                    let frame = sender.borrow();
                    stable_display_source_identity(&frame).map(|_| Arc::new(frame.clone()))
                }
            };
            let Some((frames, dispatch_identity)) = build_display_worker_frame_set(
                target.as_ref(),
                scene_frame.as_ref(),
                face_frame.as_ref(),
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

fn stable_display_source_identity(frame: &CanvasFrame) -> Option<DisplaySourceIdentity> {
    let surface = frame.surface();
    (frame.width > 0 && frame.height > 0).then_some(DisplaySourceIdentity {
        generation: surface.generation(),
        storage: surface.storage_identity(),
        width: frame.width,
        height: frame.height,
    })
}

fn build_display_worker_frame_set(
    target: &DisplayTarget,
    scene_frame: Option<&(DisplaySourceIdentity, Arc<CanvasFrame>)>,
    face_frame: Option<&Arc<CanvasFrame>>,
) -> Option<(DisplayWorkerFrameSet, StableDisplayFrameSetIdentity)> {
    match &target.canvas_source {
        DisplayCanvasSource::Scene => {
            let (frame_identity, frame) = scene_frame?;
            Some((
                DisplayWorkerFrameSet {
                    source: DisplayWorkerFrameSource::Scene(Arc::clone(frame)),
                },
                StableDisplayFrameSetIdentity {
                    source: StableDisplayFrameSourceIdentity::Scene(*frame_identity),
                },
            ))
        }
        DisplayCanvasSource::GroupDirect { .. } => {
            let face_frame = face_frame?;
            let face_identity = stable_display_source_identity(face_frame.as_ref())?;
            let scene = if target.blends_with_effect() {
                let Some((scene_identity, scene_frame)) = scene_frame else {
                    trace!(
                        backend_id = %target.backend_id,
                        device_id = %target.device_id,
                        group_canvas = true,
                        "skipping blended display face until a matching scene frame is available"
                    );
                    return None;
                };
                Some((*scene_identity, Arc::clone(scene_frame)))
            } else {
                None
            };
            let (scene_identity, scene_frame) = scene
                .map(|(identity, frame)| (Some(identity), Some(frame)))
                .unwrap_or((None, None));
            Some((
                DisplayWorkerFrameSet {
                    source: DisplayWorkerFrameSource::Face {
                        scene_frame,
                        face_frame: Arc::clone(face_frame),
                        blend_mode: target.face_blend_mode(),
                        opacity: target.face_opacity(),
                    },
                },
                StableDisplayFrameSetIdentity {
                    source: StableDisplayFrameSourceIdentity::Face {
                        scene_frame: scene_identity,
                        face_frame: face_identity,
                        blend_mode: target.face_blend_mode(),
                        opacity_bits: target.face_opacity().to_bits(),
                    },
                },
            ))
        }
    }
}

fn sync_display_canvas_receiver(
    receiver: &mut Option<watch::Receiver<CanvasFrame>>,
    event_bus: &HypercolorBus,
    subscribe: bool,
) {
    if subscribe {
        if receiver.is_none() {
            *receiver = Some(event_bus.scene_canvas_receiver());
        }
    } else {
        let _ = receiver.take();
    }
}

fn display_requires_scene_canvas(targets: &[Arc<DisplayTarget>]) -> bool {
    targets.iter().any(|target| target.requires_scene_canvas())
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
            worker.shutdown().await;
        }
    }

    for target in targets {
        let key = target.worker_key.clone();
        let needs_restart = workers
            .get(&key)
            .is_some_and(|worker| worker.config_signature != target.worker_config_signature());
        if needs_restart && let Some(worker) = workers.remove(&key) {
            worker.shutdown().await;
        }

        if workers.contains_key(&key) {
            continue;
        }

        let backend_io = {
            let manager = state.backend_manager.lock().await;
            manager.backend_io(&target.backend_id)
        };

        match backend_io {
            Some(backend_io) => {
                workers.insert(
                    key,
                    DisplayWorkerHandle::spawn(
                        Arc::clone(target),
                        backend_io,
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

async fn display_targets(
    registry: &DeviceRegistry,
    spatial_engine: &Arc<RwLock<SpatialEngine>>,
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
    event_bus: &Arc<HypercolorBus>,
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    cache: &mut DisplayTargetCache,
) -> DisplayTargetsSnapshot {
    let layout = {
        let spatial = spatial_engine.read().await;
        spatial.layout()
    };
    let (display_group_targets_revision, published_display_group_targets) =
        event_bus.display_group_targets_snapshot();
    let display_face_targets = published_display_group_targets
        .into_iter()
        .map(|(group_id, target)| {
            (
                target.device_id,
                (
                    group_id,
                    DisplayFaceTarget {
                        device_id: target.device_id,
                        blend_mode: target.blend_mode,
                        opacity: target.opacity,
                    },
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    let logical_store = logical_devices.read().await;
    let display_preview_subscribers = display_frames.read().await.subscribed_device_ids();
    let registry_generation = registry.generation();
    #[expect(
        clippy::as_conversions,
        reason = "pointer-to-usize for identity comparison"
    )]
    let layout_ptr = Arc::as_ptr(&layout) as usize;
    let logical_signature = logical_device_store_signature(&logical_store);
    let display_preview_signature = device_id_set_signature(&display_preview_subscribers);
    let dependency_key =
        DisplayTargetDependencyKey::new(registry_generation, display_group_targets_revision);
    let cache_key = DisplayTargetCacheKey::new(
        dependency_key,
        layout_ptr,
        logical_signature,
        display_preview_signature,
    );

    if cache.cache_key == Some(cache_key) {
        return DisplayTargetsSnapshot {
            version: cache.version,
            targets: Arc::clone(&cache.targets),
        };
    }

    let mut targets = Vec::new();
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
        let Some((geometry, frame_format)) =
            display_target_geometry_for_device(&tracked.info.zones).or_else(|| {
                tracked
                    .info
                    .capabilities
                    .display_resolution
                    .map(|(width, height)| {
                        (
                            DisplayGeometry {
                                width,
                                height,
                                circular: false,
                            },
                            DisplayFrameFormat::Jpeg,
                        )
                    })
            })
        else {
            continue;
        };
        let has_non_display_led_zones = tracked.info.zones.iter().any(|zone| {
            zone.led_count > 0 && !matches!(zone.topology, DeviceTopologyHint::Display { .. })
        });
        let display_target = display_face_targets
            .get(&tracked.info.id)
            .map(|(_, target)| target.clone());
        let canvas_source = display_face_targets
            .get(&tracked.info.id)
            .map(|(group_id, _)| *group_id)
            .map_or(DisplayCanvasSource::Scene, |group_id| {
                DisplayCanvasSource::GroupDirect { group_id }
            });
        let group_canvas_sender = match &canvas_source {
            DisplayCanvasSource::Scene => None,
            DisplayCanvasSource::GroupDirect { group_id } => {
                Some(event_bus.group_canvas_sender(*group_id))
            }
        };
        let viewport = display_viewport_for_device(
            layout.as_ref(),
            &logical_store,
            tracked.info.id,
            has_non_display_led_zones,
        )
        .or_else(|| {
            canvas_source
                .is_group_direct()
                .then_some(default_display_viewport())
        });
        let Some(viewport) = viewport else {
            continue;
        };

        let backend_id = backend_id_for_device(&tracked.info.family, metadata.as_ref());
        targets.push(Arc::new(DisplayTarget {
            worker_key: (backend_id.clone(), tracked.info.id),
            backend_id,
            device_id: tracked.info.id,
            name: tracked.info.name,
            target_fps: capped_display_target_fps(
                tracked.info.capabilities.max_fps,
                &canvas_source,
            ),
            brightness: tracked.user_settings.brightness,
            geometry,
            frame_format,
            preview_subscribed: display_preview_subscribers.contains(&tracked.info.id),
            canvas_source,
            group_canvas_sender,
            display_target,
            viewport,
        }));
    }

    targets.sort_by(|left, right| {
        left.backend_id
            .cmp(&right.backend_id)
            .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
    });
    cache.version = cache.version.saturating_add(1);
    cache.cache_key = Some(cache_key);
    cache.targets = Arc::from(targets);
    DisplayTargetsSnapshot {
        version: cache.version,
        targets: Arc::clone(&cache.targets),
    }
}

fn display_target_geometry_for_device(
    zones: &[hypercolor_types::device::ZoneInfo],
) -> Option<(DisplayGeometry, DisplayFrameFormat)> {
    zones.iter().find_map(|zone| match zone.topology {
        DeviceTopologyHint::Display {
            width,
            height,
            circular,
        } => Some((
            DisplayGeometry {
                width,
                height,
                circular,
            },
            DisplayFrameFormat::from_device_color_format(zone.color_format),
        )),
        _ => None,
    })
}

fn display_viewport_for_device(
    layout: &SpatialLayout,
    logical_store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
    has_non_display_led_zones: bool,
) -> Option<DisplayViewport> {
    let mut first_matching_zone = None;
    let mut explicit_display_zone = None;
    let mut generic_display_zone = None;

    for zone in &layout.zones {
        if !display_zone_targets_physical_device(
            zone.device_id.as_str(),
            logical_store,
            physical_device_id,
        ) {
            continue;
        }

        first_matching_zone.get_or_insert(zone);
        if zone.zone_name.as_deref() == Some("Display") {
            explicit_display_zone = Some(zone);
            break;
        }
        if generic_display_zone.is_none() && zone.zone_name.is_none() {
            generic_display_zone = Some(zone);
        }
    }

    explicit_display_zone.or(generic_display_zone).map_or_else(
        || {
            let first_matching_zone = first_matching_zone?;
            if !has_non_display_led_zones {
                return Some(DisplayViewport {
                    position: first_matching_zone.position,
                    size: first_matching_zone.size,
                    rotation: first_matching_zone.rotation,
                    scale: first_matching_zone.scale,
                    edge_behavior: first_matching_zone
                        .edge_behavior
                        .unwrap_or(layout.default_edge_behavior),
                });
            }

            Some(DisplayViewport {
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                edge_behavior: layout.default_edge_behavior,
            })
        },
        |zone| {
            Some(DisplayViewport {
                position: zone.position,
                size: zone.size,
                rotation: zone.rotation,
                scale: zone.scale,
                edge_behavior: zone.edge_behavior.unwrap_or(layout.default_edge_behavior),
            })
        },
    )
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

fn display_zone_targets_physical_device(
    zone_device_id: &str,
    logical_store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
) -> bool {
    logical_store
        .get(zone_device_id)
        .is_some_and(|entry| entry.physical_device_id == physical_device_id)
}

fn capped_display_target_fps(device_max_fps: u32, canvas_source: &DisplayCanvasSource) -> u32 {
    let (default_fps, max_fps) = if canvas_source.is_group_direct() {
        (DISPLAY_FACE_DEFAULT_FPS, DISPLAY_FACE_DEFAULT_FPS)
    } else {
        (DISPLAY_OUTPUT_MAX_FPS, DISPLAY_OUTPUT_MAX_FPS)
    };
    let device_limit = if device_max_fps == 0 {
        default_fps
    } else {
        device_max_fps
    };

    device_limit.clamp(1, max_fps)
}

pub(crate) fn capped_group_direct_display_target_fps(device_max_fps: u32) -> u32 {
    let device_limit = if device_max_fps == 0 {
        DISPLAY_FACE_DEFAULT_FPS
    } else {
        device_max_fps
    };

    // Keep HTML faces on the conservative default until we have budget-aware
    // upshift logic; blindly inheriting a 60 fps panel limit reintroduces the
    // exact render/composite/JPEG churn this path is supposed to avoid.
    device_limit.clamp(1, DISPLAY_FACE_DEFAULT_FPS)
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
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::device::DeviceId;

    use super::*;

    fn canvas_frame(frame_number: u32) -> Arc<CanvasFrame> {
        Arc::new(CanvasFrame::from_owned_canvas(
            Canvas::new(2, 2),
            frame_number,
            frame_number,
        ))
    }

    fn display_target(blend_mode: DisplayFaceBlendMode) -> DisplayTarget {
        let device_id = DeviceId::new();
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
            preview_subscribed: false,
            canvas_source: DisplayCanvasSource::GroupDirect {
                group_id: RenderGroupId::new(),
            },
            group_canvas_sender: None,
            display_target: Some(DisplayFaceTarget {
                device_id,
                blend_mode,
                opacity: 0.5,
            }),
            viewport: default_display_viewport(),
        }
    }

    #[test]
    fn direct_display_face_uses_unified_face_frame_source_without_scene() {
        let target = display_target(DisplayFaceBlendMode::Replace);
        let face_frame = canvas_frame(1);

        let (frames, identity) = build_display_worker_frame_set(&target, None, Some(&face_frame))
            .expect("direct face should not require a scene frame");

        let DisplayWorkerFrameSource::Face {
            scene_frame,
            face_frame: published_face,
            blend_mode,
            opacity,
        } = frames.source
        else {
            panic!("display face should use the unified face frame source");
        };
        let StableDisplayFrameSourceIdentity::Face {
            scene_frame: scene_identity,
            blend_mode: identity_blend_mode,
            opacity_bits,
            ..
        } = identity.source
        else {
            panic!("display face identity should use the unified face identity");
        };

        assert!(scene_frame.is_none());
        assert!(scene_identity.is_none());
        assert!(Arc::ptr_eq(&published_face, &face_frame));
        assert_eq!(blend_mode, DisplayFaceBlendMode::Replace);
        assert_eq!(identity_blend_mode, DisplayFaceBlendMode::Replace);
        assert_eq!(opacity, 1.0);
        assert_eq!(opacity_bits, 1.0_f32.to_bits());
    }

    #[test]
    fn blended_display_face_uses_same_face_frame_source_with_scene() {
        let target = display_target(DisplayFaceBlendMode::Alpha);
        let scene_frame = canvas_frame(1);
        let scene_identity =
            stable_display_source_identity(scene_frame.as_ref()).expect("scene should be stable");
        let face_frame = canvas_frame(2);

        let (frames, identity) = build_display_worker_frame_set(
            &target,
            Some(&(scene_identity, scene_frame)),
            Some(&face_frame),
        )
        .expect("blended face should build when scene and face are available");

        let DisplayWorkerFrameSource::Face {
            scene_frame,
            face_frame: published_face,
            blend_mode,
            opacity,
        } = frames.source
        else {
            panic!("blended display face should use the unified face frame source");
        };
        let StableDisplayFrameSourceIdentity::Face {
            scene_frame: identity_scene,
            blend_mode: identity_blend_mode,
            opacity_bits,
            ..
        } = identity.source
        else {
            panic!("blended display face identity should use the unified face identity");
        };

        assert!(scene_frame.is_some());
        assert!(Arc::ptr_eq(&published_face, &face_frame));
        assert_eq!(identity_scene, Some(scene_identity));
        assert_eq!(blend_mode, DisplayFaceBlendMode::Alpha);
        assert_eq!(identity_blend_mode, DisplayFaceBlendMode::Alpha);
        assert_eq!(opacity, 0.5);
        assert_eq!(opacity_bits, 0.5_f32.to_bits());
    }

    #[test]
    fn blended_display_face_waits_for_scene_frame() {
        let target = display_target(DisplayFaceBlendMode::Alpha);
        let face_frame = canvas_frame(1);

        assert!(build_display_worker_frame_set(&target, None, Some(&face_frame)).is_none());
    }
}
