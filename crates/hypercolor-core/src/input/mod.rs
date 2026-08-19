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
#[doc(hidden)]
pub use hypercolor_worker_retention::retention_service_identity as worker_retention_service_identity;
pub use macos::{MacosHostInput, MacosInputFoldDiagnostics};
#[cfg(feature = "macos-native-fixtures")]
pub use macos::{MacosHostInputFixture, MacosInputFixtureBackend};
pub use media::MediaSource;
pub use net::NetSource;
pub use screen::{ScreenCaptureDemand, ScreenPublicationDemandSnapshot};
pub use scroll::{LegacyWheelProjector, Q16_16_SCALE, q16_16_to_f64};
pub use sensor::SensorSource;
pub use status::{
    ScreenCaptureDiagnostics, ScreenCaptureReductionPath, SourceDiagnostics, SourceFreshness,
    SourceIssue, SourceKind, SourceResourceScanHealth, SourceSessionSlot, SourceSessionWriter,
    SourceState, SourceStatus, SourceStatusAvailability, SourceStatusError, SourceStatusHandle,
    SourceStatusRegistry, SourceStatusRegistrySnapshot, SourceStatusReporter,
    SourceStatusSubscription, SourceStatusWriter, SourceTimestampField, TerminalFailureLatch,
    classify_source_resource_scan,
};
pub use traits::{
    AudioSource, AudioSourceRole, CapabilityActionDisposition, CapabilityActionIdentity,
    DataSource, DataSourceKind, DataSourceRole, InputData, InputSource, InteractionBatch,
    InteractionData, InteractionDegradation, InteractionDiagnostics, InteractionSource,
    InteractionSourceRole, KeyboardData, ManagedSource, ManagedSourceKey, ManagedSourceRole,
    MotionAggregate, MouseData, PointerMode, ProtectedSourceAuthorizationAction,
    ResolvedProtectedSourceAction, ScreenData, ScreenSource, ScreenSourcePickerAction,
    ScreenSourceRole, ScreenZoneColors, ScrollAggregate, SourceCapabilityConflict,
    SourceCapabilityContext, SourceDiagnosticArtifact, SourceDiagnosticArtifactAction, SourceRole,
    SourceRoleBinding,
};
pub use windows::WindowsHostInput;
#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
pub use windows::WindowsHostInputFixture;

use crate::input::audio::{AudioInput, AudioPreparationRequest, PreparedAudioReconfiguration};
use crate::types::audio::AudioPipelineConfig;
use crate::types::event::TimedInputEvent;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock, Mutex, RwLock, TryLockError};
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
    source_swap: SourceSwapPlan,
    preparation: Option<AudioPreparationRequest>,
}

/// Complete audio candidate paired with its generic source-swap fence.
#[must_use = "prepared audio source swaps must be committed or discarded"]
pub struct PreparedAudioSourceSwap {
    source_swap: PreparedSourceSwap,
    failure_signal: Option<Arc<std::sync::atomic::AtomicU8>>,
}

/// Result of a nonblocking input-manager intent.
#[derive(Debug)]
#[must_use = "nonblocking input-manager intents must handle busy and stale outcomes"]
pub enum TryInputManagerIntent<T> {
    /// Another lifecycle transaction currently owns the manager.
    Busy,
    /// The caller's lock-free freshness predicate rejected the intent.
    Stale,
    /// Lifecycle ownership was acquired and the intent ran.
    Applied(T),
}

/// Desired capture state for every manager-owned capture domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputCaptureDemand {
    audio_active: bool,
    screen: ScreenCaptureDemand,
    interaction_active: bool,
}

impl InputCaptureDemand {
    /// Construct one atomic capture-demand intent.
    #[must_use]
    pub const fn new(
        audio_active: bool,
        screen: ScreenCaptureDemand,
        interaction_active: bool,
    ) -> Self {
        Self {
            audio_active,
            screen,
            interaction_active,
        }
    }
}

/// Per-domain results and the exact graph generation after capture reconciliation.
pub struct InputCaptureDemandApplication {
    source_graph_generation: u64,
    audio: InputCaptureDomainApplication,
    screen: InputCaptureDomainApplication,
    interaction: InputCaptureDomainApplication,
}

/// One capture domain's observed graph generation and application result.
pub type InputCaptureDomainApplication = (u64, anyhow::Result<()>);

impl InputCaptureDemandApplication {
    /// Split the application into its final generation and domain results.
    pub fn into_parts(
        self,
    ) -> (
        u64,
        InputCaptureDomainApplication,
        InputCaptureDomainApplication,
        InputCaptureDomainApplication,
    ) {
        (
            self.source_graph_generation,
            self.audio,
            self.screen,
            self.interaction,
        )
    }
}

/// Exact screen swap state captured while briefly holding the input manager lock.
#[must_use = "screen source swap plans must be prepared and committed"]
pub struct ScreenSourceSwapPlan {
    source_swap: SourceSwapPlan,
    expected_capture_demand: ScreenCaptureDemand,
    capture_demand: ScreenCaptureDemand,
    capacity: Option<ScreenCapacityPreparation>,
}

impl ScreenSourceSwapPlan {
    /// Whether the replacement source must be registered after commit.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        matches!(self.source_swap.target, SourceSwapTarget::Present { .. })
    }

    /// Demand state the prepared replacement must adopt before it starts.
    #[must_use]
    pub const fn capture_demand(&self) -> ScreenCaptureDemand {
        self.capture_demand
    }

    /// Graph generation reserved for a staged replacement source.
    #[must_use]
    pub fn replacement_source_graph_generation(&self) -> u64 {
        self.source_swap.replacement_source_graph_generation
    }
}

/// Fully managed screen candidate paired with demand and capacity fences.
#[must_use = "prepared screen source swaps must be committed or discarded"]
pub struct PreparedScreenSourceSwap {
    source_swap: PreparedSourceSwap,
    expected_capture_demand: ScreenCaptureDemand,
    capture_demand: ScreenCaptureDemand,
    capacity: Option<ScreenCapacityPreparation>,
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

/// Failure while atomically planning capacity and source state for screen capture.
#[derive(Debug, thiserror::Error)]
pub enum ScreenSourceSwapPlanningError {
    /// The candidate analysis or publication plan exceeds available capacity.
    #[error(transparent)]
    Capacity(#[from] ScreenCapacityPreparationError),
    /// Screen capacity admission is required but has not been installed.
    #[error("screen capacity admission is not installed")]
    CapacityUnavailable,
    /// The typed screen source cannot be planned against the current graph.
    #[error(transparent)]
    Source(#[from] SourceSwapConflict),
}

/// A concurrent input-graph transition invalidated prepared screen state.
#[derive(Debug, thiserror::Error)]
pub enum ScreenReconfigurationConflict {
    /// The generic source swap no longer describes the canonical graph.
    #[error(transparent)]
    Source(#[from] SourceSwapConflict),
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
    /// The prepared replacement does not carry the planned exact demand.
    #[error("prepared screen source demand does not match the reconfiguration plan")]
    InvalidReplacementDemand,
}

/// Failure before a screen source swap reaches its visibility fence.
#[derive(Debug, thiserror::Error)]
pub enum ScreenSourceSwapCommitError<E> {
    /// A captured graph, demand, publication, resource, or capacity fence advanced.
    #[error(transparent)]
    Conflict(#[from] ScreenReconfigurationConflict),
    /// Durable configuration persistence failed before graph mutation.
    #[error(transparent)]
    Persistence(E),
}

/// A typed source cannot enter the graph under ambiguous or conflicting metadata.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum SourceRegistrationError {
    /// Source-authored status must agree with its immutable registered role.
    #[error("input source key {key:?} declares status kind {observed:?}, expected {expected:?}")]
    StatusKindMismatch {
        /// Immutable registered role key.
        key: ManagedSourceKey,
        /// Role-derived scheduling and routing kind.
        expected: SourceKind,
        /// Kind published by the source-owned status handle.
        observed: SourceKind,
    },
}

/// Desired source presence after one generation-fenced swap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceSwapTarget {
    /// Remove the source identified by the plan key.
    Absent,
    /// Install a replacement with the stated lifecycle state.
    Present {
        /// Whether the prepared replacement must already be running.
        running: bool,
    },
}

/// Immutable compare-and-swap plan for one typed source key.
#[must_use = "source swap plans must be committed or discarded"]
pub struct SourceSwapPlan {
    key: ManagedSourceKey,
    expected_graph_generation: u64,
    expected_slot_id: Option<u64>,
    expected_running: Option<bool>,
    replacement_slot_id: u64,
    replacement_source_graph_generation: u64,
    source_capability_context: SourceCapabilityContext,
    screen_publication_hub: Arc<screen::ScreenPublicationHub>,
    target: SourceSwapTarget,
}

/// Opaque, fully managed candidate for one generic source swap.
#[must_use = "prepared source swaps must be committed or discarded"]
pub struct PreparedSourceSwap {
    plan: SourceSwapPlan,
    replacement: Option<ManagedInputSource>,
}

/// A concurrent graph change or invalid candidate rejected a typed swap.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum SourceSwapConflict {
    /// The canonical source graph changed after planning.
    #[error("input graph changed while source swap was prepared")]
    GraphChanged,
    /// The role key does not uniquely identify one source.
    #[error("input source key {key:?} is ambiguous")]
    AmbiguousKey {
        /// Ambiguous immutable role key.
        key: ManagedSourceKey,
    },
    /// The source occupying the role key changed after planning.
    #[error("input source key {key:?} changed while source swap was prepared")]
    SourceChanged {
        /// Immutable role key whose slot identity changed.
        key: ManagedSourceKey,
    },
    /// The planned source started or stopped after planning.
    #[error("input source key {key:?} changed lifecycle while source swap was prepared")]
    SourceLifecycleChanged {
        /// Immutable role key whose lifecycle changed.
        key: ManagedSourceKey,
    },
    /// Candidate presence does not match the planned target.
    #[error("prepared source presence does not match the swap target")]
    InvalidReplacementPresence,
    /// Candidate role identity does not match the planned key.
    #[error("prepared source key {observed:?} does not match {expected:?}")]
    InvalidReplacementKey {
        /// Planned immutable role key.
        expected: ManagedSourceKey,
        /// Candidate immutable role key.
        observed: ManagedSourceKey,
    },
    /// Candidate lifecycle does not match the planned target.
    #[error("prepared source running state {observed_running} does not match {expected_running}")]
    InvalidReplacementLifecycle {
        /// Planned candidate lifecycle state.
        expected_running: bool,
        /// Observed candidate lifecycle state.
        observed_running: bool,
    },
    /// Candidate status metadata conflicts with its immutable role.
    #[error("prepared source status kind {observed:?} does not match {expected:?}")]
    InvalidReplacementStatusKind {
        /// Role-derived scheduling and routing kind.
        expected: SourceKind,
        /// Kind published by the candidate status handle.
        observed: SourceKind,
    },
    /// A candidate runtime failed after construction but before commit.
    #[error("prepared source {key:?} is not ready: {issue}")]
    ReplacementNotReady {
        /// Immutable role key of the rejected candidate.
        key: ManagedSourceKey,
        /// Stable backend failure description.
        issue: Arc<str>,
    },
    /// Candidate manager state could not be staged from the captured plan.
    #[error("prepared source {key:?} rejected manager context: {issue}")]
    ReplacementPreparationFailed {
        /// Immutable role key of the rejected candidate.
        key: ManagedSourceKey,
        /// Stable preparation failure description.
        issue: Arc<str>,
    },
}

fn validate_source_swap_role(
    plan: &SourceSwapPlan,
    replacement: Option<&ManagedSourceRole>,
) -> Result<(), SourceSwapConflict> {
    let expected_running = match plan.target {
        SourceSwapTarget::Absent => {
            return if replacement.is_none() {
                Ok(())
            } else {
                Err(SourceSwapConflict::InvalidReplacementPresence)
            };
        }
        SourceSwapTarget::Present { running } => running,
    };
    let Some(replacement) = replacement else {
        return Err(SourceSwapConflict::InvalidReplacementPresence);
    };
    let observed = replacement.key();
    if observed != plan.key {
        return Err(SourceSwapConflict::InvalidReplacementKey {
            expected: plan.key,
            observed,
        });
    }
    let observed_running = replacement.source().is_running();
    if observed_running != expected_running {
        return Err(SourceSwapConflict::InvalidReplacementLifecycle {
            expected_running,
            observed_running,
        });
    }
    let expected = replacement.source_kind();
    if let Some(observed) = replacement
        .source()
        .source_status_handle()
        .map(|status| status.snapshot().kind)
        .filter(|observed| *observed != expected)
    {
        return Err(SourceSwapConflict::InvalidReplacementStatusKind { expected, observed });
    }
    Ok(())
}

/// Source detached by one successful typed graph swap.
#[must_use = "detached sources must be retired outside the input manager lock"]
pub struct SourceRetirement {
    source: Option<ManagedInputSource>,
    source_graph_generation: u64,
}

/// Sources detached from the canonical graph for retirement without its lock.
#[must_use = "detached sources must be retired outside the input manager lock"]
pub struct SourceRetirementBatch {
    sources: Vec<ManagedInputSource>,
    source_graph_generation: u64,
}

impl SourceRetirementBatch {
    /// Stop every detached source and permanently retire its status.
    pub fn retire(mut self) {
        for source in &mut self.sources {
            source.set_active_consumer_count(0);
            source.stop();
            if let Err(error) = source.retire_source_status(self.source_graph_generation) {
                error!(source = source.name(), %error, "Failed to retire input source status");
            }
            info!(source = source.name(), "Retired input source");
        }
    }
}

impl SourceRetirement {
    /// Stop the detached source and permanently retire its status.
    pub fn retire(mut self) {
        let Some(source) = &mut self.source else {
            return;
        };
        source.set_active_consumer_count(0);
        source.stop();
        if let Err(error) = source.retire_source_status(self.source_graph_generation) {
            error!(source = source.name(), %error, "Failed to retire input source status");
        }
        info!(source = source.name(), "Retired input source");
    }
}

impl SourceSwapPlan {
    /// Build slot, status, publication, and retained manager state away from commit.
    ///
    /// Candidate ownership remains in `replacement` when validation or retained
    /// context preparation fails.
    pub fn prepare(
        self,
        replacement: &mut Option<ManagedSourceRole>,
    ) -> Result<PreparedSourceSwap, SourceSwapConflict> {
        validate_source_swap_role(&self, replacement.as_ref())?;
        if let Some(source) = replacement.as_mut() {
            if let Some(screen) = source.as_screen_mut() {
                screen
                    .set_capability_context(&self.source_capability_context)
                    .map_err(|error| SourceSwapConflict::ReplacementPreparationFailed {
                        key: self.key,
                        issue: Arc::from(error.to_string()),
                    })?;
            }
            if let Some(interaction) = source.as_interaction_mut() {
                interaction
                    .set_capability_context(&self.source_capability_context)
                    .map_err(|error| SourceSwapConflict::ReplacementPreparationFailed {
                        key: self.key,
                        issue: Arc::from(error.to_string()),
                    })?;
            }
        }
        let replacement = replacement.take().map(|source| {
            let mut source = ManagedInputSource::new(
                source,
                self.replacement_slot_id,
                self.replacement_source_graph_generation,
                Arc::clone(&self.screen_publication_hub),
            );
            if source.is_running() {
                source.mark_prestarted_compatibility_live();
            }
            source
        });
        Ok(PreparedSourceSwap {
            plan: self,
            replacement,
        })
    }
}

impl PreparedSourceSwap {
    /// Whether the caller still owns a prepared replacement candidate.
    #[must_use]
    pub const fn has_replacement(&self) -> bool {
        self.replacement.is_some()
    }

    /// Stop and discard an uncommitted candidate.
    pub fn discard(mut self) {
        if let Some(source) = &mut self.replacement {
            source.stop();
        }
    }
}

impl ScreenSourceSwapPlan {
    /// Bind a prepared screen backend to the generic source-swap lane.
    ///
    /// Candidate ownership remains in `replacement` on failure.
    pub fn prepare(
        self,
        replacement: &mut Option<Box<dyn ScreenSource>>,
    ) -> Result<PreparedScreenSourceSwap, SourceSwapConflict> {
        let mut role = replacement.take().map(ManagedSourceRole::screen);
        let source_swap = match self.source_swap.prepare(&mut role) {
            Ok(prepared) => prepared,
            Err(error) => {
                *replacement = role.map(|role| match role {
                    ManagedSourceRole::Screen(source) => source,
                    ManagedSourceRole::Audio(_)
                    | ManagedSourceRole::Interaction(_)
                    | ManagedSourceRole::Data(_) => {
                        unreachable!("screen swap preparation preserves the screen role")
                    }
                });
                return Err(error);
            }
        };
        Ok(PreparedScreenSourceSwap {
            source_swap,
            expected_capture_demand: self.expected_capture_demand,
            capture_demand: self.capture_demand,
            capacity: self.capacity,
        })
    }
}

impl PreparedScreenSourceSwap {
    /// Whether the caller still owns a prepared replacement candidate.
    #[must_use]
    pub const fn has_replacement(&self) -> bool {
        self.source_swap.has_replacement()
    }

    /// Stop and discard an uncommitted candidate.
    pub fn discard(self) {
        self.source_swap.discard();
    }
}

/// One-shot infallible graph move issued only after every screen fence validates.
#[must_use = "screen source swap commits must be installed exactly once"]
pub struct ScreenSourceSwapCommit<'a> {
    shared: Arc<InputManagerShared>,
    prepared: &'a mut PreparedScreenSourceSwap,
    current: Option<usize>,
}

impl ScreenSourceSwapCommit<'_> {
    /// Move prepared state, install live config, then publish one visibility fence.
    #[must_use = "detached sources must be retired outside the input manager lock"]
    pub fn commit(self, install_live_config: impl FnOnce()) -> SourceRetirement {
        let mut inner = lock_mutex(&self.shared.inner);
        let state = inner
            .state
            .as_mut()
            .expect("compound screen commit retains attached manager state");
        if let Some(capacity) = self.prepared.capacity.take() {
            state.commit_screen_capacity_unpublished(capacity);
        }
        let retirement =
            state.commit_source_swap_unpublished(&mut self.prepared.source_swap, self.current);
        state.screen_capture_demand = Some(self.prepared.capture_demand);
        install_live_config();
        state.publish_source_status_registry();
        retirement
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
    pub fn prepare(self) -> anyhow::Result<PreparedAudioSourceSwap> {
        self.prepare_with(PreparedAudioReconfiguration::prepare)
    }

    /// Stage an in-memory capture runtime for deterministic transaction tests.
    #[doc(hidden)]
    pub fn prepare_with_synthetic_capture_for_testing(
        self,
    ) -> anyhow::Result<PreparedAudioSourceSwap> {
        self.prepare_with(PreparedAudioReconfiguration::prepare_with_synthetic_capture_for_testing)
    }

    fn prepare_with(
        self,
        prepare: impl FnOnce(AudioPreparationRequest) -> anyhow::Result<PreparedAudioReconfiguration>,
    ) -> anyhow::Result<PreparedAudioSourceSwap> {
        let prepared = self.preparation.map(prepare).transpose()?;
        let failure_signal = prepared
            .as_ref()
            .map(PreparedAudioReconfiguration::failure_signal);
        let mut replacement = prepared
            .map(|mut prepared| AudioInput::from_prepared(&mut prepared))
            .transpose()?
            .map(|source| ManagedSourceRole::audio(Box::new(source)));
        let source_swap = self.source_swap.prepare(&mut replacement)?;
        Ok(PreparedAudioSourceSwap {
            source_swap,
            failure_signal,
        })
    }
}

impl PreparedAudioSourceSwap {
    /// Unwrap the opaque generic source-swap candidate.
    pub fn into_source_swap(self) -> PreparedSourceSwap {
        self.source_swap
    }

    /// Inject a terminal failure after candidate construction.
    #[doc(hidden)]
    pub fn fail_before_commit_for_testing(&self) {
        if let Some(failure) = &self.failure_signal {
            failure.store(
                audio::AudioFailureKind::BackendUnavailable as u8,
                std::sync::atomic::Ordering::Release,
            );
        }
    }
}

/// Orchestrates multiple [`ManagedSource`] instances.
///
/// Owns a heterogeneous collection of input sources and provides batch
/// lifecycle management. The render loop holds one `InputManager` and
/// calls [`sample_all`] each frame.
///
/// # Example (conceptual)
///
/// ```rust,ignore
/// let mut mgr = InputManager::new();
/// mgr.add_source(ManagedSourceRole::audio(Box::new(audio_input)))?;
/// mgr.add_source(ManagedSourceRole::screen(Box::new(screen_capture)))?;
/// mgr.start_all()?;
///
/// loop {
///     let samples = mgr.sample_all();
///     // route Audio / Screen data into the pipeline...
/// }
/// ```
#[derive(Clone)]
pub struct InputManager {
    shared: Arc<InputManagerShared>,
}

struct InputManagerShared {
    inner: Mutex<InputManagerInner>,
    lifecycle: RwLock<()>,
    lifecycle_release_revision: watch::Sender<u64>,
    input_graph: InputGraphHandle,
    source_status_registry: SourceStatusRegistry,
    screen_publication_hub: Arc<screen::ScreenPublicationHub>,
    screen_admission: screen::ScreenByteAdmissionCoordinator,
    screen_capacity_status: screen::ScreenCapacityStatusHandle,
}

struct InputManagerInner {
    state: Option<InputManagerState>,
}

struct InputManagerState {
    sources: Vec<ManagedInputSource>,
    source_graph_generation: u64,
    next_source_slot_id: u64,
    input_graph: InputGraphHandle,
    source_status_registry: SourceStatusRegistry,
    event_scratch: Vec<TimedInputEvent>,
    audio_capture_active: Option<bool>,
    source_capability_context: SourceCapabilityContext,
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
}

fn lock_mutex<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct LifecycleReadGuard<'a> {
    guard: Option<std::sync::RwLockReadGuard<'a, ()>>,
    release_revision: &'a watch::Sender<u64>,
}

impl Drop for LifecycleReadGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        publish_lifecycle_release(self.release_revision);
    }
}

struct LifecycleWriteGuard<'a> {
    guard: Option<std::sync::RwLockWriteGuard<'a, ()>>,
    release_revision: &'a watch::Sender<u64>,
}

impl Drop for LifecycleWriteGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        publish_lifecycle_release(self.release_revision);
    }
}

fn publish_lifecycle_release(release_revision: &watch::Sender<u64>) {
    if release_revision.receiver_count() > 0 {
        release_revision.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

impl InputManagerShared {
    fn read_lifecycle(&self) -> LifecycleReadGuard<'_> {
        LifecycleReadGuard {
            guard: Some(read_lock(&self.lifecycle)),
            release_revision: &self.lifecycle_release_revision,
        }
    }

    fn write_lifecycle(&self) -> LifecycleWriteGuard<'_> {
        LifecycleWriteGuard {
            guard: Some(
                self.lifecycle
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
            release_revision: &self.lifecycle_release_revision,
        }
    }

    fn try_write_lifecycle(&self) -> Option<LifecycleWriteGuard<'_>> {
        let guard = match self.lifecycle.try_write() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(LifecycleWriteGuard {
            guard: Some(guard),
            release_revision: &self.lifecycle_release_revision,
        })
    }
}

struct DetachedInputManager<'a> {
    shared: &'a InputManagerShared,
    state: Option<InputManagerState>,
}

impl DetachedInputManager<'_> {
    fn state(&mut self) -> &mut InputManagerState {
        self.state
            .as_mut()
            .expect("detached input manager retains inner state")
    }
}

impl Drop for DetachedInputManager<'_> {
    fn drop(&mut self) {
        let state = self
            .state
            .take()
            .expect("detached input manager restores inner state once");
        let replaced = lock_mutex(&self.shared.inner).state.replace(state);
        debug_assert!(replaced.is_none());
    }
}

struct ManagedInputSource {
    source: ManagedSourceRole,
    slot: InputSourceSlot,
    compatibility_status: Option<SourceStatusReporter>,
}

impl ManagedInputSource {
    fn new(
        mut source: ManagedSourceRole,
        slot_id: u64,
        source_graph_generation: u64,
        screen_publication_hub: Arc<screen::ScreenPublicationHub>,
    ) -> Self {
        let declared_kind = source.source_kind();
        let interaction_origin = source.interaction_origin();
        if let Some(screen) = source.as_screen_mut() {
            screen.set_screen_publication_hub(screen_publication_hub);
        }
        let managed_source = source.source_mut();
        managed_source.set_source_graph_generation(source_graph_generation);
        let mut compatibility_status = managed_source.source_status_handle().is_none().then(|| {
            SourceStatusReporter::new(
                format!("compatibility:{slot_id}:{}", managed_source.name()),
                declared_kind,
                "compatibility",
                true,
                true,
                true,
            )
        });
        if let Some(status) = &mut compatibility_status {
            status.set_source_graph_generation(source_graph_generation);
        }
        let status = managed_source.source_status_handle().unwrap_or_else(|| {
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

    fn key(&self) -> ManagedSourceKey {
        self.source.key()
    }

    fn as_audio(&self) -> Option<&dyn AudioSource> {
        self.source.as_audio()
    }

    fn as_audio_mut(&mut self) -> Option<&mut (dyn AudioSource + 'static)> {
        self.source.as_audio_mut()
    }

    fn as_screen(&self) -> Option<&dyn ScreenSource> {
        self.source.as_screen()
    }

    fn as_screen_mut(&mut self) -> Option<&mut (dyn ScreenSource + 'static)> {
        self.source.as_screen_mut()
    }

    fn as_interaction(&self) -> Option<&dyn InteractionSource> {
        self.source.as_interaction()
    }

    fn as_interaction_mut(&mut self) -> Option<&mut (dyn InteractionSource + 'static)> {
        self.source.as_interaction_mut()
    }

    fn set_capability_context(&mut self, context: &SourceCapabilityContext) -> anyhow::Result<()> {
        if let Some(source) = self.source.as_screen_mut() {
            source.set_capability_context(context)?;
        }
        if let Some(source) = self.source.as_interaction_mut() {
            source.set_capability_context(context)?;
        }
        Ok(())
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
            .expect("validated compatibility source can begin its session")
            .expect("manager-bound compatibility source creates a session");
        session.mark_event_driven_live_without_deadline(1);
    }

    fn set_source_graph_generation(&mut self, source_graph_generation: u64) {
        self.source
            .source_mut()
            .set_source_graph_generation(source_graph_generation);
        if let Some(status) = &mut self.compatibility_status {
            status.set_source_graph_generation(source_graph_generation);
        }
    }

    fn set_active_consumer_count(&mut self, active_consumer_count: usize) {
        if let Err(error) = self
            .source
            .source_mut()
            .set_active_consumer_count(active_consumer_count)
        {
            error!(source = self.source.source().name(), %error, "Failed to publish active consumer count");
        }
        if let Some(status) = &mut self.compatibility_status
            && let Err(error) = status.set_active_consumer_count(active_consumer_count)
        {
            error!(source = self.source.source().name(), %error, "Failed to publish compatibility consumer count");
        }
    }

    fn retire_source_status(
        &mut self,
        source_graph_generation: u64,
    ) -> Result<(), SourceStatusError> {
        self.source
            .source_mut()
            .retire_source_status(source_graph_generation)?;
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
        match self.source.source_mut().start() {
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
        self.source.source_mut().stop();
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
            && self.source.source().is_running()
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
        let sample = self
            .source
            .source_mut()
            .sample_shared_and_drain_into(delta_secs, events);
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
    type Target = dyn ManagedSource;

    fn deref(&self) -> &Self::Target {
        self.source.source()
    }
}

impl DerefMut for ManagedInputSource {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.source.source_mut()
    }
}

impl AsRef<dyn ManagedSource> for ManagedInputSource {
    fn as_ref(&self) -> &(dyn ManagedSource + 'static) {
        self.source.source()
    }
}

impl AsMut<dyn ManagedSource> for ManagedInputSource {
    fn as_mut(&mut self) -> &mut (dyn ManagedSource + 'static) {
        self.source.source_mut()
    }
}

impl InputManager {
    /// Create an empty cloneable input service.
    #[must_use]
    pub fn new() -> Self {
        let state = InputManagerState::new();
        let (lifecycle_release_revision, _) = watch::channel(0);
        let shared = InputManagerShared {
            input_graph: state.input_graph_handle(),
            source_status_registry: state.source_status_registry(),
            screen_publication_hub: state.screen_publication_hub(),
            screen_admission: state.screen_admission_coordinator(),
            screen_capacity_status: state.screen_capacity_status_handle(),
            inner: Mutex::new(InputManagerInner { state: Some(state) }),
            lifecycle: RwLock::new(()),
            lifecycle_release_revision,
        };
        Self {
            shared: Arc::new(shared),
        }
    }

    fn with_inner<R>(&self, operation: impl FnOnce(&mut InputManagerState) -> R) -> R {
        let _lifecycle = self.shared.read_lifecycle();
        let mut inner = lock_mutex(&self.shared.inner);
        operation(
            inner
                .state
                .as_mut()
                .expect("input manager state is attached under lifecycle read access"),
        )
    }

    fn with_detached_inner<R>(&self, operation: impl FnOnce(&mut InputManagerState) -> R) -> R {
        let _lifecycle = self.shared.write_lifecycle();
        let state = lock_mutex(&self.shared.inner)
            .state
            .take()
            .expect("input manager state is attached before lifecycle detachment");
        let mut detached = DetachedInputManager {
            shared: &self.shared,
            state: Some(state),
        };
        operation(detached.state())
    }

    fn try_with_detached_inner_if<R>(
        &self,
        is_current: impl FnOnce() -> bool,
        operation: impl FnOnce(&mut InputManagerState) -> R,
    ) -> TryInputManagerIntent<R> {
        let Some(_lifecycle) = self.shared.try_write_lifecycle() else {
            return TryInputManagerIntent::Busy;
        };
        if !is_current() {
            return TryInputManagerIntent::Stale;
        }
        let state = lock_mutex(&self.shared.inner)
            .state
            .take()
            .expect("input manager state is attached after lifecycle acquisition");
        let mut detached = DetachedInputManager {
            shared: &self.shared,
            state: Some(state),
        };
        TryInputManagerIntent::Applied(operation(detached.state()))
    }

    /// Wait for lifecycle availability after a nonblocking intent returns busy.
    ///
    /// The method subscribes before immediately probing write ownership. A
    /// successful probe closes the release-before-subscribe race without an
    /// await; a second busy result waits for the owning guard's release.
    pub async fn wait_for_lifecycle_release_after_busy(&self) {
        let mut revision = self.shared.lifecycle_release_revision.subscribe();
        if let Some(guard) = self.shared.try_write_lifecycle() {
            drop(guard);
            return;
        }
        revision
            .changed()
            .await
            .expect("input manager retains the lifecycle release publisher");
    }

    pub fn add_source(&self, source: ManagedSourceRole) -> Result<(), SourceRegistrationError> {
        self.with_inner(|inner| inner.add_source(source))
    }

    pub fn plan_source_swap(
        &self,
        key: ManagedSourceKey,
        target: SourceSwapTarget,
    ) -> Result<SourceSwapPlan, SourceSwapConflict> {
        self.with_inner(|inner| inner.plan_source_swap(key, target))
    }

    pub fn commit_source_swap(
        &self,
        prepared: &mut PreparedSourceSwap,
    ) -> Result<SourceRetirement, SourceSwapConflict> {
        self.with_inner(|inner| inner.commit_source_swap(prepared))
    }

    /// Try one generation-fenced source commit after a lock-free freshness check.
    ///
    /// `is_current` runs only after exclusive lifecycle ownership is acquired.
    /// It must perform lock-free reads only and must not call back into this manager.
    pub fn try_commit_source_swap_if(
        &self,
        prepared: &mut PreparedSourceSwap,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<Result<SourceRetirement, SourceSwapConflict>> {
        self.try_with_detached_inner_if(is_current, |state| state.commit_source_swap(prepared))
    }

    #[must_use]
    pub fn input_graph_handle(&self) -> InputGraphHandle {
        self.shared.input_graph.clone()
    }

    #[must_use]
    pub fn source_status_registry(&self) -> SourceStatusRegistry {
        self.shared.source_status_registry.clone()
    }

    #[must_use]
    pub fn source_graph_generation(&self) -> u64 {
        self.with_inner(|inner| inner.source_graph_generation())
    }

    #[must_use]
    pub fn source_count(&self) -> usize {
        self.with_inner(|inner| inner.source_count())
    }

    #[must_use]
    pub fn source_names(&self) -> Vec<String> {
        self.with_inner(|inner| inner.source_names())
    }

    pub fn sample_sources(&self, delta_secs: f32) {
        self.with_detached_inner(|inner| inner.sample_sources(delta_secs));
    }

    pub fn sample_source_kinds(&self, due_sources: &[(SourceKind, f32)]) {
        self.with_detached_inner(|inner| inner.sample_source_kinds(due_sources));
    }

    /// Try sampling due sources after validating caller-owned lock-free state.
    ///
    /// `is_current` runs only after exclusive lifecycle ownership is acquired.
    /// It must perform lock-free reads only and must not call back into this manager.
    pub fn try_sample_source_kinds_if(
        &self,
        due_sources: &[(SourceKind, f32)],
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<()> {
        self.try_with_detached_inner_if(is_current, |state| {
            state.sample_source_kinds(due_sources);
        })
    }

    pub fn sample_all(&self) -> Vec<InputData> {
        self.with_detached_inner(InputManagerState::sample_all)
    }

    pub fn sample_all_with_delta_secs(&self, delta_secs: f32) -> Vec<InputData> {
        self.with_detached_inner(|inner| inner.sample_all_with_delta_secs(delta_secs))
    }

    #[must_use]
    pub fn drain_events(&self) -> Vec<TimedInputEvent> {
        self.with_detached_inner(InputManagerState::drain_events)
    }

    pub fn sample_and_drain_with_delta_secs(
        &self,
        delta_secs: f32,
    ) -> (Vec<InputData>, Vec<TimedInputEvent>) {
        self.with_detached_inner(|inner| inner.sample_and_drain_with_delta_secs(delta_secs))
    }

    pub fn set_interaction_capture_active(&self, active: bool) -> anyhow::Result<()> {
        self.with_detached_inner(|inner| inner.set_interaction_capture_active(active))
    }

    pub fn start_all(&self) -> anyhow::Result<()> {
        self.with_detached_inner(InputManagerState::start_all)
    }

    pub fn stop_all(&self) {
        self.with_detached_inner(InputManagerState::stop_all);
    }

    /// Detach every source and publish the empty graph for shutdown.
    pub fn detach_all_sources(&self) -> SourceRetirementBatch {
        let _lifecycle = self.shared.write_lifecycle();
        let mut inner = lock_mutex(&self.shared.inner);
        let state = inner
            .state
            .as_mut()
            .expect("input manager state is attached during shutdown detachment");
        let source_graph_generation = state.bump_source_graph_generation();
        let sources = std::mem::take(&mut state.sources);
        state.invalidate_capture_domains((true, true, true));
        state.publish_source_status_registry();
        SourceRetirementBatch {
            sources,
            source_graph_generation,
        }
    }

    pub fn plan_audio_runtime_config(
        &self,
        enabled: bool,
        config: &AudioPipelineConfig,
        display_name: &str,
        capture_active: bool,
    ) -> anyhow::Result<AudioRuntimeConfigPlan> {
        self.with_inner(|inner| {
            inner.plan_audio_runtime_config(enabled, config, display_name, capture_active)
        })
    }

    /// Try planning an audio replacement after a lock-free freshness check.
    pub fn try_plan_audio_runtime_config_if(
        &self,
        enabled: bool,
        config: &AudioPipelineConfig,
        display_name: &str,
        capture_active: bool,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<anyhow::Result<AudioRuntimeConfigPlan>> {
        self.try_with_detached_inner_if(is_current, |inner| {
            inner.plan_audio_runtime_config(enabled, config, display_name, capture_active)
        })
    }

    pub fn set_audio_capture_active(&self, active: bool) -> anyhow::Result<()> {
        self.with_detached_inner(|inner| inner.set_audio_capture_active(active))
    }

    pub fn set_screen_capture_demand(&self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        self.with_detached_inner(|inner| inner.set_screen_capture_demand(demand))
    }

    /// Try applying every capture domain inside one lifecycle transaction.
    ///
    /// Domain failures do not prevent later domains from running. Each domain
    /// retains its existing rollback behavior. `is_current` runs after exclusive
    /// lifecycle ownership is acquired and must perform lock-free reads only.
    pub fn try_apply_capture_demand_if(
        &self,
        demand: InputCaptureDemand,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<InputCaptureDemandApplication> {
        self.try_with_detached_inner_if(is_current, |state| {
            let audio_generation = state.source_graph_generation();
            let audio = state.set_audio_capture_active(demand.audio_active);
            let screen_generation = state.source_graph_generation();
            let screen = state.set_screen_capture_demand(demand.screen);
            let interaction_generation = state.source_graph_generation();
            let interaction = state.set_interaction_capture_active(demand.interaction_active);
            InputCaptureDemandApplication {
                source_graph_generation: state.source_graph_generation(),
                audio: (audio_generation, audio),
                screen: (screen_generation, screen),
                interaction: (interaction_generation, interaction),
            }
        })
    }

    #[must_use]
    pub fn screen_publication_hub(&self) -> Arc<screen::ScreenPublicationHub> {
        Arc::clone(&self.shared.screen_publication_hub)
    }

    #[must_use]
    pub fn screen_admission_coordinator(&self) -> screen::ScreenByteAdmissionCoordinator {
        self.shared.screen_admission.clone()
    }

    #[must_use]
    pub fn screen_capacity_status_handle(&self) -> screen::ScreenCapacityStatusHandle {
        self.shared.screen_capacity_status.clone()
    }

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

    pub fn set_screen_resource_capacity(
        &self,
        capacity: screen::ScreenAdmissionCapacity,
    ) -> Result<(), screen::ScreenByteAdmissionError> {
        self.with_inner(|inner| inner.set_screen_resource_capacity(capacity))
    }

    pub fn set_screen_capacity_plan(
        &self,
        resource: screen::ScreenAdmissionCapacity,
        total: screen::ScreenAdmissionCapacity,
        publication: screen::ScreenAdmissionCapacity,
    ) -> Result<(), screen::ScreenByteAdmissionError> {
        self.with_inner(|inner| inner.set_screen_capacity_plan(resource, total, publication))
    }

    #[must_use]
    pub fn screen_resource_capacity(&self) -> screen::ScreenAdmissionCapacity {
        self.with_inner(|inner| inner.screen_resource_capacity())
    }

    #[must_use]
    pub fn screen_total_capacity(&self) -> screen::ScreenAdmissionCapacity {
        self.with_inner(|inner| inner.screen_total_capacity())
    }

    #[must_use]
    pub fn screen_publication_capacity(&self) -> screen::ScreenAdmissionCapacity {
        self.with_inner(|inner| inner.screen_publication_capacity())
    }

    pub fn screen_analysis_resource_plan(
        &self,
    ) -> anyhow::Result<Option<screen::ScreenAnalysisResourcePlan>> {
        self.with_inner(|inner| inner.screen_analysis_resource_plan())
    }

    pub fn screen_analysis_work_plan(
        &self,
    ) -> anyhow::Result<Option<screen::ScreenAnalysisWorkPlan>> {
        self.with_inner(|inner| inner.screen_analysis_work_plan())
    }

    #[must_use]
    pub fn screen_analysis_compute_capacity(
        &self,
    ) -> Option<screen::ScreenAnalysisComputeCapacity> {
        self.with_inner(|inner| inner.screen_analysis_compute_capacity())
    }

    #[must_use]
    pub fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.with_inner(|inner| inner.screen_capture_demand())
    }

    pub fn prepare_screen_capacity(
        &self,
        analysis_peak_bytes: u64,
    ) -> Result<Option<ScreenCapacityPreparation>, ScreenCapacityPreparationError> {
        self.with_inner(|inner| inner.prepare_screen_capacity(analysis_peak_bytes))
    }

    pub fn prepare_screen_capacity_plan(
        &self,
        total_capacity: screen::ScreenAdmissionCapacity,
        analysis_peak_bytes: u64,
    ) -> Result<Option<ScreenCapacityPreparation>, ScreenCapacityPreparationError> {
        self.with_inner(|inner| {
            inner.prepare_screen_capacity_plan(total_capacity, analysis_peak_bytes)
        })
    }

    pub fn validate_screen_capacity(
        &self,
        preparation: &ScreenCapacityPreparation,
    ) -> Result<(), ScreenReconfigurationConflict> {
        self.with_inner(|inner| inner.validate_screen_capacity(preparation))
    }

    pub fn commit_screen_capacity(
        &self,
        preparation: ScreenCapacityPreparation,
    ) -> Result<(), ScreenReconfigurationConflict> {
        self.with_inner(|inner| inner.commit_screen_capacity(preparation))
    }

    #[must_use]
    pub fn screen_publication_resolution_revision(&self) -> u64 {
        self.with_inner(InputManagerState::screen_publication_resolution_revision)
    }

    #[must_use]
    pub fn screen_publication_commitment_is_current(&self) -> bool {
        self.with_detached_inner(InputManagerState::screen_publication_commitment_is_current)
    }

    /// Try probing exact-screen commitment after a lock-free freshness check.
    pub fn try_screen_publication_commitment_is_current_if(
        &self,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<bool> {
        self.try_with_detached_inner_if(is_current, |state| {
            state.screen_publication_commitment_is_current()
        })
    }

    pub fn begin_screen_publication_transition(
        &self,
        demand: ScreenPublicationDemandSnapshot,
    ) -> Result<
        Option<screen::ScreenPublicationPreparation>,
        screen::ScreenPublicationTransitionError,
    > {
        self.with_detached_inner(|inner| inner.begin_screen_publication_transition(demand))
    }

    /// Try beginning an exact-screen transition after a lock-free freshness check.
    pub fn try_begin_screen_publication_transition_if(
        &self,
        demand: &ScreenPublicationDemandSnapshot,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<
        Result<
            Option<screen::ScreenPublicationPreparation>,
            screen::ScreenPublicationTransitionError,
        >,
    > {
        self.try_with_detached_inner_if(is_current, |state| {
            state.begin_screen_publication_transition(demand.clone())
        })
    }

    pub fn commit_screen_publication_transition(
        &self,
        prepared: screen::PreparedScreenPublicationPlan,
        observed_demand_revision: screen::InputPublicationDemandRevision,
    ) -> Result<
        screen::CommittedScreenPublicationTransition,
        screen::ScreenPublicationTransitionFailure,
    > {
        self.with_detached_inner(|inner| {
            inner.commit_screen_publication_transition(prepared, observed_demand_revision)
        })
    }

    /// Try committing an exact-screen transition after a lock-free freshness check.
    pub fn try_commit_screen_publication_transition_if(
        &self,
        prepared: &mut Option<screen::PreparedScreenPublicationPlan>,
        observed_demand_revision: screen::InputPublicationDemandRevision,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<
        Result<
            screen::CommittedScreenPublicationTransition,
            screen::ScreenPublicationTransitionFailure,
        >,
    > {
        self.try_with_detached_inner_if(is_current, |state| {
            state.commit_screen_publication_transition(
                prepared
                    .take()
                    .expect("prepared screen publication plan is consumed once"),
                observed_demand_revision,
            )
        })
    }

    #[must_use]
    pub fn has_screen_source(&self) -> bool {
        self.with_inner(|inner| inner.has_screen_source())
    }

    pub fn plan_screen_source_swap(
        &self,
        enabled: bool,
        capacity: Option<ScreenCapacityPreparation>,
    ) -> Result<ScreenSourceSwapPlan, SourceSwapConflict> {
        self.with_inner(|inner| inner.plan_screen_source_swap(enabled, capacity))
    }

    /// Try planning a screen replacement after a lock-free freshness check.
    pub fn try_plan_screen_source_swap_if(
        &self,
        enabled: bool,
        capacity: Option<ScreenCapacityPreparation>,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<Result<ScreenSourceSwapPlan, SourceSwapConflict>> {
        self.try_with_detached_inner_if(is_current, |inner| {
            inner.plan_screen_source_swap(enabled, capacity)
        })
    }

    /// Try planning screen capacity and source replacement in one lifecycle transaction.
    pub fn try_plan_screen_source_swap_with_capacity_if(
        &self,
        enabled: bool,
        total_capacity: screen::ScreenAdmissionCapacity,
        analysis_peak_bytes: u64,
        is_current: impl FnOnce() -> bool,
    ) -> TryInputManagerIntent<Result<ScreenSourceSwapPlan, ScreenSourceSwapPlanningError>> {
        self.try_with_detached_inner_if(is_current, |inner| {
            let capacity =
                inner.prepare_screen_capacity_plan(total_capacity, analysis_peak_bytes)?;
            if enabled && capacity.is_none() {
                return Err(ScreenSourceSwapPlanningError::CapacityUnavailable);
            }
            Ok(inner.plan_screen_source_swap(enabled, capacity)?)
        })
    }

    pub fn commit_screen_source_swap<E>(
        &self,
        prepared: &mut PreparedScreenSourceSwap,
        persist_and_install: impl FnOnce(ScreenSourceSwapCommit<'_>) -> Result<SourceRetirement, E>,
    ) -> Result<SourceRetirement, ScreenSourceSwapCommitError<E>> {
        let _lifecycle = self.shared.write_lifecycle();
        let current = {
            let mut inner = lock_mutex(&self.shared.inner);
            let state = inner
                .state
                .as_mut()
                .expect("compound screen validation retains attached manager state");
            let current = state
                .validate_prepared_source_swap(&mut prepared.source_swap)
                .map_err(ScreenReconfigurationConflict::from)?;
            if state.current_screen_capture_demand() != prepared.expected_capture_demand {
                return Err(ScreenReconfigurationConflict::CaptureDemandChanged.into());
            }
            if prepared
                .source_swap
                .replacement
                .as_ref()
                .is_some_and(|source| {
                    source.as_screen().is_none_or(|source| {
                        source.screen_capture_demand() != prepared.capture_demand
                    })
                })
            {
                return Err(ScreenReconfigurationConflict::InvalidReplacementDemand.into());
            }
            if let Some(capacity) = &prepared.capacity {
                state.validate_screen_capacity(capacity)?;
            }
            current
        };
        persist_and_install(ScreenSourceSwapCommit {
            shared: Arc::clone(&self.shared),
            prepared,
            current,
        })
        .map_err(ScreenSourceSwapCommitError::Persistence)
    }

    #[must_use]
    pub fn has_interaction_source(&self) -> bool {
        self.with_inner(|inner| inner.has_interaction_source())
    }

    #[must_use]
    pub fn interaction_diagnostics(&self) -> Vec<InteractionDiagnostics> {
        self.with_inner(|inner| inner.interaction_diagnostics())
    }

    #[must_use]
    pub fn has_host_capture_source(&self) -> bool {
        self.with_inner(|inner| inner.has_host_capture_source())
    }

    pub fn reconfigure_screen_capture(&self, config: &screen::CaptureConfig) -> anyhow::Result<()> {
        self.with_detached_inner(|inner| inner.reconfigure_screen_capture(config))
    }

    pub fn reconfigure_screen_processing(
        &self,
        config: &screen::CaptureConfig,
    ) -> anyhow::Result<()> {
        self.with_detached_inner(|inner| inner.reconfigure_screen_processing(config))
    }

    pub fn set_source_capability_context(
        &self,
        context: SourceCapabilityContext,
    ) -> anyhow::Result<()> {
        self.with_detached_inner(|inner| inner.set_source_capability_context(context))
    }

    pub fn set_source_capability_identity(
        &self,
        owner: impl Into<Arc<str>>,
        conflict: Option<SourceCapabilityConflict>,
        identity_hash: Option<Arc<str>>,
    ) -> anyhow::Result<()> {
        let owner = owner.into();
        self.with_detached_inner(|inner| {
            inner.set_source_capability_identity(owner, conflict, identity_hash)
        })
    }

    /// Try publishing retained capability identity without waiting on lifecycle ownership.
    pub fn try_set_source_capability_identity(
        &self,
        owner: impl Into<Arc<str>>,
        conflict: Option<SourceCapabilityConflict>,
        identity_hash: Option<Arc<str>>,
    ) -> TryInputManagerIntent<anyhow::Result<()>> {
        let owner = owner.into();
        self.try_with_detached_inner_if(
            || true,
            |state| state.set_source_capability_identity(owner, conflict, identity_hash),
        )
    }

    pub fn set_source_capability_feature(
        &self,
        name: impl Into<Arc<str>>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let name = name.into();
        self.with_detached_inner(|inner| inner.set_source_capability_feature(name, enabled))
    }

    #[must_use]
    pub fn input_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        self.with_inner(|inner| inner.input_authorization_action())
    }

    #[must_use]
    pub fn resolved_input_authorization_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ProtectedSourceAuthorizationAction>> {
        self.with_inner(|inner| inner.resolved_input_authorization_action())
    }

    #[must_use]
    pub fn screen_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        self.with_inner(|inner| inner.screen_authorization_action())
    }

    #[must_use]
    pub fn resolved_screen_authorization_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ProtectedSourceAuthorizationAction>> {
        self.with_inner(|inner| inner.resolved_screen_authorization_action())
    }

    #[must_use]
    pub fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        self.with_inner(|inner| inner.screen_source_picker_action())
    }

    #[must_use]
    pub fn resolved_screen_source_picker_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ScreenSourcePickerAction>> {
        self.with_inner(|inner| inner.resolved_screen_source_picker_action())
    }

    #[must_use]
    pub fn diagnostic_artifact_action(&self) -> Option<SourceDiagnosticArtifactAction> {
        self.with_inner(|inner| inner.diagnostic_artifact_action())
    }

    pub fn reselect_screen_source(&self) -> anyhow::Result<()> {
        self.with_detached_inner(InputManagerState::reselect_screen_source)
    }
}

impl InputManagerState {
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
            source_capability_context: SourceCapabilityContext {
                owner: Arc::from("standalone"),
                ..SourceCapabilityContext::default()
            },
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
        }
    }

    /// Register a new input source.
    ///
    /// Sources are sampled in registration order. Adding a source does not
    /// start it — call [`start_all`] or start sources individually.
    pub fn add_source(&mut self, source: ManagedSourceRole) -> Result<(), SourceRegistrationError> {
        let key = source.key();
        let expected = source.source_kind();
        if let Some(observed) = source
            .source()
            .source_status_handle()
            .map(|status| status.snapshot().kind)
            .filter(|observed| *observed != expected)
        {
            return Err(SourceRegistrationError::StatusKindMismatch {
                key,
                expected,
                observed,
            });
        }
        let domains = managed_source_capture_domains(key);
        let source_graph_generation = self.bump_source_graph_generation();
        info!(source = source.source().name(), "Registered input source");
        let managed = self.create_managed_source(source, source_graph_generation);
        self.sources.push(managed);
        self.invalidate_capture_domains(domains);
        self.publish_source_status_registry();
        Ok(())
    }

    /// Capture one exact typed source state for a later compare-and-swap commit.
    ///
    /// # Errors
    ///
    /// Returns an ambiguity error when more than one registered slot has the
    /// requested role key.
    pub fn plan_source_swap(
        &self,
        key: ManagedSourceKey,
        target: SourceSwapTarget,
    ) -> Result<SourceSwapPlan, SourceSwapConflict> {
        let current = self.unique_source_index(key)?;
        Ok(SourceSwapPlan {
            key,
            expected_graph_generation: self.source_graph_generation,
            expected_slot_id: current.map(|index| self.sources[index].slot.id()),
            expected_running: current.map(|index| self.sources[index].is_running()),
            replacement_slot_id: self.next_source_slot_id,
            replacement_source_graph_generation: self
                .source_graph_generation
                .checked_add(1)
                .expect("input source graph generation exhausted"),
            source_capability_context: self.source_capability_context.clone(),
            screen_publication_hub: self.screen_plan_builder.publication_hub(),
            target,
        })
    }

    /// Commit one prepared typed source if every plan fence still matches.
    ///
    /// Every rejection leaves the candidate in the opaque prepared swap and
    /// does not mutate the graph. The detached old source remains live until
    /// the caller invokes [`SourceRetirement::retire`] outside the manager lock.
    ///
    /// # Errors
    ///
    /// Returns a typed conflict when the graph, source slot, lifecycle, or
    /// replacement changed after planning.
    pub fn commit_source_swap(
        &mut self,
        prepared: &mut PreparedSourceSwap,
    ) -> Result<SourceRetirement, SourceSwapConflict> {
        let current = self.validate_prepared_source_swap(prepared)?;
        let retirement = self.commit_source_swap_unpublished(prepared, current);
        self.publish_source_status_registry();
        Ok(retirement)
    }

    fn validate_prepared_source_swap(
        &self,
        prepared: &mut PreparedSourceSwap,
    ) -> Result<Option<usize>, SourceSwapConflict> {
        let plan = &prepared.plan;
        validate_source_swap_role(
            plan,
            prepared.replacement.as_ref().map(|source| &source.source),
        )?;
        let current = self.validate_source_swap(plan)?;
        if prepared.replacement.is_some() && self.next_source_slot_id != plan.replacement_slot_id {
            return Err(SourceSwapConflict::SourceChanged { key: plan.key });
        }
        if let Some(audio) = prepared
            .replacement
            .as_mut()
            .and_then(ManagedInputSource::as_audio_mut)
        {
            audio.ensure_prepared_source_ready().map_err(|issue| {
                SourceSwapConflict::ReplacementNotReady {
                    key: plan.key,
                    issue,
                }
            })?;
        }
        Ok(current)
    }

    fn commit_source_swap_unpublished(
        &mut self,
        prepared: &mut PreparedSourceSwap,
        current: Option<usize>,
    ) -> SourceRetirement {
        let plan = &prepared.plan;
        let prepared_audio_capture_active = (plan.key == ManagedSourceKey::Audio).then(|| {
            prepared.replacement.as_ref().map_or(Some(false), |source| {
                Some(
                    source
                        .source_status_handle()
                        .availability_at(Instant::now())
                        .demanded,
                )
            })
        });
        let source_graph_generation = self.bump_source_graph_generation();
        if current.is_none() && plan.target == SourceSwapTarget::Absent {
            if let Some(Some(active)) = prepared_audio_capture_active {
                self.audio_capture_active = Some(active);
            } else {
                self.invalidate_capture_domains(managed_source_capture_domains(plan.key));
            }
            return SourceRetirement {
                source: None,
                source_graph_generation,
            };
        }

        let replacement = prepared.replacement.take();
        if replacement.is_some() {
            self.next_source_slot_id = self
                .next_source_slot_id
                .checked_add(1)
                .expect("input source slot identity exhausted");
        }
        let retired = match (current, replacement) {
            (Some(index), Some(prepared)) => {
                Some(std::mem::replace(&mut self.sources[index], prepared))
            }
            (Some(index), None) => Some(self.sources.remove(index)),
            (None, Some(prepared)) => {
                self.sources.push(prepared);
                None
            }
            (None, None) => unreachable!("empty source swap returned before graph mutation"),
        };
        if let Some(Some(active)) = prepared_audio_capture_active {
            self.audio_capture_active = Some(active);
        } else {
            self.invalidate_capture_domains(managed_source_capture_domains(plan.key));
        }
        SourceRetirement {
            source: retired,
            source_graph_generation,
        }
    }

    fn validate_source_swap(
        &self,
        plan: &SourceSwapPlan,
    ) -> Result<Option<usize>, SourceSwapConflict> {
        if self.source_graph_generation != plan.expected_graph_generation {
            return Err(SourceSwapConflict::GraphChanged);
        }
        let current = self.unique_source_index(plan.key)?;
        if current.map(|index| self.sources[index].slot.id()) != plan.expected_slot_id {
            return Err(SourceSwapConflict::SourceChanged { key: plan.key });
        }
        if current.map(|index| self.sources[index].is_running()) != plan.expected_running {
            return Err(SourceSwapConflict::SourceLifecycleChanged { key: plan.key });
        }

        Ok(current)
    }

    fn unique_source_index(
        &self,
        key: ManagedSourceKey,
    ) -> Result<Option<usize>, SourceSwapConflict> {
        let mut matches = self
            .sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| (source.key() == key).then_some(index));
        let first = matches.next();
        if matches.next().is_some() {
            return Err(SourceSwapConflict::AmbiguousKey { key });
        }
        Ok(first)
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
            let source_kind = source.slot.kind();
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
        self.sources
            .iter_mut()
            .map(|source| {
                source
                    .sample_with_delta_secs(delta_secs)
                    .unwrap_or_else(|err| {
                        error!(source = source.name(), %err, "Input sample failed");
                        InputData::None
                    })
            })
            .collect()
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
        let mut samples = Vec::with_capacity(self.sources.len());
        let mut events = Vec::new();
        for source in &mut self.sources {
            let (sample, mut source_events) = source.sample_and_drain_with_delta_secs(delta_secs);
            samples.push(sample.unwrap_or_else(|err| {
                error!(source = source.name(), %err, "Input sample failed");
                InputData::None
            }));
            events.append(&mut source_events);
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
        self.transition_capture_demand(
            active,
            |manager| manager.interaction_capture_active,
            |manager, demand| manager.interaction_capture_active = demand,
            |source| source.as_interaction().is_some(),
            |source, demand| {
                source
                    .as_interaction_mut()
                    .expect("interaction route matches typed source")
                    .set_interaction_capture_active(demand)
            },
        )
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
                for started in self.sources[..source_index].iter_mut().rev() {
                    started.stop();
                }
                self.invalidate_capture_domains((true, true, true));
                self.publish_source_status_registry();
                return Err(err);
            }
            info!(
                source = self.sources[source_index].name(),
                "Started input source"
            );
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Stop all registered sources. Never fails — errors are logged and swallowed.
    pub fn stop_all(&mut self) {
        for source in &mut self.sources {
            info!(source = source.name(), "Stopping input source");
            source.stop();
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
    /// Returns an error if the audio role is ambiguous.
    pub fn plan_audio_runtime_config(
        &self,
        enabled: bool,
        config: &AudioPipelineConfig,
        display_name: &str,
        capture_active: bool,
    ) -> anyhow::Result<AudioRuntimeConfigPlan> {
        let source_index = self.unique_source_index(ManagedSourceKey::Audio)?;
        let source = source_index.map(|index| &self.sources[index]);
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
        let running = source.is_none_or(|source| source.is_running());
        let target = if enabled || source.is_some() {
            SourceSwapTarget::Present { running }
        } else {
            SourceSwapTarget::Absent
        };
        let source_swap = self.plan_source_swap(ManagedSourceKey::Audio, target)?;
        let preparation =
            matches!(target, SourceSwapTarget::Present { .. }).then(|| AudioPreparationRequest {
                running,
                source_graph_generation: self
                    .source_graph_generation
                    .checked_add(1)
                    .expect("input source graph generation exhausted"),
                predecessor_status: source.map(ManagedInputSource::source_status_handle),
                config: effective_config,
                name: display_name.to_owned(),
                capture_active,
            });
        Ok(AudioRuntimeConfigPlan {
            source_swap,
            preparation,
        })
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
        self.transition_capture_demand(
            active,
            |manager| manager.audio_capture_active,
            |manager, demand| manager.audio_capture_active = demand,
            |source| source.as_audio().is_some(),
            |source, demand| {
                source
                    .as_audio_mut()
                    .expect("audio route matches typed source")
                    .set_audio_capture_active(demand)
            },
        )
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
        let Some(source) = self.sources.iter().find_map(ManagedInputSource::as_screen) else {
            return Ok(None);
        };
        source.screen_analysis_resource_plan(self.current_screen_capture_demand())
    }

    /// Return the complete compatibility-analysis workload for current demand.
    pub fn screen_analysis_work_plan(
        &self,
    ) -> anyhow::Result<Option<screen::ScreenAnalysisWorkPlan>> {
        let Some(source) = self.sources.iter().find_map(ManagedInputSource::as_screen) else {
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
            .find_map(ManagedInputSource::as_screen)
            .and_then(ScreenSource::screen_analysis_compute_capacity)
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
            return Err(SourceSwapConflict::GraphChanged.into());
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
                .find_map(ManagedInputSource::as_screen)
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
            .filter_map(ManagedInputSource::as_screen)
            .count();
        let changed = source_count != self.screen_publication_source_snapshot.len()
            || self
                .sources
                .iter()
                .filter_map(|source| Some((source.slot.id(), source.as_screen()?)))
                .zip(&self.screen_publication_source_snapshot)
                .any(|((slot_id, source), observed)| {
                    *observed != (slot_id, source.screen_publication_resolution_revision())
                });
        if changed {
            self.screen_publication_source_snapshot.clear();
            self.screen_publication_source_snapshot
                .reserve(source_count);
            self.screen_publication_source_snapshot.extend(
                self.sources
                    .iter()
                    .filter_map(|source| Some((source.slot.id(), source.as_screen()?)))
                    .map(|(slot_id, source)| {
                        (slot_id, source.screen_publication_resolution_revision())
                    }),
            );
            self.screen_publication_resolution_revision = self
                .screen_publication_resolution_revision
                .checked_add(1)
                .expect("screen publication resolution revision exhausted");
        }
        self.screen_publication_resolution_revision
    }

    /// Whether the committed screen publication plan still matches the
    /// sources' current resolution state. A source-internal invalidation
    /// (a capture worker retiring its publication source) advances the
    /// resolution revision without touching the demand revision or the
    /// structural graph generation, so the publication loop must probe
    /// this to notice a committed-but-starved plan and re-plan.
    #[must_use]
    pub fn screen_publication_commitment_is_current(&mut self) -> bool {
        let current = self.screen_publication_resolution_revision();
        self.committed_screen_publication_resolution_revision == Some(current)
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
                let Some(screen) = source.as_screen() else {
                    continue;
                };
                let candidate =
                    screen
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
                        source
                            .as_screen()
                            .is_some_and(|screen| screen.owns_screen_publication_source(&source_id))
                    })
                });
            let preparation = if let Some(owner) = owner {
                match self.sources[owner]
                    .as_screen_mut()
                    .expect("screen owner index binds typed screen source")
                    .begin_screen_publication_preparation(ticket)
                {
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
            .filter_map(|source| {
                let name = Arc::from(source.name());
                source
                    .as_screen_mut()?
                    .begin_screen_publication_retirement()
                    .map(|retirement| (name, retirement))
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
        self.sources
            .iter()
            .any(|source| source.key() == ManagedSourceKey::Screen)
    }

    /// Snapshot one exact screen swap on the generic source lane.
    ///
    /// # Errors
    ///
    /// Returns an ambiguity error when more than one screen source is registered.
    pub fn plan_screen_source_swap(
        &self,
        enabled: bool,
        capacity: Option<ScreenCapacityPreparation>,
    ) -> Result<ScreenSourceSwapPlan, SourceSwapConflict> {
        let current_demand = self.current_screen_capture_demand();
        Ok(ScreenSourceSwapPlan {
            source_swap: self.plan_source_swap(
                ManagedSourceKey::Screen,
                if enabled {
                    SourceSwapTarget::Present { running: true }
                } else {
                    SourceSwapTarget::Absent
                },
            )?,
            expected_capture_demand: current_demand,
            capture_demand: if enabled {
                current_demand
            } else {
                ScreenCaptureDemand::Inactive
            },
            capacity,
        })
    }

    /// Whether any registered source captures host interaction.
    #[must_use]
    pub fn has_interaction_source(&self) -> bool {
        self.sources
            .iter()
            .any(|source| matches!(source.key(), ManagedSourceKey::Interaction(_)))
    }

    /// Collect health snapshots from every interaction source.
    #[must_use]
    pub fn interaction_diagnostics(&self) -> Vec<InteractionDiagnostics> {
        self.sources
            .iter()
            .filter_map(ManagedInputSource::as_interaction)
            .filter_map(InteractionSource::interaction_diagnostics)
            .collect()
    }

    /// Whether any registered source captures from host input hardware.
    ///
    /// Excludes the always-present browser injection source, so consent
    /// config can tell whether host capture is actually wired up.
    #[must_use]
    pub fn has_host_capture_source(&self) -> bool {
        self.sources.iter().any(|source| {
            source.key() == ManagedSourceKey::Interaction(InteractionSourceOrigin::Host)
        })
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
        if !self
            .sources
            .iter()
            .any(|source| source.key() == ManagedSourceKey::Screen)
        {
            return Ok(());
        }
        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            if source.key() == ManagedSourceKey::Screen {
                source.set_source_graph_generation(source_graph_generation);
            }
        }
        let mut result = Ok(());
        for source in &mut self.sources {
            if let Some(screen) = source.as_screen_mut() {
                if let Err(error) = screen.reconfigure_screen_capture(config) {
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
            if let Some(screen) = source.as_screen_mut() {
                screen.reconfigure_screen_processing(config)?;
            }
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Apply the active process capability context to every native source.
    ///
    /// # Errors
    ///
    /// Returns an error if a source can no longer publish status.
    pub fn set_source_capability_context(
        &mut self,
        context: SourceCapabilityContext,
    ) -> anyhow::Result<()> {
        self.source_capability_context.clone_from(&context);
        for source in &mut self.sources {
            source.set_capability_context(&context)?;
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Update capability identity without discarding retained feature probes.
    ///
    /// # Errors
    ///
    /// Returns an error if a source can no longer publish status.
    pub fn set_source_capability_identity(
        &mut self,
        owner: impl Into<Arc<str>>,
        conflict: Option<SourceCapabilityConflict>,
        identity_hash: Option<Arc<str>>,
    ) -> anyhow::Result<()> {
        self.source_capability_context.owner = owner.into();
        self.source_capability_context.conflict = conflict;
        self.source_capability_context.identity_hash = identity_hash;
        for source in &mut self.sources {
            source.set_capability_context(&self.source_capability_context)?;
        }
        self.publish_source_status_registry();
        Ok(())
    }

    /// Publish one backend capability flag into source status.
    ///
    /// # Errors
    ///
    /// Returns an error if a source can no longer publish status.
    pub fn set_source_capability_feature(
        &mut self,
        name: impl Into<Arc<str>>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        self.source_capability_context
            .features
            .insert(name.into(), enabled);
        for source in &mut self.sources {
            source.set_capability_context(&self.source_capability_context)?;
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
            .filter_map(ManagedInputSource::as_interaction)
            .find_map(InteractionSource::input_authorization_action)
    }

    fn resolve_protected_source_action<A>(
        &self,
        action: A,
        identity: CapabilityActionIdentity,
    ) -> ResolvedProtectedSourceAction<A> {
        if identity.disposition() == CapabilityActionDisposition::RequiresUi {
            return ResolvedProtectedSourceAction::RequiresUi { identity };
        }
        ResolvedProtectedSourceAction::Local { action, identity }
    }

    /// Resolve the explicit Input Monitoring request against this process.
    #[must_use]
    pub fn resolved_input_authorization_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ProtectedSourceAuthorizationAction>> {
        let action = self.input_authorization_action()?;
        let identity = action.identity().clone();
        Some(self.resolve_protected_source_action(action, identity))
    }

    /// Resolve the explicit Screen Recording request without retaining the
    /// input-manager lock while native authorization UI runs.
    #[must_use]
    pub fn screen_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        self.sources
            .iter()
            .filter_map(ManagedInputSource::as_screen)
            .find_map(ScreenSource::screen_authorization_action)
    }

    /// Resolve the explicit Screen Recording request against this process.
    #[must_use]
    pub fn resolved_screen_authorization_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ProtectedSourceAuthorizationAction>> {
        let action = self.screen_authorization_action()?;
        let identity = action.identity().clone();
        Some(self.resolve_protected_source_action(action, identity))
    }

    /// Resolve the native picker action without retaining the input-manager
    /// lock while system UI runs.
    #[must_use]
    pub fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        self.sources
            .iter()
            .filter_map(ManagedInputSource::as_screen)
            .find_map(ScreenSource::screen_source_picker_action)
    }

    /// Resolve the native picker request against its exact local executor.
    #[must_use]
    pub fn resolved_screen_source_picker_action(
        &self,
    ) -> Option<ResolvedProtectedSourceAction<ScreenSourcePickerAction>> {
        let action = self.screen_source_picker_action()?;
        let identity = action.identity().clone();
        Some(self.resolve_protected_source_action(action, identity))
    }

    #[must_use]
    pub fn diagnostic_artifact_action(&self) -> Option<SourceDiagnosticArtifactAction> {
        self.sources
            .iter()
            .filter_map(ManagedInputSource::as_screen)
            .find_map(ScreenSource::diagnostic_artifact_action)
    }

    /// Ask screen sources to discard their persisted selection and re-prompt.
    ///
    /// # Errors
    ///
    /// Returns an error if a screen source cannot restart its session.
    pub fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        if !self
            .sources
            .iter()
            .any(|source| source.key() == ManagedSourceKey::Screen)
        {
            return Ok(());
        }
        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            if source.key() == ManagedSourceKey::Screen {
                source.set_source_graph_generation(source_graph_generation);
            }
        }
        let mut result = Ok(());
        for source in &mut self.sources {
            if let Some(screen) = source.as_screen_mut() {
                if let Err(error) = screen.reselect_screen_source() {
                    result = Err(error);
                    break;
                }
                info!(source = source.name(), "Re-opened screen source picker");
            }
        }
        self.publish_source_status_registry();
        result
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
        mut source: ManagedSourceRole,
        source_graph_generation: u64,
    ) -> ManagedInputSource {
        if let Some(screen) = source.as_screen_mut() {
            screen
                .set_capability_context(&self.source_capability_context)
                .expect("new screen source accepts retained capability status");
        }
        if let Some(interaction) = source.as_interaction_mut() {
            interaction
                .set_capability_context(&self.source_capability_context)
                .expect("new interaction source accepts retained capability status");
        }
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

    fn transition_capture_demand<GetCache, SetCache, Matches, Transition>(
        &mut self,
        active: bool,
        get_cache: GetCache,
        set_cache: SetCache,
        matches: Matches,
        mut transition: Transition,
    ) -> anyhow::Result<()>
    where
        GetCache: Fn(&Self) -> Option<bool>,
        SetCache: Fn(&mut Self, Option<bool>),
        Matches: Fn(&ManagedInputSource) -> bool,
        Transition: FnMut(&mut ManagedInputSource, bool) -> anyhow::Result<()>,
    {
        let cached = get_cache(self);
        if cached == Some(active) {
            return Ok(());
        }

        let prior_demands = self
            .sources
            .iter()
            .map(|source| {
                matches(source).then(|| source.source_status_handle().snapshot().demanded)
            })
            .collect::<Vec<_>>();

        let source_graph_generation = self.bump_source_graph_generation();
        for source in &mut self.sources {
            if matches(source) {
                source.set_source_graph_generation(source_graph_generation);
            }
        }

        for source_index in 0..self.sources.len() {
            if !matches(&self.sources[source_index]) {
                continue;
            }
            let result = transition(&mut self.sources[source_index], active)
                .and_then(|()| self.sources[source_index].set_compatibility_demand(active));
            if let Err(error) = result {
                let mut rollback_succeeded = true;
                for (rollback, previous) in self.sources.iter_mut().zip(&prior_demands) {
                    if let Some(previous) = previous {
                        let rollback_result = transition(rollback, *previous)
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
                set_cache(self, restored_cache);
                self.publish_source_status_registry();
                return Err(error);
            }
        }

        set_cache(self, Some(active));
        self.publish_source_status_registry();
        Ok(())
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
            .map(|source| source.as_screen().map(ScreenSource::screen_capture_demand))
            .collect::<Vec<_>>();
        let analysis_peak_bytes = self
            .sources
            .iter()
            .filter_map(ManagedInputSource::as_screen)
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
            if source.key() == ManagedSourceKey::Screen {
                source.set_source_graph_generation(source_graph_generation);
            }
        }
        for source_index in 0..self.sources.len() {
            if self.sources[source_index].key() != ManagedSourceKey::Screen {
                continue;
            }
            let transition = self.sources[source_index]
                .as_screen_mut()
                .expect("screen key binds typed screen source")
                .set_screen_capture_demand(demand)
                .and_then(|()| {
                    self.sources[source_index].set_compatibility_demand(demand.is_active())
                });
            if let Err(error) = transition {
                let mut rollback_succeeded = true;
                for (source, previous) in self.sources.iter_mut().zip(&prior_demands) {
                    if let Some(previous) = previous {
                        let rollback_result = source
                            .as_screen_mut()
                            .expect("screen demand snapshot binds typed screen source")
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
            .filter(|source| source.key() == ManagedSourceKey::Screen)
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
            .filter(|source| source.key() == ManagedSourceKey::Screen)
        {
            let active_consumer_count = active_consumer_counts
                .iter()
                .filter(|(source_id, _)| {
                    source
                        .as_screen()
                        .expect("screen key binds typed screen source")
                        .owns_screen_publication_source(source_id)
                })
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

fn managed_source_capture_domains(key: ManagedSourceKey) -> (bool, bool, bool) {
    match key {
        ManagedSourceKey::Audio => (true, false, false),
        ManagedSourceKey::Screen => (false, true, false),
        ManagedSourceKey::Interaction(_) => (false, false, true),
        ManagedSourceKey::Data(_) => (false, false, false),
    }
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
            source.slot.clear_latest();
            return;
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

    use super::{
        InputData, InputManager, InputSource, InteractionSource, InteractionSourceOrigin,
        InteractionSourceRole, ManagedSourceKey, ManagedSourceRole, SourceRoleBinding, SourceState,
        SourceSwapConflict, SourceSwapTarget,
    };

    struct HostSource {
        name: &'static str,
        running: bool,
        stopped: Arc<AtomicBool>,
        origin: InteractionSourceOrigin,
    }

    impl HostSource {
        fn new(name: &'static str, stopped: Arc<AtomicBool>) -> Self {
            Self {
                name,
                running: false,
                stopped,
                origin: InteractionSourceOrigin::Host,
            }
        }

        fn browser(name: &'static str) -> Self {
            Self {
                name,
                running: false,
                stopped: Arc::new(AtomicBool::new(false)),
                origin: InteractionSourceOrigin::BrowserCompatibilityAggregate,
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
    }

    impl SourceRoleBinding for HostSource {
        type Role = InteractionSourceRole;
    }

    impl InteractionSource for HostSource {
        fn interaction_source_origin(&self) -> InteractionSourceOrigin {
            self.origin
        }
    }

    #[test]
    fn typed_swap_preserves_candidate_and_graph_on_conflict() {
        let mut old = Box::new(HostSource::new(
            "old-host",
            Arc::new(AtomicBool::new(false)),
        ));
        old.start().expect("old host source starts");
        let manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::interaction(old))
            .expect("old host source registers");
        let plan = manager
            .plan_source_swap(
                ManagedSourceKey::Interaction(InteractionSourceOrigin::Host),
                SourceSwapTarget::Present { running: true },
            )
            .expect("unique host source plans");
        let mut candidate = Box::new(HostSource::new(
            "candidate-host",
            Arc::new(AtomicBool::new(false)),
        ));
        candidate.start().expect("candidate host source starts");
        let mut candidate = Some(ManagedSourceRole::interaction(candidate));
        let mut prepared = plan
            .prepare(&mut candidate)
            .expect("candidate should prepare");
        manager
            .add_source(ManagedSourceRole::interaction(Box::new(
                HostSource::browser("browser"),
            )))
            .expect("browser source registers");
        let graph_generation = manager.source_graph_generation();

        assert!(matches!(
            manager.commit_source_swap(&mut prepared),
            Err(SourceSwapConflict::GraphChanged)
        ));
        assert!(prepared.has_replacement());
        assert_eq!(manager.source_graph_generation(), graph_generation);
        assert_eq!(manager.source_names(), ["old-host", "browser"]);
        prepared.discard();
    }

    #[test]
    fn typed_swap_defers_retirement_and_preserves_registration_order() {
        let old_stopped = Arc::new(AtomicBool::new(false));
        let mut old = Box::new(HostSource::new("old-host", Arc::clone(&old_stopped)));
        old.start().expect("old host source starts");
        let manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::interaction(old))
            .expect("old host source registers");
        manager
            .add_source(ManagedSourceRole::interaction(Box::new(
                HostSource::browser("browser"),
            )))
            .expect("browser source registers");
        let plan = manager
            .plan_source_swap(
                ManagedSourceKey::Interaction(InteractionSourceOrigin::Host),
                SourceSwapTarget::Present { running: true },
            )
            .expect("unique host source plans");
        let mut candidate = Box::new(HostSource::new(
            "candidate-host",
            Arc::new(AtomicBool::new(false)),
        ));
        candidate.start().expect("candidate host source starts");
        let mut candidate = Some(ManagedSourceRole::interaction(candidate));
        let mut prepared = plan
            .prepare(&mut candidate)
            .expect("matching candidate prepares");

        let retirement = manager
            .commit_source_swap(&mut prepared)
            .expect("matching candidate commits");

        assert!(candidate.is_none());
        assert_eq!(manager.source_names(), ["candidate-host", "browser"]);
        assert!(!old_stopped.load(Ordering::Acquire));
        retirement.retire();
        assert!(old_stopped.load(Ordering::Acquire));
    }

    #[test]
    fn successful_host_swap_defers_old_source_retirement() {
        let old_stopped = Arc::new(AtomicBool::new(false));
        let candidate_stopped = Arc::new(AtomicBool::new(false));
        let mut old = Box::new(HostSource::new("old-host", Arc::clone(&old_stopped)));
        old.start().expect("old host source starts");
        let manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::interaction(old))
            .expect("old host source registers");
        let initial_generation = manager.source_graph_generation();

        let plan = manager
            .plan_source_swap(
                ManagedSourceKey::Interaction(InteractionSourceOrigin::Host),
                SourceSwapTarget::Present { running: true },
            )
            .expect("unique host source plans");
        let mut candidate: Box<dyn InteractionSource> = Box::new(HostSource::new(
            "candidate-host",
            Arc::clone(&candidate_stopped),
        ));
        candidate.start().expect("candidate host source starts");
        let mut candidate = Some(ManagedSourceRole::interaction(candidate));
        let mut prepared = plan
            .prepare(&mut candidate)
            .expect("running candidate prepares");
        let retirement = manager
            .commit_source_swap(&mut prepared)
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
        let manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::interaction(old))
            .expect("old host source registers");
        let initial_generation = manager.source_graph_generation();
        let plan = manager
            .plan_source_swap(
                ManagedSourceKey::Interaction(InteractionSourceOrigin::Host),
                SourceSwapTarget::Present { running: true },
            )
            .expect("unique host source plans");
        let mut candidate = Some(ManagedSourceRole::interaction(Box::new(HostSource::new(
            "failed-candidate",
            Arc::new(AtomicBool::new(false)),
        ))));

        assert!(matches!(
            plan.prepare(&mut candidate),
            Err(SourceSwapConflict::InvalidReplacementLifecycle {
                expected_running: true,
                observed_running: false,
            })
        ));
        assert!(candidate.is_some());
        assert_eq!(manager.source_names(), ["old-host"]);
        assert_eq!(manager.source_graph_generation(), initial_generation);
        assert!(!old_stopped.load(Ordering::Acquire));
    }

    #[test]
    fn browser_candidate_rejection_preserves_candidate_and_host_source() {
        let mut old = Box::new(HostSource::new(
            "old-host",
            Arc::new(AtomicBool::new(false)),
        ));
        old.start().expect("old host source starts");
        let manager = InputManager::new();
        manager
            .add_source(ManagedSourceRole::interaction(old))
            .expect("old host source registers");
        let initial_generation = manager.source_graph_generation();
        let plan = manager
            .plan_source_swap(
                ManagedSourceKey::Interaction(InteractionSourceOrigin::Host),
                SourceSwapTarget::Present { running: true },
            )
            .expect("unique host source plans");
        let mut candidate = Box::new(HostSource::browser("browser-candidate"));
        candidate.start().expect("browser candidate starts");
        let mut candidate = Some(ManagedSourceRole::interaction(candidate));

        assert!(matches!(
            plan.prepare(&mut candidate),
            Err(SourceSwapConflict::InvalidReplacementKey {
                expected: ManagedSourceKey::Interaction(InteractionSourceOrigin::Host),
                observed: ManagedSourceKey::Interaction(
                    InteractionSourceOrigin::BrowserCompatibilityAggregate
                ),
            })
        ));
        assert!(candidate.is_some());
        assert_eq!(manager.source_names(), ["old-host"]);
        assert_eq!(manager.source_graph_generation(), initial_generation);
    }
}
