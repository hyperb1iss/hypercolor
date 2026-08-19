#[cfg(feature = "wgpu")]
use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use hypercolor_core::asset::AssetLibrary;
#[cfg(feature = "wgpu")]
use hypercolor_core::bus::DisplayGroupOutputRoute;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::{EffectRegistry, InputSourceAvailability};
use hypercolor_core::engine::FpsTier;
use hypercolor_core::input::browser::BrowserInputRegistrySnapshot;
use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteContext, InteractionRouteRequest, InteractionRouteSource,
    InteractionRouteSourceClass, InteractionRouter, RoutedInteraction, SourceIncarnation,
};
use hypercolor_core::input::screen::{
    PixelExtent, ResolvedScreenPublicationDescriptor, ScreenBranchDeliveryState, ScreenBranchLease,
    ScreenBranchPublication, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
    ScreenPlanGeneration, ScreenPublicationExecutorRequest,
};
use hypercolor_core::input::{
    InputData, InputGraphSnapshot, InputSourceSlot, InteractionData, MotionAggregate, PointerMode,
    SourceFreshness, SourceKind, SourceState, SourceStatusAvailability, SourceStatusHandle,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor, SurfaceResourceError,
};
use hypercolor_core::types::event::{FrameData, HypercolorEvent};
use hypercolor_types::audio::{AudioData, CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};
use hypercolor_types::config::RenderAccelerationMode;
#[cfg(feature = "wgpu")]
use hypercolor_types::device::DisplayFrameFormat;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::SceneId;
#[cfg(feature = "wgpu")]
use hypercolor_types::scene::{DisplayFaceTarget, ZoneId};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{Output, SpatialLayout};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use super::composition_planner::CompositionPlanner;
use super::frame_composer::RenderStageStats;
use super::frame_policy::FramePolicy;
use super::frame_policy::SkipDecision;
#[cfg(feature = "wgpu")]
use super::gpu_device::GpuRenderDevice;
use super::input_publication::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationReader, OwnedInputPublicationDemand,
};
#[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
use super::macos_screen_diagnostics::MacosScreenParityDiagnosticMailbox;
use super::producer_queue::ProducerQueue;
use super::render_groups::{
    PreparedZoneReconcile, RenderSceneContext, ZoneFrameInputs, ZoneResult, ZoneRuntime,
};
use super::scene_dependency::SceneDependencyKey;
use super::scene_snapshot::{EffectDemand, FrameSceneSnapshot, SceneSnapshotCache};
use super::scene_state::RenderSceneState;
use super::screen_canvas::screen_data_to_surface;
#[cfg(feature = "wgpu")]
use super::sparkleflinger::PendingDisplayFinalization;
use super::sparkleflinger::{
    PendingZoneSampling, SparkleFlinger, SparkleFlingerCanvasPreparation,
    SparkleFlingerSamplingPreparation,
};
use super::{RenderThreadState, micros_u32};
use crate::interaction_routing::InteractionRoutingControl;
use crate::scene_transactions::{LayoutActivationControl, LayoutTransactionRejection};

const AUDIO_LEVEL_EVENT_INTERVAL_MS: u64 = 100;
const BACKGROUND_INPUT_HZ: u32 = 1;
const DEFAULT_SCREEN_SURFACE_WIDTH: u32 = 1;
const DEFAULT_SCREEN_SURFACE_HEIGHT: u32 = 1;
const AUTHORITATIVE_INPUT_CONSUMER: ConsumerIncarnation = ConsumerIncarnation::new(1);

/// Strictly increasing delivery order for input events across all frames.
fn next_input_event_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

pub(crate) struct FrameInputs {
    pub(crate) audio: AudioData,
    pub(crate) audio_was_published: bool,
    pub(crate) interaction: hypercolor_core::input::InteractionData,
    pub(crate) screen_data: Option<hypercolor_core::input::ScreenData>,
    pub(crate) screen_publication: Option<Arc<ScreenBranchPublication>>,
    pub(crate) screen_delivery_state: Option<ScreenBranchDeliveryState>,
    pub(crate) screen_invalidation_epoch: u64,
    pub(crate) screen_compositor_epoch: u64,
    pub(crate) screen_descriptor: Option<ResolvedScreenPublicationDescriptor>,
    pub(crate) sensors: Arc<SystemSnapshot>,
    pub(crate) input_availability: InputSourceAvailability,
    empty_sensors: Arc<SystemSnapshot>,
    /// Latest media snapshot, carried across frames — the source only emits
    /// on change.
    pub(crate) media: Option<Arc<hypercolor_types::media::MediaState>>,
    /// Latest net snapshot, carried across frames between 1 Hz refreshes.
    pub(crate) net: Option<Arc<hypercolor_types::net::NetStats>>,
    /// Lighting state assembled per frame by the executor before composing.
    pub(crate) lighting: Option<Arc<hypercolor_types::lighting::LightingState>>,
    pub(crate) screen_surface: Option<PublishedSurface>,
    pub(crate) screen_sector_grid: Vec<[u8; 3]>,
    screen_surface_pool: Option<RenderSurfacePool>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameTick {
    pub(crate) frame_interval_us: u32,
    pub(crate) frame_interval: Duration,
    pub(crate) delta_secs: f32,
}

#[derive(Debug)]
pub(crate) struct FrameClockState {
    last_tick: Instant,
}

impl Default for FrameClockState {
    fn default() -> Self {
        Self {
            last_tick: Instant::now(),
        }
    }
}

impl FrameClockState {
    pub(crate) fn rebase(&mut self, frame_start: Instant) {
        self.last_tick = frame_start;
    }

    pub(crate) fn advance(&mut self, frame_start: Instant) -> FrameTick {
        let frame_interval = frame_start.saturating_duration_since(self.last_tick);
        self.last_tick = frame_start;
        FrameTick {
            frame_interval_us: micros_u32(frame_interval),
            frame_interval,
            delta_secs: frame_interval.as_secs_f32(),
        }
    }
}

pub(crate) struct InputReuseState {
    cached_inputs: FrameInputs,
    routes: InputRouteCache,
}

impl InputReuseState {
    fn with_routing(
        reader: InputPublicationReader,
        interaction_routing: InteractionRoutingControl,
    ) -> Self {
        Self {
            cached_inputs: FrameInputs::silence(),
            routes: InputRouteCache::new(reader, interaction_routing),
        }
    }

    pub(crate) fn observe_screen_plan(
        &mut self,
        state: &RenderThreadState,
        screen_target: Option<&ScreenNativeExecutionTarget>,
    ) -> ScreenPlanGeneration {
        let screen_extent = PixelExtent::new(state.canvas_dims.width(), state.canvas_dims.height())
            .expect("render canvas dimensions are non-empty");
        let (generation, publication, delivery_state) =
            self.routes.read_screen(screen_target, screen_extent);
        self.cached_inputs.screen_publication = publication;
        self.cached_inputs.screen_invalidation_epoch =
            delivery_state.map_or(0, ScreenBranchDeliveryState::invalidation_epoch);
        self.cached_inputs.screen_delivery_state = delivery_state;
        self.cached_inputs.screen_descriptor = self.routes.screen_descriptor().cloned();
        generation
    }

    pub(crate) fn inputs_for_frame<'a>(
        &'a mut self,
        state: &RenderThreadState,
        skip_decision: SkipDecision,
        _delta_secs: f32,
    ) -> &'a mut FrameInputs {
        if matches!(skip_decision, SkipDecision::None) {
            self.routes.read_into(state, &mut self.cached_inputs);
        } else {
            self.cached_inputs.clear_transient_interaction();
            self.cached_inputs.input_availability = self.routes.refresh_interaction_availability();
        }

        &mut self.cached_inputs
    }
}

#[derive(Clone)]
struct CachedInputRoute {
    slot: InputSourceSlot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AvailabilityFingerprint {
    structural_graph_generation: u64,
    session_generation: u64,
    effective: SourceStatusAvailability,
}

#[derive(Clone, Copy, Debug)]
struct CachedInteractionAvailability {
    incarnation: SourceIncarnation,
    fingerprint: AvailabilityFingerprint,
    revision: u64,
}

struct InputRouteCache {
    reader: InputPublicationReader,
    interaction_routing: InteractionRoutingControl,
    graph_generation: Option<u64>,
    browser_registry_generation: Option<u64>,
    routes: Vec<CachedInputRoute>,
    audio_routes: Vec<usize>,
    screen_routes: Vec<usize>,
    media_routes: Vec<usize>,
    network_routes: Vec<usize>,
    untyped_routes: Vec<usize>,
    interaction_sources: Vec<InteractionRouteSource>,
    interaction_statuses: Vec<SourceStatusHandle>,
    interaction_availability: Vec<CachedInteractionAvailability>,
    interaction_router: InteractionRouter,
    routed_interaction: RoutedInteraction,
    screen_publication_route: Option<ScreenPublicationRoute>,
    last_screen_observation: Option<(bool, bool, bool)>,
}

struct ScreenPublicationRoute {
    plan_generation: ScreenPlanGeneration,
    target_id: Option<ScreenNativeExecutionTargetId>,
    extent: PixelExtent,
    lease: Option<ScreenBranchLease>,
}

#[cfg(test)]
impl Default for InputRouteCache {
    fn default() -> Self {
        Self::new(
            InputPublicationReader::empty(),
            InteractionRoutingControl::default(),
        )
    }
}

impl InputRouteCache {
    fn new(reader: InputPublicationReader, interaction_routing: InteractionRoutingControl) -> Self {
        Self {
            reader,
            interaction_routing,
            graph_generation: None,
            browser_registry_generation: None,
            routes: Vec::new(),
            audio_routes: Vec::new(),
            screen_routes: Vec::new(),
            media_routes: Vec::new(),
            network_routes: Vec::new(),
            untyped_routes: Vec::new(),
            interaction_sources: Vec::new(),
            interaction_statuses: Vec::new(),
            interaction_availability: Vec::new(),
            interaction_router: InteractionRouter::default(),
            routed_interaction: RoutedInteraction::new(AUTHORITATIVE_INPUT_CONSUMER),
            screen_publication_route: None,
            last_screen_observation: None,
        }
    }

    fn read_into(&mut self, state: &RenderThreadState, inputs: &mut FrameInputs) {
        let sensors = self.reader.latest_sensor_snapshot();
        let graph = self.reader.graph_snapshot();
        let graph_changed = self.graph_generation != Some(graph.generation());
        if graph_changed {
            self.rebuild_routes(&graph);
        }
        let browser_registry = self.interaction_routing.browser_registry_snapshot();
        if graph_changed || self.browser_registry_generation != Some(browser_registry.generation())
        {
            self.rebuild_interaction_sources(&graph, &browser_registry);
        }

        inputs.prepare_for_sample(sensors);
        self.route_latest_into(inputs);
        self.route_interaction_into(&state.event_bus, &graph, &browser_registry, inputs);
    }

    fn read_screen(
        &mut self,
        target: Option<&ScreenNativeExecutionTarget>,
        extent: PixelExtent,
    ) -> (
        ScreenPlanGeneration,
        Option<Arc<ScreenBranchPublication>>,
        Option<ScreenBranchDeliveryState>,
    ) {
        let target_id = target.map(ScreenNativeExecutionTarget::id);
        let (plan_generation, observed_lease) = self.reader.screen_observation(target, extent);
        let route_is_current = self.screen_publication_route.as_ref().is_some_and(|route| {
            route.plan_generation == plan_generation
                && route.target_id == target_id
                && route.extent == extent
        });
        if !route_is_current {
            self.screen_publication_route = Some(ScreenPublicationRoute {
                plan_generation,
                target_id,
                extent,
                lease: observed_lease,
            });
        }
        let observation = self
            .screen_publication_route
            .as_ref()
            .and_then(|route| route.lease.as_ref())
            .map(|lease| lease.observe(Instant::now()));
        let (publication, delivery_state) = observation.unzip();
        let publication = publication.flatten();
        // Transition-only log: silent gaps between a healthy hub and a
        // black compositor have cost hours; state changes here name the
        // broken link (no lease = plan/observation mismatch, lease
        // without publication = starved or stale deliveries).
        let observed = (
            target_id.is_some(),
            self.screen_publication_route
                .as_ref()
                .is_some_and(|route| route.lease.is_some()),
            publication.is_some(),
        );
        if self.last_screen_observation != Some(observed) {
            self.last_screen_observation = Some(observed);
            tracing::debug!(
                target = observed.0,
                lease = observed.1,
                publication = observed.2,
                "screen publication observation changed"
            );
        }
        (plan_generation, publication, delivery_state)
    }

    fn screen_descriptor(&self) -> Option<&ResolvedScreenPublicationDescriptor> {
        self.screen_publication_route
            .as_ref()
            .and_then(|route| route.lease.as_ref())
            .map(ScreenBranchLease::descriptor)
    }

    fn route_interaction_into(
        &mut self,
        event_bus: &HypercolorBus,
        graph: &InputGraphSnapshot,
        browser_registry: &BrowserInputRegistrySnapshot,
        inputs: &mut FrameInputs,
    ) {
        self.refresh_interaction_availability_revisions();
        let routing = self.interaction_routing.snapshot();
        let request = InteractionRouteRequest {
            policy: routing.daemon_policy,
            browser_source: routing
                .authoritative_browser
                .as_ref()
                .map(|owner| SourceIncarnation::browser_child(owner.publication_id.get())),
        };
        self.interaction_router.resolve_into(
            AUTHORITATIVE_INPUT_CONSUMER,
            request,
            &self.interaction_sources,
            InteractionRouteContext {
                config_generation: routing.config_generation,
                source_graph_generation: graph.generation(),
                browser_registry_generation: browser_registry.generation(),
                now_ms: hypercolor_core::input::input_mono_ms(),
            },
            &mut self.routed_interaction,
        );
        std::mem::swap(
            &mut inputs.interaction,
            &mut self.routed_interaction.interaction,
        );
        inputs.input_availability = aggregate_interaction_availability(
            self.routed_interaction
                .diagnostics
                .selected
                .iter()
                .filter_map(|source| source.status.as_ref()),
            Instant::now(),
        );
        for event in &mut inputs.interaction.batch.events {
            event.seq = next_input_event_seq();
            event_bus.publish(HypercolorEvent::InputEventReceived {
                event: event.clone(),
            });
        }
    }

    fn refresh_interaction_availability(&mut self) -> InputSourceAvailability {
        let graph = self.reader.graph_snapshot();
        if self.graph_generation != Some(graph.generation()) {
            self.rebuild_routes(&graph);
        }
        self.interaction_availability()
    }

    fn interaction_availability(&self) -> InputSourceAvailability {
        let handles = self
            .routed_interaction
            .diagnostics
            .selected
            .iter()
            .filter_map(|source| source.status.as_ref());
        aggregate_interaction_availability(handles, Instant::now())
    }

    fn route_latest_into(&mut self, inputs: &mut FrameInputs) {
        for &route_index in &self.audio_routes {
            if let Some(sample) = self.routes[route_index].slot.latest()
                && let InputData::Audio(snapshot) = sample.as_ref()
            {
                inputs.audio.clone_from(snapshot);
                inputs.audio_was_published = true;
            }
        }
        let mut screen_seen = false;
        for &route_index in &self.screen_routes {
            if let Some(sample) = self.routes[route_index].slot.latest()
                && let InputData::Screen(snapshot) = sample.as_ref()
            {
                match &mut inputs.screen_data {
                    Some(current) => current.clone_from(snapshot),
                    None => inputs.screen_data = Some(snapshot.clone()),
                }
                screen_seen = true;
            }
        }
        for &route_index in &self.media_routes {
            if let Some(sample) = self.routes[route_index].slot.latest()
                && let InputData::Media(snapshot) = sample.as_ref()
            {
                inputs.media = Some(Arc::clone(snapshot));
            }
        }
        for &route_index in &self.network_routes {
            if let Some(sample) = self.routes[route_index].slot.latest()
                && let InputData::Net(snapshot) = sample.as_ref()
            {
                inputs.net = Some(Arc::clone(snapshot));
            }
        }
        for &route_index in &self.untyped_routes {
            let Some(sample) = self.routes[route_index].slot.latest() else {
                continue;
            };
            match sample.as_ref() {
                InputData::Audio(snapshot) => {
                    inputs.audio.clone_from(snapshot);
                    inputs.audio_was_published = true;
                }
                InputData::Interaction(_) => {}
                InputData::Media(snapshot) => inputs.media = Some(Arc::clone(snapshot)),
                InputData::Net(snapshot) => inputs.net = Some(Arc::clone(snapshot)),
                InputData::Screen(snapshot) => {
                    match &mut inputs.screen_data {
                        Some(current) => current.clone_from(snapshot),
                        None => inputs.screen_data = Some(snapshot.clone()),
                    }
                    screen_seen = true;
                }
                InputData::Sensors(snapshot) => inputs.sensors = Arc::clone(snapshot),
                InputData::None => {}
            }
        }
        if !screen_seen {
            inputs.screen_data = None;
        }
    }

    fn rebuild_routes(&mut self, graph: &InputGraphSnapshot) {
        self.routes.clear();
        self.routes
            .reserve(graph.slots().len().saturating_sub(self.routes.capacity()));
        self.clear_typed_routes();
        for slot in graph.slots() {
            let route_index = self.routes.len();
            self.routes.push(CachedInputRoute { slot: slot.clone() });
            match slot.kind() {
                Some(SourceKind::Audio) => self.audio_routes.push(route_index),
                Some(SourceKind::Screen) => self.screen_routes.push(route_index),
                Some(SourceKind::Interaction) => {}
                Some(SourceKind::Media) => self.media_routes.push(route_index),
                Some(SourceKind::Network) => self.network_routes.push(route_index),
                None => self.untyped_routes.push(route_index),
            }
        }
        self.graph_generation = Some(graph.generation());
    }

    fn rebuild_interaction_sources(
        &mut self,
        graph: &InputGraphSnapshot,
        browser_registry: &BrowserInputRegistrySnapshot,
    ) {
        let previous_availability = std::mem::take(&mut self.interaction_availability);
        self.interaction_sources.clear();
        self.interaction_statuses.clear();
        for slot in graph.slots() {
            let status = slot.status().clone();
            let descriptor = Arc::clone(&status.snapshot().source_id);
            if let Some(mut source) =
                InteractionRouteSource::manager_slot(descriptor, 1, slot.clone())
            {
                let availability =
                    cached_availability(source.incarnation, &status, &previous_availability);
                source.availability_revision = availability.revision;
                self.interaction_sources.push(source);
                self.interaction_statuses.push(status);
                self.interaction_availability.push(availability);
            }
        }
        for child in browser_registry.children() {
            let status = child.status().clone();
            let incarnation = SourceIncarnation::browser_child(child.publication_id().get());
            let availability = cached_availability(incarnation, &status, &previous_availability);
            self.interaction_sources.push(InteractionRouteSource::new(
                incarnation,
                Arc::<str>::from(child.source_id()),
                InteractionRouteSourceClass::Browser,
                availability.revision,
                Arc::new(child.clone()),
            ));
            self.interaction_statuses.push(status);
            self.interaction_availability.push(availability);
        }
        self.browser_registry_generation = Some(browser_registry.generation());
    }

    fn refresh_interaction_availability_revisions(&mut self) {
        for ((source, status), availability) in self
            .interaction_sources
            .iter_mut()
            .zip(&self.interaction_statuses)
            .zip(&mut self.interaction_availability)
        {
            let fingerprint = availability_fingerprint(status);
            if availability.fingerprint != fingerprint {
                availability.fingerprint = fingerprint;
                availability.revision = availability
                    .revision
                    .checked_add(1)
                    .expect("interaction availability revision exhausted");
            }
            source.availability_revision = availability.revision;
        }
    }

    fn clear_typed_routes(&mut self) {
        self.audio_routes.clear();
        self.screen_routes.clear();
        self.media_routes.clear();
        self.network_routes.clear();
        self.untyped_routes.clear();
    }
}

fn availability_fingerprint(status: &SourceStatusHandle) -> AvailabilityFingerprint {
    let status = status.snapshot();
    AvailabilityFingerprint {
        structural_graph_generation: status.source_graph_generation,
        session_generation: status.session_generation,
        effective: SourceStatusAvailability {
            kind: status.kind,
            configured: status.configured,
            consented: status.consented,
            demanded: status.demanded,
            state: status.state,
            freshness: status.freshness,
            retired: status.retired,
        },
    }
}

fn cached_availability(
    incarnation: SourceIncarnation,
    status: &SourceStatusHandle,
    previous: &[CachedInteractionAvailability],
) -> CachedInteractionAvailability {
    previous
        .iter()
        .find(|cached| cached.incarnation == incarnation)
        .copied()
        .unwrap_or(CachedInteractionAvailability {
            incarnation,
            fingerprint: availability_fingerprint(status),
            revision: 1,
        })
}

fn reset_audio_vector(values: &mut Vec<f32>, len: usize) {
    values.resize(len, 0.0);
    values.fill(0.0);
}

fn aggregate_interaction_availability<'a>(
    handles: impl IntoIterator<Item = &'a SourceStatusHandle>,
    now: Instant,
) -> InputSourceAvailability {
    let mut availability = InputSourceAvailability::default();
    for handle in handles {
        let source = handle.availability_at(now);
        if source.retired
            || source.kind != SourceKind::Interaction
            || !(source.configured && source.consented && source.demanded)
        {
            continue;
        }

        availability.routed = true;
        let healthy = matches!(source.state, SourceState::Live | SourceState::Degraded);
        availability.healthy |= healthy;
        availability.degraded |= source.state == SourceState::Degraded;
        availability.fresh |= healthy
            && matches!(
                source.freshness,
                SourceFreshness::Fresh | SourceFreshness::NotApplicable
            );
    }
    availability
}

impl FrameInputs {
    fn prepare_for_sample(&mut self, sensors: Option<Arc<SystemSnapshot>>) {
        reset_audio_vector(&mut self.audio.spectrum, SPECTRUM_BINS);
        reset_audio_vector(&mut self.audio.mel_bands, MEL_BANDS);
        reset_audio_vector(&mut self.audio.chromagram, CHROMA_BINS);
        self.audio.beat_detected = false;
        self.audio.beat_confidence = 0.0;
        self.audio.beat_phase = 0.0;
        self.audio.beat_pulse = 0.0;
        self.audio.bpm = 0.0;
        self.audio.rms_level = 0.0;
        self.audio.peak_level = 0.0;
        self.audio.spectral_centroid = 0.0;
        self.audio.spectral_flux = 0.0;
        self.audio.onset_detected = false;
        self.audio.onset_pulse = 0.0;
        self.audio_was_published = false;
        self.interaction.keyboard.pressed_keys.clear();
        self.interaction.keyboard.recent_keys.clear();
        self.interaction.mouse.x = 0;
        self.interaction.mouse.y = 0;
        self.interaction.mouse.buttons.clear();
        self.interaction.mouse.down = false;
        self.interaction.mouse.norm_x = 0.0;
        self.interaction.mouse.norm_y = 0.0;
        self.interaction.mouse.mode = PointerMode::None;
        self.interaction.mouse.injected = false;
        self.clear_transient_interaction();
        self.sensors = sensors.unwrap_or_else(|| Arc::clone(&self.empty_sensors));
        self.media = None;
        self.net = None;
        self.lighting = None;
        self.screen_surface = None;
        self.screen_sector_grid.clear();
    }

    fn clear_transient_interaction(&mut self) {
        self.interaction.batch.events.clear();
        self.interaction.batch.wheel_hi_res = 0;
        self.interaction.batch.motion = MotionAggregate::default();
        self.interaction.batch.window_secs = 0.0;
        self.interaction.batch.dropped_events = 0;
    }

    pub(crate) fn silence() -> Self {
        let empty_sensors = Arc::new(SystemSnapshot::empty());
        Self {
            audio: AudioData::silence(),
            audio_was_published: false,
            interaction: InteractionData::default(),
            screen_data: None,
            screen_publication: None,
            screen_delivery_state: None,
            screen_invalidation_epoch: 0,
            screen_compositor_epoch: 0,
            screen_descriptor: None,
            sensors: Arc::clone(&empty_sensors),
            input_availability: InputSourceAvailability::default(),
            empty_sensors,
            media: None,
            net: None,
            lighting: None,
            screen_surface: None,
            screen_sector_grid: Vec::new(),
            screen_surface_pool: Some(Self::new_screen_surface_pool()),
        }
    }

    pub(crate) fn screen_surface_for_frame(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<PublishedSurface>> {
        if self.screen_surface.is_none() {
            let surface_pool = self
                .screen_surface_pool
                .get_or_insert_with(Self::new_screen_surface_pool);
            self.screen_surface = match self.screen_data.as_ref() {
                Some(data) => screen_data_to_surface(
                    data,
                    width,
                    height,
                    &mut self.screen_sector_grid,
                    surface_pool,
                )?,
                None => None,
            };
        }

        Ok(self.screen_surface.clone())
    }

    fn new_screen_surface_pool() -> RenderSurfacePool {
        RenderSurfacePool::with_slot_count(
            SurfaceDescriptor::rgba8888(
                DEFAULT_SCREEN_SURFACE_WIDTH,
                DEFAULT_SCREEN_SURFACE_HEIGHT,
            ),
            2,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StaticSurfaceKey {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) color: [u8; 3],
}

#[derive(Clone)]
pub(crate) struct CachedStaticSurface {
    pub(crate) key: StaticSurfaceKey,
    pub(crate) surface: PublishedSurface,
}

pub(crate) struct OutputArtifactsState {
    static_surface_cache: Vec<CachedStaticSurface>,
    recycled_frame: FrameData,
    pub(crate) unassigned_output_cache: UnassignedOutputCache,
}

impl Default for OutputArtifactsState {
    fn default() -> Self {
        Self {
            static_surface_cache: Vec::new(),
            recycled_frame: FrameData::empty(),
            unassigned_output_cache: UnassignedOutputCache::default(),
        }
    }
}

impl OutputArtifactsState {
    pub(crate) fn static_surface(
        &mut self,
        width: u32,
        height: u32,
        color: [u8; 3],
    ) -> PublishedSurface {
        let key = StaticSurfaceKey {
            width,
            height,
            color,
        };

        if let Some(cached) = self
            .static_surface_cache
            .iter()
            .find(|cached| cached.key == key)
        {
            return cached.surface.clone();
        }

        let mut canvas = Canvas::new(width, height);
        if color != [0, 0, 0] {
            canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
        }

        let surface = PublishedSurface::from_owned_canvas(canvas, 0, 0);
        self.static_surface_cache.push(CachedStaticSurface {
            key,
            surface: surface.clone(),
        });
        surface
    }

    fn prepare_static_surfaces(
        width: u32,
        height: u32,
        colors: &[[u8; 3]],
    ) -> Result<Vec<CachedStaticSurface>, SurfaceResourceError> {
        let mut prepared = Vec::with_capacity(colors.len());
        for color in colors.iter().copied() {
            let key = StaticSurfaceKey {
                width,
                height,
                color,
            };
            if prepared
                .iter()
                .any(|cached: &CachedStaticSurface| cached.key == key)
            {
                continue;
            }
            let mut canvas = Canvas::try_new(width, height)?;
            if color != [0, 0, 0] {
                canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
            }
            prepared.push(CachedStaticSurface {
                key,
                surface: PublishedSurface::from_owned_canvas(canvas, 0, 0),
            });
        }
        Ok(prepared)
    }

    pub(crate) fn reset_for_canvas_resize(&mut self, static_surfaces: Vec<CachedStaticSurface>) {
        self.static_surface_cache = static_surfaces;
        self.unassigned_output_cache.clear();
    }

    pub(crate) fn clear_zones(&mut self) {
        self.recycled_frame.zones.clear();
    }

    pub(crate) fn zones(&self) -> &[ZoneColors] {
        &self.recycled_frame.zones
    }

    pub(crate) fn zones_mut(&mut self) -> &mut Vec<ZoneColors> {
        &mut self.recycled_frame.zones
    }

    pub(crate) fn frame_mut(&mut self) -> &mut FrameData {
        &mut self.recycled_frame
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnassignedOutputCacheKey {
    layout_ptr: usize,
    mapping_generation: u64,
}

impl UnassignedOutputCacheKey {
    #[expect(
        clippy::as_conversions,
        reason = "Arc pointer identity is the cheapest stable key for the active layout"
    )]
    pub(crate) fn new(layout: &Arc<SpatialLayout>, mapping_generation: u64) -> Self {
        Self {
            layout_ptr: Arc::as_ptr(layout) as usize,
            mapping_generation,
        }
    }
}

#[derive(Clone)]
pub(crate) struct CachedUnassignedOutput {
    pub(crate) source_layout: Arc<SpatialLayout>,
    pub(crate) zones: Arc<[Output]>,
    pub(crate) black_zones: Arc<[ZoneColors]>,
}

pub(crate) struct CachedUnassignedOutputEntry {
    key: UnassignedOutputCacheKey,
    value: CachedUnassignedOutput,
}

#[derive(Default)]
pub(crate) struct UnassignedOutputCache {
    entry: Option<CachedUnassignedOutputEntry>,
    fallback_spatial_engine: Option<CachedFallbackSpatialEngine>,
}

struct CachedFallbackSpatialEngine {
    layout: SpatialLayout,
    engine: SpatialEngine,
}

impl UnassignedOutputCache {
    pub(crate) fn get(&self, key: UnassignedOutputCacheKey) -> Option<CachedUnassignedOutput> {
        self.entry
            .as_ref()
            .filter(|entry| entry.key == key)
            .map(|entry| entry.value.clone())
    }

    pub(crate) fn store(
        &mut self,
        key: UnassignedOutputCacheKey,
        value: CachedUnassignedOutput,
    ) -> CachedUnassignedOutput {
        self.fallback_spatial_engine = None;
        self.entry = Some(CachedUnassignedOutputEntry {
            key,
            value: value.clone(),
        });
        value
    }

    pub(crate) fn clear(&mut self) {
        self.entry = None;
        self.fallback_spatial_engine = None;
    }

    pub(crate) fn fallback_spatial_engine(&self, layout: &SpatialLayout) -> Option<SpatialEngine> {
        self.fallback_spatial_engine
            .as_ref()
            .filter(|cached| &cached.layout == layout)
            .map(|cached| cached.engine.clone())
    }

    pub(crate) fn store_fallback_spatial_engine(
        &mut self,
        layout: SpatialLayout,
        engine: SpatialEngine,
    ) {
        self.fallback_spatial_engine = Some(CachedFallbackSpatialEngine { layout, engine });
    }
}

pub(crate) enum PendingZoneSamplingStatus {
    Completed(PendingZoneSampling),
    Stale(PendingZoneSampling),
}

#[derive(Default)]
pub(crate) struct DeferredSamplingState {
    pending: Option<PendingZoneSampling>,
    retired: VecDeque<PendingZoneSampling>,
    scratch: Vec<ZoneColors>,
}

impl DeferredSamplingState {
    pub(crate) fn scratch(&self) -> &[ZoneColors] {
        &self.scratch
    }

    pub(crate) fn scratch_mut(&mut self) -> &mut Vec<ZoneColors> {
        &mut self.scratch
    }

    pub(crate) fn clone_scratch_into(&self, target: &mut Vec<ZoneColors>) {
        target.clone_from(&self.scratch);
    }

    pub(crate) fn store_pending(&mut self, pending: PendingZoneSampling) {
        self.pending = Some(pending);
    }

    pub(crate) fn take_pending_status(
        &mut self,
        sparkleflinger: &mut SparkleFlinger,
        error_message: &'static str,
    ) -> Option<PendingZoneSamplingStatus> {
        let mut pending = self.pending.take()?;
        match sparkleflinger.try_finish_pending_zone_sampling(&mut pending, &mut self.scratch) {
            Ok(true) => Some(PendingZoneSamplingStatus::Completed(pending)),
            Ok(false) => Some(PendingZoneSamplingStatus::Stale(pending)),
            Err(error) => {
                tracing::warn!(%error, "{error_message}");
                None
            }
        }
    }

    pub(crate) fn finish_retired(
        &mut self,
        sparkleflinger: &mut SparkleFlinger,
        error_message: &'static str,
    ) {
        let retired_count = self.retired.len();
        for _ in 0..retired_count {
            let Some(mut retired_sampling) = self.retired.pop_front() else {
                break;
            };
            match sparkleflinger
                .try_finish_pending_zone_sampling(&mut retired_sampling, &mut self.scratch)
            {
                Ok(true) => {}
                Ok(false) => {
                    self.retired.push_back(retired_sampling);
                }
                Err(error) => {
                    tracing::warn!(%error, "{error_message}");
                }
            }
        }
    }

    pub(crate) fn retire_or_return(
        &mut self,
        sparkleflinger: &mut SparkleFlinger,
        pending: PendingZoneSampling,
    ) -> Option<PendingZoneSampling> {
        self.finish_retired(
            sparkleflinger,
            "Retired GPU spatial sampling cleanup failed; dropping stale deferred sample result",
        );

        let retired_capacity = sparkleflinger.max_pending_zone_sampling().saturating_sub(1);
        if self.retired.len() >= retired_capacity {
            return Some(pending);
        }

        self.retired.push_back(pending);
        None
    }

    pub(crate) fn discard_backlog(&mut self, sparkleflinger: &mut SparkleFlinger) {
        if let Some(pending) = self.pending.take() {
            sparkleflinger.discard_pending_zone_sampling(pending);
        }
        while let Some(pending) = self.retired.pop_front() {
            sparkleflinger.discard_pending_zone_sampling(pending);
        }
    }

    pub(crate) fn clear_for_canvas_resize(&mut self, sparkleflinger: &mut SparkleFlinger) {
        self.discard_backlog(sparkleflinger);
        self.scratch.clear();
    }
}

#[derive(Debug, Default)]
#[allow(clippy::struct_field_names)]
pub(crate) struct PublicationCadenceState {
    pub(crate) last_audio_level_update_ms: Option<u64>,
    audio_publication_observed: bool,
    pub(crate) last_canvas_preview_publish_ms: Option<u64>,
    pub(crate) last_scene_canvas_publish_ms: Option<u64>,
    pub(crate) last_screen_canvas_preview_publish_ms: Option<u64>,
    pub(crate) last_web_viewport_preview_publish_ms: Option<u64>,
    pub(crate) last_zone_preview_publish_ms: Option<u64>,
}

impl PublicationCadenceState {
    pub(crate) fn observe_audio_publication(&mut self, published: bool) {
        self.audio_publication_observed |= published;
    }

    pub(crate) const fn audio_publication_observed(&self) -> bool {
        self.audio_publication_observed
    }

    pub(crate) fn should_publish_audio_level(
        &self,
        elapsed_ms: u64,
        has_event_subscribers: bool,
    ) -> bool {
        has_event_subscribers
            && !self.last_audio_level_update_ms.is_some_and(|last_sent| {
                elapsed_ms.saturating_sub(last_sent) < AUDIO_LEVEL_EVENT_INTERVAL_MS
            })
    }

    pub(crate) fn record_audio_level_update(&mut self, elapsed_ms: u64) {
        self.last_audio_level_update_ms = Some(elapsed_ms);
    }

    pub(crate) fn canvas_preview_due(
        &self,
        elapsed_ms: u64,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_canvas_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_canvas_publication(&mut self, elapsed_ms: u64) {
        self.last_canvas_preview_publish_ms = Some(elapsed_ms);
    }

    pub(crate) fn scene_canvas_due(
        &self,
        elapsed_ms: u64,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_scene_canvas_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_scene_canvas_publication(&mut self, elapsed_ms: u64) {
        self.last_scene_canvas_publish_ms = Some(elapsed_ms);
    }

    pub(crate) fn screen_canvas_preview_due(
        &self,
        elapsed_ms: u64,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_screen_canvas_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_screen_canvas_publication(&mut self, elapsed_ms: u64) {
        self.last_screen_canvas_preview_publish_ms = Some(elapsed_ms);
    }

    pub(crate) fn web_viewport_preview_due(
        &self,
        elapsed_ms: u64,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_web_viewport_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_web_viewport_publication(&mut self, elapsed_ms: u64) {
        self.last_web_viewport_preview_publish_ms = Some(elapsed_ms);
    }

    pub(crate) fn zone_preview_due(
        &self,
        elapsed_ms: u64,
        total_receivers: usize,
        tracked_receivers: usize,
        tracked_max_fps: u32,
    ) -> bool {
        preview_publication_due(
            elapsed_ms,
            self.last_zone_preview_publish_ms,
            total_receivers,
            tracked_receivers,
            tracked_max_fps,
        )
    }

    pub(crate) fn record_zone_preview_publication(&mut self, elapsed_ms: u64) {
        self.last_zone_preview_publish_ms = Some(elapsed_ms);
    }
}

#[derive(Debug, Default)]
pub(crate) struct ThrottleState {
    pub(crate) idle_black_pushed: bool,
    pub(crate) sleep_black_pushed: bool,
}

impl ThrottleState {
    pub(crate) fn reset_for_canvas_resize(&mut self) {
        self.idle_black_pushed = false;
        self.sleep_black_pushed = false;
    }

    pub(crate) fn note_effect_running(&mut self) {
        self.idle_black_pushed = false;
    }

    pub(crate) fn idle_black_pushed(&self) -> bool {
        self.idle_black_pushed
    }

    pub(crate) fn note_idle_frame_without_effect(&mut self) {
        self.idle_black_pushed = true;
    }

    pub(crate) fn clear_sleep(&mut self) {
        self.sleep_black_pushed = false;
    }

    pub(crate) fn sleep_black_pushed(&self) -> bool {
        self.sleep_black_pushed
    }

    pub(crate) fn note_sleep_frame_published(&mut self) {
        self.sleep_black_pushed = true;
    }
}

#[derive(Debug, Default)]
pub(crate) struct OutputReuseState {
    pub(crate) last_key: Option<OutputReuseKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputReuseKey {
    pub(crate) output_brightness_bits: u32,
    pub(crate) device_output_brightness_generation: u64,
    pub(crate) routing_signature: u64,
    pub(crate) zone_shape_signature: u64,
    pub(crate) unassigned_behavior_generation: u64,
}

impl OutputReuseKey {
    pub(crate) const fn new(
        output_brightness_bits: u32,
        device_output_brightness_generation: u64,
        routing_signature: u64,
        zone_shape_signature: u64,
        unassigned_behavior_generation: u64,
    ) -> Self {
        Self {
            output_brightness_bits,
            device_output_brightness_generation,
            routing_signature,
            zone_shape_signature,
            unassigned_behavior_generation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFrameSource {
    RoutedReuse,
    PublishedFrame,
    CurrentFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputReuseDecision {
    key: OutputReuseKey,
    source: OutputFrameSource,
}

impl OutputReuseDecision {
    pub(crate) const fn source(self) -> OutputFrameSource {
        self.source
    }

    pub(crate) const fn key(self) -> OutputReuseKey {
        self.key
    }
}

impl OutputReuseState {
    pub(crate) fn matches(&self, key: OutputReuseKey) -> bool {
        self.last_key == Some(key)
    }

    pub(crate) fn decide_frame_source(
        &self,
        reuses_published_frame: bool,
        key: OutputReuseKey,
        routed_outputs_reusable: impl FnOnce() -> bool,
    ) -> OutputReuseDecision {
        let source = if reuses_published_frame && self.matches(key) && routed_outputs_reusable() {
            OutputFrameSource::RoutedReuse
        } else if reuses_published_frame {
            OutputFrameSource::PublishedFrame
        } else {
            OutputFrameSource::CurrentFrame
        };

        OutputReuseDecision { key, source }
    }

    pub(crate) fn record_decision(&mut self, decision: OutputReuseDecision) {
        self.record(decision.key);
    }

    fn record(&mut self, key: OutputReuseKey) {
        self.last_key = Some(key);
    }
}

fn preview_publish_fps_limit(
    total_receivers: usize,
    tracked_receivers: usize,
    tracked_max_fps: u32,
) -> Option<u32> {
    (total_receivers > 0 && total_receivers == tracked_receivers).then_some(tracked_max_fps.max(1))
}

fn should_publish_preview_frame(
    elapsed_ms: u64,
    last_publish_ms: Option<u64>,
    target_fps: Option<u32>,
) -> bool {
    let Some(target_fps) = target_fps else {
        return true;
    };
    let interval_ms = 1000_u64.div_ceil(u64::from(target_fps.max(1)));
    last_publish_ms.is_none_or(|last_sent| elapsed_ms.saturating_sub(last_sent) >= interval_ms)
}

fn preview_publication_due(
    elapsed_ms: u64,
    last_publish_ms: Option<u64>,
    total_receivers: usize,
    tracked_receivers: usize,
    tracked_max_fps: u32,
) -> bool {
    if total_receivers == 0 {
        return false;
    }

    should_publish_preview_frame(
        elapsed_ms,
        last_publish_ms,
        preview_publish_fps_limit(total_receivers, tracked_receivers, tracked_max_fps),
    )
}

pub(crate) struct FrameLoopState {
    pub(crate) clock: FrameClockState,
    pub(crate) inputs: InputReuseState,
    pub(crate) throttle: ThrottleState,
    pub(crate) publication_cadence: PublicationCadenceState,
    pub(crate) output_reuse: OutputReuseState,
    pub(crate) lighting_feed: super::lighting_feed::LightingFeedState,
    pub(crate) input_demands: InputPublicationDemandHandle,
    pub(crate) authoritative_input_demand: OwnedInputPublicationDemand,
}

impl FrameLoopState {
    pub(crate) fn publish_input_demands(
        &mut self,
        effect_demand: EffectDemand,
        requested_hz: u32,
        screen_extent: PixelExtent,
        screen_target: Option<&ScreenNativeExecutionTarget>,
    ) {
        let authoritative =
            authoritative_input_demand(effect_demand, requested_hz, screen_extent, screen_target);
        self.authoritative_input_demand.publish(authoritative);
    }

    pub(crate) fn clear_input_demands(&mut self) {
        self.authoritative_input_demand.clear();
    }

    pub(crate) fn has_active_input_demand(&self) -> bool {
        self.input_demands.is_active()
    }

    pub(crate) fn has_screen_input_demand(&self) -> bool {
        self.input_demands.requested_hz(SourceKind::Screen) > 0
    }
}

fn authoritative_input_demand(
    effect_demand: EffectDemand,
    requested_hz: u32,
    screen_extent: PixelExtent,
    screen_target: Option<&ScreenNativeExecutionTarget>,
) -> InputPublicationDemand {
    let mut demand = InputPublicationDemand::default();
    if effect_demand.audio_capture_active {
        demand = demand.with_source(SourceKind::Audio, requested_hz);
    }
    if effect_demand.screen_capture_active {
        demand = demand.with_screen_executor(
            requested_hz,
            screen_extent,
            screen_target.map_or(ScreenPublicationExecutorRequest::Cpu, |target| {
                ScreenPublicationExecutorRequest::SourceNative(target.clone())
            }),
        );
    }
    if effect_demand.interaction_capture_active {
        demand = demand.with_source(SourceKind::Interaction, requested_hz);
    }
    if effect_demand.media_input_active {
        demand = demand.with_source(SourceKind::Media, BACKGROUND_INPUT_HZ);
    }
    if effect_demand.network_input_active {
        demand = demand.with_source(SourceKind::Network, BACKGROUND_INPUT_HZ);
    }
    demand
}

pub(crate) struct RenderCaches {
    pub(crate) screen_queue: ProducerQueue,
    pub(crate) composition_planner: CompositionPlanner,
    pub(crate) sparkleflinger: SparkleFlinger,
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    pub(crate) macos_screen_parity_mailbox: MacosScreenParityDiagnosticMailbox,
    #[cfg(feature = "wgpu")]
    pub(crate) display_sparkleflinger: SparkleFlinger,
    #[cfg(feature = "wgpu")]
    pub(crate) display_finalize_runtime: DisplayFinalizeRuntime,
    pub(crate) deferred_sampling: DeferredSamplingState,
    pub(crate) zone_transition_planner: ZoneTransitionPlanner,
    pub(crate) render_group_runtime: ZoneRuntime,
    pub(crate) output_artifacts: OutputArtifactsState,
    pub(crate) pending_layout_activation: Option<PreparedLayoutActivation>,
    effect_delta_clock: EffectDeltaClock,
}

pub(crate) struct PreparedCanvasResize {
    render_group_runtime: super::render_groups::PreparedSceneResize,
    static_surfaces: Vec<CachedStaticSurface>,
    sparkleflinger_preparation: SparkleFlingerCanvasPreparation,
    sampling_preparation: SparkleFlingerSamplingPreparation,
    #[cfg(feature = "wgpu")]
    display_sparkleflinger_preparation: SparkleFlingerCanvasPreparation,
}

impl PreparedCanvasResize {
    #[cfg(feature = "wgpu")]
    pub(crate) const fn render_gpu_output_admitted(&self) -> bool {
        self.sparkleflinger_preparation.gpu_output_admitted()
    }

    pub(crate) const fn render_preparation(&self) -> &SparkleFlingerCanvasPreparation {
        &self.sparkleflinger_preparation
    }

    pub(crate) fn prepare_scene_cpu_backing(&mut self) -> Result<()> {
        self.render_group_runtime.prepare_cpu_backing()?;
        Ok(())
    }
}

pub(crate) struct PreparedLayoutActivation {
    pub(crate) spatial_engine: SpatialEngine,
    pub(crate) expected_layout: SpatialLayout,
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) source_active_render_groups_revision: u64,
    pub(crate) prepared_resize: Option<PreparedCanvasResize>,
    pub(crate) sampling_preparation: Option<SparkleFlingerSamplingPreparation>,
    pub(crate) prepared_groups: PreparedZoneReconcile,
    pub(crate) prepared_projected_scene:
        super::sparkleflinger::SparkleFlingerProjectedScenePreparation,
    pub(crate) activation: LayoutActivationControl,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl Drop for RenderCaches {
    fn drop(&mut self) {
        if let Some(prepared) = self.pending_layout_activation.take() {
            prepared
                .activation
                .complete(Err(LayoutTransactionRejection::RendererStopped));
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EffectDeltaClock {
    retained_delta: Duration,
    dependency_key: Option<SceneDependencyKey>,
}

impl EffectDeltaClock {
    fn record_retained_frame(&mut self, dependency_key: SceneDependencyKey, frame_delta: Duration) {
        if self.dependency_key != Some(dependency_key) {
            self.retained_delta = Duration::ZERO;
            self.dependency_key = Some(dependency_key);
        }
        self.retained_delta = self.retained_delta.saturating_add(frame_delta);
    }

    fn take_render_delta(
        &mut self,
        dependency_key: SceneDependencyKey,
        frame_delta: Duration,
    ) -> f32 {
        let retained_delta = if self.dependency_key == Some(dependency_key) {
            self.retained_delta
        } else {
            Duration::ZERO
        };
        self.clear();
        let render_delta = retained_delta.saturating_add(frame_delta);
        render_delta.as_secs_f32()
    }

    fn clear(&mut self) {
        self.retained_delta = Duration::ZERO;
        self.dependency_key = None;
    }
}

pub(crate) struct ComposeRuntime<'a> {
    pub(crate) screen_queue: &'a mut ProducerQueue,
    pub(crate) composition_planner: &'a mut CompositionPlanner,
    pub(crate) sparkleflinger: &'a mut SparkleFlinger,
    #[cfg(feature = "wgpu")]
    pub(crate) display_sparkleflinger: &'a mut SparkleFlinger,
    #[cfg(feature = "wgpu")]
    pub(crate) display_finalize_runtime: &'a mut DisplayFinalizeRuntime,
    pub(crate) render_group_runtime: &'a mut ZoneRuntime,
    pub(crate) output_artifacts: &'a mut OutputArtifactsState,
    pub(crate) effect_delta_clock: &'a mut EffectDeltaClock,
}

#[cfg(feature = "wgpu")]
pub(crate) struct PendingDisplayFinalizeWork {
    pub(crate) dependency_key: SceneDependencyKey,
    pub(crate) display_target: DisplayFaceTarget,
    pub(crate) display_route: DisplayGroupOutputRoute,
    pub(crate) frame_format: DisplayFrameFormat,
    pub(crate) pending: PendingDisplayFinalization,
}

#[cfg(feature = "wgpu")]
impl PendingDisplayFinalizeWork {
    pub(crate) fn matches(
        &self,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        frame_format: DisplayFrameFormat,
    ) -> bool {
        self.dependency_key == dependency_key
            && self.display_target == *display_target
            && self.display_route == *display_route
            && self.frame_format == frame_format
    }
}

#[cfg(feature = "wgpu")]
#[derive(Default)]
pub(crate) struct DisplayFinalizeRuntime {
    pending: HashMap<ZoneId, PendingDisplayFinalizeWork>,
}

#[cfg(feature = "wgpu")]
impl DisplayFinalizeRuntime {
    pub(crate) fn take(&mut self, group_id: ZoneId) -> Option<PendingDisplayFinalizeWork> {
        self.pending.remove(&group_id)
    }

    pub(crate) fn insert(&mut self, group_id: ZoneId, work: PendingDisplayFinalizeWork) {
        self.pending.insert(group_id, work);
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = PendingDisplayFinalizeWork> + '_ {
        self.pending.drain().map(|(_, pending)| pending)
    }

    pub(crate) fn take_inactive(
        &mut self,
        active_group_ids: &[ZoneId],
    ) -> Vec<PendingDisplayFinalizeWork> {
        let inactive_group_ids = self
            .pending
            .keys()
            .copied()
            .filter(|group_id| !active_group_ids.iter().any(|active| active == group_id))
            .collect::<Vec<_>>();

        inactive_group_ids
            .into_iter()
            .filter_map(|group_id| self.pending.remove(&group_id))
            .collect()
    }
}

impl ComposeRuntime<'_> {
    pub(crate) fn clear_inactive_groups(&mut self) {
        #[cfg(feature = "wgpu")]
        for pending in self.display_finalize_runtime.drain() {
            self.display_sparkleflinger
                .discard_pending_display_finalization(pending.pending);
        }
        #[cfg(feature = "wgpu")]
        self.display_sparkleflinger
            .retain_display_finalize_groups(&[]);
        self.render_group_runtime.clear_inactive_groups();
        self.effect_delta_clock.clear();
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn discard_display_finalizations_except(&mut self, active_group_ids: &[ZoneId]) {
        for pending in self
            .display_finalize_runtime
            .take_inactive(active_group_ids)
        {
            self.display_sparkleflinger
                .discard_pending_display_finalization(pending.pending);
        }
        self.display_sparkleflinger
            .retain_display_finalize_groups(active_group_ids);
    }

    pub(crate) fn reuse_or_render_scene(
        &mut self,
        scene_snapshot: &FrameSceneSnapshot,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        skip_decision: SkipDecision,
        frame_delta: Duration,
        inputs: &FrameInputs,
    ) -> (Result<ZoneResult>, bool) {
        if skip_decision == SkipDecision::ReuseCanvas
            && let Some(retained) = self.render_group_runtime.reuse_scene(dependency_key)
        {
            self.effect_delta_clock
                .record_retained_frame(dependency_key, frame_delta);
            return (Ok(retained), true);
        }

        let delta_secs = self
            .effect_delta_clock
            .take_render_delta(dependency_key, frame_delta);

        if let Err(error) = self.render_group_runtime.admit_reconcile(
            scene_snapshot.scene_runtime.active_render_groups.as_ref(),
            scene_snapshot.scene_runtime.active_scene_id,
            dependency_key,
            registry,
            &scene_snapshot
                .scene_runtime
                .active_display_group_descriptors,
            Some(&scene_snapshot.spatial_engine),
            self.sparkleflinger,
        ) {
            return (Err(error), false);
        }

        let zones = self.output_artifacts.zones_mut();
        let context = RenderSceneContext {
            groups: scene_snapshot.scene_runtime.active_render_groups.as_ref(),
            active_scene_id: scene_snapshot.scene_runtime.active_scene_id,
            dependency_key,
            elapsed_ms: scene_snapshot.elapsed_ms,
            display_group_target_fps: &scene_snapshot.scene_runtime.active_display_group_target_fps,
            display_group_descriptors: &scene_snapshot
                .scene_runtime
                .active_display_group_descriptors,
            registry,
            authoritative_spatial_engine: Some(&scene_snapshot.spatial_engine),
            inputs: ZoneFrameInputs {
                delta_secs,
                audio: &inputs.audio,
                interaction: &inputs.interaction,
                input_availability: inputs.input_availability,
                screen: inputs.screen_data.as_ref(),
                sensors: inputs.sensors.as_ref(),
                media: inputs.media.as_deref(),
                net: inputs.net.as_deref(),
                lighting: inputs.lighting.as_deref(),
            },
        };
        (
            self.render_group_runtime
                .render_scene(context, self.sparkleflinger, zones),
            false,
        )
    }
}

pub(crate) struct SamplingRuntime<'a> {
    pub(crate) sparkleflinger: &'a mut SparkleFlinger,
    pub(crate) deferred_sampling: &'a mut DeferredSamplingState,
    pub(crate) zone_transition_planner: &'a mut ZoneTransitionPlanner,
    pub(crate) output_artifacts: &'a mut OutputArtifactsState,
}

#[derive(Default)]
pub(crate) struct PendingSamplingWork {
    pub(crate) completed: Option<PendingZoneSampling>,
    pub(crate) stale: Option<PendingZoneSampling>,
}

impl SamplingRuntime<'_> {
    pub(crate) fn take_pending_status(
        &mut self,
        error_message: &'static str,
    ) -> Option<PendingZoneSamplingStatus> {
        self.deferred_sampling
            .take_pending_status(self.sparkleflinger, error_message)
    }

    pub(crate) fn finish_retired(&mut self, error_message: &'static str) {
        self.deferred_sampling
            .finish_retired(self.sparkleflinger, error_message);
    }

    pub(crate) fn prepare_pending_work(
        &mut self,
        retired_error_message: &'static str,
        pending_error_message: &'static str,
    ) -> PendingSamplingWork {
        self.finish_retired(retired_error_message);

        let mut work = PendingSamplingWork::default();
        if let Some(status) = self.take_pending_status(pending_error_message) {
            match status {
                PendingZoneSamplingStatus::Completed(deferred_sampling) => {
                    work.completed = Some(deferred_sampling);
                }
                PendingZoneSamplingStatus::Stale(deferred_sampling) => {
                    work.stale = Some(deferred_sampling);
                }
            }
        }

        work
    }

    pub(crate) fn retire_or_return(
        &mut self,
        pending: PendingZoneSampling,
    ) -> Option<PendingZoneSampling> {
        self.deferred_sampling
            .retire_or_return(self.sparkleflinger, pending)
    }

    pub(crate) fn discard_backlog(&mut self) {
        self.deferred_sampling.discard_backlog(self.sparkleflinger);
    }
}

pub(crate) struct PreviewRuntime<'a> {
    sparkleflinger: &'a mut SparkleFlinger,
}

impl PreviewRuntime<'_> {
    pub(crate) fn advance_gpu_preview(&mut self, render_stage: &mut RenderStageStats) {
        if !needs_gpu_preview_advance(render_stage) {
            return;
        }

        if let Err(error) = self.sparkleflinger.submit_pending_preview_work() {
            warn!(
                %error,
                "GPU preview submit failed; continuing without an overlapped preview finalize"
            );
        }

        self.resolve_gpu_preview(
            render_stage,
            "GPU preview early finalize failed; continuing without an early preview surface",
        );
    }

    pub(crate) fn finalize_gpu_preview(&mut self, render_stage: &mut RenderStageStats) {
        self.resolve_gpu_preview(
            render_stage,
            "GPU preview finalize failed; continuing without a preview surface",
        );
    }

    fn resolve_gpu_preview(
        &mut self,
        render_stage: &mut RenderStageStats,
        error_message: &'static str,
    ) {
        if !needs_gpu_preview_advance(render_stage)
            || render_stage.composed_frame.preview_surface.is_some()
        {
            return;
        }

        match self.sparkleflinger.resolve_preview_surface() {
            Ok(Some(preview_surface)) => {
                render_stage.composed_frame.preview_surface = Some(preview_surface);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(%error, "{error_message}");
            }
        }
    }
}

pub(crate) fn needs_gpu_preview_advance(render_stage: &RenderStageStats) -> bool {
    render_stage.preview_requested
        && render_stage.composed_frame.preview_surface.is_none()
        && matches!(
            render_stage.composed_frame.backend,
            crate::performance::CompositorBackendKind::Gpu
        )
}

pub(crate) struct SceneSnapshotState {
    pub(crate) snapshot_cache: SceneSnapshotCache,
    pub(crate) render_state: RenderSceneState,
}

impl SceneSnapshotState {
    pub(crate) fn new(
        initial_spatial_engine: SpatialEngine,
        screen_capture_configured: bool,
    ) -> Self {
        Self {
            snapshot_cache: SceneSnapshotCache::new(),
            render_state: RenderSceneState::new(initial_spatial_engine, screen_capture_configured),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RenderSurfaceSnapshot {
    pub(crate) slot_count: u32,
    pub(crate) free_slots: u32,
    pub(crate) published_slots: u32,
    pub(crate) dequeued_slots: u32,
    pub(crate) canvas_receivers: u32,
    /// Monotonic counter from the render-group runtime's scene surface pool:
    /// how many times a dequeue had to reuse a still-shared Published
    /// slot and allocate a fresh canvas. Only fires at the pool's cap.
    pub(crate) scene_pool_saturation_reallocs: u64,
    /// Same counter summed across per-group direct-canvas pools.
    pub(crate) direct_pool_saturation_reallocs: u64,
    /// Current slot count above the scene surface pool's initial size. Grows
    /// once per high-water mark, then settles.
    pub(crate) scene_pool_grown_slots: u32,
    /// Same gauge summed across per-group direct-canvas pools.
    pub(crate) direct_pool_grown_slots: u32,
    pub(crate) scene_pool_slot_count: u32,
    pub(crate) scene_pool_max_slots: u32,
    pub(crate) direct_pool_slot_count: u32,
    pub(crate) direct_pool_max_slots: u32,
    pub(crate) scene_pool_shared_published_slots: u32,
    pub(crate) scene_pool_max_ref_count: u32,
    pub(crate) direct_pool_shared_published_slots: u32,
    pub(crate) direct_pool_max_ref_count: u32,
    pub(crate) scene_pool_free_slots: u32,
    pub(crate) scene_pool_published_slots: u32,
    pub(crate) scene_pool_dequeued_slots: u32,
    pub(crate) direct_pool_free_slots: u32,
    pub(crate) direct_pool_published_slots: u32,
    pub(crate) direct_pool_dequeued_slots: u32,
    pub(crate) preview_pool_slot_count: u32,
    pub(crate) preview_pool_free_slots: u32,
    pub(crate) preview_pool_published_slots: u32,
    pub(crate) preview_pool_dequeued_slots: u32,
    pub(crate) compositor_pool_slot_count: u32,
    pub(crate) compositor_pool_free_slots: u32,
    pub(crate) compositor_pool_published_slots: u32,
    pub(crate) compositor_pool_dequeued_slots: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SceneTransitionKey {
    pub(crate) from_scene: SceneId,
    pub(crate) to_scene: SceneId,
}

pub(crate) struct RetainedZoneFrame {
    pub(crate) layout: Arc<SpatialLayout>,
    pub(crate) zones: Vec<ZoneColors>,
}

#[derive(Default)]
pub(crate) struct ZoneTransitionPlanner {
    pub(crate) active_transition: Option<SceneTransitionKey>,
    pub(crate) transition_base: Option<RetainedZoneFrame>,
    /// Layout of the most recent stable (non-transition) frame. Zone colors
    /// are deliberately not retained here: every stable frame's zones end up
    /// in (or already are) the published frame, so the transition base
    /// captures them lazily from the published frame when a transition starts
    /// — one zone deep copy per transition instead of one per stable frame.
    pub(crate) last_stable_layout: Option<Arc<SpatialLayout>>,
}

impl ZoneTransitionPlanner {
    pub(crate) fn clear(&mut self) {
        self.active_transition = None;
        self.transition_base = None;
    }

    pub(crate) fn record_stable(&mut self, layout: Arc<SpatialLayout>) {
        self.clear();
        self.last_stable_layout = Some(layout);
    }
}

impl RenderCaches {
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    pub(crate) fn service_macos_screen_parity(
        &mut self,
        render_device: &GpuRenderDevice,
        publication: Option<&Arc<ScreenBranchPublication>>,
        descriptor: Option<&ResolvedScreenPublicationDescriptor>,
        spatial_engine: &SpatialEngine,
    ) {
        self.macos_screen_parity_mailbox.service(
            render_device,
            &mut self.sparkleflinger,
            publication,
            descriptor,
            spatial_engine,
        );
    }

    pub(crate) fn clear_inactive_groups(&mut self) {
        #[cfg(feature = "wgpu")]
        for pending in self.display_finalize_runtime.drain() {
            self.display_sparkleflinger
                .discard_pending_display_finalization(pending.pending);
        }
        #[cfg(feature = "wgpu")]
        self.display_sparkleflinger
            .retain_display_finalize_groups(&[]);
        self.render_group_runtime.clear_inactive_groups();
        self.effect_delta_clock.clear();
    }

    pub(crate) fn compose_runtime(&mut self) -> ComposeRuntime<'_> {
        ComposeRuntime {
            screen_queue: &mut self.screen_queue,
            composition_planner: &mut self.composition_planner,
            sparkleflinger: &mut self.sparkleflinger,
            #[cfg(feature = "wgpu")]
            display_sparkleflinger: &mut self.display_sparkleflinger,
            #[cfg(feature = "wgpu")]
            display_finalize_runtime: &mut self.display_finalize_runtime,
            render_group_runtime: &mut self.render_group_runtime,
            output_artifacts: &mut self.output_artifacts,
            effect_delta_clock: &mut self.effect_delta_clock,
        }
    }

    pub(crate) fn sampling_runtime(&mut self) -> SamplingRuntime<'_> {
        SamplingRuntime {
            sparkleflinger: &mut self.sparkleflinger,
            deferred_sampling: &mut self.deferred_sampling,
            zone_transition_planner: &mut self.zone_transition_planner,
            output_artifacts: &mut self.output_artifacts,
        }
    }

    pub(crate) fn preview_runtime(&mut self) -> PreviewRuntime<'_> {
        PreviewRuntime {
            sparkleflinger: &mut self.sparkleflinger,
        }
    }

    pub(crate) fn prepare_canvas_resize(
        &mut self,
        width: u32,
        height: u32,
        off_color: [u8; 3],
        spatial_engine: &SpatialEngine,
    ) -> Result<PreparedCanvasResize> {
        let render_group_runtime = self
            .render_group_runtime
            .prepare_scene_resize(width, height)?
            .expect("canvas resize preparation requires changed dimensions");
        let static_surfaces =
            OutputArtifactsState::prepare_static_surfaces(width, height, &[[0, 0, 0], off_color])?;
        let sampling_preparation = self.sparkleflinger.prepare_zone_sampling_plan(
            width,
            height,
            spatial_engine.sampling_plan().as_ref(),
        );
        #[cfg_attr(not(feature = "wgpu"), allow(unused_mut))]
        let mut sparkleflinger_preparation =
            self.sparkleflinger.prepare_canvas_resize(width, height)?;
        #[cfg(feature = "wgpu")]
        let mut display_sparkleflinger_preparation = self
            .display_sparkleflinger
            .prepare_canvas_resize(width, height)?;
        #[cfg(feature = "wgpu")]
        if !sparkleflinger_preparation.is_admitted()
            || !display_sparkleflinger_preparation.is_admitted()
        {
            sparkleflinger_preparation.force_cpu_fallback(width, height)?;
            display_sparkleflinger_preparation.force_cpu_fallback(width, height)?;
        }
        if sampling_preparation.requires_cpu_sampling()
            || sparkleflinger_preparation.requires_cpu_sampling()
        {
            spatial_engine.try_prepare_sampling_canvas(width, height)?;
        }
        Ok(PreparedCanvasResize {
            render_group_runtime,
            static_surfaces,
            sparkleflinger_preparation,
            sampling_preparation,
            #[cfg(feature = "wgpu")]
            display_sparkleflinger_preparation,
        })
    }

    pub(crate) fn commit_canvas_resize(&mut self, prepared: PreparedCanvasResize) {
        self.deferred_sampling
            .clear_for_canvas_resize(&mut self.sparkleflinger);
        #[cfg(feature = "wgpu")]
        for pending in self.display_finalize_runtime.drain() {
            self.display_sparkleflinger
                .discard_pending_display_finalization(pending.pending);
        }
        self.sparkleflinger
            .apply_canvas_resize(prepared.sparkleflinger_preparation);
        self.sparkleflinger
            .apply_zone_sampling_plan(prepared.sampling_preparation);
        #[cfg(feature = "wgpu")]
        self.display_sparkleflinger
            .apply_canvas_resize(prepared.display_sparkleflinger_preparation);
        self.render_group_runtime
            .commit_scene_resize(prepared.render_group_runtime);
        self.composition_planner = CompositionPlanner::new();
        self.zone_transition_planner = ZoneTransitionPlanner::default();
        self.output_artifacts
            .reset_for_canvas_resize(prepared.static_surfaces);
    }

    pub(crate) fn prepare_spatial_sampling_plan(
        &mut self,
        spatial_engine: &SpatialEngine,
    ) -> Result<SparkleFlingerSamplingPreparation> {
        let layout = spatial_engine.layout();
        let preparation = self.sparkleflinger.prepare_zone_sampling_plan(
            layout.canvas_width,
            layout.canvas_height,
            spatial_engine.sampling_plan().as_ref(),
        );
        if preparation.requires_cpu_sampling() {
            spatial_engine
                .try_prepare_sampling_canvas(layout.canvas_width, layout.canvas_height)?;
        }
        Ok(preparation)
    }

    pub(crate) fn commit_spatial_sampling_plan(
        &mut self,
        preparation: SparkleFlingerSamplingPreparation,
    ) {
        self.sparkleflinger.apply_zone_sampling_plan(preparation);
    }

    #[cfg(test)]
    pub(crate) fn apply_spatial_sampling_plan(
        &mut self,
        spatial_engine: &SpatialEngine,
    ) -> Result<bool> {
        let preparation = self.prepare_spatial_sampling_plan(spatial_engine)?;
        #[cfg(feature = "wgpu")]
        let admitted = preparation.is_admitted();
        #[cfg(not(feature = "wgpu"))]
        let admitted = false;
        self.commit_spatial_sampling_plan(preparation);
        Ok(admitted)
    }

    pub(crate) fn render_surface_snapshot(
        &mut self,
        canvas_receiver_count: usize,
    ) -> RenderSurfaceSnapshot {
        let scene_counts = self.render_group_runtime.scene_surface_pool_state_counts();
        let direct_counts = self.render_group_runtime.direct_surface_pool_state_counts();
        let sparkleflinger_counts = self.sparkleflinger.surface_pool_counts();
        #[cfg(feature = "wgpu")]
        let sparkleflinger_counts =
            sparkleflinger_counts.merged(self.display_sparkleflinger.surface_pool_counts());
        let count = |value| u32::try_from(value).unwrap_or(u32::MAX);
        let scene_slot_count = count(
            scene_counts
                .free
                .saturating_add(scene_counts.published)
                .saturating_add(scene_counts.dequeued),
        );
        let preview_slot_count = count(
            sparkleflinger_counts
                .preview
                .free
                .saturating_add(sparkleflinger_counts.preview.published)
                .saturating_add(sparkleflinger_counts.preview.dequeued),
        );
        let compositor_slot_count = count(
            sparkleflinger_counts
                .compositor
                .free
                .saturating_add(sparkleflinger_counts.compositor.published)
                .saturating_add(sparkleflinger_counts.compositor.dequeued),
        );
        let mut snapshot = RenderSurfaceSnapshot {
            slot_count: scene_slot_count,
            free_slots: count(scene_counts.free),
            published_slots: count(scene_counts.published),
            dequeued_slots: count(scene_counts.dequeued),
            canvas_receivers: u32::try_from(canvas_receiver_count).unwrap_or(u32::MAX),
            scene_pool_free_slots: count(scene_counts.free),
            scene_pool_published_slots: count(scene_counts.published),
            scene_pool_dequeued_slots: count(scene_counts.dequeued),
            direct_pool_free_slots: count(direct_counts.free),
            direct_pool_published_slots: count(direct_counts.published),
            direct_pool_dequeued_slots: count(direct_counts.dequeued),
            preview_pool_slot_count: preview_slot_count,
            preview_pool_free_slots: count(sparkleflinger_counts.preview.free),
            preview_pool_published_slots: count(sparkleflinger_counts.preview.published),
            preview_pool_dequeued_slots: count(sparkleflinger_counts.preview.dequeued),
            compositor_pool_slot_count: compositor_slot_count,
            compositor_pool_free_slots: count(sparkleflinger_counts.compositor.free),
            compositor_pool_published_slots: count(sparkleflinger_counts.compositor.published),
            compositor_pool_dequeued_slots: count(sparkleflinger_counts.compositor.dequeued),
            ..RenderSurfaceSnapshot::default()
        };
        snapshot.scene_pool_saturation_reallocs = self
            .render_group_runtime
            .scene_surface_pool_saturation_reallocs();
        snapshot.direct_pool_saturation_reallocs = self
            .render_group_runtime
            .direct_surface_pool_saturation_reallocs();
        snapshot.scene_pool_grown_slots =
            self.render_group_runtime.scene_surface_pool_grown_slots();
        snapshot.direct_pool_grown_slots =
            self.render_group_runtime.direct_surface_pool_grown_slots();
        snapshot.scene_pool_slot_count = scene_slot_count;
        snapshot.scene_pool_max_slots = self.render_group_runtime.scene_surface_pool_max_slots();
        snapshot.direct_pool_slot_count =
            self.render_group_runtime.direct_surface_pool_slot_count();
        snapshot.direct_pool_max_slots = self.render_group_runtime.direct_surface_pool_max_slots();
        snapshot.scene_pool_shared_published_slots = self
            .render_group_runtime
            .scene_surface_pool_shared_published_slots();
        snapshot.scene_pool_max_ref_count =
            self.render_group_runtime.scene_surface_pool_max_ref_count();
        snapshot.direct_pool_shared_published_slots = self
            .render_group_runtime
            .direct_surface_pool_shared_published_slots();
        snapshot.direct_pool_max_ref_count = self
            .render_group_runtime
            .direct_surface_pool_max_ref_count();

        snapshot
    }
}

pub(crate) struct PipelineRuntime {
    pub(crate) scene: SceneSnapshotState,
    pub(crate) frame_loop: FrameLoopState,
    pub(crate) render: RenderCaches,
    pub(crate) frame_policy: FramePolicy,
}

impl PipelineRuntime {
    pub(crate) async fn from_state(
        state: &RenderThreadState,
        input_reader: InputPublicationReader,
        input_demands: InputPublicationDemandHandle,
        #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
        macos_screen_parity_mailbox: MacosScreenParityDiagnosticMailbox,
    ) -> Result<Self> {
        let initial_spatial_engine = state.spatial_engine.read().await.clone();
        let pipeline = Self::new_with_gpu_device(
            state.canvas_dims.width(),
            state.canvas_dims.height(),
            initial_spatial_engine,
            state.screen_capture_configured,
            state.render_acceleration_mode,
            #[cfg(feature = "wgpu")]
            state.render_gpu_device.clone(),
            #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
            macos_screen_parity_mailbox,
            Some(Arc::clone(&state.asset_library)),
            state.configured_max_fps_tier.get(),
            input_reader,
            input_demands,
            state.interaction_routing.clone(),
        )?;
        #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
        state
            .input_manager
            .lock()
            .await
            .set_source_capability_feature(
                "metal4",
                pipeline.render.sparkleflinger.macos_metal4_capability(),
            )?;
        Ok(pipeline)
    }

    #[cfg(test)]
    pub(crate) fn new(
        canvas_width: u32,
        canvas_height: u32,
        initial_spatial_engine: SpatialEngine,
        screen_capture_configured: bool,
        render_acceleration_mode: RenderAccelerationMode,
        configured_max_fps_tier: FpsTier,
    ) -> Result<Self> {
        let input_demands = InputPublicationDemandHandle::new();
        #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
        let (_, macos_screen_parity_mailbox) =
            super::macos_screen_diagnostics::macos_screen_parity_diagnostic_channel();
        Self::new_with_gpu_device(
            canvas_width,
            canvas_height,
            initial_spatial_engine,
            screen_capture_configured,
            render_acceleration_mode,
            #[cfg(feature = "wgpu")]
            None,
            #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
            macos_screen_parity_mailbox,
            None,
            configured_max_fps_tier,
            InputPublicationReader::empty(),
            input_demands,
            InteractionRoutingControl::default(),
        )
    }

    fn new_with_gpu_device(
        canvas_width: u32,
        canvas_height: u32,
        initial_spatial_engine: SpatialEngine,
        screen_capture_configured: bool,
        render_acceleration_mode: RenderAccelerationMode,
        #[cfg(feature = "wgpu")] render_gpu_device: Option<GpuRenderDevice>,
        #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
        macos_screen_parity_mailbox: MacosScreenParityDiagnosticMailbox,
        asset_library: Option<Arc<RwLock<AssetLibrary>>>,
        configured_max_fps_tier: FpsTier,
        input_reader: InputPublicationReader,
        input_demands: InputPublicationDemandHandle,
        interaction_routing: InteractionRoutingControl,
    ) -> Result<Self> {
        let mut sparkleflinger = SparkleFlinger::new_with_gpu_device(
            render_acceleration_mode,
            #[cfg(feature = "wgpu")]
            render_gpu_device.clone(),
        )?;
        #[cfg(feature = "wgpu")]
        let mut display_sparkleflinger =
            SparkleFlinger::new_with_gpu_device(render_acceleration_mode, render_gpu_device)?;
        #[cfg_attr(not(feature = "wgpu"), allow(unused_mut))]
        let mut sparkleflinger_preparation =
            sparkleflinger.prepare_canvas_resize(canvas_width, canvas_height)?;
        let sampling_preparation = sparkleflinger.prepare_zone_sampling_plan(
            canvas_width,
            canvas_height,
            initial_spatial_engine.sampling_plan().as_ref(),
        );
        #[cfg(feature = "wgpu")]
        let mut display_sparkleflinger_preparation =
            display_sparkleflinger.prepare_canvas_resize(canvas_width, canvas_height)?;
        #[cfg(feature = "wgpu")]
        if !sparkleflinger_preparation.is_admitted()
            || !display_sparkleflinger_preparation.is_admitted()
        {
            sparkleflinger_preparation.force_cpu_fallback(canvas_width, canvas_height)?;
            display_sparkleflinger_preparation.force_cpu_fallback(canvas_width, canvas_height)?;
        }
        if sampling_preparation.requires_cpu_sampling()
            || sparkleflinger_preparation.requires_cpu_sampling()
        {
            initial_spatial_engine.try_prepare_sampling_canvas(canvas_width, canvas_height)?;
        }
        sparkleflinger.apply_canvas_resize(sparkleflinger_preparation);
        sparkleflinger.apply_zone_sampling_plan(sampling_preparation);
        #[cfg(feature = "wgpu")]
        display_sparkleflinger.apply_canvas_resize(display_sparkleflinger_preparation);

        Ok(Self {
            scene: SceneSnapshotState::new(initial_spatial_engine, screen_capture_configured),
            frame_loop: FrameLoopState {
                clock: FrameClockState::default(),
                inputs: InputReuseState::with_routing(input_reader, interaction_routing),
                throttle: ThrottleState::default(),
                publication_cadence: PublicationCadenceState::default(),
                output_reuse: OutputReuseState::default(),
                lighting_feed: super::lighting_feed::LightingFeedState::default(),
                input_demands: input_demands.clone(),
                authoritative_input_demand: OwnedInputPublicationDemand::new(
                    &input_demands,
                    InputPublicationConsumer::Authoritative,
                ),
            },
            render: RenderCaches {
                screen_queue: ProducerQueue::new(),
                composition_planner: CompositionPlanner::new(),
                sparkleflinger,
                #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
                macos_screen_parity_mailbox,
                #[cfg(feature = "wgpu")]
                display_sparkleflinger,
                #[cfg(feature = "wgpu")]
                display_finalize_runtime: DisplayFinalizeRuntime::default(),
                deferred_sampling: DeferredSamplingState::default(),
                zone_transition_planner: ZoneTransitionPlanner::default(),
                render_group_runtime: match asset_library {
                    Some(asset_library) => ZoneRuntime::try_with_asset_library(
                        canvas_width,
                        canvas_height,
                        asset_library,
                    )?,
                    None => ZoneRuntime::try_new(canvas_width, canvas_height)?,
                },
                output_artifacts: OutputArtifactsState::default(),
                pending_layout_activation: None,
                effect_delta_clock: EffectDeltaClock::default(),
            },
            frame_policy: FramePolicy::new(configured_max_fps_tier),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::mem::size_of;
    use std::num::{NonZeroU32, NonZeroU64};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use hypercolor_core::bus::HypercolorBus;
    use hypercolor_core::engine::FpsTier;
    use hypercolor_core::input::browser::{
        BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputEdge, BrowserInputSource,
        BrowserPreviewId,
    };
    use hypercolor_core::input::screen::{
        PixelExtent, PlatformGpuApi, ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId,
        ScreenNativePreparationPayload, ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
        ScreenPhysicalGpuDeviceIdentity, ScreenPublicationExecutorRequest,
    };
    use hypercolor_core::input::{
        InputData, InputGraphSnapshot, InputManager, InputSource, InputSourceSlot, InteractionData,
        MotionAggregate, Q16_16_SCALE, ScrollAggregate, SourceIssue, SourceKind,
        SourceStatusWriter,
    };
    use hypercolor_core::spatial::{
        SpatialEngine, SpatialSamplingCapacity, SpatialSamplingWorkspaceUsage,
    };
    use hypercolor_types::config::{InteractionRoutePolicy, RenderAccelerationMode};
    use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent};
    use hypercolor_types::sensor::SystemSnapshot;
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    };

    use super::{
        EffectDeltaClock, FrameClockState, FrameInputs, InputRouteCache, OutputFrameSource,
        OutputReuseKey, OutputReuseState, PipelineRuntime, aggregate_interaction_availability,
        authoritative_input_demand, should_publish_preview_frame,
    };
    use crate::interaction_routing::InteractionRoutingControl;
    use crate::render_thread::input_publication::{InputPublicationDemand, InputPublicationReader};
    use crate::render_thread::scene_snapshot::EffectDemand;

    #[test]
    fn frame_clock_rebase_excludes_paused_wall_time() {
        let start = Instant::now();
        let mut clock = FrameClockState { last_tick: start };
        let resumed_at = start + Duration::from_secs(30);

        clock.rebase(resumed_at);
        let resumed = clock.advance(resumed_at);

        assert_eq!(resumed.frame_interval, Duration::ZERO);
        assert_eq!(resumed.delta_secs, 0.0);
    }

    struct FixedInteractionSource {
        sample: InteractionData,
        events: Vec<TimedInputEvent>,
        running: bool,
    }

    struct WarmingUntypedSensorSource {
        snapshot: Arc<SystemSnapshot>,
        samples: usize,
        running: bool,
    }

    struct EventOnceSource {
        events: Vec<TimedInputEvent>,
        running: bool,
    }

    struct InertNativeTargetPreparer;

    impl ScreenNativeTargetPreparer for InertNativeTargetPreparer {
        fn quote_retained_bytes(
            &self,
            _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
            _platform: &ScreenNativePreparationPayload,
        ) -> anyhow::Result<u64> {
            anyhow::bail!("test target does not quote renderer resources")
        }

        fn prepare(
            &self,
            _descriptor: &hypercolor_core::input::screen::ResolvedScreenPublicationDescriptor,
            _platform: &ScreenNativePreparationPayload,
        ) -> anyhow::Result<ScreenNativeTargetPreparation> {
            anyhow::bail!("test target does not prepare renderer resources")
        }
    }

    impl EventOnceSource {
        fn new(key: &str) -> Self {
            Self {
                events: vec![TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: "route-cache".into(),
                        key: key.to_owned(),
                        state: InputButtonState::Pressed,
                    },
                    at_ms: 1,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                }],
                running: false,
            }
        }
    }

    impl InputSource for EventOnceSource {
        fn name(&self) -> &'static str {
            "event-once"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn drain_events(&mut self) -> Vec<TimedInputEvent> {
            std::mem::take(&mut self.events)
        }

        fn is_interaction_source(&self) -> bool {
            true
        }
    }

    impl WarmingUntypedSensorSource {
        fn new(snapshot: Arc<SystemSnapshot>) -> Self {
            Self {
                snapshot,
                samples: 0,
                running: false,
            }
        }
    }

    impl InputSource for WarmingUntypedSensorSource {
        fn name(&self) -> &'static str {
            "warming-untyped-sensor"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            if !self.running {
                return Ok(InputData::None);
            }
            self.samples += 1;
            match self.samples {
                1 => Ok(InputData::None),
                2 => Ok(InputData::Sensors(Arc::clone(&self.snapshot))),
                _ => Ok(InputData::None),
            }
        }

        fn is_running(&self) -> bool {
            self.running
        }
    }

    impl FixedInteractionSource {
        fn new(key: &str, generation: u64) -> Self {
            let mut sample = InteractionData::default();
            sample.keyboard.pressed_keys.push(key.to_owned());
            sample.generation = generation;
            Self {
                sample,
                events: vec![TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: "fixed-interaction".into(),
                        key: key.to_owned(),
                        state: InputButtonState::Pressed,
                    },
                    at_ms: 1,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                }],
                running: false,
            }
        }
    }

    impl InputSource for FixedInteractionSource {
        fn name(&self) -> &'static str {
            "fixed-interaction"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            if self.running {
                Ok(InputData::Interaction(self.sample.clone()))
            } else {
                Ok(InputData::None)
            }
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn drain_events(&mut self) -> Vec<TimedInputEvent> {
            std::mem::take(&mut self.events)
        }

        fn is_interaction_source(&self) -> bool {
            true
        }
    }

    fn empty_layout() -> SpatialLayout {
        SpatialLayout {
            id: "test".into(),
            name: "Test Layout".into(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    fn area_layout(width: u32, height: u32) -> SpatialLayout {
        SpatialLayout {
            id: "area-test".into(),
            name: "Area Test".into(),
            description: None,
            canvas_width: width,
            canvas_height: height,
            zones: vec![Output {
                id: "zone".into(),
                name: "zone".into(),
                device_id: "device:zone".into(),
                zone_name: None,
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Point,
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(SamplingMode::AreaAverage {
                    radius_x: 1.0,
                    radius_y: 1.0,
                }),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: None,
                shape_preset: None,
                display_order: 0,
                attachment: None,
                brightness: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    fn preview_key(connection: u64, preview: &str) -> BrowserInputChildKey {
        BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(connection),
            BrowserPreviewId::new(Arc::<str>::from(preview)),
        )
    }

    fn resolve_authoritative(
        routes: &mut InputRouteCache,
        graph: &InputGraphSnapshot,
        event_bus: &HypercolorBus,
        inputs: &mut FrameInputs,
    ) {
        let graph_changed = routes.graph_generation != Some(graph.generation());
        if graph_changed {
            routes.rebuild_routes(graph);
        }
        let registry = routes.interaction_routing.browser_registry_snapshot();
        if graph_changed || routes.browser_registry_generation != Some(registry.generation()) {
            routes.rebuild_interaction_sources(graph, &registry);
        }
        inputs.prepare_for_sample(None);
        routes.route_latest_into(inputs);
        routes.route_interaction_into(event_bus, graph, &registry, inputs);
    }

    #[test]
    fn interaction_availability_tracks_idle_health_and_terminal_failure() {
        let (writer, handle) = SourceStatusWriter::new(
            "test_interaction",
            SourceKind::Interaction,
            "test",
            true,
            true,
            true,
        );
        let session = writer
            .begin_session(1)
            .expect("eligible source should begin");
        assert!(session.mark_event_driven_live_without_deadline(1));

        let idle =
            aggregate_interaction_availability(std::slice::from_ref(&handle), Instant::now());
        assert_eq!(
            idle,
            hypercolor_core::effect::InputSourceAvailability {
                routed: true,
                healthy: true,
                fresh: true,
                degraded: false,
            }
        );

        assert!(session.failed(SourceIssue::new(
            "worker_failed",
            "interaction worker stopped",
            true,
        )));
        let failed = aggregate_interaction_availability(&[handle], Instant::now());
        assert_eq!(
            failed,
            hypercolor_core::effect::InputSourceAvailability {
                routed: true,
                healthy: false,
                fresh: false,
                degraded: false,
            }
        );
    }

    #[test]
    fn statusless_interaction_source_is_routed_through_its_graph_slot() {
        let mut manager = InputManager::new();
        manager.add_source(Box::new(EventOnceSource::new("KeyA")));
        manager
            .start_all()
            .expect("statusless interaction source should start");
        let graph = manager.input_graph_handle().snapshot();

        let availability = aggregate_interaction_availability(
            graph.slots().iter().map(InputSourceSlot::status),
            Instant::now(),
        );

        assert_eq!(
            availability,
            hypercolor_core::effect::InputSourceAvailability {
                routed: true,
                healthy: true,
                fresh: true,
                degraded: false,
            }
        );
    }

    #[test]
    fn authoritative_route_selects_host_exact_browser_and_merge_with_releases() {
        let mut manager = InputManager::new();
        manager.add_source(Box::new(FixedInteractionSource::new("KeyA", 7)));
        let browser_source = BrowserInputSource::new();
        let browser = browser_source.handle();
        manager.add_source(Box::new(browser_source));
        manager.start_all().expect("input sources should start");
        manager
            .set_interaction_capture_active(true)
            .expect("interaction demand should activate");
        let selected = browser
            .attach(preview_key(1, "selected"))
            .expect("selected preview should attach");
        let suppressed = browser
            .attach(preview_key(2, "suppressed"))
            .expect("suppressed preview should attach");
        let control = InteractionRoutingControl::new(
            browser.registry(),
            1,
            InteractionRoutePolicy::Host,
            InteractionRoutePolicy::Browser,
        );
        let mut routes = InputRouteCache::new(InputPublicationReader::empty(), control.clone());
        let mut inputs = FrameInputs::silence();
        let event_bus = HypercolorBus::new();

        let graph = manager.input_graph_handle().snapshot();
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        manager.sample_sources(1.0 / 60.0);
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        assert_eq!(inputs.interaction.keyboard.pressed_keys, ["KeyA"]);

        assert_eq!(
            control.claim_authoritative(&selected),
            Ok(crate::interaction_routing::AuthoritativeClaimOutcome::Granted)
        );
        control.publish_policies(
            2,
            InteractionRoutePolicy::Merge,
            InteractionRoutePolicy::Browser,
        );
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        selected
            .inject([BrowserInputEdge::Key {
                key: "KeyB".to_owned(),
                state: InputButtonState::Pressed,
            }])
            .expect("selected preview input should publish");
        suppressed
            .inject([BrowserInputEdge::Key {
                key: "KeyC".to_owned(),
                state: InputButtonState::Pressed,
            }])
            .expect("suppressed preview input should publish");
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        assert_eq!(
            inputs.interaction.keyboard.pressed_keys,
            ["KeyA".to_owned(), "KeyB".to_owned()]
        );
        assert!(
            !inputs
                .interaction
                .keyboard
                .pressed_keys
                .contains(&"KeyC".to_owned())
        );

        control.publish_policies(
            3,
            InteractionRoutePolicy::Browser,
            InteractionRoutePolicy::Browser,
        );
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        assert_eq!(inputs.interaction.keyboard.pressed_keys, ["KeyB"]);
        assert!(inputs.interaction.batch.events.iter().any(|event| matches!(
            &event.event,
            InputEvent::Key { key, state, .. }
                if key == "KeyA" && *state == InputButtonState::Released
        )));

        control.publish_policies(
            4,
            InteractionRoutePolicy::Host,
            InteractionRoutePolicy::Browser,
        );
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        assert!(inputs.interaction.keyboard.pressed_keys.is_empty());
        assert!(inputs.interaction.batch.events.iter().any(|event| matches!(
            &event.event,
            InputEvent::Key { key, state, .. }
                if key == "KeyB" && *state == InputButtonState::Released
        )));
    }

    #[test]
    fn untyped_source_is_routed_after_warming_up_without_a_graph_change() {
        let snapshot = Arc::new(SystemSnapshot::empty());
        let mut manager = InputManager::new();
        manager.add_source(Box::new(WarmingUntypedSensorSource::new(Arc::clone(
            &snapshot,
        ))));
        manager.start_all().expect("sensor source should start");
        let graph = manager.input_graph_handle();
        let mut routes = InputRouteCache::default();
        let mut inputs = FrameInputs::silence();

        manager.sample_sources(1.0 / 60.0);
        routes.rebuild_routes(&graph.snapshot());
        inputs.prepare_for_sample(None);
        routes.route_latest_into(&mut inputs);
        assert!(!Arc::ptr_eq(&inputs.sensors, &snapshot));

        manager.sample_sources(1.0 / 60.0);
        inputs.prepare_for_sample(None);
        routes.route_latest_into(&mut inputs);
        assert!(Arc::ptr_eq(&inputs.sensors, &snapshot));

        manager.sample_sources(1.0 / 60.0);
        inputs.prepare_for_sample(None);
        routes.route_latest_into(&mut inputs);
        assert!(!Arc::ptr_eq(&inputs.sensors, &snapshot));
    }

    #[test]
    fn source_replacement_and_availability_changes_invalidate_reuse_identity() {
        let mut manager = InputManager::new();
        manager.add_source(Box::new(FixedInteractionSource::new("KeyA", 7)));
        manager.start_all().expect("first source should start");
        manager
            .set_interaction_capture_active(true)
            .expect("interaction demand should activate");
        let mut routes = InputRouteCache::default();
        let mut inputs = FrameInputs::silence();
        let event_bus = HypercolorBus::new();

        let first_graph = manager.input_graph_handle().snapshot();
        resolve_authoritative(&mut routes, &first_graph, &event_bus, &mut inputs);
        manager.sample_sources(1.0 / 60.0);
        resolve_authoritative(&mut routes, &first_graph, &event_bus, &mut inputs);
        let live_reuse = routes.routed_interaction.reuse_key.clone();
        assert_eq!(inputs.interaction.keyboard.pressed_keys, ["KeyA"]);

        manager
            .set_interaction_capture_active(false)
            .expect("interaction demand should deactivate");
        let inactive_graph = manager.input_graph_handle().snapshot();
        resolve_authoritative(&mut routes, &inactive_graph, &event_bus, &mut inputs);
        let inactive_reuse = routes.routed_interaction.reuse_key.clone();
        assert_ne!(inactive_reuse, live_reuse);
        assert!(!inputs.input_availability.routed);

        let replacement =
            manager.replace_source(0, Box::new(FixedInteractionSource::new("KeyB", 7)));
        assert!(replacement.is_ok(), "replacement index should exist");
        manager
            .start_all()
            .expect("replacement source should start");
        manager
            .set_interaction_capture_active(true)
            .expect("replacement demand should activate");
        manager.sample_sources(1.0 / 60.0);
        let replacement_graph = manager.input_graph_handle().snapshot();
        resolve_authoritative(&mut routes, &replacement_graph, &event_bus, &mut inputs);
        assert!(inputs.interaction.keyboard.pressed_keys.is_empty());
        assert_ne!(routes.routed_interaction.reuse_key, inactive_reuse);
        assert!(inputs.interaction.batch.events.iter().any(|event| matches!(
            &event.event,
            InputEvent::Key { key, state, .. }
                if key == "KeyA" && *state == InputButtonState::Released
        )));
    }

    #[test]
    fn browser_transients_and_wheel_are_delivered_once_across_superseded_publications() {
        let mut manager = InputManager::new();
        let browser_source = BrowserInputSource::new();
        let browser = browser_source.handle();
        manager.add_source(Box::new(browser_source));
        manager.start_all().expect("browser source should start");
        manager
            .set_interaction_capture_active(true)
            .expect("browser demand should activate");
        let preview = browser
            .attach(preview_key(7, "motion"))
            .expect("preview should attach");
        let control = InteractionRoutingControl::new(
            browser.registry(),
            1,
            InteractionRoutePolicy::Browser,
            InteractionRoutePolicy::Browser,
        );
        assert!(control.claim_authoritative(&preview).is_ok());
        let mut routes = InputRouteCache::new(InputPublicationReader::empty(), control);
        let mut inputs = FrameInputs::silence();
        let event_bus = HypercolorBus::new();
        let graph = manager.input_graph_handle().snapshot();

        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        preview
            .inject([BrowserInputEdge::Move {
                norm_x: 0.1,
                norm_y: 0.2,
            }])
            .expect("initial pointer should publish");
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);

        preview
            .inject([
                BrowserInputEdge::Move {
                    norm_x: 0.3,
                    norm_y: 0.4,
                },
                BrowserInputEdge::Wheel { delta_hi_res: 120 },
            ])
            .expect("first transient batch should publish");
        preview
            .inject([
                BrowserInputEdge::Move {
                    norm_x: 0.5,
                    norm_y: 0.6,
                },
                BrowserInputEdge::Wheel { delta_hi_res: -40 },
            ])
            .expect("superseding transient batch should publish");
        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);

        assert!((inputs.interaction.batch.motion.dx - 0.4).abs() < 0.000_1);
        assert!((inputs.interaction.batch.motion.dy - 0.4).abs() < 0.000_1);
        assert_eq!(inputs.interaction.batch.wheel_hi_res, 80);
        assert_eq!(
            inputs.interaction.batch.scroll.line120_y_q16_16,
            80 * Q16_16_SCALE
        );
        assert_eq!(
            inputs
                .interaction
                .batch
                .events
                .iter()
                .filter(|event| matches!(event.event, InputEvent::MouseWheel { .. }))
                .count(),
            2
        );
        assert_eq!(
            inputs
                .interaction
                .batch
                .events
                .iter()
                .filter(|event| matches!(event.event, InputEvent::PointerScroll { .. }))
                .count(),
            2
        );
        assert!(
            inputs
                .interaction
                .batch
                .events
                .chunks_exact(2)
                .all(
                    |pair| matches!(pair[0].event, InputEvent::PointerScroll { .. })
                        && matches!(pair[1].event, InputEvent::MouseWheel { .. })
                )
        );

        resolve_authoritative(&mut routes, &graph, &event_bus, &mut inputs);
        assert_eq!(inputs.interaction.batch.motion, MotionAggregate::default());
        assert_eq!(inputs.interaction.batch.wheel_hi_res, 0);
        assert_eq!(inputs.interaction.batch.scroll, ScrollAggregate::default());
        assert!(inputs.interaction.batch.events.is_empty());
    }

    #[test]
    fn authoritative_demand_survives_output_sleep_policy() {
        let effect_demand = EffectDemand {
            effect_running: true,
            audio_capture_active: true,
            screen_capture_active: true,
            interaction_capture_active: true,
            media_input_active: true,
            network_input_active: true,
        };

        let screen_extent = PixelExtent::new(5_120, 2_160)
            .expect("test screen publication extent should be non-empty");
        let demand = authoritative_input_demand(effect_demand, 60, screen_extent, None);
        let expected = InputPublicationDemand::default()
            .with_source(SourceKind::Audio, 60)
            .with_screen(60, screen_extent)
            .with_source(SourceKind::Interaction, 60)
            .with_source(SourceKind::Media, 1)
            .with_source(SourceKind::Network, 1);

        assert_eq!(demand, expected);
    }

    #[test]
    fn authoritative_screen_demand_binds_the_renderer_target() {
        let effect_demand = EffectDemand {
            effect_running: true,
            audio_capture_active: false,
            screen_capture_active: true,
            interaction_capture_active: false,
            media_input_active: false,
            network_input_active: false,
        };
        let screen_extent = PixelExtent::new(7_680, 4_320)
            .expect("test screen publication extent should be non-empty");
        let target = ScreenNativeExecutionTarget::new(
            ScreenNativeExecutionTargetId::new(NonZeroU64::MIN),
            PlatformGpuApi::Direct3d11,
            ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
                low_part: 7,
                high_part: 11,
            },
            NonZeroU32::new(16_384).expect("test texture limit is non-zero"),
            Arc::new(InertNativeTargetPreparer),
        );

        let demand = authoritative_input_demand(effect_demand, 144, screen_extent, Some(&target));
        let expected = InputPublicationDemand::default().with_screen_executor(
            144,
            screen_extent,
            ScreenPublicationExecutorRequest::SourceNative(target),
        );

        assert_eq!(demand, expected);
    }

    #[test]
    fn pipeline_runtime_rejects_unresolved_auto_acceleration_mode() {
        let Err(error) = PipelineRuntime::new(
            320,
            200,
            SpatialEngine::new(empty_layout()),
            false,
            RenderAccelerationMode::Auto,
            FpsTier::Full,
        ) else {
            panic!("render runtime should only receive a resolved acceleration mode");
        };

        assert!(
            error
                .to_string()
                .contains("must be resolved before constructing SparkleFlinger")
        );
    }

    #[test]
    fn cpu_sampling_workspace_is_resident_before_runtime_acceptance() {
        let engine = SpatialEngine::new(area_layout(4, 4));
        assert_eq!(
            engine.sampling_workspace_usage(),
            SpatialSamplingWorkspaceUsage::default()
        );

        let _runtime = PipelineRuntime::new(
            4,
            4,
            engine.clone(),
            false,
            RenderAccelerationMode::Cpu,
            FpsTier::Full,
        )
        .expect("CPU runtime admission should prepare exact area sampling");

        assert_eq!(engine.sampling_workspace_usage().retained_workspaces, 1);
    }

    #[test]
    fn spatial_plan_cpu_fallback_is_prepared_before_apply_returns() {
        let mut runtime = PipelineRuntime::new(
            4,
            4,
            SpatialEngine::new(empty_layout()),
            false,
            RenderAccelerationMode::Cpu,
            FpsTier::Full,
        )
        .expect("CPU runtime should initialize");
        let candidate = SpatialEngine::new(area_layout(4, 4));

        let gpu_admitted = runtime
            .render
            .apply_spatial_sampling_plan(&candidate)
            .expect("CPU fallback workspace should prepare transactionally");

        assert!(!gpu_admitted);
        assert_eq!(candidate.sampling_workspace_usage().retained_workspaces, 1);
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn transient_gpu_fallback_prepares_cpu_workspace_before_plan_acceptance() {
        let mut runtime = match PipelineRuntime::new(
            4,
            4,
            SpatialEngine::new(empty_layout()),
            false,
            RenderAccelerationMode::Gpu,
            FpsTier::Full,
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("GPU test environment unavailable: {error}");
                return;
            }
        };
        let candidate = SpatialEngine::new(area_layout(4, 4));
        assert!(
            runtime
                .render
                .sparkleflinger
                .fail_next_gpu_sampling_preparation()
        );

        let gpu_admitted = runtime
            .render
            .apply_spatial_sampling_plan(&candidate)
            .expect("transient GPU fallback should prepare its CPU workspace");

        assert!(!gpu_admitted);
        assert_eq!(candidate.sampling_workspace_usage().retained_workspaces, 1);
    }

    #[test]
    fn failed_resize_cpu_workspace_preparation_preserves_last_good_workspace() {
        let initial_bytes = 5 * 5 * size_of::<[u64; 3]>();
        let engine = SpatialEngine::try_new_with_sampling_capacity(
            area_layout(4, 4),
            SpatialSamplingCapacity::new(initial_bytes),
        )
        .expect("initial area layout should fit its exact workspace capacity");
        let mut runtime = PipelineRuntime::new(
            4,
            4,
            engine.clone(),
            false,
            RenderAccelerationMode::Cpu,
            FpsTier::Full,
        )
        .expect("initial CPU runtime should prepare its area workspace");

        let active_engine = runtime.scene.render_state.spatial_engine().clone();
        let Err(error) = runtime
            .render
            .prepare_canvas_resize(8, 8, [0, 0, 0], &active_engine)
        else {
            panic!("larger fallback workspace must exceed the configured capacity");
        };

        assert!(error.to_string().contains("capacity is 600 bytes"));
        assert_eq!(engine.sampling_workspace_usage().retained_workspaces, 1);
        engine
            .try_prepare_sampling_canvas(4, 4)
            .expect("the last-good workspace must remain resident after rejection");
    }

    #[test]
    fn render_surface_snapshot_reports_actual_runtime_pools() {
        let mut runtime = PipelineRuntime::new(
            320,
            200,
            SpatialEngine::new(empty_layout()),
            false,
            RenderAccelerationMode::Cpu,
            FpsTier::Full,
        )
        .expect("CPU render runtime should initialize");

        let lazy_snapshot = runtime.render.render_surface_snapshot(3);
        assert_eq!(lazy_snapshot.scene_pool_slot_count, 0);
        assert_eq!(lazy_snapshot.scene_pool_free_slots, 0);

        runtime
            .render
            .render_group_runtime
            .try_resize_scene(321, 200)
            .expect("CPU scene resize should prepare its surface pool");
        let snapshot = runtime.render.render_surface_snapshot(3);

        assert_eq!(snapshot.canvas_receivers, 3);
        assert_eq!(snapshot.slot_count, snapshot.scene_pool_slot_count);
        assert_eq!(snapshot.free_slots, snapshot.scene_pool_free_slots);
        assert_eq!(
            snapshot.published_slots,
            snapshot.scene_pool_published_slots
        );
        assert_eq!(snapshot.dequeued_slots, snapshot.scene_pool_dequeued_slots);
        assert_eq!(snapshot.scene_pool_slot_count, 8);
        assert_eq!(snapshot.scene_pool_free_slots, 8);
        assert_eq!(snapshot.direct_pool_slot_count, 0);
        #[cfg(not(feature = "wgpu"))]
        let (preview_slots, compositor_slots) = (2, 4);
        #[cfg(feature = "wgpu")]
        let (preview_slots, compositor_slots) = (4, 8);
        assert_eq!(snapshot.preview_pool_slot_count, preview_slots);
        assert_eq!(snapshot.preview_pool_free_slots, preview_slots);
        assert_eq!(snapshot.compositor_pool_slot_count, compositor_slots);
        assert_eq!(snapshot.compositor_pool_free_slots, compositor_slots);
    }

    #[test]
    fn retained_canvas_delta_is_applied_to_the_next_effect_render() {
        let mut clock = EffectDeltaClock::default();
        let key = super::SceneDependencyKey::new(1, 2);
        clock.record_retained_frame(key, Duration::from_millis(16));
        clock.record_retained_frame(key, Duration::from_millis(17));

        let render_delta = clock.take_render_delta(key, Duration::from_millis(18));

        assert!((render_delta - 0.051).abs() < f32::EPSILON);
        assert_eq!(clock.retained_delta, Duration::ZERO);
        assert_eq!(clock.dependency_key, None);
    }

    #[test]
    fn retained_canvas_delta_does_not_cross_scene_dependencies() {
        let mut clock = EffectDeltaClock::default();
        clock.record_retained_frame(
            super::SceneDependencyKey::new(1, 2),
            Duration::from_millis(16),
        );

        let render_delta = clock.take_render_delta(
            super::SceneDependencyKey::new(2, 3),
            Duration::from_millis(17),
        );

        assert!((render_delta - 0.017).abs() < f32::EPSILON);
        assert_eq!(clock.retained_delta, Duration::ZERO);
        assert_eq!(clock.dependency_key, None);
    }

    #[test]
    fn preview_cadence_remains_live_after_u32_millisecond_uptime() {
        let last_publish_ms = u64::from(u32::MAX) + 1_000;

        assert!(!should_publish_preview_frame(
            last_publish_ms + 15,
            Some(last_publish_ms),
            Some(60),
        ));
        assert!(should_publish_preview_frame(
            last_publish_ms + 17,
            Some(last_publish_ms),
            Some(60),
        ));
    }

    #[test]
    fn output_frame_source_reuses_routed_outputs_when_dependencies_match() {
        let mut reuse = OutputReuseState::default();
        let key = OutputReuseKey::new(1, 7, 11, 13, 17);
        reuse.record(key);

        let source = reuse.decide_frame_source(true, key, || true).source();

        assert_eq!(source, OutputFrameSource::RoutedReuse);
    }

    #[test]
    fn output_frame_source_falls_back_to_published_frame_when_route_reuse_is_unavailable() {
        let mut reuse = OutputReuseState::default();
        let key = OutputReuseKey::new(1, 7, 11, 13, 17);
        reuse.record(key);

        let source = reuse.decide_frame_source(true, key, || false).source();

        assert_eq!(source, OutputFrameSource::PublishedFrame);
    }

    #[test]
    fn output_frame_source_skips_route_reuse_probe_without_published_frame_reuse() {
        let reuse = OutputReuseState::default();
        let route_probe_calls = Cell::new(0_u32);
        let key = OutputReuseKey::new(1, 7, 11, 13, 17);

        let source = reuse
            .decide_frame_source(false, key, || {
                route_probe_calls.set(route_probe_calls.get() + 1);
                true
            })
            .source();

        assert_eq!(source, OutputFrameSource::CurrentFrame);
        assert_eq!(route_probe_calls.get(), 0);
    }

    #[test]
    fn output_frame_source_skips_route_reuse_probe_when_reuse_metadata_mismatches() {
        let mut reuse = OutputReuseState::default();
        reuse.record(OutputReuseKey::new(1, 7, 11, 13, 17));
        let route_probe_calls = Cell::new(0_u32);

        let source = reuse
            .decide_frame_source(true, OutputReuseKey::new(1, 8, 11, 13, 17), || {
                route_probe_calls.set(route_probe_calls.get() + 1);
                true
            })
            .source();

        assert_eq!(source, OutputFrameSource::PublishedFrame);
        assert_eq!(route_probe_calls.get(), 0);
    }

    #[test]
    fn output_frame_source_uses_published_frame_when_routing_signature_changes() {
        let mut reuse = OutputReuseState::default();
        reuse.record(OutputReuseKey::new(1, 7, 11, 13, 17));
        let route_probe_calls = Cell::new(0_u32);

        let source = reuse
            .decide_frame_source(true, OutputReuseKey::new(1, 7, 12, 13, 17), || {
                route_probe_calls.set(route_probe_calls.get() + 1);
                true
            })
            .source();

        assert_eq!(source, OutputFrameSource::PublishedFrame);
        assert_eq!(route_probe_calls.get(), 0);
    }

    #[test]
    fn output_frame_source_uses_published_frame_when_zone_shape_changes() {
        let mut reuse = OutputReuseState::default();
        reuse.record(OutputReuseKey::new(1, 7, 11, 13, 17));
        let route_probe_calls = Cell::new(0_u32);

        let source = reuse
            .decide_frame_source(true, OutputReuseKey::new(1, 7, 11, 14, 17), || {
                route_probe_calls.set(route_probe_calls.get() + 1);
                true
            })
            .source();

        assert_eq!(source, OutputFrameSource::PublishedFrame);
        assert_eq!(route_probe_calls.get(), 0);
    }

    #[test]
    fn output_frame_source_uses_published_frame_when_unassigned_behavior_changes() {
        let mut reuse = OutputReuseState::default();
        reuse.record(OutputReuseKey::new(1, 7, 11, 13, 17));
        let route_probe_calls = Cell::new(0_u32);

        let source = reuse
            .decide_frame_source(true, OutputReuseKey::new(1, 7, 11, 13, 18), || {
                route_probe_calls.set(route_probe_calls.get() + 1);
                true
            })
            .source();

        assert_eq!(source, OutputFrameSource::PublishedFrame);
        assert_eq!(route_probe_calls.get(), 0);
    }

    #[test]
    fn output_frame_source_records_reuse_metadata_after_decision() {
        let mut reuse = OutputReuseState::default();
        let key = OutputReuseKey::new(1, 7, 11, 13, 17);

        let decision = reuse.decide_frame_source(false, key, || true);
        reuse.record_decision(decision);

        assert!(reuse.matches(key));
    }
}
