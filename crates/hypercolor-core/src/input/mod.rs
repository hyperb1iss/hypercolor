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
#[cfg(target_os = "macos")]
pub mod interaction;
pub mod keymap;
pub mod media;
pub mod net;
pub mod routing;
pub mod screen;
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
#[cfg(target_os = "macos")]
pub use interaction::InteractionInput;
pub use media::MediaSource;
pub use net::NetSource;
pub use sensor::SensorPoller;
pub use status::{
    ScreenCaptureDiagnostics, ScreenCaptureReductionPath, SourceDiagnostics, SourceFreshness,
    SourceIssue, SourceKind, SourceResourceScanHealth, SourceSessionSlot, SourceSessionWriter,
    SourceState, SourceStatus, SourceStatusAvailability, SourceStatusError, SourceStatusHandle,
    SourceStatusRegistry, SourceStatusRegistrySnapshot, SourceStatusReporter,
    SourceStatusSubscription, SourceStatusWriter, SourceTimestampField, TerminalFailureLatch,
    classify_source_resource_scan,
};
pub use traits::{
    InputData, InputSource, InteractionBatch, InteractionData, InteractionDegradation,
    InteractionDiagnostics, KeyboardData, MotionAggregate, MouseData, PointerMode, ScreenData,
};
pub use windows::WindowsHostInput;

use crate::input::audio::{
    AudioInput, AudioPreparationRequest, AudioRuntimeRetirement, PreparedAudioReconfiguration,
};
use crate::types::audio::AudioPipelineConfig;
use crate::types::event::TimedInputEvent;
use hypercolor_types::sensor::SystemSnapshot;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};
use std::time::Instant;
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
    enabled: bool,
    capture_active: bool,
}

impl ScreenRuntimeConfigPlan {
    /// Whether the replacement source must be registered after commit.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Demand state the prepared replacement must adopt before it starts.
    #[must_use]
    pub const fn capture_active(&self) -> bool {
        self.capture_active
    }

    /// Graph generation reserved for a staged replacement source.
    #[must_use]
    pub fn replacement_source_graph_generation(&self) -> u64 {
        self.expected_graph_generation
            .checked_add(1)
            .expect("input source graph generation exhausted")
    }
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
    /// The prepared replacement does not match the plan.
    #[error("prepared screen source does not match the reconfiguration plan")]
    InvalidReplacement,
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
    screen_capture_active: Option<bool>,
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
    fn new(mut source: Box<dyn InputSource>, slot_id: u64, source_graph_generation: u64) -> Self {
        let declared_kind = declared_source_kind(source.as_ref());
        let interaction_origin = source
            .is_interaction_source()
            .then(|| source.interaction_source_origin());
        source.set_source_graph_generation(source_graph_generation);
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

    fn set_source_graph_generation(&mut self, source_graph_generation: u64) {
        self.source
            .set_source_graph_generation(source_graph_generation);
        if let Some(status) = &mut self.compatibility_status {
            status.set_source_graph_generation(source_graph_generation);
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
    Screen,
    Interaction,
}

impl CaptureDomain {
    fn matches(self, source: &dyn InputSource) -> bool {
        match self {
            Self::Audio => source.is_audio_source(),
            Self::Screen => source.is_screen_source(),
            Self::Interaction => source.is_interaction_source(),
        }
    }

    fn transition(self, source: &mut dyn InputSource, active: bool) -> anyhow::Result<()> {
        match self {
            Self::Audio => source.set_audio_capture_active(active),
            Self::Screen => source.set_screen_capture_active(active),
            Self::Interaction => source.set_interaction_capture_active(active),
        }
    }
}

impl InputManager {
    /// Create an empty manager with no sources.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            source_graph_generation: 0,
            next_source_slot_id: 1,
            input_graph: InputGraphHandle::new(),
            source_status_registry: SourceStatusRegistry::new(),
            event_scratch: Vec::with_capacity(INPUT_EVENT_RING_CAPACITY),
            audio_capture_active: None,
            screen_capture_active: None,
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
                return Err(err);
            }
            info!(
                source = self.sources[source_index].name(),
                "Started input source"
            );
        }
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

    /// Toggle live screen capture for any registered screen sources.
    ///
    /// This keeps the input graph intact while allowing the capture backend to
    /// pause or resume compositor capture based on current render demand.
    ///
    /// # Errors
    ///
    /// Returns an error if a screen source cannot update its capture state.
    pub fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.transition_capture_demand(CaptureDomain::Screen, active)
    }

    /// Whether any registered source handles screen capture.
    #[must_use]
    pub fn has_screen_source(&self) -> bool {
        self.sources.iter().any(|source| source.is_screen_source())
    }

    /// Snapshot a generation-fenced screen-source replacement plan.
    pub fn plan_screen_runtime_config(&self, enabled: bool) -> ScreenRuntimeConfigPlan {
        let source = self.sources.iter().find(|source| source.is_screen_source());
        let current_demand = self.screen_capture_active.unwrap_or_else(|| {
            source.is_some_and(|source| source.source_status_handle().snapshot().demanded)
        });
        ScreenRuntimeConfigPlan {
            expected_graph_generation: self.source_graph_generation,
            expected_source_present: source.is_some(),
            expected_source_running: source.is_some_and(|source| source.is_running()),
            enabled,
            capture_active: enabled && current_demand,
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
        self.screen_capture_active = Some(plan.capture_active);
        if topology_changed {
            self.publish_source_status_registry();
        }
        Ok(ScreenRuntimeRetirement {
            sources: retired,
            source_graph_generation,
        })
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
        if replacement.as_ref().is_some() != plan.enabled
            || replacement
                .as_ref()
                .is_some_and(|source| !source.is_screen_source() || !source.is_running())
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
        self.screen_capture_active = None;
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
        source: Box<dyn InputSource>,
        source_graph_generation: u64,
    ) -> ManagedInputSource {
        let id = self.next_source_slot_id;
        self.next_source_slot_id = self
            .next_source_slot_id
            .checked_add(1)
            .expect("input source slot identity exhausted");
        ManagedInputSource::new(source, id, source_graph_generation)
    }

    fn transition_capture_demand(
        &mut self,
        domain: CaptureDomain,
        active: bool,
    ) -> anyhow::Result<()> {
        let cached = match domain {
            CaptureDomain::Audio => self.audio_capture_active,
            CaptureDomain::Screen => self.screen_capture_active,
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
            CaptureDomain::Screen => self.screen_capture_active = demand,
            CaptureDomain::Interaction => self.interaction_capture_active = demand,
        }
    }

    fn invalidate_capture_domains(&mut self, domains: (bool, bool, bool)) {
        if domains.0 {
            self.audio_capture_active = None;
        }
        if domains.1 {
            self.screen_capture_active = None;
        }
        if domains.2 {
            self.interaction_capture_active = None;
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
    }
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
