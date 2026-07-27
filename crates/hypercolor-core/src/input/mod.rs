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
pub mod screen;
pub mod sensor;
mod status;
mod traits;
pub mod windows;

pub use browser::{BrowserInputEdge, BrowserInputHandle, BrowserInputSource};
#[cfg(target_os = "linux")]
pub use evdev::{DeviceOpenState, DeviceOpenStatus, EvdevHostInput};
pub use graph::{
    INPUT_EVENT_RING_CAPACITY, InputEventRead, InputGraphHandle, InputGraphSnapshot,
    InputSourceSlot,
};
#[cfg(target_os = "macos")]
pub use interaction::InteractionInput;
pub use media::MediaSource;
pub use net::NetSource;
pub use sensor::SensorPoller;
pub use status::{
    SourceFreshness, SourceIssue, SourceKind, SourceResourceScanHealth, SourceSessionSlot,
    SourceSessionWriter, SourceState, SourceStatus, SourceStatusError, SourceStatusHandle,
    SourceStatusRegistry, SourceStatusRegistrySnapshot, SourceStatusReporter,
    SourceStatusSubscription, SourceStatusWriter, SourceTimestampField, TerminalFailureLatch,
    classify_source_resource_scan,
};
pub use traits::{
    InputData, InputSource, InteractionBatch, InteractionData, InteractionDegradation,
    InteractionDiagnostics, KeyboardData, MotionAggregate, MouseData, PointerMode, ScreenData,
};
pub use windows::WindowsHostInput;

use crate::input::audio::AudioInput;
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
}

impl ManagedInputSource {
    fn new(source: Box<dyn InputSource>, slot: InputSourceSlot) -> Self {
        Self { source, slot }
    }

    fn into_source(self) -> Box<dyn InputSource> {
        self.source
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
    pub fn add_source(&mut self, mut source: Box<dyn InputSource>) {
        let domains = (
            source.is_audio_source(),
            source.is_screen_source(),
            source.is_interaction_source(),
        );
        let source_graph_generation = self.bump_source_graph_generation();
        source.set_source_graph_generation(source_graph_generation);
        let slot = self.create_source_slot(source.as_ref());
        info!(source = source.name(), "Registered input source");
        self.sources.push(ManagedInputSource::new(source, slot));
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
        mut source: Box<dyn InputSource>,
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
        source.set_source_graph_generation(source_graph_generation);
        let slot = self.create_source_slot(source.as_ref());
        let mut previous = std::mem::replace(
            &mut self.sources[index],
            ManagedInputSource::new(source, slot),
        );
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
            self.event_scratch.clear();
            let sample =
                match source.sample_shared_and_drain_into(delta_secs, &mut self.event_scratch) {
                    Ok(sample) => sample,
                    Err(error) => {
                        error!(source = source.name(), %error, "Input sample failed");
                        None
                    }
                };
            source.slot.publish_sample(sample);
            source.slot.publish_events(&mut self.event_scratch);
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

        for (idx, source) in self.sources.iter_mut().enumerate() {
            if let Err(err) = source.start() {
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
                if let Some(sensor_poller) = self.sensor_poller.as_mut() {
                    sensor_poller.stop();
                }
                // Roll back: stop everything we already started.
                for prev in &mut self.sources[..idx] {
                    prev.stop();
                }
                return Err(err);
            }
            info!(source = source.name(), "Started input source");
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

    /// Apply a live audio config change without rebuilding unrelated sources.
    ///
    /// If an audio source already exists, it is reconfigured in place. If audio
    /// is being enabled and no audio source exists yet, one is created and
    /// started. Disabling audio reconfigures the existing source to silence.
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
        let effective_capture_active = enabled && capture_active;
        let effective_config = if enabled {
            config.clone()
        } else {
            let mut disabled = config.clone();
            disabled.source = crate::types::audio::AudioSourceType::None;
            disabled
        };

        if let Some(index) = self
            .sources
            .iter()
            .position(|source| source.is_audio_source())
        {
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
                    "Reconfigured live audio input source"
                );
                self.audio_capture_active = Some(effective_capture_active);
            }
            self.publish_source_status_registry();
            return result;
        }

        if !enabled {
            self.audio_capture_active = Some(false);
            return Ok(());
        }

        let mut audio_input = AudioInput::new(&effective_config).with_name(display_name.to_owned());
        audio_input.set_capture_active(effective_capture_active)?;
        self.add_source(Box::new(audio_input));
        let start_result = self
            .sources
            .last_mut()
            .expect("audio source was just registered")
            .start();
        if let Err(error) = start_result {
            let mut failed = self
                .sources
                .pop()
                .expect("audio source was just registered");
            failed.stop();
            let removal_generation = self.bump_source_graph_generation();
            if let Err(status_error) = failed.retire_source_status(removal_generation) {
                error!(source = failed.name(), %status_error, "Failed to retire rejected audio source status");
            }
            self.publish_source_status_registry();
            return Err(error);
        }
        info!(
            source = display_name,
            capture_active = effective_capture_active,
            "Added live audio input source"
        );
        self.audio_capture_active = Some(effective_capture_active);
        Ok(())
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

    fn create_source_slot(&mut self, source: &dyn InputSource) -> InputSourceSlot {
        let id = self.next_source_slot_id;
        self.next_source_slot_id = self
            .next_source_slot_id
            .checked_add(1)
            .expect("input source slot identity exhausted");
        InputSourceSlot::new(id, declared_source_kind(source))
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
                domain.matches(source.as_ref()).then(|| {
                    source.source_status_handle().map_or_else(
                        || cached.unwrap_or(!active),
                        |handle| handle.snapshot().demanded,
                    )
                })
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
            if let Err(error) = domain.transition(self.sources[source_index].as_mut(), active) {
                let mut rollback_succeeded = true;
                for (rollback, previous) in self.sources.iter_mut().zip(&prior_demands) {
                    if let Some(previous) = previous
                        && let Err(rollback_error) = domain.transition(rollback.as_mut(), *previous)
                    {
                        rollback_succeeded = false;
                        error!(
                            source = rollback.name(),
                            %rollback_error,
                            "Failed to roll back input capture demand"
                        );
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
            .filter_map(|source| source.source_status_handle())
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

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}
