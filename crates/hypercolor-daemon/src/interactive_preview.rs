use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::{EffectRegistry, InputSourceAvailability};
use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteCatalog, InteractionRouteDiagnostics, InteractionRouter,
    RoutedInteraction,
};
use hypercolor_core::input::screen::consumer::{PixelExtent, ScreenBranchLease};
use hypercolor_core::input::screen::planner::{
    ScreenPlanGeneration, ScreenPublicationHub, ScreenPublicationKind,
};
use hypercolor_core::input::{
    BrowserInputAttachment, BrowserInputChildKey, BrowserInputPublicationId, InputData,
    InputGraphHandle, InputGraphSnapshot, ScreenBranchPublication, SourceKind,
};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{PublishedSurface, SurfaceDescriptor};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::event::{HypercolorEvent, ZoneColors};
use hypercolor_types::layer::{BindingSource, LayerSource};
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use hypercolor_types::sensor::SystemSnapshot;
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::domain::scene::SceneService;
use crate::interaction_routing::{InteractionRoutingControl, selected_input_availability};
use crate::preview_runtime::PreviewPixelFormat;
#[cfg(feature = "wgpu")]
use crate::render_thread::gpu_device::GpuRenderDevice;
use crate::render_thread::sparkleflinger::{PreviewSurfaceRequest, SparkleFlinger};
use crate::render_thread::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationDemandRegistration, InteractivePreviewZoneRuntime, ProducerFrame,
    RenderSceneContext, SceneDependencyKey, ZoneFrameInputs,
};

mod executor;
mod resources;

pub(crate) use executor::PreviewWorkerPool;
pub(crate) use resources::{PreviewCapacityLedger, PreviewResourceLease};
pub use resources::{PreviewCapacitySnapshot, PreviewResourceLedger};

const MAX_PREVIEW_FPS: u32 = 60;
const BACKGROUND_INPUT_HZ: u32 = 1;
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
    pub spec_generation: u64,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,
    pub surface: PublishedSurface,
    pub(crate) resource_lease: PreviewResourceLease,
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
    pub spec_generation: u64,
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
    #[error(transparent)]
    Capacity(#[from] resources::PreviewCapacityError),
}

#[derive(Clone)]
pub struct InteractivePreviewAcceleration {
    mode: RenderAccelerationMode,
    gpu_requested: bool,
    #[cfg(feature = "wgpu")]
    render_device: Option<GpuRenderDevice>,
}

impl InteractivePreviewAcceleration {
    #[must_use]
    pub fn cpu() -> Self {
        Self {
            mode: RenderAccelerationMode::Cpu,
            gpu_requested: false,
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
            gpu_requested: mode == RenderAccelerationMode::Gpu,
            #[cfg(feature = "wgpu")]
            render_device,
        }
    }

    #[cfg_attr(not(feature = "wgpu"), allow(unused_mut))]
    fn prepare(mut self) -> Self {
        if self.mode != RenderAccelerationMode::Gpu {
            return self;
        }
        #[cfg(feature = "wgpu")]
        {
            self.render_device = self.render_device.as_ref().and_then(|authoritative| {
                authoritative
                    .independent_device("interactive preview executor")
                    .ok()
            });
            if self.render_device.is_none() {
                self.mode = RenderAccelerationMode::Cpu;
            }
        }
        self
    }

    fn uses_gpu(&self) -> bool {
        self.mode == RenderAccelerationMode::Gpu
    }
}

pub struct InteractivePreviewContext {
    pub scene_manager: SceneService,
    pub effect_registry: Arc<RwLock<EffectRegistry>>,
    pub asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    pub event_bus: Arc<HypercolorBus>,
    pub input_graph: InputGraphHandle,
    pub interaction_routing: InteractionRoutingControl,
    pub input_demands: InputPublicationDemandHandle,
    /// Exact screen publication authority the preview leases its surface from.
    pub screen_publications: Arc<ScreenPublicationHub>,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub acceleration: InteractivePreviewAcceleration,
    pub resource_capacity_bytes: u64,
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
    input_demands: InputPublicationDemandHandle,
    screen_publications: Arc<ScreenPublicationHub>,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    acceleration: InteractivePreviewAcceleration,
    render_workers: PreviewWorkerPool,
    encode_workers: PreviewWorkerPool,
    resources: PreviewCapacityLedger,
    catalog_cancel: CancellationToken,
    catalog_task: Mutex<Option<JoinHandle<()>>>,
}

struct PreviewLaneEntry {
    publication_id: BrowserInputPublicationId,
    commands: mpsc::Sender<PreviewLaneCommand>,
    cancel: CancellationToken,
    telemetry: Arc<PreviewLaneTelemetry>,
}

pub struct InteractivePreviewLaneLease {
    key: BrowserInputChildKey,
    publication_id: BrowserInputPublicationId,
    executor: Weak<InteractivePreviewExecutorInner>,
    frames: watch::Receiver<Option<Arc<InteractivePreviewFrame>>>,
    spec_generation: watch::Receiver<u64>,
    retired: watch::Receiver<bool>,
    cancel: CancellationToken,
    encode_workers: PreviewWorkerPool,
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
    spec_generation: AtomicU64,
    route_diagnostics: ArcSwap<InteractionRouteDiagnostics>,
    last_error: ArcSwap<Option<Arc<str>>>,
}

enum PreviewLaneCommand {
    Update {
        spec: InteractivePreviewSpec,
        resources: PreviewResourceLease,
        response: oneshot::Sender<Result<(), String>>,
    },
    #[cfg(test)]
    Panic { started: oneshot::Sender<()> },
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
    active_zones_revision: u64,
    active_zones: Arc<[Zone]>,
    scenes: Arc<[PreviewSceneEntry]>,
    registry: Arc<EffectRegistry>,
}

struct PreviewSceneEntry {
    id: SceneId,
    zones_revision: u64,
    zones: Arc<[Zone]>,
}

struct ResolvedPreviewScene {
    scene_id: Option<SceneId>,
    zones_revision: u64,
    zones: Arc<[Zone]>,
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
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    acceleration: InteractivePreviewAcceleration,
    zone_runtime: InteractivePreviewZoneRuntime,
    sparkleflinger: SparkleFlinger,
    resources: PreviewResourceLease,
    input: PreviewLaneInput,
    demand: InputPublicationDemandRegistration,
    current_demand: InputPublicationDemand,
    frame_tx: watch::Sender<Option<Arc<InteractivePreviewFrame>>>,
    spec_generation_tx: watch::Sender<u64>,
    telemetry: Arc<PreviewLaneTelemetry>,
    frame_number: u32,
    spec_generation: u64,
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
    routing: InteractionRoutingControl,
    screen_publications: Arc<ScreenPublicationHub>,
    publication_id: BrowserInputPublicationId,
    interaction_catalog: InteractionRouteCatalog,
    router: InteractionRouter,
    routed: RoutedInteraction,
    audio: Option<Arc<InputData>>,
    screen_route: Option<PreviewScreenRoute>,
    screen: Option<Arc<ScreenBranchPublication>>,
    media: Option<Arc<InputData>>,
    network: Option<Arc<InputData>>,
    sensors: Option<Arc<InputData>>,
    empty_audio: AudioData,
    sensor_snapshot: Arc<SystemSnapshot>,
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
            context.scene_manager.clone(),
            Arc::clone(&context.effect_registry),
            Arc::clone(&context.event_bus),
            context.canvas_width,
            context.canvas_height,
            catalog_cancel.clone(),
        );
        let worker_count = std::thread::available_parallelism().map_or(1, usize::from);
        let render_workers = PreviewWorkerPool::new("preview-render", worker_count)
            .map_err(|error| InteractivePreviewError::Initialization(error.to_string()))?;
        let encode_workers = PreviewWorkerPool::new("preview-encode", worker_count)
            .map_err(|error| InteractivePreviewError::Initialization(error.to_string()))?;
        let acceleration = context.acceleration.prepare();
        Ok(Self {
            inner: Arc::new(InteractivePreviewExecutorInner {
                lanes: Mutex::new(HashMap::new()),
                catalog,
                interaction_routing: context.interaction_routing,
                input_graph: context.input_graph,
                input_demands: context.input_demands,
                screen_publications: context.screen_publications,
                asset_library: context.asset_library,
                acceleration,
                render_workers,
                encode_workers,
                resources: PreviewCapacityLedger::new(context.resource_capacity_bytes),
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
        let scene = self
            .inner
            .catalog
            .snapshot()
            .resolve(spec.target)
            .ok_or(InteractivePreviewError::TargetUnavailable)?;
        let resource_ledger = PreviewResourceLedger::for_lane(
            spec,
            scene.canvas_width,
            scene.canvas_height,
            self.inner.acceleration.uses_gpu(),
            attachment.key().preview_id().as_str().len(),
        )
        .map_err(|error| InteractivePreviewError::InvalidSpec(error.to_string()))?;
        let resources = self.inner.resources.try_reserve(resource_ledger)?;

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
            spec_generation: AtomicU64::new(1),
            route_diagnostics: ArcSwap::from(initial_diagnostics),
            last_error: ArcSwap::from_pointee(None),
        });
        let (command_tx, command_rx) = mpsc::channel(1);
        let lane_cancel = CancellationToken::new();
        let (frame_tx, frame_rx) = watch::channel(None);
        let (spec_generation_tx, spec_generation_rx) = watch::channel(1);
        let (retired_tx, retired_rx) = watch::channel(false);
        let id = PreviewLaneId {
            key: attachment.key().clone(),
            publication_id: attachment.publication_id(),
        };

        let weak_executor = Arc::downgrade(&self.inner);
        let lane_context = PreviewLaneContext {
            id: id.clone(),
            consumer,
            spec,
            catalog: self.inner.catalog.clone(),
            graph: self.inner.input_graph.clone(),
            routing: self.inner.interaction_routing.clone(),
            demands: self.inner.input_demands.clone(),
            screen_publications: Arc::clone(&self.inner.screen_publications),
            asset_library: self.inner.asset_library.clone(),
            acceleration: self.inner.acceleration.clone(),
            resources,
            frame_tx,
            spec_generation_tx,
            telemetry: Arc::clone(&telemetry),
        };
        let (ready_tx, ready_rx) = oneshot::channel();
        self.inner
            .render_workers
            .execute(move || {
                let _ = ready_tx.send(PreviewLane::new(lane_context));
            })
            .map_err(|_| InteractivePreviewError::WorkerClosed)?;
        let (lane, backend) = ready_rx
            .await
            .map_err(|_| InteractivePreviewError::WorkerClosed)?
            .map_err(InteractivePreviewError::Initialization)?;
        telemetry
            .backend
            .store(backend_to_u8(backend), Ordering::Release);

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
                    cancel: lane_cancel.clone(),
                    telemetry: Arc::clone(&telemetry),
                },
            );
        }
        let render_workers = self.inner.render_workers.clone();
        let lease_cancel = lane_cancel.clone();
        tokio::spawn(run_preview_lane(
            lane,
            command_rx,
            weak_executor,
            render_workers,
            lane_cancel,
            retired_tx,
        ));

        Ok(InteractivePreviewLaneLease {
            key: id.key,
            publication_id: id.publication_id,
            executor: Arc::downgrade(&self.inner),
            frames: frame_rx,
            spec_generation: spec_generation_rx,
            retired: retired_rx,
            cancel: lease_cancel,
            encode_workers: self.inner.encode_workers.clone(),
            telemetry,
            closed: false,
        })
    }

    #[must_use]
    pub fn lane_count(&self) -> usize {
        lock(&self.inner.lanes).len()
    }

    #[must_use]
    pub fn resource_snapshot(&self) -> PreviewCapacitySnapshot {
        self.inner.resources.snapshot()
    }

    #[must_use]
    pub fn render_worker_count(&self) -> usize {
        self.inner.render_workers.worker_count()
    }

    #[must_use]
    pub fn encode_worker_count(&self) -> usize {
        self.inner.encode_workers.worker_count()
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

    pub(crate) fn spec_generation_receiver(&self) -> watch::Receiver<u64> {
        self.spec_generation.clone()
    }

    pub(crate) fn encode_workers(&self) -> PreviewWorkerPool {
        self.encode_workers.clone()
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
        let scene = executor
            .catalog
            .snapshot()
            .resolve(spec.target)
            .ok_or(InteractivePreviewError::TargetUnavailable)?;
        let ledger = PreviewResourceLedger::for_lane(
            spec,
            scene.canvas_width,
            scene.canvas_height,
            executor.acceleration.uses_gpu(),
            self.key.preview_id().as_str().len(),
        )
        .map_err(|error| InteractivePreviewError::InvalidSpec(error.to_string()))?;
        let resources = executor.resources.try_reserve(ledger)?;
        let commands = executor
            .commands_exact(&PreviewLaneId {
                key: self.key.clone(),
                publication_id: self.publication_id,
            })
            .ok_or(InteractivePreviewError::WorkerClosed)?;
        request_preview_lane_update(&commands, &self.cancel, spec, resources).await
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

    pub async fn close_and_wait(&mut self) -> bool {
        let closed = self.close();
        while !*self.retired.borrow() && self.retired.changed().await.is_ok() {}
        closed
    }
}

async fn request_preview_lane_update(
    commands: &mpsc::Sender<PreviewLaneCommand>,
    cancel: &CancellationToken,
    spec: InteractivePreviewSpec,
    resources: PreviewResourceLease,
) -> Result<(), InteractivePreviewError> {
    let (response_tx, response_rx) = oneshot::channel();
    let command = PreviewLaneCommand::Update {
        spec,
        resources,
        response: response_tx,
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(InteractivePreviewError::WorkerClosed),
        result = commands.send(command) => {
            result.map_err(|_| InteractivePreviewError::WorkerClosed)?;
        }
    }
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(InteractivePreviewError::WorkerClosed),
        result = response_rx => result
            .map_err(|_| InteractivePreviewError::WorkerClosed)?
            .map_err(InteractivePreviewError::Update),
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
        let lanes = lock(&self.lanes);
        let Some(entry) = lanes
            .get(&id.key)
            .filter(|entry| entry.publication_id == id.publication_id)
        else {
            return false;
        };
        entry.telemetry.active.store(false, Ordering::Release);
        entry.cancel.cancel();
        true
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
            entry.cancel.cancel();
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
            spec_generation: self.spec_generation.load(Ordering::Acquire),
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

struct PreviewLaneContext {
    id: PreviewLaneId,
    consumer: ConsumerIncarnation,
    spec: InteractivePreviewSpec,
    catalog: PreviewSceneCatalogSource,
    graph: InputGraphHandle,
    routing: InteractionRoutingControl,
    demands: InputPublicationDemandHandle,
    screen_publications: Arc<ScreenPublicationHub>,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    acceleration: InteractivePreviewAcceleration,
    resources: PreviewResourceLease,
    frame_tx: watch::Sender<Option<Arc<InteractivePreviewFrame>>>,
    spec_generation_tx: watch::Sender<u64>,
    telemetry: Arc<PreviewLaneTelemetry>,
}

async fn run_preview_lane(
    lane: PreviewLane,
    mut commands: mpsc::Receiver<PreviewLaneCommand>,
    executor: Weak<InteractivePreviewExecutorInner>,
    workers: PreviewWorkerPool,
    cancel: CancellationToken,
    retired: watch::Sender<bool>,
) {
    let _retirement = PreviewLaneRetirementGuard {
        id: lane.id.clone(),
        executor,
        retired,
        telemetry: Arc::clone(&lane.telemetry),
        frame_tx: lane.frame_tx.clone(),
    };
    let mut lane = Some(lane);
    let mut next_frame = Instant::now();
    'run: loop {
        let deadline = tokio::time::Instant::from_std(next_frame);
        let action = tokio::select! {
            biased;
            () = cancel.cancelled() => PreviewLaneAction::Stop,
            command = commands.recv() => match command {
                Some(command) => PreviewLaneAction::Command(command),
                None => PreviewLaneAction::Stop,
            },
            () = tokio::time::sleep_until(deadline) => PreviewLaneAction::Render(Instant::now()),
        };
        if matches!(action, PreviewLaneAction::Stop) {
            break;
        }
        let mut dispatched_lane = lane
            .take()
            .expect("preview lane must return before another action is dispatched");
        let (result_tx, result_rx) = oneshot::channel();
        if workers
            .execute(move || {
                let outcome = match action {
                    PreviewLaneAction::Command(command) => {
                        let keep_running = dispatched_lane.apply_command(command, &mut next_frame);
                        (dispatched_lane, next_frame, keep_running)
                    }
                    PreviewLaneAction::Render(now) => {
                        dispatched_lane.render(now);
                        let next = advance_deadline(
                            next_frame,
                            preview_interval(dispatched_lane.spec.fps),
                            now,
                        );
                        (dispatched_lane, next, true)
                    }
                    PreviewLaneAction::Stop => {
                        unreachable!("stop actions are handled before dispatch")
                    }
                };
                let _ = result_tx.send(outcome);
            })
            .is_err()
        {
            break;
        }
        match result_rx.await {
            Ok((returned_lane, returned_deadline, keep_running)) => {
                lane = Some(returned_lane);
                next_frame = returned_deadline;
                if !keep_running {
                    break 'run;
                }
            }
            Err(_) => break,
        }
    }

    if let Some(mut lane) = lane {
        lane.telemetry.active.store(false, Ordering::Release);
        lane.frame_tx.send_replace(None);
        lane.retained_frame = None;
    }
}

struct PreviewLaneRetirementGuard {
    id: PreviewLaneId,
    executor: Weak<InteractivePreviewExecutorInner>,
    retired: watch::Sender<bool>,
    telemetry: Arc<PreviewLaneTelemetry>,
    frame_tx: watch::Sender<Option<Arc<InteractivePreviewFrame>>>,
}

impl Drop for PreviewLaneRetirementGuard {
    fn drop(&mut self) {
        self.telemetry.active.store(false, Ordering::Release);
        self.frame_tx.send_replace(None);
        if let Some(executor) = self.executor.upgrade() {
            executor.retire_exact(&self.id);
        }
        self.retired.send_replace(true);
    }
}

enum PreviewLaneAction {
    Command(PreviewLaneCommand),
    Render(Instant),
    Stop,
}

impl PreviewLane {
    fn new(context: PreviewLaneContext) -> Result<(Self, InteractivePreviewBackend), String> {
        let catalog = context.catalog.snapshot();
        let resolved = catalog
            .resolve(context.spec.target)
            .ok_or_else(|| "preview target disappeared during open".to_owned())?;
        let (sparkleflinger, backend) = create_preview_compositor(&context.acceleration);
        let zone_runtime = preview_zone_runtime(
            resolved.canvas_width,
            resolved.canvas_height,
            context.asset_library.clone(),
        )?;
        let current_demand = preview_input_demand(&resolved, context.spec.fps);
        let demand = context
            .demands
            .register(InputPublicationConsumer::Preview, current_demand.clone());
        let input = PreviewLaneInput::new(
            context.graph,
            context.routing,
            context.screen_publications,
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
                asset_library: context.asset_library,
                acceleration: context.acceleration,
                zone_runtime,
                sparkleflinger,
                resources: context.resources,
                input,
                demand,
                current_demand,
                frame_tx: context.frame_tx,
                spec_generation_tx: context.spec_generation_tx,
                telemetry: context.telemetry,
                frame_number: 0,
                spec_generation: 1,
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
            PreviewLaneCommand::Update {
                spec,
                resources,
                response,
            } => {
                let result = self.apply_update(spec, resources, Instant::now());
                if result.is_ok() {
                    *next_frame = Instant::now();
                }
                let _ = response.send(result);
                true
            }
            #[cfg(test)]
            PreviewLaneCommand::Panic { started } => {
                let _ = started.send(());
                panic!("injected preview lane worker panic");
            }
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
        if !demand.same_publication_request(&self.current_demand) {
            self.demand.update(demand.clone());
            self.current_demand = demand;
        }
        let screen_extent = PixelExtent::new(scene.canvas_width, scene.canvas_height)
            .expect("resolved preview canvas dimensions are non-empty");
        self.input.read(screen_extent);
        self.telemetry
            .route_diagnostics
            .store(Arc::clone(&self.input.routed.diagnostics));
        let delta_secs = now.saturating_duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        let result = render_preview_scene(
            &mut self.zone_runtime,
            &mut self.sparkleflinger,
            &mut self.zones,
            &self.display_descriptors,
            &scene,
            &self.input,
            self.spec,
            duration_millis_u64(self.started.elapsed()),
            delta_secs,
        );
        match result {
            Ok(surface) => self.publish_surface(surface),
            Err(error) => self.telemetry.publish_error(error),
        }
    }

    fn apply_update(
        &mut self,
        spec: InteractivePreviewSpec,
        resources: PreviewResourceLease,
        now: Instant,
    ) -> Result<(), String> {
        let scene = self
            .catalog
            .snapshot()
            .resolve(spec.target)
            .ok_or_else(|| "preview target is unavailable".to_owned())?;
        let mut candidate = PreviewLaneCandidate::new(
            &scene,
            self.asset_library.clone(),
            &self.acceleration,
            resources,
        )?;
        let surface = render_preview_scene(
            &mut candidate.zone_runtime,
            &mut candidate.sparkleflinger,
            &mut candidate.zones,
            &candidate.display_descriptors,
            &scene,
            &self.input,
            spec,
            duration_millis_u64(self.started.elapsed()),
            now.saturating_duration_since(self.last_tick).as_secs_f32(),
        )?;
        let demand = preview_input_demand(&scene, spec.fps);
        if !demand.same_publication_request(&self.current_demand) {
            self.demand.update(demand.clone());
            self.current_demand = demand;
        }
        self.spec = spec;
        self.zone_runtime = candidate.zone_runtime;
        self.sparkleflinger = candidate.sparkleflinger;
        self.resources = candidate.resources;
        self.zones = candidate.zones;
        self.display_descriptors = candidate.display_descriptors;
        self.last_tick = now;
        self.spec_generation = self
            .spec_generation
            .checked_add(1)
            .expect("interactive preview spec generation exhausted");
        self.telemetry.spec.store(Arc::new(spec));
        self.telemetry
            .backend
            .store(backend_to_u8(candidate.backend), Ordering::Release);
        self.telemetry
            .spec_generation
            .store(self.spec_generation, Ordering::Release);
        self.spec_generation_tx.send_replace(self.spec_generation);
        self.publish_surface(surface);
        Ok(())
    }

    fn publish_surface(&mut self, surface: PublishedSurface) {
        let frame = Arc::new(InteractivePreviewFrame {
            publication_id: self.id.publication_id,
            spec_generation: self.spec_generation,
            frame_number: self.frame_number,
            timestamp_ms: duration_millis_u32(self.started.elapsed()),
            width: surface.width(),
            height: surface.height(),
            format: self.spec.format,
            surface,
            resource_lease: self.resources.clone(),
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
}

impl Drop for PreviewLane {
    fn drop(&mut self) {
        let _ = self
            .input
            .router
            .remove_consumer(self.consumer, input_mono_ms());
    }
}

struct PreviewLaneCandidate {
    zone_runtime: InteractivePreviewZoneRuntime,
    sparkleflinger: SparkleFlinger,
    resources: PreviewResourceLease,
    backend: InteractivePreviewBackend,
    zones: Vec<ZoneColors>,
    display_descriptors: HashMap<ZoneId, DisplayDescriptor>,
}

impl PreviewLaneCandidate {
    fn new(
        scene: &ResolvedPreviewScene,
        asset_library: Option<Arc<RwLock<AssetLibrary>>>,
        acceleration: &InteractivePreviewAcceleration,
        resources: PreviewResourceLease,
    ) -> Result<Self, String> {
        let zone_runtime =
            preview_zone_runtime(scene.canvas_width, scene.canvas_height, asset_library)?;
        let (mut sparkleflinger, backend) = create_preview_compositor(acceleration);
        let canvas = sparkleflinger
            .prepare_canvas_resize(scene.canvas_width, scene.canvas_height)
            .map_err(|error| error.to_string())?;
        sparkleflinger.apply_canvas_resize(canvas);
        Ok(Self {
            zone_runtime,
            sparkleflinger,
            resources,
            backend,
            zones: Vec::new(),
            display_descriptors: HashMap::new(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn render_preview_scene(
    zone_runtime: &mut InteractivePreviewZoneRuntime,
    sparkleflinger: &mut SparkleFlinger,
    zones: &mut Vec<ZoneColors>,
    display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
    scene: &ResolvedPreviewScene,
    input: &PreviewLaneInput,
    spec: InteractivePreviewSpec,
    elapsed_ms: u64,
    delta_secs: f32,
) -> Result<PublishedSurface, String> {
    let context = RenderSceneContext {
        zones: &scene.zones,
        active_scene_id: scene.scene_id,
        dependency_key: SceneDependencyKey::new(scene.zones_revision, scene.catalog_generation),
        elapsed_ms,
        display_zone_target_fps: &HashMap::new(),
        display_zone_descriptors: display_descriptors,
        registry: &scene.registry,
        authoritative_spatial_engine: None,
        inputs: ZoneFrameInputs {
            delta_secs,
            audio: input.audio(),
            interaction: &input.routed.interaction,
            screen: input.screen(),
            sensors: input.sensors(),
            input_availability: input.interaction_availability(),
            media: input.media(),
            net: input.network(),
            lighting: None,
        },
    };
    zone_runtime
        .render_scene(context, sparkleflinger, zones)
        .map_err(|error| error.to_string())
        .and_then(|rendered| {
            preview_surface(sparkleflinger, rendered, spec.width, spec.height).ok_or_else(|| {
                "preview frame could not be materialized on its isolated device".to_owned()
            })
        })
}

fn preview_zone_runtime(
    width: u32,
    height: u32,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
) -> Result<InteractivePreviewZoneRuntime, String> {
    match asset_library {
        Some(library) => InteractivePreviewZoneRuntime::with_asset_library(width, height, library),
        None => InteractivePreviewZoneRuntime::new(width, height),
    }
    .map_err(|error| error.to_string())
}

/// One exact surface lease matched to the preview canvas extent.
struct PreviewScreenRoute {
    plan_generation: ScreenPlanGeneration,
    extent: PixelExtent,
    lease: Option<ScreenBranchLease>,
}

impl PreviewLaneInput {
    fn new(
        graph: InputGraphHandle,
        routing: InteractionRoutingControl,
        screen_publications: Arc<ScreenPublicationHub>,
        publication_id: BrowserInputPublicationId,
        consumer: ConsumerIncarnation,
    ) -> Self {
        Self {
            graph,
            routing,
            screen_publications,
            publication_id,
            interaction_catalog: InteractionRouteCatalog::default(),
            router: InteractionRouter::default(),
            routed: RoutedInteraction::new(consumer),
            audio: None,
            screen_route: None,
            screen: None,
            media: None,
            network: None,
            sensors: None,
            empty_audio: AudioData::silence(),
            sensor_snapshot: Arc::new(SystemSnapshot::empty()),
        }
    }

    fn read(&mut self, screen_extent: PixelExtent) {
        let graph = self.graph.snapshot();
        let browser = self.routing.browser_registry_snapshot();
        self.interaction_catalog
            .refresh(&graph, &browser, Instant::now());
        self.read_typed(&graph);
        self.read_screen(screen_extent);
        let routing = self.routing.snapshot();
        self.interaction_catalog.resolve_into(
            &mut self.router,
            self.routed.diagnostics.consumer,
            routing.preview_request(self.publication_id),
            routing.config_generation,
            input_mono_ms(),
            &mut self.routed,
        );
    }

    /// Lease the exact surface branch published at the preview canvas
    /// extent and read its latest publication.
    ///
    /// The preview registers its own surface demand, so the branch it
    /// leases is the one its consumer asked for; the lease is re-resolved
    /// only when the committed plan generation or canvas extent moves.
    fn read_screen(&mut self, extent: PixelExtent) {
        let (plan_generation, observed_lease) =
            self.screen_publications
                .observe_matching_lease(|descriptor| {
                    descriptor.kind() == ScreenPublicationKind::Surface
                        && descriptor.geometry().output_extent() == extent
                });
        let route_is_current = self.screen_route.as_ref().is_some_and(|route| {
            route.plan_generation == plan_generation && route.extent == extent
        });
        if !route_is_current {
            self.screen_route = Some(PreviewScreenRoute {
                plan_generation,
                extent,
                lease: observed_lease,
            });
        }
        self.screen = self
            .screen_route
            .as_ref()
            .and_then(|route| route.lease.as_ref())
            .and_then(ScreenBranchLease::read);
    }

    fn read_typed(&mut self, graph: &InputGraphSnapshot) {
        self.audio = None;
        self.media = None;
        self.network = None;
        self.sensors = None;
        for slot in graph.slots() {
            let Some(sample) = slot.latest() else {
                continue;
            };
            match sample.as_ref() {
                InputData::Audio(_) => self.audio = Some(sample),
                InputData::Media(_) => self.media = Some(sample),
                InputData::Net(_) => self.network = Some(sample),
                InputData::Sensors(_) => self.sensors = Some(sample),
                InputData::Screen(_) | InputData::Interaction(_) | InputData::None => {}
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

    fn screen(&self) -> Option<&Arc<ScreenBranchPublication>> {
        self.screen.as_ref()
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
        selected_input_availability(
            self.routed
                .diagnostics
                .selected
                .iter()
                .filter_map(|source| source.status.as_ref()),
            Instant::now(),
        )
    }
}

impl PreviewSceneCatalogSource {
    async fn capture(
        scene_manager: &SceneService,
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
        let (scene_id, zones_revision, zones) = match target {
            InteractivePreviewTarget::ActiveScene => (
                self.active_scene_id,
                self.active_zones_revision,
                Arc::clone(&self.active_zones),
            ),
            InteractivePreviewTarget::Scene(scene_id) => {
                let scene = self.scenes.iter().find(|scene| scene.id == scene_id)?;
                (
                    Some(scene.id),
                    scene.zones_revision,
                    Arc::clone(&scene.zones),
                )
            }
        };
        Some(ResolvedPreviewScene {
            scene_id,
            zones_revision,
            zones,
            registry: Arc::clone(&self.registry),
            catalog_generation: self.generation,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        })
    }
}

fn spawn_catalog_publisher(
    catalog: PreviewSceneCatalogSource,
    scene_manager: SceneService,
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
    scene_manager: &SceneService,
    effect_registry: &Arc<RwLock<EffectRegistry>>,
    canvas_width: u32,
    canvas_height: u32,
    generation: u64,
) -> Arc<PreviewSceneCatalog> {
    let (active_scene_id, active_zones_revision, active_zones, scenes) = {
        let manager = scene_manager.snapshot().await;
        let scenes = manager
            .list()
            .into_iter()
            .map(|scene| PreviewSceneEntry {
                id: scene.id,
                zones_revision: scene.zones_revision,
                zones: scene.zones.clone().into(),
            })
            .collect::<Vec<_>>()
            .into();
        (
            manager.active_scene_id().copied(),
            manager.resolved_zones_revision(),
            manager.resolved_zones(),
            scenes,
        )
    };
    let registry = Arc::new(effect_registry.read().await.clone());
    Arc::new(PreviewSceneCatalog {
        generation,
        canvas_width,
        canvas_height,
        active_scene_id,
        active_zones_revision,
        active_zones,
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
            | HypercolorEvent::ZoneChanged { .. }
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
    let mut sensors = false;
    for zone in scene.zones.iter().filter(|zone| zone.enabled) {
        for layer in zone.layers.iter().filter(|layer| layer.enabled) {
            sensors |= layer
                .bindings
                .iter()
                .any(|binding| matches!(&binding.source, BindingSource::Sensor { .. }));
            match &layer.source {
                LayerSource::Effect {
                    effect_id,
                    control_bindings,
                    ..
                } => {
                    sensors |= !control_bindings.is_empty();
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
                        sensors |= entry.metadata.requires_sensors();
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
        demand = demand.with_source(SourceKind::Media, BACKGROUND_INPUT_HZ);
    }
    if network {
        demand = demand.with_source(SourceKind::Network, BACKGROUND_INPUT_HZ);
    }
    if sensors {
        demand = demand.with_source(SourceKind::Sensors, BACKGROUND_INPUT_HZ);
    }
    demand
}

fn create_preview_compositor(
    acceleration: &InteractivePreviewAcceleration,
) -> (SparkleFlinger, InteractivePreviewBackend) {
    if acceleration.mode != RenderAccelerationMode::Gpu {
        let backend = if acceleration.gpu_requested {
            InteractivePreviewBackend::CpuAfterGpuFailure
        } else {
            InteractivePreviewBackend::Cpu
        };
        return (SparkleFlinger::cpu(), backend);
    }
    #[cfg(feature = "wgpu")]
    if let Some(device) = acceleration.render_device.clone()
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
        ProducerFrame::Canvas(_)
        | ProducerFrame::Surface(_)
        | ProducerFrame::ScreenPublication(_) => {
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
