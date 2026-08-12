//! Input sources — audio, screen capture, and future sensor inputs.
//!
//! This module defines the [`InputSource`] trait for pluggable data sources
//! and the [`InputManager`] that orchestrates them. The render loop calls
//! `sample_all()` each frame to collect fresh data from every active source.

pub mod audio;
pub mod browser;
#[cfg(target_os = "linux")]
pub mod evdev;
mod graph;
pub mod keymap;
pub mod macos;
pub mod media;
pub mod net;
pub mod routing;
pub mod screen;
mod scroll;
pub mod sensor;
mod status;
mod traits;
pub mod windows;
mod worker_retention;

pub use browser::{
    BROWSER_RETIRED_LEGACY_CAPACITY, BrowserConnectionIncarnation, BrowserInputAttachment,
    BrowserInputChildKey, BrowserInputChildSlot, BrowserInputEdge, BrowserInputHandle,
    BrowserInputPublicationId, BrowserInputRegistryError, BrowserInputRegistryHandle,
    BrowserInputRegistrySnapshot, BrowserInputSource, BrowserPreviewId,
};
#[cfg(target_os = "linux")]
pub use evdev::{DeviceOpenState, DeviceOpenStatus, EvdevHostInput};
pub use graph::{
    INPUT_EVENT_RING_CAPACITY, InputEventRead, InputGraphHandle, InputGraphSnapshot,
    InputPublicationRead, InputSourceSlot, InteractionSourceOrigin, InteractionTransientTotals,
};
pub use macos::{MacosHostInput, MacosInputFoldDiagnostics};
#[cfg(feature = "macos-native-fixtures")]
pub use macos::{MacosHostInputFixture, MacosInputFixtureBackend};
pub use media::MediaSource;
pub use net::NetSource;
pub use screen::{ScreenCaptureDemand, ScreenPublicationDemandSnapshot};
pub use scroll::{LegacyWheelProjector, Q16_16_SCALE, q16_16_to_f64};
pub use sensor::SensorPoller;
pub use status::{
    MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner, MacosDaemonOwnerConflict,
    MacosInputPlatformStatus, MacosProtectedSourceState, MacosScreenPlatformStatus,
    MacosSelectionState, MacosTahoeCapabilities, MacosTahoeSelectionCapabilities,
    ScreenCaptureDiagnostics, ScreenCaptureReductionPath, SourceDiagnostics, SourceFreshness,
    SourceIssue, SourceKind, SourcePlatformStatus, SourceResourceScanHealth, SourceSessionSlot,
    SourceSessionWriter, SourceState, SourceStatus, SourceStatusAvailability, SourceStatusError,
    SourceStatusHandle, SourceStatusRegistry, SourceStatusRegistrySnapshot, SourceStatusReporter,
    SourceStatusSubscription, SourceStatusWriter, SourceTimestampField, TerminalFailureLatch,
    classify_source_resource_scan,
};
#[cfg(target_os = "macos")]
pub use traits::MacosScreenshotReferenceAction;
pub use traits::{
    InputData, InputSource, InteractionBatch, InteractionData, InteractionDegradation,
    InteractionDiagnostics, KeyboardData, MotionAggregate, MouseData, PointerMode,
    ProtectedSourceActionExecutor, ProtectedSourceActionOwner, ProtectedSourceAuthorizationAction,
    ResolvedProtectedSourceAction, ScreenData, ScreenSourcePickerAction, ScreenZoneColors,
    ScrollAggregate,
};
pub use windows::WindowsHostInput;
#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
pub use windows::WindowsHostInputFixture;

use crate::input::audio::{
    AudioInput, AudioPreparationRequest, AudioRuntimeRetirement, PreparedAudioReconfiguration,
};
use crate::types::audio::AudioPipelineConfig;
use crate::types::event::TimedInputEvent;
use hypercolor_types::sensor::SystemSnapshot;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use thiserror::Error;
use tokio::sync::watch;

use tracing::{error, info};

/// Milliseconds on a process-wide monotonic clock, for input capture stamps.
///
/// Only differences between stamps are meaningful; the epoch is the first
/// call in this process.
#[must_use]
pub fn input_mono_ms() -> u64 {
    static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
    let elapsed = EPOCH.elapsed();
    // Process uptime in ms won't exceed u64.
    #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
    let ms = elapsed.as_millis() as u64;
    ms
}

// ── InputManager ───────────────────────────────────────────────────────────

/// Generation-fenced audio configuration captured while briefly holding the
/// input manager lock.
#[must_use = "audio reconfiguration plans must be prepared and committed"]
pub struct AudioRuntimeConfigPlan {
    expected_graph_generation: u64,
    expected_source_present: bool,
    expected_source_running: bool,
    enabled: bool,
    config: AudioPipelineConfig,
    display_name: String,
    capture_active: bool,
}

/// A concurrent input-graph transition invalidated prepared audio state.
#[derive(Debug, thiserror::Error)]
pub enum AudioReconfigurationConflict {
    /// The canonical source graph changed after preparation began.
    #[error("input graph changed while audio reconfiguration was prepared")]
    GraphChanged,
    /// An audio source was added or removed after preparation began.
    #[error("audio source topology changed while reconfiguration was prepared")]
    SourceTopologyChanged,
    /// The target audio source started or stopped after preparation began.
    #[error("audio source lifecycle changed while reconfiguration was prepared")]
    SourceLifecycleChanged,
}

/// Generation-fenced screen configuration captured while briefly holding the
/// input manager lock.
#[must_use = "screen reconfiguration plans must be prepared and committed"]
pub struct ScreenRuntimeConfigPlan {
    expected_graph_generation: u64,
    expected_source_present: bool,
    expected_source_running: bool,
    expected_capture_demand: ScreenCaptureDemand,
    enabled: bool,
    capture_demand: ScreenCaptureDemand,
}

impl ScreenRuntimeConfigPlan {
    /// Whether the replacement source must be registered after commit.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Demand state the prepared replacement must adopt before it starts.
    #[must_use]
    pub const fn capture_demand(&self) -> ScreenCaptureDemand {
        self.capture_demand
    }

    /// Graph generation reserved for a staged replacement source.
    #[must_use]
    pub fn replacement_source_graph_generation(&self) -> u64 {
        self.expected_graph_generation
            .checked_add(1)
            .expect("input source graph generation exhausted")
    }
}

/// Exact steady-state screen capacity prepared against one manager revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "screen capacity preparations must be committed or discarded"]
pub struct ScreenCapacityPreparation {
    expected_graph_generation: u64,
    expected_capture_demand: Option<ScreenCaptureDemand>,
    expected_plan_generation: screen::ScreenPlanGeneration,
    expected_demand_revision: screen::InputPublicationDemandRevision,
    expected_resource_capacity_revision: u64,
    expected_capacity_generation: u64,
    expected_total_capacity: screen::ScreenAdmissionCapacity,
    total_capacity: screen::ScreenAdmissionCapacity,
    publication_capacity: screen::ScreenAdmissionCapacity,
    analysis_peak_bytes: u64,
}

impl ScreenCapacityPreparation {
    /// Publication capacity left after the exact candidate analysis quote.
    #[must_use]
    pub const fn publication_capacity(self) -> screen::ScreenAdmissionCapacity {
        self.publication_capacity
    }

    /// Peak candidate analysis bytes subtracted from both steady fences.
    #[must_use]
    pub const fn analysis_peak_bytes(self) -> u64 {
        self.analysis_peak_bytes
    }
}

/// Rejection while coupling analysis and publication capacity.
#[derive(Debug, thiserror::Error)]
pub enum ScreenCapacityPreparationError {
    /// Candidate analysis alone exceeds the configured steady-state total.
    #[error(
        "screen analysis needs {requested_bytes} bytes; steady capacity is {available_bytes} bytes"
    )]
    AnalysisCapacityExceeded {
        /// Exact candidate analysis peak.
        requested_bytes: u64,
        /// Capacity shared by both configured and physical fences.
        available_bytes: u64,
    },
    /// The active publication state cannot fit the candidate remainder.
    #[error(transparent)]
    Publication(#[from] screen::ScreenPlanError),
}

/// A concurrent input-graph transition invalidated prepared screen state.
#[derive(Debug, thiserror::Error)]
pub enum ScreenReconfigurationConflict {
    /// The canonical source graph changed after preparation began.
    #[error("input graph changed while screen reconfiguration was prepared")]
    GraphChanged,
    /// A screen source was added or removed after preparation began.
    #[error("screen source topology changed while reconfiguration was prepared")]
    SourceTopologyChanged,
    /// The target screen source started or stopped after preparation began.
    #[error("screen source lifecycle changed while reconfiguration was prepared")]
    SourceLifecycleChanged,
    /// Screen demand changed after preparation began.
    #[error("screen capture demand changed while reconfiguration was prepared")]
    CaptureDemandChanged,
    /// Exact publication state changed after capacity preparation began.
    #[error("screen publication state changed while reconfiguration was prepared")]
    PublicationStateChanged,
    /// The shared physical resource fence changed after preparation began.
    #[error("screen resource capacity changed while reconfiguration was prepared")]
    ResourceCapacityChanged,
    /// The configured analysis/publication split changed after preparation began.
    #[error("screen capacity policy changed while reconfiguration was prepared")]
    CapacityPolicyChanged,
    /// The prepared replacement does not match the plan.
    #[error("prepared screen source does not match the reconfiguration plan")]
    InvalidReplacement,
}

/// Rejected host-input source swap.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum HostReconfigurationError {
    /// More than one host source violates the manager's replacement invariant.
    #[error("more than one host input source is registered")]
    SourceTopologyChanged,
    /// The candidate is not a running host interaction source.
    #[error("prepared host input replacement is invalid")]
    InvalidReplacement,
}

/// Host sources detached by an atomic graph commit.
#[must_use = "retired host sources must be stopped outside the input manager lock"]
pub struct HostRuntimeRetirement {
    source: Option<ManagedInputSource>,
    source_graph_generation: u64,
}

impl HostRuntimeRetirement {
    /// Stop the detached source and retire its status handle.
    pub fn retire(mut self) {
        let Some(source) = &mut self.source else {
            return;
        };
        source.stop();
        if let Err(error) = source.retire_source_status(self.source_graph_generation) {
            error!(source = source.name(), %error, "Failed to retire host input source status");
        }
        info!(source = source.name(), "Retired host input source");
    }
}

/// Screen sources detached by an atomic graph commit.
#[must_use = "retired screen sources must be stopped outside the input manager lock"]
pub struct ScreenRuntimeRetirement {
    sources: Vec<ManagedInputSource>,
    source_graph_generation: u64,
}

impl ScreenRuntimeRetirement {
    /// Stop detached workers and retire their status handles.
    pub fn retire(mut self) {
        for source in &mut self.sources {
            source.set_active_consumer_count(0);
            source.stop();
            if let Err(error) = source.retire_source_status(self.source_graph_generation) {
                error!(source = source.name(), %error, "Failed to retire screen input source status");
            }
            info!(source = source.name(), "Retired screen capture source");
        }
    }
}

impl AudioRuntimeConfigPlan {
    /// Perform native device discovery and stream construction.
    ///
    /// This can block and must run without the render-owned input manager lock.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested native stream cannot be staged.
    pub fn prepare(self) -> anyhow::Result<PreparedAudioReconfiguration> {
        PreparedAudioReconfiguration::prepare(self.into_request())
    }

    /// Stage an in-memory capture runtime for deterministic transaction tests.
    #[doc(hidden)]
    pub fn prepare_with_synthetic_capture_for_testing(
        self,
    ) -> anyhow::Result<PreparedAudioReconfiguration> {
        PreparedAudioReconfiguration::prepare_with_synthetic_capture_for_testing(
            self.into_request(),
        )
    }

    fn into_request(self) -> AudioPreparationRequest {
        AudioPreparationRequest {
            expected_graph_generation: self.expected_graph_generation,
            expected_source_present: self.expected_source_present,
            expected_source_running: self.expected_source_running,
            enabled: self.enabled,
            config: self.config,
            name: self.display_name,
            capture_active: self.capture_active,
        }
    }
}

/// Orchestrates multiple [`InputSource`] instances.
///
/// Owns a heterogeneous collection of input sources and provides batch
/// lifecycle management. The render loop holds one `InputManager` and
/// calls [`sample_all`] each frame.
///
/// # Example (conceptual)
///
/// ```rust,ignore
/// let mut mgr = InputManager::new();
/// mgr.add_source(Box::new(audio_input));
/// mgr.add_source(Box::new(screen_capture));
/// mgr.start_all()?;
///
/// loop {
///     let samples = mgr.sample_all();
///     // route Audio / Screen data into the pipeline...
/// }
/// ```
pub struct InputManager {
    sources: Vec<ManagedInputSource>,
    source_graph_generation: u64,
    next_source_slot_id: u64,
    input_graph: InputGraphHandle,
    source_status_registry: SourceStatusRegistry,
    event_scratch: Vec<TimedInputEvent>,
    audio_capture_active: Option<bool>,
    macos_capability_owner: MacosCapabilityOwner,
    macos_owner_conflict: Option<MacosDaemonOwnerConflict>,
    macos_owner_designated_requirement_hash: Option<Arc<str>>,
    macos_metal4: bool,
    screen_capture_demand: Option<ScreenCaptureDemand>,
    screen_publication_demand: Option<ScreenPublicationDemandSnapshot>,
    screen_publication_source_snapshot: Vec<(u64, u64)>,
    screen_publication_resolution_revision: u64,
    committed_screen_publication_resolution_revision: Option<u64>,
    screen_plan_builder: screen::ScreenPlanBuilder,
    screen_capacity_status: screen::ScreenCapacityStatusHandle,
    screen_resource_capacity: screen::ScreenAdmissionCapacity,
    screen_total_capacity: screen::ScreenAdmissionCapacity,
    screen_publication_capacity: screen::ScreenAdmissionCapacity,
    screen_capacity_enforced: bool,
    screen_capacity_generation: u64,
    interaction_capture_active: Option<bool>,
    sensor_poller: Option<SensorPoller>,
    sensor_snapshot_rx: Option<watch::Receiver<Arc<SystemSnapshot>>>,
}

struct ManagedInputSource {
    source: Box<dyn InputSource>,
    slot: InputSourceSlot,
    compatibility_status: Option<SourceStatusReporter>,
}

impl ManagedInputSource {
    fn new(
        mut source: Box<dyn InputSource>,
        slot_id: u64,
        source_graph_generation: u64,
        screen_publication_hub: Arc<screen::ScreenPublicationHub>,
    ) -> Self {
        let declared_kind = declared_source_kind(source.as_ref());
        let interaction_origin = source
            .is_interaction_source()
            .then(|| source.interaction_source_origin());
        source.set_source_graph_generation(source_graph_generation);
        if source.is_screen_source() {
            source.set_screen_publication_hub(screen_publication_hub);
        }
        let mut compatibility_status = source.source_status_handle().is_none().then(|| {
            SourceStatusReporter::new(
                format!("compatibility:{slot_id}:{}", source.name()),
                declared_kind.unwrap_or(SourceKind::Interaction),
                "compatibility",
                true,
                true,
                true,
            )
        });
        if let Some(status) = &mut compatibility_status {
            status.set_source_graph_generation(source_graph_generation);
        }
        let status = source.source_status_handle().unwrap_or_else(|| {
            compatibility_status
                .as_ref()
                .expect("compatibility status exists for an uninstrumented source")
                .handle()
        });
        let slot = InputSourceSlot::new(slot_id, declared_kind, interaction_origin, status);
        Self {
            source,
            slot,
            compatibility_status,
        }
    }

    fn into_source(self) -> Box<dyn InputSource> {
        self.source
    }

    fn source_status_handle(&self) -> SourceStatusHandle {
        self.slot.status().clone()
    }

    fn mark_prestarted_compatibility_live(&mut self) {
        let Some(status) = &mut self.compatibility_status else {
            return;
        };
        let session = status
            .begin_session()
            .expect("validated compatibility host source can begin its session")
            .expect("manager-bound compatibility host source creates a session");
        session.mark_event_driven_live_without_deadline(1);
    }

    fn set_source_graph_generation(&mut self, source_graph_generation: u64) {
        self.source
            .set_source_graph_generation(source_graph_generation);
        if let Some(status) = &mut self.compatibility_status {
            status.set_source_graph_generation(source_graph_generation);
        }
    }

    fn set_active_consumer_count(&mut self, active_consumer_count: usize) {
        if let Err(error) = self.source.set_active_consumer_count(active_consumer_count) {
            error!(source = self.source.name(), %error, "Failed to publish active consumer count");
        }
        if let Some(status) = &mut self.compatibility_status
            && let Err(error) = status.set_active_consumer_count(active_consumer_count)
        {
            error!(source = self.source.name(), %error, "Failed to publish compatibility consumer count");
        }
    }

    fn retire_source_status(
        &mut self,
        source_graph_generation: u64,
    ) -> Result<(), SourceStatusError> {
        self.source.retire_source_status(source_graph_generation)?;
        if let Some(status) = &mut self.compatibility_status {
            status.retire(source_graph_generation)?;
        }
        Ok(())
    }

    fn start(&mut self) -> anyhow::Result<()> {
        let compatibility_session = self
            .compatibility_status
            .as_mut()
            .map(SourceStatusReporter::begin_session)
            .transpose()?
            .flatten();
        match self.source.start() {
            Ok(()) => {
                if let Some(session) = compatibility_session {
                    session.mark_event_driven_live_without_deadline(1);
                }
                Ok(())
            }
            Err(error) => {
                if let Some(session) = compatibility_session {
                    session.failed(SourceIssue::new(
                        "source_start_failed",
                        error.to_string(),
                        true,
                    ));
                }
                Err(error)
            }
        }
    }

    fn stop(&mut self) {
        self.source.stop();
        if let Some(status) = &mut self.compatibility_status {
            status.stop();
        }
    }

    fn set_compatibility_demand(&mut self, demanded: bool) -> anyhow::Result<()> {
        let Some(status) = &mut self.compatibility_status else {
            return Ok(());
        };
        status.set_policy(true, true, demanded)?;
        if demanded
            && self.source.is_running()
            && let Some(session) = status.begin_session()?
        {
            session.mark_event_driven_live_without_deadline(1);
        }
        Ok(())
    }

    fn sample_shared_and_drain_into(
        &mut self,
        delta_secs: f32,
        events: &mut Vec<TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        let sample = self.source.sample_shared_and_drain_into(delta_secs, events);
        if let Some(session) = self
            .compatibility_status
            .as_ref()
            .and_then(SourceStatusReporter::session)
        {
            match &sample {
                Ok(_) => {
                    session.mark_event_driven_live_without_deadline(1);
                }
                Err(error) => {
                    session.degraded(SourceIssue::new(
                        "source_sample_failed",
                        error.to_string(),
                        true,
                    ));
                }
            }
        }
        sample
    }
}

impl Deref for ManagedInputSource {
    type Target = dyn InputSource;

    fn deref(&self) -> &Self::Target {
        self.source.as_ref()
    }
}

impl DerefMut for ManagedInputSource {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.source.as_mut()
    }
}

impl AsRef<dyn InputSource> for ManagedInputSource {
    fn as_ref(&self) -> &(dyn InputSource + 'static) {
        self.source.as_ref()
    }
}

impl AsMut<dyn InputSource> for ManagedInputSource {
    fn as_mut(&mut self) -> &mut (dyn InputSource + 'static) {
        self.source.as_mut()
    }
}

#[derive(Clone, Copy)]
enum CaptureDomain {
    Audio,
    Interaction,
}

impl CaptureDomain {
    fn matches(self, source: &dyn InputSource) -> bool {
        match self {
            Self::Audio => source.is_audio_source(),
            Self::Interaction => source.is_interaction_source(),
        }
    }

    fn transition(self, source: &mut dyn InputSource, active: bool) -> anyhow::Result<()> {
        match self {
            Self::Audio => source.set_audio_capture_active(active),
            Self::Interaction => source.set_interaction_capture_active(active),
        }
    }
}

impl InputManager {
    /// Create an empty manager with no sources.
    #[must_use]
    pub fn new() -> Self {
        let screen_plan_builder = screen::ScreenPlanBuilder::new();
        let screen_admission = screen_plan_builder.admission_coordinator();
        let unlimited = screen::ScreenAdmissionCapacity::new(u64::MAX, u64::MAX);
        let initial_screen_capacity_policy = screen::ScreenCapacityPolicySnapshot::new(
            false,
            unlimited,
            unlimited,
            ScreenCaptureDemand::Inactive,
            None,
            None,
            None,
        );
        let screen_capacity_status = screen::ScreenCapacityStatusHandle::new(
            &initial_screen_capacity_policy,
            screen_admission,
        );
        Self {
            sources: Vec::new(),
            source_graph_generation: 0,
            next_source_slot_id: 1,
            input_graph: InputGraphHandle::new(),
            source_status_registry: SourceStatusRegistry::new(),
            event_scratch: Vec::with_capacity(INPUT_EVENT_RING_CAPACITY),
            audio_capture_active: None,
            macos_capability_owner: MacosCapabilityOwner::Standalone,
            macos_owner_conflict: None,
            macos_owner_designated_requirement_hash: None,
            macos_metal4: false,
            screen_capture_demand: None,
            screen_publication_demand: None,
            screen_publication_source_snapshot: Vec::new(),
            screen_publication_resolution_revision: 0,
            committed_screen_publication_resolution_revision: None,
            screen_plan_builder,
            screen_capacity_status,
            screen_resource_capacity: unlimited,
            screen_total_capacity: unlimited,
            screen_publication_capacity: unlimited,
            screen_capacity_enforced: false,
            screen_capacity_generation: 0,
            interaction_capture_active: None,
            sensor_poller: None,
            sensor_snapshot_rx: None,
        }
    }

    /// Register a new input source.
    ///
    /// Sources are sampled in registration order. Adding a source does not
    /// start it — call [`start_all`] or start sources individually.
    pub fn add_source(&mut self, source: Box<dyn InputSource>) {
        let domains = (
            source.is_audio_source(),
            source.is_screen_source(),
            source.is_interaction_source(),
        );
        let source_graph_generation = self.bump_source_graph_generation();
        info!(source = source.name(), "Registered input source");
        let managed = self.create_managed_source(source, source_graph_generation);
        self.sources.push(managed);
        self.invalidate_capture_domains(domains);
        self.publish_source_status_registry();
    }

    /// Replace one source without changing registration order.
    ///
    /// Returns the retired previous source, or the supplied source unchanged if
    /// `index` is outside the current graph.
    pub fn replace_source(
        &mut self,
        index: usize,
        source: Box<dyn InputSource>,
    ) -> Result<Box<dyn InputSource>, Box<dyn InputSource>> {
        if index >= self.sources.len() {
            return Err(source);
        }
        let source_graph_generation = self.bump_source_graph_generation();
        let previous_domains = (
            self.sources[index].is_audio_source(),
            self.sources[index].is_screen_source(),
            self.sources[index].is_interaction_source(),
        );
        let replacement_domains = (
            source.is_audio_source(),
            source.is_screen_source(),
            source.is_interaction_source(),
        );
        let replacement = self.create_managed_source(source, source_graph_generation);
        let mut previous = std::mem::replace(&mut self.sources[index], replacement);
        if previous_domains.1 {
            previous.set_active_consumer_count(0);
        }
        previous.stop();
        if let Err(error) = previous.retire_source_status(source_graph_generation) {
            error!(source = previous.name(), %error, "Failed to retire replaced input source status");
        }
        self.invalidate_capture_domains((
            previous_domains.0 || replacement_domains.0,
            previous_domains.1 || replacement_domains.1,
            previous_domains.2 || replacement_domains.2,
        ));
        self.publish_source_status_registry();
        Ok(previous.into_source())
    }

    /// Clone the lock-free immutable input graph retained by render consumers.
    #[must_use]
    pub fn input_graph_handle(&self) -> InputGraphHandle {
        self.input_graph.clone()
    }

    /// Clone the lock-free source-status registry retained outside the manager.
    #[must_use]
    pub fn source_status_registry(&self) -> SourceStatusRegistry {
        self.source_status_registry.clone()
    }

    /// Current canonical input graph generation.
    #[must_use]
    pub fn source_graph_generation(&self) -> u64 {
        self.source_graph_generation
    }

    /// Attach a background system-sensor poller to this input graph.
    pub fn set_sensor_poller(&mut self, poller: SensorPoller) {
        self.set_sensor_snapshot_receiver(poller.receiver());
        self.sensor_poller = Some(poller);
    }

    /// Attach a latest-value sensor stream to this input graph.
    pub fn set_sensor_snapshot_receiver(&mut self, receiver: watch::Receiver<Arc<SystemSnapshot>>) {
        self.sensor_snapshot_rx = Some(receiver);
    }

    /// Clone the configured latest-value sensor receiver, if one exists.
    #[must_use]
    pub fn sensor_snapshot_receiver(&self) -> Option<watch::Receiver<Arc<SystemSnapshot>>> {
        self.sensor_snapshot_rx.as_ref().cloned()
    }

    /// Number of registered input sources.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Snapshot of source names in registration order.
    #[must_use]
    pub fn source_names(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|source| source.name().to_owned())
            .collect()
    }

    /// Sample each source into its stable latest-value publication.
    ///
    /// Manager-owned route and event scratch storage is retained across calls;
    /// graph consumers perform typed aggregation after releasing the manager.
    pub fn sample_sources(&mut self, delta_secs: f32) {
        for source in &mut self.sources {
            sample_managed_source(source, delta_secs, &mut self.event_scratch);
        }
    }

    /// Sample only source kinds whose independent publication deadlines are due.
    ///
    /// Each entry carries the elapsed time for that source kind. The caller must
    /// provide at most one entry per kind; an empty slice samples nothing.
    pub fn sample_source_kinds(&mut self, due_sources: &[(SourceKind, f32)]) {
        debug_assert!(due_sources.iter().enumerate().all(|(index, (kind, _))| {
            !due_sources[..index]
                .iter()
                .any(|(previous, _)| previous == kind)
        }));
        for source in &mut self.sources {
            let source_kind = source.slot.kind().unwrap_or(SourceKind::Interaction);
            let Some((_, delta_secs)) = due_sources.iter().find(|(kind, _)| *kind == source_kind)
            else {
                continue;
            };
            sample_managed_source(source, *delta_secs, &mut self.event_scratch);
        }
    }

    /// Sample every registered source, collecting one [`InputData`] per source.
    ///
    /// Sources that fail to sample emit a warning and produce [`InputData::None`]
    /// for that frame — a single broken source never crashes the render loop.
    pub fn sample_all(&mut self) -> Vec<InputData> {
        self.sample_all_with_delta_secs(0.0)
    }

    /// Sample every registered source using the current frame delta.
    ///
    /// Sources that ignore cadence can rely on the default trait behavior; the
    /// audio pipeline uses this to keep analysis state aligned with real frame
    /// timing when the render loop shifts tiers or misses budget.
    pub fn sample_all_with_delta_secs(&mut self, delta_secs: f32) -> Vec<InputData> {
        let mut samples = self
            .sources
            .iter_mut()
            .map(|source| {
                source
                    .sample_with_delta_secs(delta_secs)
                    .unwrap_or_else(|err| {
                        error!(source = source.name(), %err, "Input sample failed");
                        InputData::None
                    })
            })
            .collect::<Vec<_>>();

        if let Some(snapshot) = self.latest_sensor_snapshot() {
            samples.push(InputData::Sensors(snapshot));
        }

        samples
    }

    /// Drain discrete input events from every registered source.
    #[must_use]
    pub fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        self.sources
            .iter_mut()
            .filter(|source| source.is_running())
            .flat_map(|source| source.drain_events())
            .collect()
    }

    /// Sample every source and drain its events in one pass.
    ///
    /// Each source's snapshot and events come from one internal lock
    /// acquisition, so a frame can never carry an event edge whose held
    /// state is missing from the same frame's snapshot.
    pub fn sample_and_drain_with_delta_secs(
        &mut self,
        delta_secs: f32,
    ) -> (Vec<InputData>, Vec<TimedInputEvent>) {
        let mut samples = Vec::with_capacity(self.sources.len() + 1);
        let mut events = Vec::new();
        for source in &mut self.sources {
            let (sample, mut source_events) = source.sample_and_drain_with_delta_secs(delta_secs);
            samples.push(sample.unwrap_or_else(|err| {
                error!(source = source.name(), %err, "Input sample failed");
                InputData::None
            }));
            events.append(&mut source_events);
        }

        if let Some(snapshot) = self.latest_sensor_snapshot() {
            samples.push(InputData::Sensors(snapshot));
        }

        (samples, events)
    }

    /// Toggle live host-input capture for any registered interaction sources.
    ///
    /// Mirrors the audio/screen demand model: sources stay registered but
    /// close their device handles and clear held state while inactive.
    ///
    /// # Errors
    ///
    /// Returns an error if an interaction source cannot update its capture state.
    pub fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.transition_capture_demand(CaptureDomain::Interaction, active)
    }

    /// Start all registered sources.
    ///
    /// Iterates in registration order. If any source fails to start, previously
    /// started sources are stopped and the first error is returned.
    ///
    /// # Errors
    ///
    /// Returns the first error encountered during startup.
    pub fn start_all(&mut self) -> anyhow::Result<()> {
        self.invalidate_capture_domains((true, true, true));
        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            source.set_source_graph_generation(source_graph_generation);
        }
        self.publish_source_status_registry();

        if let Some(sensor_poller) = self.sensor_poller.as_mut() {
            sensor_poller.start()?;
        }

        for source_index in 0..self.sources.len() {
            let start_result = self.sources[source_index].start();
            if let Err(err) = start_result {
                let source = &mut self.sources[source_index];
                error!(source = source.name(), %err, "Failed to start input source");
                if let Some(status) = source
                    .source_status_reporter()
                    .and_then(|status| status.session())
                {
                    status.failed(SourceIssue::new(
                        "source_start_failed",
                        err.to_string(),
                        true,
                    ));
                }
                for started in self.sources[..=source_index].iter_mut().rev() {
                    started.stop();
                }
                if let Some(sensor_poller) = self.sensor_poller.as_mut() {
                    sensor_poller.stop();
                }
                self.invalidate_capture_domains((true, true, true));
                self.publish_screen_capacity_status();
                return Err(err);
            }
            info!(
                source = self.sources[source_index].name(),
                "Started input source"
            );
        }
        self.publish_screen_capacity_status();
        Ok(())
    }

    /// Stop all registered sources. Never fails — errors are logged and swallowed.
    pub fn stop_all(&mut self) {
        for source in &mut self.sources {
            info!(source = source.name(), "Stopping input source");
            source.stop();
        }
        if let Some(sensor_poller) = self.sensor_poller.as_mut() {
            sensor_poller.stop();
        }
        self.invalidate_capture_domains((true, true, true));
        self.publish_screen_capacity_status();
    }

    /// Snapshot a generation-fenced audio reconfiguration plan.
    ///
    /// The returned plan owns all data needed for native preparation after the
    /// manager lock is released.
    ///
    /// # Errors
    ///
    /// Returns an error if the registered audio source cannot accept a staged
    /// native runtime.
    pub fn plan_audio_runtime_config(
        &self,
        enabled: bool,
        config: &AudioPipelineConfig,
        display_name: &str,
        capture_active: bool,
    ) -> anyhow::Result<AudioRuntimeConfigPlan> {
        let source = self.sources.iter().find(|source| source.is_audio_source());
        if source.is_some_and(|source| !source.supports_prepared_audio_reconfiguration()) {
            anyhow::bail!("registered audio source does not support prepared reconfiguration");
        }
        let mut effective_config = config.clone();
        if !enabled {
            effective_config.source = crate::types::audio::AudioSourceType::None;
        }
        let capture_active = enabled
            && capture_active
            && !matches!(
                effective_config.source,
                crate::types::audio::AudioSourceType::None
            );
        Ok(AudioRuntimeConfigPlan {
            expected_graph_generation: self.source_graph_generation,
            expected_source_present: source.is_some(),
            expected_source_running: source.is_some_and(|source| source.is_running()),
            enabled,
            config: effective_config,
            display_name: display_name.to_owned(),
            capture_active,
        })
    }

    /// Commit a prepared audio runtime if the input graph is unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when a concurrent graph or lifecycle transition made
    /// the prepared runtime stale, or when the source rejects the commit.
    pub fn commit_audio_runtime_config(
        &mut self,
        prepared: &mut PreparedAudioReconfiguration,
    ) -> anyhow::Result<AudioRuntimeRetirement> {
        if self.source_graph_generation != prepared.expected_graph_generation {
            return Err(AudioReconfigurationConflict::GraphChanged.into());
        }
        prepared.ensure_ready()?;
        let source_index = self
            .sources
            .iter()
            .position(|source| source.is_audio_source());
        if source_index.is_some() != prepared.expected_source_present {
            return Err(AudioReconfigurationConflict::SourceTopologyChanged.into());
        }
        if let Some(index) = source_index {
            if self.sources[index].is_running() != prepared.expected_source_running {
                return Err(AudioReconfigurationConflict::SourceLifecycleChanged.into());
            }
            let capture_active = prepared.capture_active;
            let source_graph_generation = self.bump_source_graph_generation();
            let result = {
                let source = &mut self.sources[index];
                source.set_source_graph_generation(source_graph_generation);
                source.commit_prepared_audio_reconfiguration(prepared)
            };
            if result.is_ok() {
                self.audio_capture_active = Some(capture_active);
                info!(
                    source = self.sources[index].name(),
                    capture_active, "Committed prepared live audio input source"
                );
            }
            self.publish_source_status_registry();
            return result;
        }

        if !prepared.enabled {
            self.bump_source_graph_generation();
            self.audio_capture_active = Some(false);
            return Ok(AudioRuntimeRetirement::empty());
        }

        let capture_active = prepared.capture_active;
        let source_graph_generation = self
            .source_graph_generation
            .checked_add(1)
            .expect("input source graph generation exhausted");
        let audio_input = AudioInput::from_prepared(prepared, source_graph_generation)?;
        self.source_graph_generation = source_graph_generation;
        let managed = self.create_managed_source(Box::new(audio_input), source_graph_generation);
        self.sources.push(managed);
        self.audio_capture_active = Some(capture_active);
        self.publish_source_status_registry();
        info!(
            source = self
                .sources
                .last()
                .expect("audio source was just registered")
                .name(),
            capture_active, "Added prepared live audio input source"
        );
        Ok(AudioRuntimeRetirement::empty())
    }

    /// Apply a live audio config change without rebuilding unrelated sources.
    ///
    /// If an audio source already exists, it is reconfigured in place. If audio
    /// is being enabled and no audio source exists yet, one is created and
    /// started. Disabling audio reconfigures the existing source to silence.
    /// Native preparation may enumerate devices and block. Callers that share
    /// the manager across latency-sensitive work should use
    /// [`Self::plan_audio_runtime_config`], prepare after releasing the manager,
    /// then reacquire it for [`Self::commit_audio_runtime_config`].
    ///
    /// # Errors
    ///
    /// Returns an error if the live audio source switch fails.
    pub fn apply_audio_runtime_config(
        &mut self,
        enabled: bool,
        config: &AudioPipelineConfig,
        display_name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        let audio_source = self.sources.iter().find(|source| source.is_audio_source());
        if audio_source.is_none_or(|source| source.supports_prepared_audio_reconfiguration()) {
            let mut prepared = self
                .plan_audio_runtime_config(enabled, config, display_name, capture_active)?
                .prepare()?;
            let retirement = self.commit_audio_runtime_config(&mut prepared)?;
            retirement.retire();
            return Ok(());
        }

        let effective_capture_active = enabled && capture_active;
        let effective_config = if enabled {
            config.clone()
        } else {
            let mut disabled = config.clone();
            disabled.source = crate::types::audio::AudioSourceType::None;
            disabled
        };

        let index = self
            .sources
            .iter()
            .position(|source| source.is_audio_source())
            .expect("unsupported prepared reconfiguration requires an audio source");
        let source_graph_generation = self.bump_source_graph_generation();
        let result = {
            let source = &mut self.sources[index];
            source.set_source_graph_generation(source_graph_generation);
            source.reconfigure_audio(&effective_config, display_name, effective_capture_active)
        };
        if result.is_ok() {
            info!(
                source = display_name,
                enabled,
                capture_active = effective_capture_active,
                "Reconfigured compatibility audio input source"
            );
            self.audio_capture_active = Some(effective_capture_active);
        }
        self.publish_source_status_registry();
        result
    }

    /// Toggle live audio capture for any registered audio sources.
    ///
    /// This keeps the input graph intact while allowing the audio backend to
    /// pause or resume hardware capture based on current render demand.
    ///
    /// # Errors
    ///
    /// Returns an error if an audio source cannot update its capture state.
    pub fn set_audio_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.transition_capture_demand(CaptureDomain::Audio, active)
    }

    /// Apply live screen publication demand to every registered screen source.
    ///
    /// This keeps the input graph intact while allowing the capture backend to
    /// pause or resume compositor capture based on current render demand.
    ///
    /// # Errors
    ///
    /// Returns an error if a screen source cannot update its capture state.
    pub fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        self.transition_screen_capture_demand(demand)
    }

    /// Stable lock-free publication authority shared across screen-source replacement.
    #[must_use]
    pub fn screen_publication_hub(&self) -> Arc<screen::ScreenPublicationHub> {
        self.screen_plan_builder.publication_hub()
    }

    /// Shared byte admission authority for preconstructed screen resources.
    #[must_use]
    pub fn screen_admission_coordinator(&self) -> screen::ScreenByteAdmissionCoordinator {
        self.screen_plan_builder.admission_coordinator()
    }

    /// Clone the lock-free exact screen capacity status handle.
    #[must_use]
    pub fn screen_capacity_status_handle(&self) -> screen::ScreenCapacityStatusHandle {
        self.screen_capacity_status.clone()
    }

    /// Construct compatibility screen analysis inside this manager's byte fence.
    pub fn prepare_screen_capture_input(
        &self,
        config: screen::CaptureConfig,
        requested_extent: screen::PixelExtent,
    ) -> Result<screen::ScreenCaptureInput, screen::ScreenAnalysisAdmissionError> {
        screen::ScreenCaptureInput::with_requested_extent_and_admission(
            config,
            requested_extent,
            self.screen_admission_coordinator(),
        )
    }

    /// Set the process and backend byte fences shared by all screen resources.
    pub fn set_screen_resource_capacity(
        &mut self,
        capacity: screen::ScreenAdmissionCapacity,
    ) -> Result<(), screen::ScreenByteAdmissionError> {
        let policy = self.screen_capacity_policy_snapshot();
        let transition = self.screen_capacity_status.begin_transition();
        self.set_screen_resource_capacity_unpublished(capacity)?;
        transition.publish(&policy);
        Ok(())
    }

    /// Install the physical transition fence, configured steady total, and
    /// exact initial publication remainder.
    pub fn set_screen_capacity_plan(
        &mut self,
        resource: screen::ScreenAdmissionCapacity,
        total: screen::ScreenAdmissionCapacity,
        publication: screen::ScreenAdmissionCapacity,
    ) -> Result<(), screen::ScreenByteAdmissionError> {
        let policy = self.screen_capacity_policy_snapshot_with(true, total, publication);
        let transition = self.screen_capacity_status.begin_transition();
        self.set_screen_resource_capacity_unpublished(resource)?;
        self.screen_total_capacity = total;
        self.screen_publication_capacity = publication;
        self.screen_capacity_enforced = true;
        self.screen_capacity_generation = self
            .screen_capacity_generation
            .checked_add(1)
            .expect("screen capacity policy generation exhausted");
        transition.publish(&policy);
        Ok(())
    }

    fn set_screen_resource_capacity_unpublished(
        &mut self,
        capacity: screen::ScreenAdmissionCapacity,
    ) -> Result<(), screen::ScreenByteAdmissionError> {
        self.screen_plan_builder
            .admission_coordinator()
            .try_set_capacity(capacity)?;
        self.screen_resource_capacity = capacity;
        Ok(())
    }

    /// Return the total byte fences shared by analysis and publication.
    #[must_use]
    pub const fn screen_resource_capacity(&self) -> screen::ScreenAdmissionCapacity {
        self.screen_resource_capacity
    }

    /// Return the configured steady-state capacity shared by analysis and publication.
    #[must_use]
    pub const fn screen_total_capacity(&self) -> screen::ScreenAdmissionCapacity {
        self.screen_total_capacity
    }

    /// Return the byte fences installed for screen publication admission.
    #[must_use]
    pub const fn screen_publication_capacity(&self) -> screen::ScreenAdmissionCapacity {
        self.screen_publication_capacity
    }

    /// Return the exact analysis plan for the currently installed screen source.
    ///
    /// # Errors
    ///
    /// Returns an error when the source cannot represent its current demand.
    pub fn screen_analysis_resource_plan(
        &self,
    ) -> anyhow::Result<Option<screen::ScreenAnalysisResourcePlan>> {
        let Some(source) = self.sources.iter().find(|source| source.is_screen_source()) else {
            return Ok(None);
        };
        source.screen_analysis_resource_plan(self.current_screen_capture_demand())
    }

    /// Return the complete compatibility-analysis workload for current demand.
    pub fn screen_analysis_work_plan(
        &self,
    ) -> anyhow::Result<Option<screen::ScreenAnalysisWorkPlan>> {
        let Some(source) = self.sources.iter().find(|source| source.is_screen_source()) else {
            return Ok(None);
        };
        source.screen_analysis_work_plan(self.current_screen_capture_demand())
    }

    /// Return calibrated compatibility-analysis capacity, when configured.
    #[must_use]
    pub fn screen_analysis_compute_capacity(
        &self,
    ) -> Option<screen::ScreenAnalysisComputeCapacity> {
        self.sources
            .iter()
            .find(|source| source.is_screen_source())
            .and_then(|source| source.screen_analysis_compute_capacity())
    }

    /// Return the manager's authoritative current screen-capture demand.
    #[must_use]
    pub fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.current_screen_capture_demand()
    }

    /// Prepare an exact publication remainder for one candidate analysis peak.
    ///
    /// The shared byte coordinator remains the physical overlap authority.
    /// This preparation separately proves that the candidate steady state fits
    /// the configured total and snapshots every manager-owned commit fence.
    ///
    /// # Errors
    ///
    /// Returns a typed analysis or active-publication capacity rejection.
    pub fn prepare_screen_capacity(
        &self,
        analysis_peak_bytes: u64,
    ) -> Result<Option<ScreenCapacityPreparation>, ScreenCapacityPreparationError> {
        self.prepare_screen_capacity_plan(self.screen_total_capacity, analysis_peak_bytes)
    }

    /// Prepare a replacement steady-state total and exact analysis split.
    ///
    /// # Errors
    ///
    /// Returns a typed analysis or active-publication capacity rejection.
    pub fn prepare_screen_capacity_plan(
        &self,
        total_capacity: screen::ScreenAdmissionCapacity,
        analysis_peak_bytes: u64,
    ) -> Result<Option<ScreenCapacityPreparation>, ScreenCapacityPreparationError> {
        if !self.screen_capacity_enforced {
            return Ok(None);
        }
        let available_bytes = total_capacity
            .byte_budget()
            .min(total_capacity.backend_capacity());
        let publication_capacity = screen::ScreenAdmissionCapacity::new(
            total_capacity
                .byte_budget()
                .checked_sub(analysis_peak_bytes)
                .ok_or(ScreenCapacityPreparationError::AnalysisCapacityExceeded {
                    requested_bytes: analysis_peak_bytes,
                    available_bytes,
                })?,
            total_capacity
                .backend_capacity()
                .checked_sub(analysis_peak_bytes)
                .ok_or(ScreenCapacityPreparationError::AnalysisCapacityExceeded {
                    requested_bytes: analysis_peak_bytes,
                    available_bytes,
                })?,
        );
        self.screen_plan_builder
            .validate_capacity(publication_capacity)?;
        let plan = self.screen_plan_builder.current();
        let resource_snapshot = self.screen_plan_builder.admission_coordinator().snapshot();
        Ok(Some(ScreenCapacityPreparation {
            expected_graph_generation: self.source_graph_generation,
            expected_capture_demand: self.screen_capture_demand,
            expected_plan_generation: plan.generation(),
            expected_demand_revision: plan.demand_revision(),
            expected_resource_capacity_revision: resource_snapshot.capacity_revision(),
            expected_capacity_generation: self.screen_capacity_generation,
            expected_total_capacity: self.screen_total_capacity,
            total_capacity,
            publication_capacity,
            analysis_peak_bytes,
        }))
    }

    /// Verify that an exact capacity preparation still describes this manager.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict when demand, publication, or capacity advanced.
    pub fn validate_screen_capacity(
        &self,
        preparation: &ScreenCapacityPreparation,
    ) -> Result<(), ScreenReconfigurationConflict> {
        if self.source_graph_generation != preparation.expected_graph_generation {
            return Err(ScreenReconfigurationConflict::GraphChanged);
        }
        if self.screen_capture_demand != preparation.expected_capture_demand {
            return Err(ScreenReconfigurationConflict::CaptureDemandChanged);
        }
        let plan = self.screen_plan_builder.current();
        if plan.generation() != preparation.expected_plan_generation
            || plan.demand_revision() != preparation.expected_demand_revision
        {
            return Err(ScreenReconfigurationConflict::PublicationStateChanged);
        }
        let resource_snapshot = self.screen_plan_builder.admission_coordinator().snapshot();
        if resource_snapshot.capacity_revision() != preparation.expected_resource_capacity_revision
            || self.screen_total_capacity != preparation.expected_total_capacity
        {
            return Err(ScreenReconfigurationConflict::ResourceCapacityChanged);
        }
        if self.screen_capacity_generation != preparation.expected_capacity_generation {
            return Err(ScreenReconfigurationConflict::CapacityPolicyChanged);
        }
        Ok(())
    }

    /// Commit a previously validated exact publication remainder.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict if any preparation fence advanced.
    pub fn commit_screen_capacity(
        &mut self,
        preparation: ScreenCapacityPreparation,
    ) -> Result<(), ScreenReconfigurationConflict> {
        self.validate_screen_capacity(&preparation)?;
        self.commit_screen_capacity_unpublished(preparation);
        self.publish_screen_capacity_status();
        Ok(())
    }

    fn commit_screen_capacity_unpublished(&mut self, preparation: ScreenCapacityPreparation) {
        self.screen_total_capacity = preparation.total_capacity;
        self.screen_publication_capacity = preparation.publication_capacity;
        self.screen_capacity_generation = self
            .screen_capacity_generation
            .checked_add(1)
            .expect("screen capacity policy generation exhausted");
    }

    fn current_screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.screen_capture_demand.unwrap_or_else(|| {
            self.sources
                .iter()
                .find(|source| source.is_screen_source())
                .map_or(ScreenCaptureDemand::Inactive, |source| {
                    source.screen_capture_demand()
                })
        })
    }

    /// Refresh the process-wide revision of exact screen source metadata.
    ///
    /// The returned revision changes only when a screen source is replaced or
    /// advances its own publication-resolution revision. Comparing the full
    /// ordered source snapshot avoids lossy hashing while structural graph
    /// generation fences make source order and slot identity stable.
    #[must_use]
    pub fn screen_publication_resolution_revision(&mut self) -> u64 {
        let source_count = self
            .sources
            .iter()
            .filter(|source| source.is_screen_source())
            .count();
        let changed = source_count != self.screen_publication_source_snapshot.len()
            || self
                .sources
                .iter()
                .filter(|source| source.is_screen_source())
                .zip(&self.screen_publication_source_snapshot)
                .any(|(source, observed)| {
                    *observed
                        != (
                            source.slot.id(),
                            source.screen_publication_resolution_revision(),
                        )
                });
        if changed {
            self.screen_publication_source_snapshot.clear();
            self.screen_publication_source_snapshot
                .reserve(source_count);
            self.screen_publication_source_snapshot.extend(
                self.sources
                    .iter()
                    .filter(|source| source.is_screen_source())
                    .map(|source| {
                        (
                            source.slot.id(),
                            source.screen_publication_resolution_revision(),
                        )
                    }),
            );
            self.screen_publication_resolution_revision = self
                .screen_publication_resolution_revision
                .checked_add(1)
                .expect("screen publication resolution revision exhausted");
        }
        self.screen_publication_resolution_revision
    }

    /// Resolve and start one exact screen-plan preparation transaction.
    ///
    /// Source methods may only enqueue worker-owned work here. Awaiting real
    /// allocation or backend negotiation happens through the returned handle
    /// after the caller releases the input-manager lock.
    ///
    /// # Errors
    ///
    /// Rejects stale graph snapshots, unresolved or multiply-owned branches,
    /// plan admission failures, and sources that cannot enqueue their ticket.
    pub fn begin_screen_publication_transition(
        &mut self,
        demand: ScreenPublicationDemandSnapshot,
    ) -> Result<
        Option<screen::ScreenPublicationPreparation>,
        screen::ScreenPublicationTransitionError,
    > {
        let observed_graph = screen::ScreenInputGraphGeneration::new(self.source_graph_generation);
        if demand.graph_generation() != observed_graph {
            return Err(
                screen::ScreenPublicationTransitionError::GraphGenerationConflict {
                    expected: demand.graph_generation(),
                    observed: observed_graph,
                },
            );
        }
        let source_resolution_revision = self.screen_publication_resolution_revision();
        if self.screen_publication_demand.as_ref() == Some(&demand)
            && self.committed_screen_publication_resolution_revision
                == Some(source_resolution_revision)
        {
            return Ok(None);
        }

        let mut resolved = Vec::new();
        resolved
            .try_reserve_exact(demand.branches().len())
            .map_err(|_| screen::ScreenPlanError::AllocationFailed)?;
        let mut owners: Vec<(screen::CaptureSourceId, usize, usize)> = Vec::new();
        for (branch_index, branch) in demand.branches().iter().enumerate() {
            let mut resolution = None;
            for (source_index, source) in self.sources.iter().enumerate() {
                if !source.is_screen_source() {
                    continue;
                }
                let candidate =
                    source
                        .resolve_screen_publication_branch(branch)
                        .map_err(|error| {
                            screen::ScreenPublicationTransitionError::SourceResolutionFailed {
                                source_name: Arc::from(source.name()),
                                branch_index,
                                message: Arc::from(error.to_string()),
                            }
                        })?;
                let Some(candidate) = candidate else {
                    continue;
                };
                if resolution.is_some() {
                    return Err(screen::ScreenPublicationTransitionError::AmbiguousBranch {
                        branch_index,
                    });
                }
                resolution = Some((source_index, candidate));
            }
            let Some((source_index, branch)) = resolution else {
                return Err(screen::ScreenPublicationTransitionError::UnresolvedBranch {
                    branch_index,
                });
            };
            let source_id = branch.descriptor().source_epoch().source_id.clone();
            if let Some((_, owner, active_consumer_count)) = owners
                .iter_mut()
                .find(|(candidate, _, _)| *candidate == source_id)
            {
                if *owner != source_index {
                    return Err(
                        screen::ScreenPublicationTransitionError::SourceOwnershipConflict {
                            source_id,
                        },
                    );
                }
                *active_consumer_count += 1;
            } else {
                owners
                    .try_reserve(1)
                    .map_err(|_| screen::ScreenPlanError::AllocationFailed)?;
                owners.push((source_id, source_index, 1));
            }
            resolved.push(branch);
        }

        let mut active_consumer_counts = Vec::new();
        active_consumer_counts
            .try_reserve_exact(owners.len())
            .map_err(|_| screen::ScreenPlanError::AllocationFailed)?;
        active_consumer_counts.extend(
            owners
                .iter()
                .map(|(source_id, _, count)| (source_id.clone(), *count)),
        );

        let compatibility_surface =
            resolved_compatibility_descriptor(&demand, &resolved, demand.compatibility_surface());
        let compatibility_zones =
            resolved_compatibility_descriptor(&demand, &resolved, demand.compatibility_zones());
        let compatibility = compatibility_surface
            .map(|surface| {
                screen::ScreenCompatibilitySelection::try_new(surface, compatibility_zones)
            })
            .transpose()?;
        let mut preparing = self.screen_plan_builder.prepare(
            resolved,
            compatibility.as_ref(),
            demand.revision(),
            demand.graph_generation(),
            self.screen_publication_capacity(),
        )?;

        let required_sources = preparing.required_sources().to_vec();
        let mut workers = Vec::new();
        if workers.try_reserve_exact(required_sources.len()).is_err() {
            drop(preparing.abort());
            return Err(screen::ScreenPlanError::AllocationFailed.into());
        }
        for source_id in required_sources {
            let ticket = match preparing.worker_ticket(&source_id) {
                Ok(ticket) => ticket,
                Err(error) => {
                    drop(preparing.abort());
                    return Err(error.into());
                }
            };
            let owner = owners
                .iter()
                .find(|(candidate, _, _)| candidate == &source_id)
                .map(|(_, source_index, _)| *source_index)
                .or_else(|| {
                    self.sources.iter().position(|source| {
                        source.is_screen_source()
                            && source.owns_screen_publication_source(&source_id)
                    })
                });
            let preparation = if let Some(owner) = owner {
                match self.sources[owner].begin_screen_publication_preparation(ticket) {
                    Ok(preparation) => preparation,
                    Err(error) => {
                        drop(preparing.abort());
                        return Err(
                            screen::ScreenPublicationTransitionError::WorkerPreparationStartFailed {
                                source_id,
                                message: Arc::from(error.to_string()),
                            },
                        );
                    }
                }
            } else if ticket.source_delta().added_branches().is_empty()
                && ticket.source_delta().retained_branches().is_empty()
            {
                let ledger = match screen::ScreenExactResourceLedger::try_new([]) {
                    Ok(ledger) => ledger,
                    Err(error) => {
                        drop(preparing.abort());
                        return Err(error.into());
                    }
                };
                let token = match ticket.acknowledge(ledger, &[]) {
                    Ok(token) => token,
                    Err(error) => {
                        drop(preparing.abort());
                        return Err(error.into());
                    }
                };
                screen::ScreenWorkerPreparation::new(async move { Ok(token) })
            } else {
                drop(preparing.abort());
                return Err(
                    screen::ScreenPublicationTransitionError::WorkerOwnerMissing { source_id },
                );
            };
            workers.push(screen::PendingScreenWorkerPreparation {
                source_id,
                preparation,
            });
        }

        Ok(Some(screen::ScreenPublicationPreparation::new(
            preparing,
            workers,
            demand,
            source_resolution_revision,
            active_consumer_counts,
        )))
    }

    /// Arm and atomically commit one fully acknowledged exact screen plan.
    ///
    /// The caller supplies the latest demand revision after reacquiring the
    /// manager lock. Structural graph generation comes directly from the
    /// manager, so both fences are rechecked at arm and commit.
    ///
    /// # Errors
    ///
    /// Returns an explicit abort receipt after any fence or commit conflict.
    pub fn commit_screen_publication_transition(
        &mut self,
        mut prepared: screen::PreparedScreenPublicationPlan,
        observed_demand_revision: screen::InputPublicationDemandRevision,
    ) -> Result<
        screen::CommittedScreenPublicationTransition,
        screen::ScreenPublicationTransitionFailure,
    > {
        let demand = prepared.demand().clone();
        let active_consumer_counts = prepared.active_consumer_counts().to_vec();
        let expected_source_resolution_revision = prepared.source_resolution_revision();
        let observed_source_resolution_revision = self.screen_publication_resolution_revision();
        if expected_source_resolution_revision != observed_source_resolution_revision {
            let abort = prepared.take_preparing().abort();
            return Err(screen::ScreenPublicationTransitionFailure::new(
                screen::ScreenPublicationTransitionError::SourceResolutionRevisionConflict {
                    expected: expected_source_resolution_revision,
                    observed: observed_source_resolution_revision,
                },
                abort,
            ));
        }
        let preparing = prepared.take_preparing();
        let graph_generation =
            screen::ScreenInputGraphGeneration::new(self.source_graph_generation);
        let armed = preparing
            .arm(
                self.screen_plan_builder.current().generation(),
                observed_demand_revision,
                graph_generation,
            )
            .map_err(|failure| {
                let error = failure.error().clone();
                screen::ScreenPublicationTransitionFailure::new(
                    screen::ScreenPublicationTransitionError::Plan(error),
                    failure.into_preparing().abort(),
                )
            })?;
        let committed = self
            .screen_plan_builder
            .commit(armed, observed_demand_revision, graph_generation)
            .map_err(|failure| {
                let error = failure.error().clone();
                screen::ScreenPublicationTransitionFailure::new(
                    screen::ScreenPublicationTransitionError::Plan(error),
                    failure.into_armed().abort(),
                )
            })?;
        prepared.disarm_worker_aborts();
        self.set_screen_publication_active_consumer_counts(&active_consumer_counts);
        self.screen_publication_demand = Some(demand);
        self.committed_screen_publication_resolution_revision =
            Some(observed_source_resolution_revision);
        let worker_retirements = self
            .sources
            .iter_mut()
            .filter(|source| source.is_screen_source())
            .filter_map(|source| {
                source
                    .begin_screen_publication_retirement()
                    .map(|retirement| (Arc::from(source.name()), retirement))
            })
            .collect();
        Ok(screen::CommittedScreenPublicationTransition::new(
            committed,
            worker_retirements,
        ))
    }

    /// Whether any registered source handles screen capture.
    #[must_use]
    pub fn has_screen_source(&self) -> bool {
        self.sources.iter().any(|source| source.is_screen_source())
    }

    /// Snapshot a generation-fenced screen-source replacement plan.
    pub fn plan_screen_runtime_config(&self, enabled: bool) -> ScreenRuntimeConfigPlan {
        let source = self.sources.iter().find(|source| source.is_screen_source());
        let current_demand = self.current_screen_capture_demand();
        ScreenRuntimeConfigPlan {
            expected_graph_generation: self.source_graph_generation,
            expected_source_present: source.is_some(),
            expected_source_running: source.is_some_and(|source| source.is_running()),
            expected_capture_demand: current_demand,
            enabled,
            capture_demand: if enabled {
                current_demand
            } else {
                ScreenCaptureDemand::Inactive
            },
        }
    }

    /// Atomically install a prepared screen source if the input graph is unchanged.
    ///
    /// `replacement` remains owned by the caller on error so dropping or stopping
    /// a prepared backend never occurs while the input manager lock is held.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict when graph topology or lifecycle changed during
    /// preparation, or the replacement does not match the plan.
    pub fn commit_screen_runtime_config(
        &mut self,
        plan: &ScreenRuntimeConfigPlan,
        replacement: &mut Option<Box<dyn InputSource>>,
    ) -> Result<ScreenRuntimeRetirement, ScreenReconfigurationConflict> {
        self.validate_screen_runtime_config(plan, replacement)?;
        let retirement = self.commit_screen_runtime_config_unpublished(plan, replacement);
        self.publish_source_status_registry();
        Ok(retirement)
    }

    /// Atomically install a prepared screen capacity policy and source runtime.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict before either prepared transaction mutates the
    /// manager when a capacity or runtime fence has advanced.
    pub fn commit_screen_capacity_and_runtime_config(
        &mut self,
        capacity: ScreenCapacityPreparation,
        plan: &ScreenRuntimeConfigPlan,
        replacement: &mut Option<Box<dyn InputSource>>,
    ) -> Result<ScreenRuntimeRetirement, ScreenReconfigurationConflict> {
        self.validate_screen_runtime_config(plan, replacement)?;
        self.validate_screen_capacity(&capacity)?;
        self.commit_screen_capacity_unpublished(capacity);
        let retirement = self.commit_screen_runtime_config_unpublished(plan, replacement);
        self.publish_source_status_registry();
        Ok(retirement)
    }

    fn commit_screen_runtime_config_unpublished(
        &mut self,
        plan: &ScreenRuntimeConfigPlan,
        replacement: &mut Option<Box<dyn InputSource>>,
    ) -> ScreenRuntimeRetirement {
        let source_index = self
            .sources
            .iter()
            .position(|source| source.is_screen_source());
        let topology_changed = source_index.is_some() || replacement.is_some();
        let source_graph_generation = if topology_changed {
            self.bump_source_graph_generation()
        } else {
            self.source_graph_generation
        };
        let mut retired = Vec::with_capacity(usize::from(source_index.is_some()));
        match (source_index, replacement.take()) {
            (Some(index), Some(source)) => {
                let replacement = self.create_managed_source(source, source_graph_generation);
                retired.push(std::mem::replace(&mut self.sources[index], replacement));
            }
            (Some(index), None) => retired.push(self.sources.remove(index)),
            (None, Some(source)) => {
                let replacement = self.create_managed_source(source, source_graph_generation);
                self.sources.push(replacement);
            }
            (None, None) => {}
        }
        if topology_changed {
            self.invalidate_capture_domains((false, true, false));
        }
        self.screen_capture_demand = Some(plan.capture_demand);
        ScreenRuntimeRetirement {
            sources: retired,
            source_graph_generation,
        }
    }

    /// Verify that a prepared screen replacement can still commit.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict when the source graph changed during preparation.
    pub fn validate_screen_runtime_config(
        &self,
        plan: &ScreenRuntimeConfigPlan,
        replacement: &Option<Box<dyn InputSource>>,
    ) -> Result<(), ScreenReconfigurationConflict> {
        if self.source_graph_generation != plan.expected_graph_generation {
            return Err(ScreenReconfigurationConflict::GraphChanged);
        }
        let source_index = self
            .sources
            .iter()
            .position(|source| source.is_screen_source());
        if source_index.is_some() != plan.expected_source_present {
            return Err(ScreenReconfigurationConflict::SourceTopologyChanged);
        }
        if source_index
            .is_some_and(|index| self.sources[index].is_running() != plan.expected_source_running)
        {
            return Err(ScreenReconfigurationConflict::SourceLifecycleChanged);
        }
        if self.current_screen_capture_demand() != plan.expected_capture_demand {
            return Err(ScreenReconfigurationConflict::CaptureDemandChanged);
        }
        if replacement.as_ref().is_some() != plan.enabled
            || replacement.as_ref().is_some_and(|source| {
                !source.is_screen_source()
                    || !source.is_running()
                    || source.screen_capture_demand() != plan.capture_demand
            })
        {
            return Err(ScreenReconfigurationConflict::InvalidReplacement);
        }
        Ok(())
    }

    /// Whether any registered source captures host interaction.
    #[must_use]
    pub fn has_interaction_source(&self) -> bool {
        self.sources
            .iter()
            .any(|source| source.is_interaction_source())
    }

    /// Collect health snapshots from every interaction source.
    #[must_use]
    pub fn interaction_diagnostics(&self) -> Vec<InteractionDiagnostics> {
        self.sources
            .iter()
            .filter_map(|source| source.interaction_diagnostics())
            .collect()
    }

    /// Whether any registered source captures from host input hardware.
    ///
    /// Excludes the always-present browser injection source, so consent
    /// config can tell whether host capture is actually wired up.
    #[must_use]
    pub fn has_host_capture_source(&self) -> bool {
        self.sources
            .iter()
            .any(|source| source.is_host_capture_source())
    }

    /// Atomically replace the registered host source with one pre-started candidate.
    ///
    /// The detached source remains running in the returned retirement owner until
    /// the caller releases it outside the input-manager lock. A rejected candidate
    /// leaves the current source and graph generation unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when the manager does not contain exactly zero or one host
    /// source, or when the supplied candidate is not a running host interaction
    /// source.
    pub fn swap_host_capture_source(
        &mut self,
        replacement: &mut Option<Box<dyn InputSource>>,
    ) -> Result<HostRuntimeRetirement, HostReconfigurationError> {
        let mut host_indices = self
            .sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| source.is_host_capture_source().then_some(index));
        let current_index = host_indices.next();
        if host_indices.next().is_some() {
            return Err(HostReconfigurationError::SourceTopologyChanged);
        }
        if replacement.as_ref().is_some_and(|source| {
            !source.is_host_capture_source()
                || !source.is_interaction_source()
                || !source.is_running()
        }) {
            return Err(HostReconfigurationError::InvalidReplacement);
        }
        if current_index.is_none() && replacement.is_none() {
            return Ok(HostRuntimeRetirement {
                source: None,
                source_graph_generation: self.source_graph_generation,
            });
        }

        let source_graph_generation = self.bump_source_graph_generation();
        let prepared = replacement.take().map(|source| {
            let mut prepared = self.create_managed_source(source, source_graph_generation);
            prepared.mark_prestarted_compatibility_live();
            prepared
        });
        let retired = match (current_index, prepared) {
            (Some(index), Some(prepared)) => {
                Some(std::mem::replace(&mut self.sources[index], prepared))
            }
            (Some(index), None) => Some(self.sources.remove(index)),
            (None, Some(prepared)) => {
                self.sources.push(prepared);
                None
            }
            (None, None) => unreachable!("empty host swap returned before graph mutation"),
        };
        self.interaction_capture_active = None;
        self.publish_source_status_registry();
        Ok(HostRuntimeRetirement {
            source: retired,
            source_graph_generation,
        })
    }

    /// Stop and remove only host hardware capture sources.
    ///
    /// Leaves the browser injection source in place so disabling host
    /// consent never breaks browser-preview input.
    pub fn remove_host_capture_sources(&mut self) {
        if !self
            .sources
            .iter()
            .any(|source| source.is_host_capture_source())
        {
            return;
        }
        let source_graph_generation = self.bump_source_graph_generation();
        self.sources.retain_mut(|source| {
            if source.is_host_capture_source() {
                source.stop();
                if let Err(error) = source.retire_source_status(source_graph_generation) {
                    error!(source = source.name(), %error, "Failed to retire host input source status");
                }
                info!(source = source.name(), "Removed host capture source");
                false
            } else {
                true
            }
        });
        self.interaction_capture_active = None;
        self.publish_source_status_registry();
    }

    /// Stop and remove all registered screen sources.
    pub fn remove_screen_sources(&mut self) {
        if !self.sources.iter().any(|source| source.is_screen_source()) {
            return;
        }
        let source_graph_generation = self.bump_source_graph_generation();
        self.sources.retain_mut(|source| {
            if source.is_screen_source() {
                source.set_active_consumer_count(0);
                source.stop();
                if let Err(error) = source.retire_source_status(source_graph_generation) {
                    error!(source = source.name(), %error, "Failed to retire screen input source status");
                }
                info!(source = source.name(), "Removed screen capture source");
                false
            } else {
                true
            }
        });
        self.invalidate_capture_domains((false, true, false));
        self.publish_source_status_registry();
    }

    /// Apply new capture settings to any registered screen sources.
    ///
    /// # Errors
    ///
    /// Returns an error if a screen source cannot apply the new settings.
    pub fn reconfigure_screen_capture(
        &mut self,
        config: &screen::CaptureConfig,
    ) -> anyhow::Result<()> {
        if !self.sources.iter().any(|source| source.is_screen_source()) {
            return Ok(());
        }
        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            if source.is_screen_source() {
                source.set_source_graph_generation(source_graph_generation);
            }
        }
        let mut result = Ok(());
        for source in &mut self.sources {
            if source.is_screen_source() {
                if let Err(error) = source.reconfigure_screen_capture(config) {
                    result = Err(error);
                    break;
                }
                info!(
                    source = source.name(),
                    "Applied live screen capture settings"
                );
            }
        }
        self.publish_source_status_registry();
        result
    }

    /// Apply processing-only screen settings without rebuilding native capture.
    ///
    /// # Errors
    ///
    /// Returns an error if a registered screen source rejects the profile.
    pub fn reconfigure_screen_processing(
        &mut self,
        config: &screen::CaptureConfig,
    ) -> anyhow::Result<()> {
        for source in &mut self.sources {
            if source.is_screen_source() {
                source.reconfigure_screen_processing(config)?;
            }
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Mirror the active macOS daemon topology into every native source.
    ///
    /// # Errors
    ///
    /// Returns an error if a source can no longer publish status.
    pub fn set_macos_daemon_ownership(
        &mut self,
        owner: MacosCapabilityOwner,
        conflict: Option<MacosDaemonOwnerConflict>,
        designated_requirement_hash: Option<Arc<str>>,
    ) -> anyhow::Result<()> {
        self.macos_capability_owner = owner;
        self.macos_owner_conflict.clone_from(&conflict);
        self.macos_owner_designated_requirement_hash
            .clone_from(&designated_requirement_hash);
        for source in &mut self.sources {
            source.set_macos_daemon_ownership(
                owner,
                conflict.clone(),
                designated_requirement_hash.clone(),
            )?;
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Publish the active renderer device's Metal 4 capability into macOS source status.
    ///
    /// # Errors
    ///
    /// Returns an error if a source can no longer publish status.
    pub fn set_macos_metal4_capability(&mut self, metal4: bool) -> anyhow::Result<()> {
        self.macos_metal4 = metal4;
        for source in &mut self.sources {
            source.set_macos_metal4_capability(metal4)?;
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Resolve the explicit Input Monitoring request without retaining the
    /// input-manager lock while native authorization UI runs.
    #[must_use]
    pub fn input_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        self.sources
            .iter()
            .find_map(|source| source.input_authorization_action())
    }

    fn resolve_protected_source_action<A>(
        &self,
        action: A,
        executor: ProtectedSourceActionExecutor,
        presentation_required: bool,
    ) -> ResolvedProtectedSourceAction<A> {
        if executor == ProtectedSourceActionExecutor::PlatformBackend {
            return ResolvedProtectedSourceAction::Local {
                action,
                owner: ProtectedSourceActionOwner::PlatformBackend,
            };
        }

        let active_owner = self.macos_capability_owner;
        let requires_app_ui = matches!(
            active_owner,
            MacosCapabilityOwner::App | MacosCapabilityOwner::Broker
        ) || presentation_required
            && matches!(
                active_owner,
                MacosCapabilityOwner::LaunchdService | MacosCapabilityOwner::HomebrewService
            );
        if requires_app_ui {
            return ResolvedProtectedSourceAction::RequiresAppUi { active_owner };
        }
        ResolvedProtectedSourceAction::Local {
            action,
            owner: ProtectedSourceActionOwner::Macos(active_owner),
        }
    }

    /// Resolve the explicit Input Monitoring request against this process.
    #[must_use]
    pub fn resolved_input_authorization_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ProtectedSourceAuthorizationAction>> {
        let action = self.input_authorization_action()?;
        let executor = action.executor();
        Some(self.resolve_protected_source_action(action, executor, false))
    }

    /// Resolve the explicit Screen Recording request without retaining the
    /// input-manager lock while native authorization UI runs.
    #[must_use]
    pub fn screen_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        self.sources
            .iter()
            .find_map(|source| source.screen_authorization_action())
    }

    /// Resolve the explicit Screen Recording request against this process.
    #[must_use]
    pub fn resolved_screen_authorization_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ProtectedSourceAuthorizationAction>> {
        let action = self.screen_authorization_action()?;
        let executor = action.executor();
        Some(self.resolve_protected_source_action(action, executor, false))
    }

    /// Resolve the native picker action without retaining the input-manager
    /// lock while system UI runs.
    #[must_use]
    pub fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        self.sources
            .iter()
            .find_map(|source| source.screen_source_picker_action())
    }

    /// Resolve the native picker request against its exact local executor.
    #[must_use]
    pub fn resolved_screen_source_picker_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ScreenSourcePickerAction>> {
        let action = self.screen_source_picker_action()?;
        let executor = action.executor();
        Some(self.resolve_protected_source_action(action, executor, true))
    }

    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn macos_screenshot_reference_action(&self) -> Option<MacosScreenshotReferenceAction> {
        self.sources
            .iter()
            .find_map(|source| source.macos_screenshot_reference_action())
    }

    /// Ask screen sources to discard their persisted selection and re-prompt.
    ///
    /// # Errors
    ///
    /// Returns an error if a screen source cannot restart its session.
    pub fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        if !self.sources.iter().any(|source| source.is_screen_source()) {
            return Ok(());
        }
        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            if source.is_screen_source() {
                source.set_source_graph_generation(source_graph_generation);
            }
        }
        let mut result = Ok(());
        for source in &mut self.sources {
            if source.is_screen_source() {
                if let Err(error) = source.reselect_screen_source() {
                    result = Err(error);
                    break;
                }
                info!(source = source.name(), "Re-opened screen source picker");
            }
        }
        self.publish_source_status_registry();
        result
    }

    /// Return the latest system sensor snapshot, if one is configured.
    #[must_use]
    pub fn latest_sensor_snapshot(&self) -> Option<Arc<SystemSnapshot>> {
        self.sensor_snapshot_rx
            .as_ref()
            .map(|receiver| Arc::clone(&receiver.borrow()))
    }

    fn bump_source_graph_generation(&mut self) -> u64 {
        self.source_graph_generation = self
            .source_graph_generation
            .checked_add(1)
            .expect("input source graph generation exhausted");
        self.source_graph_generation
    }

    fn create_managed_source(
        &mut self,
        mut source: Box<dyn InputSource>,
        source_graph_generation: u64,
    ) -> ManagedInputSource {
        source
            .set_macos_daemon_ownership(
                self.macos_capability_owner,
                self.macos_owner_conflict.clone(),
                self.macos_owner_designated_requirement_hash.clone(),
            )
            .expect("new source accepts retained macOS ownership status");
        source
            .set_macos_metal4_capability(self.macos_metal4)
            .expect("new source accepts retained macOS Metal 4 status");
        let id = self.next_source_slot_id;
        self.next_source_slot_id = self
            .next_source_slot_id
            .checked_add(1)
            .expect("input source slot identity exhausted");
        ManagedInputSource::new(
            source,
            id,
            source_graph_generation,
            self.screen_plan_builder.publication_hub(),
        )
    }

    fn transition_capture_demand(
        &mut self,
        domain: CaptureDomain,
        active: bool,
    ) -> anyhow::Result<()> {
        let cached = match domain {
            CaptureDomain::Audio => self.audio_capture_active,
            CaptureDomain::Interaction => self.interaction_capture_active,
        };
        if cached == Some(active) {
            return Ok(());
        }

        let prior_demands = self
            .sources
            .iter()
            .map(|source| {
                domain
                    .matches(source.as_ref())
                    .then(|| source.source_status_handle().snapshot().demanded)
            })
            .collect::<Vec<_>>();

        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            if domain.matches(source.as_ref()) {
                source.set_source_graph_generation(source_graph_generation);
            }
        }

        for source_index in 0..self.sources.len() {
            if !domain.matches(self.sources[source_index].as_ref()) {
                continue;
            }
            let transition = domain
                .transition(self.sources[source_index].as_mut(), active)
                .and_then(|()| self.sources[source_index].set_compatibility_demand(active));
            if let Err(error) = transition {
                let mut rollback_succeeded = true;
                for (rollback, previous) in self.sources.iter_mut().zip(&prior_demands) {
                    if let Some(previous) = previous {
                        let rollback_result = domain
                            .transition(rollback.as_mut(), *previous)
                            .and_then(|()| rollback.set_compatibility_demand(*previous));
                        if let Err(rollback_error) = rollback_result {
                            rollback_succeeded = false;
                            error!(
                                source = rollback.name(),
                                %rollback_error,
                                "Failed to roll back input capture demand"
                            );
                        }
                    }
                }
                let restored_cache = if rollback_succeeded {
                    let mut restored_demands = prior_demands.iter().flatten().copied();
                    restored_demands
                        .next()
                        .filter(|first| restored_demands.all(|demand| demand == *first))
                } else {
                    None
                };
                self.set_capture_demand_cache(domain, restored_cache);
                self.publish_source_status_registry();
                return Err(error);
            }
        }

        self.set_capture_demand_cache(domain, Some(active));
        self.publish_source_status_registry();
        Ok(())
    }

    fn set_capture_demand_cache(&mut self, domain: CaptureDomain, demand: Option<bool>) {
        match domain {
            CaptureDomain::Audio => self.audio_capture_active = demand,
            CaptureDomain::Interaction => self.interaction_capture_active = demand,
        }
    }

    fn transition_screen_capture_demand(
        &mut self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<()> {
        if self.screen_capture_demand == Some(demand) {
            return Ok(());
        }

        let previous_cached_demand = self.screen_capture_demand;
        let prior_demands = self
            .sources
            .iter()
            .map(|source| {
                source
                    .is_screen_source()
                    .then(|| source.screen_capture_demand())
            })
            .collect::<Vec<_>>();
        let analysis_peak_bytes = self
            .sources
            .iter()
            .filter(|source| source.is_screen_source())
            .try_fold(0_u64, |total, source| {
                let bytes = source
                    .screen_analysis_resource_plan(demand)?
                    .map_or(0, screen::ScreenAnalysisResourcePlan::peak_bytes);
                total
                    .checked_add(bytes)
                    .ok_or_else(|| anyhow::anyhow!("screen analysis capacity overflow"))
            })?;
        let mut capacity_preparation = self.prepare_screen_capacity(analysis_peak_bytes)?;
        let source_graph_generation = self.bump_source_graph_generation();
        if let Some(preparation) = &mut capacity_preparation {
            preparation.expected_graph_generation = source_graph_generation;
        }
        for source in &mut self.sources {
            if source.is_screen_source() {
                source.set_source_graph_generation(source_graph_generation);
            }
        }
        for source_index in 0..self.sources.len() {
            if !self.sources[source_index].is_screen_source() {
                continue;
            }
            let transition = self.sources[source_index]
                .set_screen_capture_demand(demand)
                .and_then(|()| {
                    self.sources[source_index].set_compatibility_demand(demand.is_active())
                });
            if let Err(error) = transition {
                let mut rollback_succeeded = true;
                for (source, previous) in self.sources.iter_mut().zip(&prior_demands) {
                    if let Some(previous) = previous {
                        let rollback_result = source
                            .set_screen_capture_demand(*previous)
                            .and_then(|()| source.set_compatibility_demand(previous.is_active()));
                        if let Err(rollback_error) = rollback_result {
                            rollback_succeeded = false;
                            error!(
                                source = source.name(),
                                %rollback_error,
                                "Failed to roll back screen capture demand"
                            );
                        }
                    }
                }
                self.screen_capture_demand = if rollback_succeeded {
                    previous_cached_demand.or_else(|| {
                        let mut restored = prior_demands.iter().flatten().copied();
                        restored
                            .next()
                            .filter(|first| restored.all(|demand| demand == *first))
                    })
                } else {
                    None
                };
                self.publish_source_status_registry();
                return Err(error);
            }
        }

        if let Some(capacity_preparation) = capacity_preparation {
            self.validate_screen_capacity(&capacity_preparation)?;
            self.commit_screen_capacity_unpublished(capacity_preparation);
        }
        self.screen_capture_demand = Some(demand);
        self.publish_source_status_registry();
        Ok(())
    }

    fn invalidate_capture_domains(&mut self, domains: (bool, bool, bool)) {
        if domains.0 {
            self.audio_capture_active = None;
        }
        if domains.1 {
            self.screen_capture_demand = None;
            self.screen_publication_demand = None;
            self.committed_screen_publication_resolution_revision = None;
            self.set_screen_publication_active_consumer_count(0);
        }
        if domains.2 {
            self.interaction_capture_active = None;
        }
    }

    fn set_screen_publication_active_consumer_count(&mut self, active_consumer_count: usize) {
        for source in self
            .sources
            .iter_mut()
            .filter(|source| source.is_screen_source())
        {
            source.set_active_consumer_count(active_consumer_count);
        }
    }

    fn set_screen_publication_active_consumer_counts(
        &mut self,
        active_consumer_counts: &[(screen::CaptureSourceId, usize)],
    ) {
        for source in self
            .sources
            .iter_mut()
            .filter(|source| source.is_screen_source())
        {
            let active_consumer_count = active_consumer_counts
                .iter()
                .filter(|(source_id, _)| source.owns_screen_publication_source(source_id))
                .map(|(_, count)| *count)
                .sum();
            source.set_active_consumer_count(active_consumer_count);
        }
    }

    fn publish_source_status_registry(&self) {
        let slots = self
            .sources
            .iter()
            .map(|source| source.slot.clone())
            .collect::<Vec<_>>()
            .into();
        self.input_graph
            .publish(self.source_graph_generation, slots);
        let handles = self
            .sources
            .iter()
            .map(ManagedInputSource::source_status_handle)
            .collect();
        self.source_status_registry
            .publish(self.source_graph_generation, handles);
        self.publish_screen_capacity_status();
    }

    fn publish_screen_capacity_status(&self) {
        let policy = self.screen_capacity_policy_snapshot();
        self.screen_capacity_status.publish(&policy);
    }

    fn screen_capacity_policy_snapshot(&self) -> screen::ScreenCapacityPolicySnapshot {
        self.screen_capacity_policy_snapshot_with(
            self.screen_capacity_enforced,
            self.screen_total_capacity,
            self.screen_publication_capacity,
        )
    }

    fn screen_capacity_policy_snapshot_with(
        &self,
        capacity_enforced: bool,
        total_capacity: screen::ScreenAdmissionCapacity,
        publication_capacity: screen::ScreenAdmissionCapacity,
    ) -> screen::ScreenCapacityPolicySnapshot {
        screen::ScreenCapacityPolicySnapshot::new(
            capacity_enforced,
            total_capacity,
            publication_capacity,
            self.current_screen_capture_demand(),
            self.screen_analysis_resource_plan().ok().flatten(),
            self.screen_analysis_work_plan().ok().flatten(),
            self.screen_analysis_compute_capacity(),
        )
    }
}

fn resolved_compatibility_descriptor(
    demand: &ScreenPublicationDemandSnapshot,
    resolved: &[screen::ResolvedScreenBranchDemand],
    selection: Option<&screen::RegisteredScreenBranchDemand>,
) -> Option<screen::ResolvedScreenPublicationDescriptor> {
    let selection = selection?;
    demand
        .branches()
        .iter()
        .position(|branch| branch == selection)
        .and_then(|index| resolved.get(index))
        .map(|branch| branch.descriptor().clone())
}

fn declared_source_kind(source: &dyn InputSource) -> Option<SourceKind> {
    source
        .source_status_handle()
        .map(|handle| handle.snapshot().kind)
        .or_else(|| source.is_audio_source().then_some(SourceKind::Audio))
        .or_else(|| source.is_screen_source().then_some(SourceKind::Screen))
        .or_else(|| {
            source
                .is_interaction_source()
                .then_some(SourceKind::Interaction)
        })
}

fn sample_managed_source(
    source: &mut ManagedInputSource,
    delta_secs: f32,
    event_scratch: &mut Vec<TimedInputEvent>,
) {
    event_scratch.clear();
    let sample = match source.sample_shared_and_drain_into(delta_secs, event_scratch) {
        Ok(sample) => sample,
        Err(error) => {
            error!(source = source.name(), %error, "Input sample failed");
            None
        }
    };
    source.slot.publish_batch(sample, event_scratch);
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod host_source_swap_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{HostReconfigurationError, InputData, InputManager, InputSource, SourceState};

    struct HostSource {
        name: &'static str,
        running: bool,
        stopped: Arc<AtomicBool>,
    }

    impl HostSource {
        fn new(name: &'static str, stopped: Arc<AtomicBool>) -> Self {
            Self {
                name,
                running: false,
                stopped,
            }
        }
    }

    impl InputSource for HostSource {
        fn name(&self) -> &'static str {
            self.name
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
            self.stopped.store(true, Ordering::Release);
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn is_interaction_source(&self) -> bool {
            true
        }

        fn is_host_capture_source(&self) -> bool {
            true
        }
    }

    #[test]
    fn successful_host_swap_defers_old_source_retirement() {
        let old_stopped = Arc::new(AtomicBool::new(false));
        let candidate_stopped = Arc::new(AtomicBool::new(false));
        let mut old = Box::new(HostSource::new("old-host", Arc::clone(&old_stopped)));
        old.start().expect("old host source starts");
        let mut manager = InputManager::new();
        manager.add_source(old);
        let initial_generation = manager.source_graph_generation();

        let mut candidate: Option<Box<dyn InputSource>> = Some(Box::new(HostSource::new(
            "candidate-host",
            Arc::clone(&candidate_stopped),
        )));
        candidate
            .as_mut()
            .expect("candidate exists")
            .start()
            .expect("candidate host source starts");
        let retirement = manager
            .swap_host_capture_source(&mut candidate)
            .expect("running candidate swaps atomically");

        assert!(candidate.is_none());
        assert_eq!(manager.source_names(), ["candidate-host"]);
        assert!(manager.source_graph_generation() > initial_generation);
        assert_eq!(
            manager.source_status_registry().snapshot().statuses()[0].state,
            SourceState::Live
        );
        assert!(!old_stopped.load(Ordering::Acquire));
        assert!(!candidate_stopped.load(Ordering::Acquire));

        retirement.retire();
        assert!(old_stopped.load(Ordering::Acquire));
        assert!(!candidate_stopped.load(Ordering::Acquire));
    }

    #[test]
    fn nonrunning_candidate_preserves_last_good_host_source() {
        let old_stopped = Arc::new(AtomicBool::new(false));
        let mut old = Box::new(HostSource::new("old-host", Arc::clone(&old_stopped)));
        old.start().expect("old host source starts");
        let mut manager = InputManager::new();
        manager.add_source(old);
        let initial_generation = manager.source_graph_generation();
        let mut candidate: Option<Box<dyn InputSource>> = Some(Box::new(HostSource::new(
            "failed-candidate",
            Arc::new(AtomicBool::new(false)),
        )));

        assert!(matches!(
            manager.swap_host_capture_source(&mut candidate),
            Err(HostReconfigurationError::InvalidReplacement)
        ));
        assert!(candidate.is_some());
        assert_eq!(manager.source_names(), ["old-host"]);
        assert_eq!(manager.source_graph_generation(), initial_generation);
        assert!(!old_stopped.load(Ordering::Acquire));
    }
}
