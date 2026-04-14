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
use tracing::{debug, info, warn};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::device::{DeviceId, DeviceTopologyHint};
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, RenderGroupId};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SpatialLayout};

use self::render::display_viewport_signature;
use crate::discovery::backend_id_for_device;
use crate::display_frames::DisplayFrameRuntime;
use crate::logical_devices::LogicalDevice;
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
    /// Active scene stack used to route display faces to per-group canvases.
    pub scene_manager: Arc<RwLock<SceneManager>>,
    /// Logical-device aliases used to match physical devices to layout zones.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    /// Event bus canvas stream produced by the render thread.
    pub event_bus: Arc<HypercolorBus>,
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
struct DisplayTarget {
    worker_key: DisplayWorkerKey,
    backend_id: String,
    device_id: DeviceId,
    name: String,
    target_fps: u32,
    brightness: f32,
    geometry: DisplayGeometry,
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
    canvas_source: DisplayCanvasSourceSignature,
    face_blend_mode: DisplayFaceBlendMode,
    face_opacity_bits: u32,
    viewport: DisplayViewportSignature,
}

#[derive(Default)]
struct DisplayTargetCache {
    initialized: bool,
    version: u64,
    registry_generation: u64,
    scene_revision: u64,
    layout_ptr: usize,
    logical_signature: u64,
    targets: Arc<[Arc<DisplayTarget>]>,
}

#[derive(Clone, Debug)]
enum DisplayCanvasSource {
    Global,
    GroupDirect { group_id: RenderGroupId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DisplayCanvasSourceSignature {
    Global,
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
pub(super) struct DisplayWorkerFrameSet {
    pub effect_frame: Option<Arc<CanvasFrame>>,
    pub face_frame: Option<Arc<CanvasFrame>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StableDisplaySourceIdentity {
    generation: u64,
    storage: PublishedSurfaceStorageIdentity,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StableDisplayFrameSetIdentity {
    effect_frame: Option<StableDisplaySourceIdentity>,
    face_frame: Option<StableDisplaySourceIdentity>,
}

impl DisplayTarget {
    pub(super) fn worker_config_signature(&self) -> DisplayWorkerConfigSignature {
        DisplayWorkerConfigSignature {
            target_fps: self.target_fps,
            brightness_bits: self.brightness.to_bits(),
            geometry: self.geometry.clone(),
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

    fn face_opacity(&self) -> f32 {
        self.display_target
            .as_ref()
            .map_or(1.0, |target| target.opacity.clamp(0.0, 1.0))
    }
}

impl DisplayCanvasSource {
    fn signature(&self) -> DisplayCanvasSourceSignature {
        match self {
            Self::Global => DisplayCanvasSourceSignature::Global,
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
        let canvas_rx = state.event_bus.canvas_receiver();
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
                runtime.block_on(run_display_output(state, canvas_rx, shutdown_rx));
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

async fn run_display_output(
    state: DisplayOutputState,
    mut canvas_rx: watch::Receiver<CanvasFrame>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let mut workers = HashMap::<DisplayWorkerKey, DisplayWorkerHandle>::new();
    let mut targets_cache = DisplayTargetCache::default();
    let mut last_reconciled_target_version = None::<u64>;
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
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    debug!("display output task exiting because canvas stream closed");
                    break;
                }
                let _ = canvas_rx.borrow_and_update();
            }
            _ = dispatch_tick.tick() => {
            }
        }

        let targets = display_targets(
            &state.device_registry,
            &state.spatial_engine,
            &state.scene_manager,
            &state.logical_devices,
            &state.event_bus,
            &mut targets_cache,
        )
        .await;
        if last_reconciled_target_version != Some(targets.version) {
            reconcile_display_workers(&state, &mut workers, targets.targets.as_ref()).await;
            last_reconciled_target_version = Some(targets.version);
            last_dispatched_sources.clear();
        }
        if targets.targets.is_empty() {
            last_dispatched_sources.clear();
            continue;
        }

        let global_frame = {
            let frame = canvas_rx.borrow();
            stable_display_source_identity(&frame)
                .map(|identity| (identity, Arc::new(frame.clone())))
        };

        for target in targets.targets.iter() {
            let effect_frame = match &target.canvas_source {
                DisplayCanvasSource::Global => {
                    global_frame.as_ref().map(|(_, frame)| Arc::clone(frame))
                }
                DisplayCanvasSource::GroupDirect { .. } => {
                    global_frame.as_ref().map(|(_, frame)| Arc::clone(frame))
                }
            };
            let face_frame = match &target.canvas_source {
                DisplayCanvasSource::Global => None,
                DisplayCanvasSource::GroupDirect { .. } => {
                    let Some(sender) = target.group_canvas_sender.as_ref() else {
                        continue;
                    };
                    let frame = sender.borrow();
                    stable_display_source_identity(&frame).map(|_| Arc::new(frame.clone()))
                }
            };
            let dispatch_identity = StableDisplayFrameSetIdentity {
                effect_frame: effect_frame
                    .as_deref()
                    .and_then(stable_display_source_identity),
                face_frame: face_frame
                    .as_deref()
                    .and_then(stable_display_source_identity),
            };
            if dispatch_identity.effect_frame.is_none() && dispatch_identity.face_frame.is_none() {
                continue;
            }
            if last_dispatched_sources.get(&target.worker_key) == Some(&dispatch_identity) {
                continue;
            }
            if let Some(worker) = workers.get(&target.worker_key) {
                worker.push(DisplayWorkerFrameSet {
                    effect_frame: effect_frame.as_ref().map(Arc::clone),
                    face_frame: face_frame.as_ref().map(Arc::clone),
                });
                last_dispatched_sources.insert(target.worker_key.clone(), dispatch_identity);
            }
        }
    }

    for (_, worker) in workers {
        worker.shutdown().await;
    }
}

fn stable_display_source_identity(frame: &CanvasFrame) -> Option<StableDisplaySourceIdentity> {
    let surface = frame.surface();
    (frame.width > 0 && frame.height > 0).then_some(StableDisplaySourceIdentity {
        generation: surface.generation(),
        storage: surface.storage_identity(),
        width: frame.width,
        height: frame.height,
    })
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
    scene_manager: &Arc<RwLock<SceneManager>>,
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
    event_bus: &Arc<HypercolorBus>,
    cache: &mut DisplayTargetCache,
) -> DisplayTargetsSnapshot {
    let layout = {
        let spatial = spatial_engine.read().await;
        spatial.layout()
    };
    let (scene_revision, display_face_targets) = {
        let manager = scene_manager.read().await;
        (
            manager.active_render_groups_revision(),
            manager
                .active_render_groups()
                .iter()
                .filter_map(|group| {
                    group
                        .display_target
                        .as_ref()
                        .map(|target| (target.device_id, (group.id, target.clone().normalized())))
                })
                .collect::<HashMap<_, _>>(),
        )
    };
    let logical_store = logical_devices.read().await;
    let registry_generation = registry.generation();
    #[expect(
        clippy::as_conversions,
        reason = "pointer-to-usize for identity comparison"
    )]
    let layout_ptr = Arc::as_ptr(&layout) as usize;
    let logical_signature = logical_device_store_signature(&logical_store);

    if cache.initialized
        && cache.registry_generation == registry_generation
        && cache.scene_revision == scene_revision
        && cache.layout_ptr == layout_ptr
        && cache.logical_signature == logical_signature
    {
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
        let Some(geometry) = display_geometry_for_device(&tracked.info.zones).or_else(|| {
            tracked
                .info
                .capabilities
                .display_resolution
                .map(|(width, height)| DisplayGeometry {
                    width,
                    height,
                    circular: false,
                })
        }) else {
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
            .map_or(DisplayCanvasSource::Global, |group_id| {
                DisplayCanvasSource::GroupDirect { group_id }
            });
        let group_canvas_sender = match &canvas_source {
            DisplayCanvasSource::Global => None,
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
    cache.initialized = true;
    cache.version = cache.version.saturating_add(1);
    cache.registry_generation = registry_generation;
    cache.scene_revision = scene_revision;
    cache.layout_ptr = layout_ptr;
    cache.logical_signature = logical_signature;
    cache.targets = Arc::from(targets);
    DisplayTargetsSnapshot {
        version: cache.version,
        targets: Arc::clone(&cache.targets),
    }
}

fn display_geometry_for_device(
    zones: &[hypercolor_types::device::ZoneInfo],
) -> Option<DisplayGeometry> {
    zones.iter().find_map(|zone| match zone.topology {
        DeviceTopologyHint::Display {
            width,
            height,
            circular,
        } => Some(DisplayGeometry {
            width,
            height,
            circular,
        }),
        _ => None,
    })
}

fn display_viewport_for_device(
    layout: &SpatialLayout,
    logical_store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
    has_non_display_led_zones: bool,
) -> Option<DisplayViewport> {
    let physical_alias = physical_device_id.to_string();
    let legacy_alias = format!("device:{physical_device_id}");
    let mut first_matching_zone = None;
    let mut explicit_display_zone = None;
    let mut generic_display_zone = None;

    for zone in &layout.zones {
        if !display_zone_targets_physical_device(
            zone.device_id.as_str(),
            logical_store,
            physical_device_id,
            physical_alias.as_str(),
            legacy_alias.as_str(),
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
    physical_alias: &str,
    legacy_alias: &str,
) -> bool {
    zone_device_id == physical_alias
        || zone_device_id == legacy_alias
        || logical_store
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

fn panic_payload_message(panic: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}
