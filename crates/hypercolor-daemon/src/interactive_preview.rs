use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::{EffectRegistry, InputSourceAvailability};
use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteContext, InteractionRouteDiagnostics,
    InteractionRouteRequest, InteractionRouteSource, InteractionRouteSourceClass,
    InteractionRouter, RoutedInteraction, SourceIncarnation,
};
use hypercolor_core::input::screen::PixelExtent;
use hypercolor_core::input::{
    BrowserInputAttachment, BrowserInputChildKey, BrowserInputPublicationId, InputData,
    InputGraphHandle, InputGraphSnapshot, SourceFreshness, SourceKind, SourceState,
    SourceStatusHandle,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{PublishedSurface, SurfaceDescriptor};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::event::{HypercolorEvent, ZoneColors};
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use hypercolor_types::sensor::SystemSnapshot;
use tokio::sync::{RwLock, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::interaction_routing::InteractionRoutingControl;
use crate::preview_runtime::PreviewPixelFormat;
#[cfg(feature = "wgpu")]
use crate::render_thread::gpu_device::GpuRenderDevice;
use crate::render_thread::sparkleflinger::{PreviewSurfaceRequest, SparkleFlinger};
use crate::render_thread::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationDemandRegistration, InteractivePreviewZoneRuntime, ProducerFrame,
    RenderSceneContext, SceneDependencyKey, ZoneFrameInputs,
};

const MAX_PREVIEW_FPS: u32 = 60;
static NEXT_CONSUMER_INCARNATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractivePreviewTarget {
    ActiveScene,
    Scene(SceneId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractivePreviewSpec {
    pub target: InteractivePreviewTarget,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
}

impl InteractivePreviewSpec {
    pub fn validate(self) -> Result<Self, InteractivePreviewError> {
        if !(1..=MAX_PREVIEW_FPS).contains(&self.fps) {
            return Err(InteractivePreviewError::InvalidSpec(format!(
                "preview fps must be between 1 and {MAX_PREVIEW_FPS}"
            )));
        }
        SurfaceDescriptor::rgba8888(self.width, self.height)
            .try_non_empty_byte_len()
            .map_err(|error| InteractivePreviewError::InvalidSpec(error.to_string()))?;
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractivePreviewBackend {
    Cpu,
    Gpu,
    CpuAfterGpuFailure,
}

#[derive(Clone, Debug)]
pub struct InteractivePreviewFrame {
    pub publication_id: BrowserInputPublicationId,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub surface: PublishedSurface,
}

#[derive(Clone, Debug)]
pub struct InteractivePreviewLaneSnapshot {
    pub publication_id: BrowserInputPublicationId,
    pub consumer_incarnation: u64,
    pub active: bool,
    pub backend: InteractivePreviewBackend,
    pub spec: InteractivePreviewSpec,
    pub frames_published: u64,
    pub last_frame_number: u32,
    pub route_generation: u64,
    pub selected_sources: Arc<[Arc<str>]>,
    pub last_error: Option<Arc<str>>,
}

#[derive(Debug, thiserror::Error)]
pub enum InteractivePreviewError {
    #[error("invalid interactive preview configuration: {0}")]
    InvalidSpec(String),
    #[error("interactive preview target is unavailable")]
    TargetUnavailable,
    #[error("interactive preview is already active")]
    AlreadyOpen,
    #[error("interactive preview worker is no longer active")]
    WorkerClosed,
    #[error("interactive preview worker failed to initialize: {0}")]
    Initialization(String),
    #[error("interactive preview worker rejected an update: {0}")]
    Update(String),
}

#[derive(Clone)]
pub struct InteractivePreviewAcceleration {
    mode: RenderAccelerationMode,
    #[cfg(feature = "wgpu")]
    render_device: Option<GpuRenderDevice>,
}

impl InteractivePreviewAcceleration {
    #[must_use]
    pub fn cpu() -> Self {
        Self {
            mode: RenderAccelerationMode::Cpu,
            #[cfg(feature = "wgpu")]
            render_device: None,
        }
    }

    #[must_use]
    pub(crate) fn from_authoritative(
        mode: RenderAccelerationMode,
        #[cfg(feature = "wgpu")] render_device: Option<GpuRenderDevice>,
    ) -> Self {
        Self {
            mode,
            #[cfg(feature = "wgpu")]
            render_device,
        }
    }
}

pub struct InteractivePreviewContext {
    pub scene_manager: Arc<RwLock<SceneManager>>,
    pub effect_registry: Arc<RwLock<EffectRegistry>>,
    pub asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    pub event_bus: Arc<HypercolorBus>,
    pub input_graph: InputGraphHandle,
    pub sensor_snapshots: Option<watch::Receiver<Arc<SystemSnapshot>>>,
    pub interaction_routing: InteractionRoutingControl,
    pub input_demands: InputPublicationDemandHandle,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub acceleration: InteractivePreviewAcceleration,
}

#[derive(Clone)]
pub struct InteractivePreviewExecutor {
    inner: Arc<InteractivePreviewExecutorInner>,
}

struct InteractivePreviewExecutorInner {
    lanes: Mutex<HashMap<BrowserInputChildKey, PreviewLaneEntry>>,
    catalog: PreviewSceneCatalogSource,
    interaction_routing: InteractionRoutingControl,
    input_graph: InputGraphHandle,
    sensor_snapshots: Option<watch::Receiver<Arc<SystemSnapshot>>>,
    input_demands: InputPublicationDemandHandle,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    acceleration: InteractivePreviewAcceleration,
    catalog_cancel: CancellationToken,
    catalog_task: Mutex<Option<JoinHandle<()>>>,
}

struct PreviewLaneEntry {
    publication_id: BrowserInputPublicationId,
    commands: mpsc::Sender<PreviewLaneCommand>,
    telemetry: Arc<PreviewLaneTelemetry>,
}

pub struct InteractivePreviewLaneLease {
    key: BrowserInputChildKey,
    publication_id: BrowserInputPublicationId,
    executor: Weak<InteractivePreviewExecutorInner>,
    frames: watch::Receiver<Option<Arc<InteractivePreviewFrame>>>,
    telemetry: Arc<PreviewLaneTelemetry>,
    closed: bool,
}

struct PreviewLaneTelemetry {
    consumer_incarnation: u64,
    publication_id: BrowserInputPublicationId,
    active: AtomicBool,
    backend: AtomicU8,
    spec: ArcSwap<InteractivePreviewSpec>,
    frames_published: AtomicU64,
    last_frame_number: AtomicU32,
    route_diagnostics: ArcSwap<InteractionRouteDiagnostics>,
    last_error: ArcSwap<Option<Arc<str>>>,
}

enum PreviewLaneCommand {
    Update {
        spec: InteractivePreviewSpec,
        response: oneshot::Sender<Result<(), String>>,
    },
    Close,
}

#[derive(Clone)]
struct PreviewSceneCatalogSource {
    latest: Arc<ArcSwap<PreviewSceneCatalog>>,
}

struct PreviewSceneCatalog {
    generation: u64,
    canvas_width: u32,
    canvas_height: u32,
    active_scene_id: Option<SceneId>,
    active_groups_revision: u64,
    active_groups: Arc<[Zone]>,
    scenes: Arc<[PreviewSceneEntry]>,
    registry: Arc<EffectRegistry>,
}

struct PreviewSceneEntry {
    id: SceneId,
    groups_revision: u64,
    groups: Arc<[Zone]>,
}

struct ResolvedPreviewScene {
    scene_id: Option<SceneId>,
    groups_revision: u64,
    groups: Arc<[Zone]>,
    registry: Arc<EffectRegistry>,
    catalog_generation: u64,
    canvas_width: u32,
    canvas_height: u32,
}

struct PreviewLane {
    id: PreviewLaneId,
    consumer: ConsumerIncarnation,
    spec: InteractivePreviewSpec,
    catalog: PreviewSceneCatalogSource,
    zone_runtime: InteractivePreviewZoneRuntime,
    sparkleflinger: SparkleFlinger,
    input: PreviewLaneInput,
    demand: InputPublicationDemandRegistration,
    current_demand: InputPublicationDemand,
    frame_tx: watch::Sender<Option<Arc<InteractivePreviewFrame>>>,
    telemetry: Arc<PreviewLaneTelemetry>,
    frame_number: u32,
    started: Instant,
    last_tick: Instant,
    retained_frame: Option<Arc<InteractivePreviewFrame>>,
    zones: Vec<ZoneColors>,
    display_descriptors: HashMap<ZoneId, DisplayDescriptor>,
}

#[derive(Clone)]
struct PreviewLaneId {
    key: BrowserInputChildKey,
    publication_id: BrowserInputPublicationId,
}

struct PreviewLaneInput {
    graph: InputGraphHandle,
    sensor_snapshots: Option<watch::Receiver<Arc<SystemSnapshot>>>,
    routing: InteractionRoutingControl,
    publication_id: BrowserInputPublicationId,
    graph_generation: Option<u64>,
    browser_generation: Option<u64>,
    interaction_sources: Vec<InteractionRouteSource>,
    interaction_statuses: BTreeMap<SourceIncarnation, SourceStatusHandle>,
    availability: BTreeMap<SourceIncarnation, CachedAvailability>,
    router: InteractionRouter,
    routed: RoutedInteraction,
    audio: Option<Arc<InputData>>,
    screen: Option<Arc<InputData>>,
    media: Option<Arc<InputData>>,
    network: Option<Arc<InputData>>,
    sensors: Option<Arc<InputData>>,
    empty_audio: AudioData,
    sensor_snapshot: Arc<SystemSnapshot>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct AvailabilityFingerprint {
    configured: bool,
    consented: bool,
    demanded: bool,
    state: SourceState,
    freshness: SourceFreshness,
    session_generation: u64,
    retired: bool,
}

#[derive(Clone, Copy)]
struct CachedAvailability {
    fingerprint: AvailabilityFingerprint,
    revision: u64,
}

impl InteractivePreviewExecutor {
    pub async fn start(
        context: InteractivePreviewContext,
    ) -> Result<Self, InteractivePreviewError> {
        let catalog = PreviewSceneCatalogSource::capture(
            &context.scene_manager,
            &context.effect_registry,
            context.canvas_width,
            context.canvas_height,
            1,
        )
        .await;
        let catalog_cancel = CancellationToken::new();
        let task = spawn_catalog_publisher(
            catalog.clone(),
            Arc::clone(&context.scene_manager),
            Arc::clone(&context.effect_registry),
            Arc::clone(&context.event_bus),
            context.canvas_width,
            context.canvas_height,
            catalog_cancel.clone(),
        );
        Ok(Self {
            inner: Arc::new(InteractivePreviewExecutorInner {
                lanes: Mutex::new(HashMap::new()),
                catalog,
                interaction_routing: context.interaction_routing,
                input_graph: context.input_graph,
                sensor_snapshots: context.sensor_snapshots,
                input_demands: context.input_demands,
                asset_library: context.asset_library,
                acceleration: context.acceleration,
                catalog_cancel,
                catalog_task: Mutex::new(Some(task)),
            }),
        })
    }

    pub async fn start_cpu(
        mut context: InteractivePreviewContext,
    ) -> Result<Self, InteractivePreviewError> {
        context.acceleration = InteractivePreviewAcceleration::cpu();
        Self::start(context).await
    }

    pub async fn open(
        &self,
        attachment: &BrowserInputAttachment,
        spec: InteractivePreviewSpec,
    ) -> Result<InteractivePreviewLaneLease, InteractivePreviewError> {
        let spec = spec.validate()?;
        if self.inner.catalog.snapshot().resolve(spec.target).is_none() {
            return Err(InteractivePreviewError::TargetUnavailable);
        }

        let consumer_value = NEXT_CONSUMER_INCARNATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("interactive preview consumer incarnation exhausted");
        let consumer = ConsumerIncarnation::new(consumer_value);
        let initial_diagnostics = Arc::new(
            RoutedInteraction::new(consumer)
                .diagnostics
                .as_ref()
                .clone(),
        );
        let telemetry = Arc::new(PreviewLaneTelemetry {
            consumer_incarnation: consumer_value,
            publication_id: attachment.publication_id(),
            active: AtomicBool::new(true),
            backend: AtomicU8::new(backend_to_u8(InteractivePreviewBackend::Cpu)),
            spec: ArcSwap::from_pointee(spec),
            frames_published: AtomicU64::new(0),
            last_frame_number: AtomicU32::new(0),
            route_diagnostics: ArcSwap::from(initial_diagnostics),
            last_error: ArcSwap::from_pointee(None),
        });
        let (command_tx, command_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = watch::channel(None);
        let (ready_tx, ready_rx) = oneshot::channel();
        let (start_tx, start_rx) = mpsc::channel();
        let id = PreviewLaneId {
            key: attachment.key().clone(),
            publication_id: attachment.publication_id(),
        };

        {
            let mut lanes = lock(&self.inner.lanes);
            if lanes.contains_key(&id.key) {
                return Err(InteractivePreviewError::AlreadyOpen);
            }
            lanes.insert(
                id.key.clone(),
                PreviewLaneEntry {
                    publication_id: id.publication_id,
                    commands: command_tx,
                    telemetry: Arc::clone(&telemetry),
                },
            );
        }

        let weak_executor = Arc::downgrade(&self.inner);
        let lane_context = PreviewLaneThreadContext {
            id: id.clone(),
            consumer,
            spec,
            catalog: self.inner.catalog.clone(),
            graph: self.inner.input_graph.clone(),
            sensor_snapshots: self.inner.sensor_snapshots.clone(),
            routing: self.inner.interaction_routing.clone(),
            demands: self.inner.input_demands.clone(),
            asset_library: self.inner.asset_library.clone(),
            acceleration: self.inner.acceleration.clone(),
            frame_tx,
            telemetry: Arc::clone(&telemetry),
        };
        let spawn = thread::Builder::new()
            .name(format!("hypercolor-preview-{consumer_value}"))
            .spawn(move || {
                run_preview_lane(lane_context, command_rx, start_rx, ready_tx, weak_executor);
            });
        if let Err(error) = spawn {
            self.inner.close_exact(&id);
            return Err(InteractivePreviewError::Initialization(error.to_string()));
        }

        match ready_rx.await {
            Ok(Ok(backend)) => {
                telemetry
                    .backend
                    .store(backend_to_u8(backend), Ordering::Release);
            }
            Ok(Err(error)) => {
                self.inner.close_exact(&id);
                return Err(InteractivePreviewError::Initialization(error));
            }
            Err(_) => {
                self.inner.close_exact(&id);
                return Err(InteractivePreviewError::WorkerClosed);
            }
        }
        start_tx
            .send(())
            .map_err(|_| InteractivePreviewError::WorkerClosed)?;

        Ok(InteractivePreviewLaneLease {
            key: id.key,
            publication_id: id.publication_id,
            executor: Arc::downgrade(&self.inner),
            frames: frame_rx,
            telemetry,
            closed: false,
        })
    }

    #[must_use]
    pub fn lane_count(&self) -> usize {
        lock(&self.inner.lanes).len()
    }

    #[must_use]
    pub fn lane_snapshot(
        &self,
        key: &BrowserInputChildKey,
    ) -> Option<InteractivePreviewLaneSnapshot> {
        lock(&self.inner.lanes)
            .get(key)
            .map(|entry| entry.telemetry.snapshot())
    }
}

impl std::fmt::Debug for InteractivePreviewExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InteractivePreviewExecutor")
            .field("lane_count", &self.lane_count())
            .finish_non_exhaustive()
    }
}

impl InteractivePreviewLaneLease {
    #[must_use]
    pub fn key(&self) -> &BrowserInputChildKey {
        &self.key
    }

    #[must_use]
    pub fn publication_id(&self) -> BrowserInputPublicationId {
        self.publication_id
    }

    #[must_use]
    pub fn frame_receiver(&self) -> watch::Receiver<Option<Arc<InteractivePreviewFrame>>> {
        self.frames.clone()
    }

    #[must_use]
    pub fn snapshot(&self) -> InteractivePreviewLaneSnapshot {
        self.telemetry.snapshot()
    }

    pub async fn resize_or_retarget(
        &self,
        spec: InteractivePreviewSpec,
    ) -> Result<(), InteractivePreviewError> {
        let spec = spec.validate()?;
        let Some(executor) = self.executor.upgrade() else {
            return Err(InteractivePreviewError::WorkerClosed);
        };
        if executor.catalog.snapshot().resolve(spec.target).is_none() {
            return Err(InteractivePreviewError::TargetUnavailable);
        }
        let commands = executor
            .commands_exact(&PreviewLaneId {
                key: self.key.clone(),
                publication_id: self.publication_id,
            })
            .ok_or(InteractivePreviewError::WorkerClosed)?;
        let (response_tx, response_rx) = oneshot::channel();
        commands
            .send(PreviewLaneCommand::Update {
                spec,
                response: response_tx,
            })
            .map_err(|_| InteractivePreviewError::WorkerClosed)?;
        response_rx
            .await
            .map_err(|_| InteractivePreviewError::WorkerClosed)?
            .map_err(InteractivePreviewError::Update)
    }

    pub fn close(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.closed = true;
        self.executor.upgrade().is_some_and(|executor| {
            executor.close_exact(&PreviewLaneId {
                key: self.key.clone(),
                publication_id: self.publication_id,
            })
        })
    }
}

impl Drop for InteractivePreviewLaneLease {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl InteractivePreviewExecutorInner {
    fn commands_exact(&self, id: &PreviewLaneId) -> Option<mpsc::Sender<PreviewLaneCommand>> {
        lock(&self.lanes).get(&id.key).and_then(|entry| {
            (entry.publication_id == id.publication_id).then(|| entry.commands.clone())
        })
    }

    fn close_exact(&self, id: &PreviewLaneId) -> bool {
        let entry = {
            let mut lanes = lock(&self.lanes);
            if lanes
                .get(&id.key)
                .is_none_or(|entry| entry.publication_id != id.publication_id)
            {
                return false;
            }
            lanes.remove(&id.key)
        };
        if let Some(entry) = entry {
            entry.telemetry.active.store(false, Ordering::Release);
            let _ = entry.commands.send(PreviewLaneCommand::Close);
            return true;
        }
        false
    }

    fn retire_exact(&self, id: &PreviewLaneId) {
        let mut lanes = lock(&self.lanes);
        if lanes
            .get(&id.key)
            .is_some_and(|entry| entry.publication_id == id.publication_id)
            && let Some(entry) = lanes.remove(&id.key)
        {
            entry.telemetry.active.store(false, Ordering::Release);
        }
    }
}

impl Drop for InteractivePreviewExecutorInner {
    fn drop(&mut self) {
        self.catalog_cancel.cancel();
        if let Some(task) = lock(&self.catalog_task).take() {
            task.abort();
        }
        let entries = lock(&self.lanes)
            .drain()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        for entry in entries {
            entry.telemetry.active.store(false, Ordering::Release);
            let _ = entry.commands.send(PreviewLaneCommand::Close);
        }
    }
}

impl PreviewLaneTelemetry {
    fn snapshot(&self) -> InteractivePreviewLaneSnapshot {
        let diagnostics = self.route_diagnostics.load_full();
        InteractivePreviewLaneSnapshot {
            publication_id: self.publication_id,
            consumer_incarnation: self.consumer_incarnation,
            active: self.active.load(Ordering::Acquire),
            backend: backend_from_u8(self.backend.load(Ordering::Acquire)),
            spec: **self.spec.load(),
            frames_published: self.frames_published.load(Ordering::Relaxed),
            last_frame_number: self.last_frame_number.load(Ordering::Relaxed),
            route_generation: diagnostics.route_generation,
            selected_sources: diagnostics
                .selected
                .iter()
                .map(|source| Arc::clone(&source.descriptor))
                .collect::<Vec<_>>()
                .into(),
            last_error: self.last_error.load_full().as_ref().clone(),
        }
    }

    fn publish_error(&self, error: impl Into<Arc<str>>) {
        self.last_error.store(Arc::new(Some(error.into())));
    }

    fn clear_error(&self) {
        self.last_error.store(Arc::new(None));
    }
}

struct PreviewLaneThreadContext {
    id: PreviewLaneId,
    consumer: ConsumerIncarnation,
    spec: InteractivePreviewSpec,
    catalog: PreviewSceneCatalogSource,
    graph: InputGraphHandle,
    sensor_snapshots: Option<watch::Receiver<Arc<SystemSnapshot>>>,
    routing: InteractionRoutingControl,
    demands: InputPublicationDemandHandle,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    acceleration: InteractivePreviewAcceleration,
    frame_tx: watch::Sender<Option<Arc<InteractivePreviewFrame>>>,
    telemetry: Arc<PreviewLaneTelemetry>,
}

fn run_preview_lane(
    context: PreviewLaneThreadContext,
    commands: mpsc::Receiver<PreviewLaneCommand>,
    start: mpsc::Receiver<()>,
    ready: oneshot::Sender<Result<InteractivePreviewBackend, String>>,
    executor: Weak<InteractivePreviewExecutorInner>,
) {
    let id = context.id.clone();
    let initialized = PreviewLane::new(context);
    let mut lane = match initialized {
        Ok((lane, backend)) => {
            let _ = ready.send(Ok(backend));
            lane
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            if let Some(executor) = executor.upgrade() {
                executor.retire_exact(&id);
            }
            return;
        }
    };
    if start.recv().is_err() {
        if let Some(executor) = executor.upgrade() {
            executor.retire_exact(&id);
        }
        return;
    }

    let mut next_frame = Instant::now();
    'run: loop {
        while let Ok(command) = commands.try_recv() {
            if !lane.apply_command(command, &mut next_frame) {
                break 'run;
            }
        }
        let now = Instant::now();
        if now >= next_frame {
            lane.render(now);
            next_frame = advance_deadline(next_frame, preview_interval(lane.spec.fps), now);
            continue;
        }
        match commands.recv_timeout(next_frame.saturating_duration_since(now)) {
            Ok(command) => {
                if !lane.apply_command(command, &mut next_frame) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    lane.telemetry.active.store(false, Ordering::Release);
    let _ = lane
        .input
        .router
        .remove_consumer(lane.consumer, input_mono_ms());
    if let Some(executor) = executor.upgrade() {
        executor.retire_exact(&id);
    }
}

impl PreviewLane {
    fn new(context: PreviewLaneThreadContext) -> Result<(Self, InteractivePreviewBackend), String> {
        let catalog = context.catalog.snapshot();
        let resolved = catalog
            .resolve(context.spec.target)
            .ok_or_else(|| "preview target disappeared during open".to_owned())?;
        let (sparkleflinger, backend) = create_preview_compositor(&context.acceleration);
        let zone_runtime = match context.asset_library {
            Some(library) => InteractivePreviewZoneRuntime::with_asset_library(
                resolved.canvas_width,
                resolved.canvas_height,
                library,
            ),
            None => {
                InteractivePreviewZoneRuntime::new(resolved.canvas_width, resolved.canvas_height)
            }
        }
        .map_err(|error| error.to_string())?;
        let current_demand = preview_input_demand(&resolved, context.spec.fps);
        let demand = context
            .demands
            .register(InputPublicationConsumer::Preview, current_demand.clone());
        let input = PreviewLaneInput::new(
            context.graph,
            context.sensor_snapshots,
            context.routing,
            context.id.publication_id,
            context.consumer,
        );
        let now = Instant::now();
        Ok((
            Self {
                id: context.id,
                consumer: context.consumer,
                spec: context.spec,
                catalog: context.catalog,
                zone_runtime,
                sparkleflinger,
                input,
                demand,
                current_demand,
                frame_tx: context.frame_tx,
                telemetry: context.telemetry,
                frame_number: 0,
                started: now,
                last_tick: now,
                retained_frame: None,
                zones: Vec::new(),
                display_descriptors: HashMap::new(),
            },
            backend,
        ))
    }

    fn apply_command(&mut self, command: PreviewLaneCommand, next_frame: &mut Instant) -> bool {
        match command {
            PreviewLaneCommand::Update { spec, response } => {
                let result = if self.catalog.snapshot().resolve(spec.target).is_some() {
                    self.spec = spec;
                    self.telemetry.spec.store(Arc::new(spec));
                    *next_frame = Instant::now();
                    Ok(())
                } else {
                    Err("preview target is unavailable".to_owned())
                };
                let _ = response.send(result);
                true
            }
            PreviewLaneCommand::Close => false,
        }
    }

    fn render(&mut self, now: Instant) {
        let catalog = self.catalog.snapshot();
        let Some(scene) = catalog.resolve(self.spec.target) else {
            self.telemetry
                .publish_error("preview target is unavailable");
            return;
        };
        if let Err(error) = self
            .zone_runtime
            .resize_scene(scene.canvas_width, scene.canvas_height)
        {
            self.telemetry.publish_error(format!(
                "preview canvas resources could not be prepared: {error}"
            ));
            return;
        }
        let demand = preview_input_demand(&scene, self.spec.fps);
        if demand != self.current_demand {
            self.demand.update(demand.clone());
            self.current_demand = demand;
        }
        self.input.read();
        self.telemetry
            .route_diagnostics
            .store(Arc::clone(&self.input.routed.diagnostics));
        let delta_secs = now.saturating_duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        let context = RenderSceneContext {
            groups: &scene.groups,
            active_scene_id: scene.scene_id,
            dependency_key: SceneDependencyKey::new(
                scene.groups_revision,
                scene.catalog_generation,
            ),
            elapsed_ms: duration_millis_u64(self.started.elapsed()),
            display_group_target_fps: &HashMap::new(),
            display_group_descriptors: &self.display_descriptors,
            registry: &scene.registry,
            inputs: ZoneFrameInputs {
                delta_secs,
                audio: self.input.audio(),
                interaction: &self.input.routed.interaction,
                screen: self.input.screen(),
                sensors: self.input.sensors(),
                input_availability: self.input.interaction_availability(),
                media: self.input.media(),
                net: self.input.network(),
                lighting: None,
            },
        };
        let result = self
            .zone_runtime
            .render_scene(context, &mut self.sparkleflinger, &mut self.zones)
            .map_err(|error| error.to_string())
            .and_then(|rendered| {
                preview_surface(
                    &mut self.sparkleflinger,
                    rendered,
                    self.spec.width,
                    self.spec.height,
                )
                .ok_or_else(|| {
                    "preview frame could not be materialized on its isolated device".to_owned()
                })
            });
        match result {
            Ok(surface) => {
                let frame = Arc::new(InteractivePreviewFrame {
                    publication_id: self.id.publication_id,
                    frame_number: self.frame_number,
                    timestamp_ms: duration_millis_u32(self.started.elapsed()),
                    width: surface.width(),
                    height: surface.height(),
                    format: self.spec.format,
                    surface,
                });
                self.frame_tx.send_replace(Some(Arc::clone(&frame)));
                self.retained_frame = Some(frame);
                self.telemetry
                    .frames_published
                    .fetch_add(1, Ordering::Relaxed);
                self.telemetry
                    .last_frame_number
                    .store(self.frame_number, Ordering::Relaxed);
                self.telemetry.clear_error();
                self.frame_number = self.frame_number.wrapping_add(1);
            }
            Err(error) => self.telemetry.publish_error(error),
        }
    }
}

impl PreviewLaneInput {
    fn new(
        graph: InputGraphHandle,
        sensor_snapshots: Option<watch::Receiver<Arc<SystemSnapshot>>>,
        routing: InteractionRoutingControl,
        publication_id: BrowserInputPublicationId,
        consumer: ConsumerIncarnation,
    ) -> Self {
        Self {
            graph,
            sensor_snapshots,
            routing,
            publication_id,
            graph_generation: None,
            browser_generation: None,
            interaction_sources: Vec::new(),
            interaction_statuses: BTreeMap::new(),
            availability: BTreeMap::new(),
            router: InteractionRouter::default(),
            routed: RoutedInteraction::new(consumer),
            audio: None,
            screen: None,
            media: None,
            network: None,
            sensors: None,
            empty_audio: AudioData::silence(),
            sensor_snapshot: Arc::new(SystemSnapshot::empty()),
        }
    }

    fn read(&mut self) {
        let graph = self.graph.snapshot();
        let browser = self.routing.browser_registry_snapshot();
        if self.graph_generation != Some(graph.generation())
            || self.browser_generation != Some(browser.generation())
        {
            self.rebuild_interaction_sources(&graph, &browser);
        }
        self.read_typed(&graph);
        if let Some(sensors) = self.sensor_snapshots.as_ref() {
            self.sensor_snapshot = Arc::clone(&sensors.borrow());
        }
        self.refresh_availability();
        let routing = self.routing.snapshot();
        self.router.resolve_into(
            self.routed.diagnostics.consumer,
            InteractionRouteRequest {
                policy: routing.preview_policy,
                browser_source: Some(SourceIncarnation::browser_child(self.publication_id.get())),
            },
            &self.interaction_sources,
            InteractionRouteContext {
                config_generation: routing.config_generation,
                source_graph_generation: graph.generation(),
                browser_registry_generation: browser.generation(),
                now_ms: input_mono_ms(),
            },
            &mut self.routed,
        );
    }

    fn rebuild_interaction_sources(
        &mut self,
        graph: &InputGraphSnapshot,
        browser: &hypercolor_core::input::BrowserInputRegistrySnapshot,
    ) {
        self.interaction_sources.clear();
        self.interaction_statuses.clear();
        for slot in graph.slots() {
            let status = slot.status();
            let descriptor = Arc::clone(&status.snapshot().source_id);
            if let Some(source) = InteractionRouteSource::manager_slot(descriptor, 1, slot.clone())
            {
                self.interaction_statuses
                    .insert(source.incarnation, status.clone());
                self.interaction_sources.push(source);
            }
        }
        for child in browser.children() {
            let source = InteractionRouteSource::new(
                SourceIncarnation::browser_child(child.publication_id().get()),
                Arc::<str>::from(child.source_id()),
                InteractionRouteSourceClass::Browser,
                1,
                Arc::new(child.clone()),
            );
            self.interaction_statuses
                .insert(source.incarnation, child.status().clone());
            self.interaction_sources.push(source);
        }
        self.availability
            .retain(|incarnation, _| self.interaction_statuses.contains_key(incarnation));
        self.graph_generation = Some(graph.generation());
        self.browser_generation = Some(browser.generation());
        self.refresh_availability();
    }

    fn refresh_availability(&mut self) {
        for source in &mut self.interaction_sources {
            let Some(status) = self.interaction_statuses.get(&source.incarnation) else {
                continue;
            };
            let fingerprint = availability_fingerprint(status);
            let cached =
                self.availability
                    .entry(source.incarnation)
                    .or_insert(CachedAvailability {
                        fingerprint,
                        revision: 1,
                    });
            if cached.fingerprint != fingerprint {
                cached.fingerprint = fingerprint;
                cached.revision = cached
                    .revision
                    .checked_add(1)
                    .expect("preview availability revision exhausted");
            }
            source.availability_revision = cached.revision;
        }
    }

    fn read_typed(&mut self, graph: &InputGraphSnapshot) {
        self.audio = None;
        self.screen = None;
        self.media = None;
        self.network = None;
        self.sensors = None;
        for slot in graph.slots() {
            let Some(sample) = slot.latest() else {
                continue;
            };
            match sample.as_ref() {
                InputData::Audio(_) => self.audio = Some(sample),
                InputData::Screen(_) => self.screen = Some(sample),
                InputData::Media(_) => self.media = Some(sample),
                InputData::Net(_) => self.network = Some(sample),
                InputData::Sensors(_) => self.sensors = Some(sample),
                InputData::Interaction(_) | InputData::None => {}
            }
        }
    }

    fn audio(&self) -> &AudioData {
        self.audio
            .as_deref()
            .and_then(|sample| match sample {
                InputData::Audio(audio) => Some(audio),
                _ => None,
            })
            .unwrap_or(&self.empty_audio)
    }

    fn screen(&self) -> Option<&hypercolor_core::input::ScreenData> {
        self.screen.as_deref().and_then(|sample| match sample {
            InputData::Screen(screen) => Some(screen),
            _ => None,
        })
    }

    fn media(&self) -> Option<&hypercolor_types::media::MediaState> {
        self.media.as_deref().and_then(|sample| match sample {
            InputData::Media(media) => Some(media.as_ref()),
            _ => None,
        })
    }

    fn network(&self) -> Option<&hypercolor_types::net::NetStats> {
        self.network.as_deref().and_then(|sample| match sample {
            InputData::Net(network) => Some(network.as_ref()),
            _ => None,
        })
    }

    fn sensors(&self) -> &SystemSnapshot {
        self.sensors
            .as_deref()
            .and_then(|sample| match sample {
                InputData::Sensors(sensors) => Some(sensors.as_ref()),
                _ => None,
            })
            .unwrap_or(&self.sensor_snapshot)
    }

    fn interaction_availability(&self) -> InputSourceAvailability {
        aggregate_interaction_availability(
            self.routed
                .diagnostics
                .selected
                .iter()
                .filter_map(|source| source.status.as_ref()),
        )
    }
}

impl PreviewSceneCatalogSource {
    async fn capture(
        scene_manager: &Arc<RwLock<SceneManager>>,
        effect_registry: &Arc<RwLock<EffectRegistry>>,
        canvas_width: u32,
        canvas_height: u32,
        generation: u64,
    ) -> Self {
        let snapshot = capture_catalog(
            scene_manager,
            effect_registry,
            canvas_width,
            canvas_height,
            generation,
        )
        .await;
        Self {
            latest: Arc::new(ArcSwap::from(snapshot)),
        }
    }

    fn snapshot(&self) -> Arc<PreviewSceneCatalog> {
        self.latest.load_full()
    }
}

impl PreviewSceneCatalog {
    fn resolve(&self, target: InteractivePreviewTarget) -> Option<ResolvedPreviewScene> {
        let (scene_id, groups_revision, groups) = match target {
            InteractivePreviewTarget::ActiveScene => (
                self.active_scene_id,
                self.active_groups_revision,
                Arc::clone(&self.active_groups),
            ),
            InteractivePreviewTarget::Scene(scene_id) => {
                let scene = self.scenes.iter().find(|scene| scene.id == scene_id)?;
                (
                    Some(scene.id),
                    scene.groups_revision,
                    Arc::clone(&scene.groups),
                )
            }
        };
        Some(ResolvedPreviewScene {
            scene_id,
            groups_revision,
            groups,
            registry: Arc::clone(&self.registry),
            catalog_generation: self.generation,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        })
    }
}

fn spawn_catalog_publisher(
    catalog: PreviewSceneCatalogSource,
    scene_manager: Arc<RwLock<SceneManager>>,
    effect_registry: Arc<RwLock<EffectRegistry>>,
    event_bus: Arc<HypercolorBus>,
    canvas_width: u32,
    canvas_height: u32,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = event_bus.subscribe_all();
        let mut generation = catalog.snapshot().generation;
        loop {
            let event = tokio::select! {
                () = cancel.cancelled() => break,
                event = events.recv() => event,
            };
            let should_refresh = match event {
                Ok(event) => catalog_event_invalidates(&event.event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if !should_refresh {
                continue;
            }
            generation = generation
                .checked_add(1)
                .expect("preview scene catalog generation exhausted");
            let snapshot = capture_catalog(
                &scene_manager,
                &effect_registry,
                canvas_width,
                canvas_height,
                generation,
            )
            .await;
            catalog.latest.store(snapshot);
        }
    })
}

async fn capture_catalog(
    scene_manager: &Arc<RwLock<SceneManager>>,
    effect_registry: &Arc<RwLock<EffectRegistry>>,
    canvas_width: u32,
    canvas_height: u32,
    generation: u64,
) -> Arc<PreviewSceneCatalog> {
    let (active_scene_id, active_groups_revision, active_groups, scenes) = {
        let manager = scene_manager.read().await;
        let scenes = manager
            .list()
            .into_iter()
            .map(|scene| PreviewSceneEntry {
                id: scene.id,
                groups_revision: scene.groups_revision,
                groups: scene.groups.clone().into(),
            })
            .collect::<Vec<_>>()
            .into();
        (
            manager.active_scene_id().copied(),
            manager.active_render_groups_revision(),
            manager.active_render_groups(),
            scenes,
        )
    };
    let registry = Arc::new(effect_registry.read().await.clone());
    Arc::new(PreviewSceneCatalog {
        generation,
        canvas_width,
        canvas_height,
        active_scene_id,
        active_groups_revision,
        active_groups,
        scenes,
        registry,
    })
}

fn catalog_event_invalidates(event: &HypercolorEvent) -> bool {
    matches!(
        event,
        HypercolorEvent::EffectRegistryUpdated { .. }
            | HypercolorEvent::SceneActivated { .. }
            | HypercolorEvent::SceneTransitionStarted { .. }
            | HypercolorEvent::SceneTransitionComplete { .. }
            | HypercolorEvent::SceneEnabled { .. }
            | HypercolorEvent::RenderGroupChanged { .. }
            | HypercolorEvent::LayerStackChanged { .. }
            | HypercolorEvent::SceneSettingsChanged { .. }
            | HypercolorEvent::SceneLibraryChanged { .. }
            | HypercolorEvent::ActiveSceneChanged { .. }
    )
}

fn preview_input_demand(scene: &ResolvedPreviewScene, requested_hz: u32) -> InputPublicationDemand {
    let mut demand = InputPublicationDemand::default();
    let screen_extent = PixelExtent::new(scene.canvas_width, scene.canvas_height)
        .expect("resolved preview canvas dimensions are non-empty");
    let mut media = false;
    let mut network = false;
    for group in scene.groups.iter().filter(|group| group.enabled) {
        for layer in group
            .effective_layers()
            .into_iter()
            .filter(|layer| layer.enabled)
        {
            match &layer.source {
                LayerSource::Effect { effect_id, .. } => {
                    if let Some(entry) = scene.registry.get(effect_id) {
                        if entry.metadata.audio_reactive {
                            demand = demand.with_source(SourceKind::Audio, requested_hz);
                        }
                        if entry.metadata.screen_reactive {
                            demand = demand.with_screen(requested_hz, screen_extent);
                        }
                        if entry.metadata.requires_interaction() {
                            demand = demand.with_source(SourceKind::Interaction, requested_hz);
                        }
                        media |= entry
                            .metadata
                            .tags
                            .iter()
                            .any(|tag| tag.eq_ignore_ascii_case("media"));
                        network |= entry
                            .metadata
                            .tags
                            .iter()
                            .any(|tag| tag.eq_ignore_ascii_case("net"));
                    }
                }
                LayerSource::ScreenRegion { .. } => {
                    demand = demand.with_screen(requested_hz, screen_extent);
                }
                LayerSource::Media { .. } => media = true,
                LayerSource::ColorFill { .. } | LayerSource::WebViewport { .. } => {}
            }
        }
    }
    if media {
        demand = demand.with_source(SourceKind::Media, 1);
    }
    if network {
        demand = demand.with_source(SourceKind::Network, 1);
    }
    demand
}

fn create_preview_compositor(
    acceleration: &InteractivePreviewAcceleration,
) -> (SparkleFlinger, InteractivePreviewBackend) {
    if acceleration.mode != RenderAccelerationMode::Gpu {
        return (SparkleFlinger::cpu(), InteractivePreviewBackend::Cpu);
    }
    #[cfg(feature = "wgpu")]
    if let Some(authoritative) = acceleration.render_device.as_ref()
        && let Ok(device) = authoritative.independent_device("interactive preview compositor")
        && let Ok(compositor) =
            SparkleFlinger::new_with_gpu_device(RenderAccelerationMode::Gpu, Some(device))
    {
        return (compositor, InteractivePreviewBackend::Gpu);
    }
    (
        SparkleFlinger::cpu(),
        InteractivePreviewBackend::CpuAfterGpuFailure,
    )
}

fn preview_surface(
    sparkleflinger: &mut SparkleFlinger,
    frame: ProducerFrame,
    width: u32,
    height: u32,
) -> Option<PublishedSurface> {
    let request = Some(PreviewSurfaceRequest { width, height });
    match frame {
        ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => {
            sparkleflinger
                .preview_only_frame(frame, request)
                .preview_surface
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => {
            sparkleflinger
                .materialize_output_surface(frame)
                .and_then(|surface| {
                    sparkleflinger
                        .preview_only_frame(ProducerFrame::Surface(surface), request)
                        .preview_surface
                })
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
    }
}

fn aggregate_interaction_availability<'a>(
    statuses: impl IntoIterator<Item = &'a SourceStatusHandle>,
) -> InputSourceAvailability {
    let now = Instant::now();
    let mut result = InputSourceAvailability::default();
    for status in statuses {
        let source = status.availability_at(now);
        if source.retired
            || source.kind != SourceKind::Interaction
            || !(source.configured && source.consented && source.demanded)
        {
            continue;
        }
        result.routed = true;
        let healthy = matches!(source.state, SourceState::Live | SourceState::Degraded);
        result.healthy |= healthy;
        result.degraded |= source.state == SourceState::Degraded;
        result.fresh |= healthy
            && matches!(
                source.freshness,
                SourceFreshness::Fresh | SourceFreshness::NotApplicable
            );
    }
    result
}

fn availability_fingerprint(status: &SourceStatusHandle) -> AvailabilityFingerprint {
    let status = status.snapshot();
    AvailabilityFingerprint {
        configured: status.configured,
        consented: status.consented,
        demanded: status.demanded,
        state: status.state,
        freshness: status.freshness,
        session_generation: status.session_generation,
        retired: status.retired,
    }
}

fn preview_interval(fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}

fn advance_deadline(mut deadline: Instant, interval: Duration, now: Instant) -> Instant {
    loop {
        deadline = deadline.checked_add(interval).unwrap_or(now);
        if deadline > now {
            return deadline;
        }
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn duration_millis_u32(duration: Duration) -> u32 {
    let modulus = u128::from(u32::MAX) + 1;
    (duration.as_millis() % modulus)
        .try_into()
        .expect("duration modulo u32 range must fit")
}

fn input_mono_ms() -> u64 {
    hypercolor_core::input::input_mono_ms()
}

const fn backend_to_u8(backend: InteractivePreviewBackend) -> u8 {
    match backend {
        InteractivePreviewBackend::Cpu => 0,
        InteractivePreviewBackend::Gpu => 1,
        InteractivePreviewBackend::CpuAfterGpuFailure => 2,
    }
}

const fn backend_from_u8(backend: u8) -> InteractivePreviewBackend {
    match backend {
        1 => InteractivePreviewBackend::Gpu,
        2 => InteractivePreviewBackend::CpuAfterGpuFailure,
        _ => InteractivePreviewBackend::Cpu,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
#[path = "interactive_preview/tests.rs"]
mod tests;
