use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use arc_swap::ArcSwap;
use hypercolor_core::input::screen::ScreenRendererExecutionState;
use hypercolor_core::input::screen::consumer::{
    PixelExtent, ScreenCaptureDemand, ScreenPublicationDemandSnapshot,
};
use hypercolor_core::input::screen::planner::{
    CommittedScreenPublicationTransition, InputPublicationDemandRevision, LedToneMapCalibration,
    RegisteredScreenBranchDemand, ScreenAspectPolicy, ScreenBranchLease, ScreenExtentRequest,
    ScreenHdrPolicy, ScreenInputGraphGeneration, ScreenNativeExecutionPolicy,
    ScreenNativeExecutionTarget, ScreenPlanGeneration, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationExecutorRequest, ScreenPublicationHub,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenPublicationRetirement,
    ScreenSourceSelector, ScreenToneMapOperator, ScreenToneMapPolicy, ScreenUpscalePolicy,
};
use hypercolor_core::input::{
    InputGraphHandle, InputGraphSnapshot, InputManager, SourceKind, SourceState,
    TryInputManagerIntent,
};
use tokio::sync::{oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant as TokioInstant, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use super::capture_demand::{CaptureDemand, CaptureDemandReconcile, CaptureDemandState};

const STOP_TIMEOUT: Duration = Duration::from_secs(1);
const LIFECYCLE_PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// Retry cadence for an exact plan that failed with no committed plan to
/// replace: the source itself must change (session, consent, capacity)
/// before another attempt can succeed, and nothing graph-visible signals
/// that yet, so this is a bounded liveness probe rather than a recovery
/// path. Spec 74 folds the source resolution revision into the pump key,
/// which turns this into an event-driven re-arm.
const EXACT_PLAN_UNAVAILABLE_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const SOURCE_KINDS: [SourceKind; 6] = [
    SourceKind::Audio,
    SourceKind::Screen,
    SourceKind::Interaction,
    SourceKind::Media,
    SourceKind::Network,
    SourceKind::Sensors,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A consumer class contributing input-publication cadence demand.
pub enum InputPublicationConsumer {
    /// The hardware-authoritative scene renderer.
    Authoritative,
    /// An isolated interactive preview renderer.
    Preview,
    /// A latest-value stream that does not render hardware output.
    PassiveStream,
    /// A diagnostic reader that explicitly requests live samples.
    Diagnostic,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Per-source publication rates requested by one consumer class.
pub struct InputPublicationDemand {
    audio: u32,
    screen: Arc<[InputScreenBranchDemand]>,
    interaction: u32,
    media: u32,
    network: u32,
    sensors: u32,
    screen_renderer_execution: ScreenRendererExecutionState,
    screen_renderer_target: Option<ScreenNativeExecutionTarget>,
    /// Screen requests without an explicit executor. The registry binds
    /// them at snapshot time under the screen source's native execution
    /// policy: to the authoritative renderer target when native execution
    /// is required, to exact CPU reduction otherwise.
    renderer_screen: Arc<[InputScreenBranchRequest]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputScreenBranchDemand {
    branch: RegisteredScreenBranchDemand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InputScreenBranchRequest {
    selector: ScreenSourceSelector,
    requested_hz: NonZeroU32,
    kind: ScreenPublicationKind,
    extent: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
    processing_profile: Arc<ScreenProcessingProfile>,
}

impl InputScreenBranchRequest {
    pub(crate) fn new(
        selector: ScreenSourceSelector,
        kind: ScreenPublicationKind,
        extent: ScreenExtentRequest,
        aspect: ScreenAspectPolicy,
        processing_profile: Arc<ScreenProcessingProfile>,
        requested_hz: NonZeroU32,
    ) -> Self {
        Self {
            selector,
            requested_hz,
            kind,
            extent,
            aspect,
            processing_profile,
        }
    }

    pub(crate) fn surface(requested_hz: u32, requested_extent: PixelExtent) -> Option<Self> {
        let requested_hz = NonZeroU32::new(requested_hz)?;
        let extent = ScreenExtentRequest::bounded(
            NonZeroU32::new(requested_extent.width()),
            NonZeroU32::new(requested_extent.height()),
            ScreenUpscalePolicy::Never,
        );
        // The default profile rejects HDR sources outright, which makes an
        // HDR-configured capture stream permanently unresolvable. Enabling
        // the spec 76 BT.2390 tone map here is what admits HDR sources at
        // all; the platform source refreshes the calibration from the live
        // capture config during branch resolution, so the default
        // calibration never reaches a kernel.
        let profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                ScreenToneMapOperator::Bt2390Eetf,
                LedToneMapCalibration::DEFAULT,
            )),
            ..ScreenProcessingProfileConfig::default()
        });
        Some(Self::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            extent,
            ScreenAspectPolicy::Contain,
            Arc::new(profile),
            requested_hz,
        ))
    }

    pub(crate) fn bind(
        self,
        executor: ScreenPublicationExecutorRequest,
    ) -> InputScreenBranchDemand {
        let request = ScreenPublicationRequest::new(
            self.selector,
            self.kind,
            executor,
            self.extent,
            self.aspect,
            self.processing_profile,
        );
        InputScreenBranchDemand::new(RegisteredScreenBranchDemand::new(
            request,
            self.requested_hz,
        ))
    }

    pub(crate) fn from_registered(demand: &InputScreenBranchDemand) -> Self {
        let request = demand.branch.request();
        Self {
            selector: request.selector().clone(),
            requested_hz: demand.branch.requested_hz(),
            kind: request.kind(),
            extent: request.extent(),
            aspect: request.aspect(),
            processing_profile: Arc::clone(request.processing_profile()),
        }
    }

    pub(crate) fn bind_native_required(
        self,
        target: &ScreenNativeExecutionTarget,
    ) -> InputScreenBranchDemand {
        self.bind(ScreenPublicationExecutorRequest::SourceNativeRequired(
            target.clone(),
        ))
    }

    #[cfg(test)]
    pub(crate) const fn kind(&self) -> ScreenPublicationKind {
        self.kind
    }

    #[cfg(test)]
    pub(crate) const fn extent(&self) -> ScreenExtentRequest {
        self.extent
    }

    #[cfg(test)]
    pub(crate) fn processing_profile(&self) -> &Arc<ScreenProcessingProfile> {
        &self.processing_profile
    }

    pub(crate) const fn requested_hz(&self) -> NonZeroU32 {
        self.requested_hz
    }
}

impl InputScreenBranchDemand {
    /// Wrap one exact unresolved branch for registration.
    #[must_use]
    pub const fn new(branch: RegisteredScreenBranchDemand) -> Self {
        Self { branch }
    }

    /// Exact unresolved branch preserved by the authoritative registry.
    #[must_use]
    pub const fn branch(&self) -> &RegisteredScreenBranchDemand {
        &self.branch
    }

    fn surface(
        requested_hz: u32,
        requested_extent: PixelExtent,
        executor: ScreenPublicationExecutorRequest,
    ) -> Option<Self> {
        InputScreenBranchRequest::surface(requested_hz, requested_extent)
            .map(|request| request.bind(executor))
    }

    const fn requested_hz(&self) -> u32 {
        self.branch.requested_hz().get()
    }

    fn same_publication_request(&self, other: &Self) -> bool {
        self.branch.request() == other.branch.request()
            && self.branch.requested_hz() == other.branch.requested_hz()
    }
}

impl InputPublicationDemand {
    pub(crate) fn same_publication_request(&self, other: &Self) -> bool {
        self.audio == other.audio
            && self.interaction == other.interaction
            && self.media == other.media
            && self.network == other.network
            && self.sensors == other.sensors
            && self.screen_renderer_execution == other.screen_renderer_execution
            && self.screen_renderer_target == other.screen_renderer_target
            && self.screen.len() == other.screen.len()
            && self
                .screen
                .iter()
                .zip(other.screen.iter())
                .all(|(left, right)| left.same_publication_request(right))
            && self.renderer_screen == other.renderer_screen
    }

    /// Request the same rate for every typed source.
    #[must_use]
    pub fn all_sources(requested_hz: u32, screen_extent: PixelExtent) -> Self {
        let demand = Self {
            audio: requested_hz,
            interaction: requested_hz,
            media: requested_hz,
            network: requested_hz,
            sensors: requested_hz,
            ..Self::default()
        };
        demand.with_screen(requested_hz, screen_extent)
    }

    /// Set one scalar source rate, preserving the other source rates.
    ///
    /// Screen demand must use [`Self::with_screen`] so its extent cannot be
    /// separated from its cadence.
    ///
    /// # Panics
    ///
    /// Panics when `source` is [`SourceKind::Screen`] and `requested_hz` is
    /// non-zero.
    #[must_use]
    pub fn with_source(mut self, source: SourceKind, requested_hz: u32) -> Self {
        match source {
            SourceKind::Audio => self.audio = requested_hz,
            SourceKind::Screen => {
                assert!(
                    requested_hz == 0,
                    "screen publication demand requires an explicit extent"
                );
                self.screen = Arc::default();
            }
            SourceKind::Interaction => self.interaction = requested_hz,
            SourceKind::Media => self.media = requested_hz,
            SourceKind::Network => self.network = requested_hz,
            SourceKind::Sensors => self.sensors = requested_hz,
        }
        self
    }

    /// Set screen publication cadence and extent together.
    #[must_use]
    pub fn with_screen(mut self, requested_hz: u32, requested_extent: PixelExtent) -> Self {
        self.screen = Arc::default();
        self.renderer_screen = InputScreenBranchRequest::surface(requested_hz, requested_extent)
            .into_iter()
            .collect::<Vec<_>>()
            .into();
        self
    }

    /// Set screen cadence, extent, and one renderer-bound execution request.
    #[must_use]
    pub fn with_screen_executor(
        mut self,
        requested_hz: u32,
        requested_extent: PixelExtent,
        executor: ScreenPublicationExecutorRequest,
    ) -> Self {
        if matches!(executor, ScreenPublicationExecutorRequest::Cpu) {
            return self.with_screen(requested_hz, requested_extent);
        }
        self.screen = InputScreenBranchDemand::surface(requested_hz, requested_extent, executor)
            .into_iter()
            .collect::<Vec<_>>()
            .into();
        self
    }

    /// Replace screen demand with an immutable set of independent exact branches.
    #[must_use]
    pub fn with_screen_branches(
        mut self,
        branches: impl IntoIterator<Item = InputScreenBranchDemand>,
    ) -> Self {
        let mut exact = Vec::new();
        let mut renderer = Vec::new();
        for branch in branches {
            if matches!(
                branch.branch.request().executor(),
                ScreenPublicationExecutorRequest::Cpu
            ) {
                renderer.push(InputScreenBranchRequest::from_registered(&branch));
            } else {
                exact.push(branch);
            }
        }
        self.screen = exact.into();
        self.renderer_screen = renderer.into();
        self
    }

    #[cfg(test)]
    fn with_fixture_screen(mut self, requested_hz: u32, requested_extent: PixelExtent) -> Self {
        self.screen = InputScreenBranchDemand::surface(
            requested_hz,
            requested_extent,
            ScreenPublicationExecutorRequest::Cpu,
        )
        .into_iter()
        .collect::<Vec<_>>()
        .into();
        self.renderer_screen = Arc::default();
        self
    }

    #[cfg(test)]
    fn with_fixture_screen_branches(
        mut self,
        branches: impl IntoIterator<Item = InputScreenBranchDemand>,
    ) -> Self {
        self.screen = branches.into_iter().collect::<Vec<_>>().into();
        self.renderer_screen = Arc::default();
        self
    }

    pub(crate) fn with_renderer_screen_requests(
        mut self,
        requests: impl IntoIterator<Item = InputScreenBranchRequest>,
    ) -> Self {
        self.renderer_screen = requests.into_iter().collect::<Vec<_>>().into();
        self
    }

    #[cfg(test)]
    pub(crate) fn renderer_screen_requests(&self) -> &[InputScreenBranchRequest] {
        &self.renderer_screen
    }

    pub(crate) fn with_screen_renderer_target(
        mut self,
        target: Option<&ScreenNativeExecutionTarget>,
    ) -> Self {
        self.screen_renderer_target = target.cloned();
        self
    }

    pub(crate) fn with_screen_renderer_execution(
        mut self,
        state: ScreenRendererExecutionState,
    ) -> Self {
        self.screen_renderer_execution = state;
        self
    }

    pub(crate) fn requested_hz(&self, source: SourceKind) -> u32 {
        match source {
            SourceKind::Audio => self.audio,
            SourceKind::Screen => {
                let exact = self
                    .screen
                    .iter()
                    .map(InputScreenBranchDemand::requested_hz)
                    .max()
                    .unwrap_or(0);
                exact.max(
                    self.renderer_screen
                        .iter()
                        .map(|request| request.requested_hz().get())
                        .max()
                        .unwrap_or(0),
                )
            }
            SourceKind::Interaction => self.interaction,
            SourceKind::Media => self.media,
            SourceKind::Network => self.network,
            SourceKind::Sensors => self.sensors,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct InputPublicationCadence {
    requested_hz: [u32; SOURCE_KINDS.len()],
}

impl InputPublicationCadence {
    fn merge_demand(&mut self, demand: &InputPublicationDemand) {
        for source in SOURCE_KINDS {
            self.requested_hz[source_kind_index(source)] =
                self.requested_hz(source).max(demand.requested_hz(source));
        }
    }

    const fn with_source(mut self, source: SourceKind, requested_hz: u32) -> Self {
        self.requested_hz[source_kind_index(source)] = requested_hz;
        self
    }

    const fn requested_hz(self, source: SourceKind) -> u32 {
        self.requested_hz[source_kind_index(source)]
    }

    fn max_requested_hz(self) -> u32 {
        self.requested_hz.into_iter().max().unwrap_or(0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputPublicationDemandEntry {
    id: u64,
    consumer: InputPublicationConsumer,
    demand: InputPublicationDemand,
}

struct ResolvedInputScreenDemand {
    screen: InputScreenBranchDemand,
}

#[derive(Clone, Debug, Default)]
struct InputPublicationDemandSnapshot {
    entries: Arc<[InputPublicationDemandEntry]>,
    cadence: InputPublicationCadence,
    screen_branches: Arc<[RegisteredScreenBranchDemand]>,
    revision: InputPublicationDemandRevision,
    screen_renderer_execution: ScreenRendererExecutionState,
}

impl InputPublicationDemandSnapshot {
    fn from_entries(
        entries: Vec<InputPublicationDemandEntry>,
        revision: InputPublicationDemandRevision,
        policy: ScreenNativeExecutionPolicy,
    ) -> Self {
        let mut cadence = InputPublicationCadence::default();
        let renderer_target = entries
            .iter()
            .find(|entry| entry.consumer == InputPublicationConsumer::Authoritative)
            .and_then(|entry| entry.demand.screen_renderer_target.as_ref());
        let mut resolved_screens = Vec::new();
        for entry in &entries {
            cadence.merge_demand(&entry.demand);
            resolved_screens.extend(
                entry
                    .demand
                    .screen
                    .iter()
                    .cloned()
                    .map(|screen| ResolvedInputScreenDemand { screen }),
            );
            match policy {
                ScreenNativeExecutionPolicy::Required => {
                    if let Some(target) = renderer_target {
                        resolved_screens.extend(entry.demand.renderer_screen.iter().cloned().map(
                            |request| ResolvedInputScreenDemand {
                                screen: request.bind_native_required(target),
                            },
                        ));
                    }
                }
                ScreenNativeExecutionPolicy::Preferred => {
                    resolved_screens.extend(entry.demand.renderer_screen.iter().cloned().map(
                        |request| ResolvedInputScreenDemand {
                            screen: request.bind(ScreenPublicationExecutorRequest::Cpu),
                        },
                    ));
                }
            }
        }
        cadence = cadence.with_source(
            SourceKind::Screen,
            resolved_screens
                .iter()
                .map(|screen| screen.screen.requested_hz())
                .max()
                .unwrap_or(0),
        );
        let screen_branches = resolved_screens
            .iter()
            .map(|resolved| resolved.screen.branch.clone())
            .collect::<Vec<_>>();
        let screen_renderer_execution = entries
            .iter()
            .find(|entry| entry.consumer == InputPublicationConsumer::Authoritative)
            .map_or(ScreenRendererExecutionState::Inactive, |entry| {
                entry.demand.screen_renderer_execution
            });
        Self {
            entries: entries.into(),
            cadence,
            screen_branches: screen_branches.into(),
            revision,
            screen_renderer_execution,
        }
    }

    fn requested_hz(&self, source: SourceKind) -> u32 {
        self.cadence.requested_hz(source)
    }

    fn max_requested_hz(&self) -> u32 {
        self.cadence.max_requested_hz()
    }

    fn registration_count(&self, consumer: InputPublicationConsumer) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.consumer == consumer)
            .count()
    }

    /// Capture worker lifecycle implied by this snapshot.
    ///
    /// The screen worker runs exactly while at least one exact branch is
    /// registered; no branch geometry participates, so consumers with
    /// different extents never form a synthetic union.
    fn capture_demand(&self) -> CaptureDemand {
        let screen = if self.screen_branches.is_empty() {
            ScreenCaptureDemand::Inactive
        } else {
            ScreenCaptureDemand::active()
        };
        CaptureDemand::new(
            self.requested_hz(SourceKind::Audio) > 0,
            screen,
            self.requested_hz(SourceKind::Interaction) > 0,
        )
    }

    const fn revision(&self) -> InputPublicationDemandRevision {
        self.revision
    }

    const fn screen_renderer_execution(&self) -> ScreenRendererExecutionState {
        self.screen_renderer_execution
    }

    fn exact_screen_demand(&self, graph_generation: u64) -> ScreenPublicationDemandSnapshot {
        ScreenPublicationDemandSnapshot::new(
            self.revision,
            ScreenInputGraphGeneration::new(graph_generation),
            Arc::clone(&self.screen_branches),
        )
    }

    fn exact_screen_retirement_demand(
        &self,
        graph_generation: u64,
    ) -> ScreenPublicationDemandSnapshot {
        ScreenPublicationDemandSnapshot::new(
            self.revision,
            ScreenInputGraphGeneration::new(graph_generation),
            Arc::default(),
        )
    }
}

struct InputPublicationDemandRegistry {
    native_execution_policy: ScreenNativeExecutionPolicy,
    next_id: AtomicU64,
    latest: ArcSwap<InputPublicationDemandSnapshot>,
    revision_tx: watch::Sender<InputPublicationDemandRevision>,
    revision_gate: SyncMutex<()>,
    revision_gate_release_tx: watch::Sender<u64>,
    #[cfg(test)]
    commit_test_hook: SyncMutex<Option<ExactScreenCommitTestHook>>,
}

enum TryDemandCommit<T> {
    Busy,
    Stale,
    Committed(T),
}

struct DemandRevisionGateGuard<'a> {
    guard: Option<std::sync::MutexGuard<'a, ()>>,
    release_tx: &'a watch::Sender<u64>,
}

impl Drop for DemandRevisionGateGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        if self.release_tx.receiver_count() > 0 {
            self.release_tx
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }
}

#[cfg(test)]
#[derive(Clone)]
struct ExactScreenCommitTestHook {
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
struct ExactScreenCommitTestPause {
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
impl ExactScreenCommitTestPause {
    async fn wait_until_reached(&self) {
        self.reached.notified().await;
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

#[derive(Clone)]
/// Lock-free latest-value demand reads for all input consumers.
pub struct InputPublicationDemandHandle {
    registry: Arc<InputPublicationDemandRegistry>,
}

impl InputPublicationDemandHandle {
    /// Create an empty demand publication.
    ///
    /// The policy decides how screen requests without an explicit executor
    /// bind: to the authoritative renderer target when the screen source
    /// requires native execution, to exact CPU reduction otherwise.
    #[must_use]
    pub fn new(native_execution_policy: ScreenNativeExecutionPolicy) -> Self {
        let (revision_tx, _) = watch::channel(InputPublicationDemandRevision::default());
        let (revision_gate_release_tx, _) = watch::channel(0);
        Self {
            registry: Arc::new(InputPublicationDemandRegistry {
                native_execution_policy,
                next_id: AtomicU64::new(1),
                latest: ArcSwap::from_pointee(InputPublicationDemandSnapshot::default()),
                revision_tx,
                revision_gate: SyncMutex::new(()),
                revision_gate_release_tx,
                #[cfg(test)]
                commit_test_hook: SyncMutex::new(None),
            }),
        }
    }

    /// The native execution policy this publication binds screen requests under.
    #[must_use]
    pub fn native_execution_policy(&self) -> ScreenNativeExecutionPolicy {
        self.registry.native_execution_policy
    }

    /// Register one independently owned demand contribution.
    #[must_use = "dropping the registration immediately removes its demand"]
    pub fn register(
        &self,
        consumer: InputPublicationConsumer,
        demand: InputPublicationDemand,
    ) -> InputPublicationDemandRegistration {
        let id = self
            .registry
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next_id| {
                next_id.checked_add(1)
            })
            .expect("input publication demand registration identity exhausted");
        self.registry.update_entries(|entries| {
            entries.push(InputPublicationDemandEntry {
                id,
                consumer,
                demand: demand.clone(),
            });
        });
        InputPublicationDemandRegistration {
            registry: Arc::clone(&self.registry),
            id,
        }
    }

    /// Count live registrations owned by one consumer class.
    #[must_use]
    pub fn registration_count(&self, consumer: InputPublicationConsumer) -> usize {
        self.snapshot().registration_count(consumer)
    }

    /// Read the current aggregate rate for one source domain.
    #[must_use]
    pub fn requested_hz(&self, source: SourceKind) -> u32 {
        self.snapshot().requested_hz(source)
    }

    /// Read every independently registered unresolved screen branch.
    #[must_use]
    pub fn screen_branches(&self) -> Arc<[RegisteredScreenBranchDemand]> {
        Arc::clone(&self.snapshot().screen_branches)
    }

    /// Read the monotonic revision of the immutable demand snapshot.
    #[must_use]
    pub fn revision(&self) -> InputPublicationDemandRevision {
        self.snapshot().revision()
    }

    fn snapshot(&self) -> Arc<InputPublicationDemandSnapshot> {
        self.registry.latest.load_full()
    }

    fn subscribe_revision(&self) -> watch::Receiver<InputPublicationDemandRevision> {
        self.registry.revision_tx.subscribe()
    }

    async fn wait_for_revision_gate_release_after_busy(&self) {
        let mut revision = self.registry.revision_gate_release_tx.subscribe();
        if let Some(guard) = self.registry.try_lock_revision_gate() {
            drop(guard);
            return;
        }
        revision
            .changed()
            .await
            .expect("input publication demand retains the revision gate publisher");
    }

    fn try_commit_if_revision<T>(
        &self,
        expected: InputPublicationDemandRevision,
        commit: impl FnOnce() -> T,
    ) -> TryDemandCommit<T> {
        let Some(_revision_guard) = self.registry.try_lock_revision_gate() else {
            return TryDemandCommit::Busy;
        };
        if self.registry.latest.load().revision() != expected {
            return TryDemandCommit::Stale;
        }
        TryDemandCommit::Committed(commit())
    }

    #[cfg(test)]
    fn pause_next_exact_screen_commit(&self) -> ExactScreenCommitTestPause {
        let pause = ExactScreenCommitTestPause {
            reached: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        };
        let hook = ExactScreenCommitTestHook {
            reached: Arc::clone(&pause.reached),
            release: Arc::clone(&pause.release),
        };
        *self
            .registry
            .commit_test_hook
            .lock()
            .expect("input publication commit test hook is healthy") = Some(hook);
        pause
    }

    #[cfg(test)]
    async fn wait_at_exact_screen_commit_test_hook(&self) {
        let hook = self
            .registry
            .commit_test_hook
            .lock()
            .expect("input publication commit test hook is healthy")
            .take();
        if let Some(hook) = hook {
            hook.reached.notify_one();
            hook.release.notified().await;
        }
    }
}

impl Default for InputPublicationDemandHandle {
    fn default() -> Self {
        Self::new(ScreenNativeExecutionPolicy::default())
    }
}

impl InputPublicationDemandRegistry {
    fn lock_revision_gate(&self) -> DemandRevisionGateGuard<'_> {
        DemandRevisionGateGuard {
            guard: Some(
                self.revision_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
            release_tx: &self.revision_gate_release_tx,
        }
    }

    fn try_lock_revision_gate(&self) -> Option<DemandRevisionGateGuard<'_>> {
        let guard = match self.revision_gate.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        Some(DemandRevisionGateGuard {
            guard: Some(guard),
            release_tx: &self.revision_gate_release_tx,
        })
    }

    fn update_entries(&self, update: impl Fn(&mut Vec<InputPublicationDemandEntry>)) {
        let _revision_guard = self.lock_revision_gate();
        self.latest.rcu(|current| {
            let mut entries = current.entries.to_vec();
            update(&mut entries);
            if entries.as_slice() == current.entries.as_ref() {
                return Arc::clone(current);
            }
            let revision = current
                .revision()
                .next()
                .expect("input publication demand revision exhausted");
            Arc::new(InputPublicationDemandSnapshot::from_entries(
                entries,
                revision,
                self.native_execution_policy,
            ))
        });
        self.publish_revision(self.latest.load().revision());
    }

    fn publish_revision(&self, revision: InputPublicationDemandRevision) {
        self.revision_tx.send_if_modified(|published| {
            if *published >= revision {
                return false;
            }
            *published = revision;
            true
        });
    }

    fn update_registration(&self, id: u64, demand: InputPublicationDemand) {
        self.update_entries(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.demand = demand.clone();
            }
        });
    }

    fn remove_registration(&self, id: u64) {
        self.update_entries(|entries| entries.retain(|entry| entry.id != id));
    }
}

/// RAII ownership of one input-publication demand contribution.
pub struct InputPublicationDemandRegistration {
    registry: Arc<InputPublicationDemandRegistry>,
    id: u64,
}

impl InputPublicationDemandRegistration {
    /// Replace this registration's typed demand atomically.
    pub fn update(&self, demand: InputPublicationDemand) {
        self.registry.update_registration(self.id, demand);
    }
}

impl Drop for InputPublicationDemandRegistration {
    fn drop(&mut self) {
        self.registry.remove_registration(self.id);
    }
}

pub(crate) struct OwnedInputPublicationDemand {
    registration: InputPublicationDemandRegistration,
    current: InputPublicationDemand,
}

impl OwnedInputPublicationDemand {
    pub(crate) fn new(
        demands: &InputPublicationDemandHandle,
        consumer: InputPublicationConsumer,
    ) -> Self {
        Self {
            registration: demands.register(consumer, InputPublicationDemand::default()),
            current: InputPublicationDemand::default(),
        }
    }

    pub(crate) fn publish(&mut self, demand: InputPublicationDemand) {
        if !demand.same_publication_request(&self.current) {
            self.registration.update(demand.clone());
            self.current = demand;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.publish(InputPublicationDemand::default());
    }
}

#[derive(Clone)]
pub(crate) struct InputPublicationReader {
    graph: InputGraphHandle,
    #[allow(
        dead_code,
        reason = "screen publication leases are optional for pump consumers"
    )]
    screen_publications: Arc<ScreenPublicationHub>,
    native_execution_policy: ScreenNativeExecutionPolicy,
}

impl InputPublicationReader {
    fn new(
        graph: InputGraphHandle,
        screen_publications: Arc<ScreenPublicationHub>,
        native_execution_policy: ScreenNativeExecutionPolicy,
    ) -> Self {
        Self {
            graph,
            screen_publications,
            native_execution_policy,
        }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self::new(
            InputGraphHandle::default(),
            hypercolor_core::input::screen::ScreenPlanBuilder::new().publication_hub(),
            ScreenNativeExecutionPolicy::default(),
        )
    }

    pub(crate) fn graph_snapshot(&self) -> Arc<InputGraphSnapshot> {
        self.graph.snapshot()
    }

    #[allow(
        dead_code,
        reason = "screen publication leases are optional for pump consumers"
    )]
    pub(crate) fn screen_publications(&self) -> Arc<ScreenPublicationHub> {
        Arc::clone(&self.screen_publications)
    }

    pub(crate) fn screen_observation(
        &self,
        target: Option<&ScreenNativeExecutionTarget>,
        extent: PixelExtent,
    ) -> (ScreenPlanGeneration, Option<ScreenBranchLease>) {
        self.screen_publications.observe_preferred_matching_lease(
            |descriptor| {
                descriptor.kind() == ScreenPublicationKind::Surface
                    && descriptor.geometry().output_extent() == extent
                    && screen_executor_matches_render_target(
                        target,
                        descriptor.requested_executor(),
                        self.native_execution_policy,
                    )
            },
            |descriptor| {
                descriptor.kind() == ScreenPublicationKind::Surface
                    && descriptor.geometry().output_extent() == extent
                    && screen_executor_matches_render_target(
                        None,
                        descriptor.requested_executor(),
                        self.native_execution_policy,
                    )
            },
        )
    }

    /// Lease the first committed CPU zones branch, whatever grid and extent
    /// its registering consumer asked for.
    ///
    /// Zones publications are always CPU-resident RGB grids, so the lease
    /// never depends on a renderer execution target.
    pub(crate) fn screen_zones_observation(
        &self,
    ) -> (ScreenPlanGeneration, Option<ScreenBranchLease>) {
        self.screen_publications
            .observe_matching_lease(|descriptor| {
                matches!(descriptor.kind(), ScreenPublicationKind::Zones { .. })
            })
    }
}

fn screen_executor_matches_render_target(
    target: Option<&ScreenNativeExecutionTarget>,
    requested: &ScreenPublicationExecutorRequest,
    policy: ScreenNativeExecutionPolicy,
) -> bool {
    match (target, requested) {
        (
            Some(target),
            ScreenPublicationExecutorRequest::SourceNative(selected)
            | ScreenPublicationExecutorRequest::SourceNativeRequired(selected),
        ) => selected.id() == target.id(),
        (None, ScreenPublicationExecutorRequest::Cpu) => {
            policy == ScreenNativeExecutionPolicy::Preferred
        }
        _ => false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Observable lifecycle state of the dedicated input-publication pump.
pub enum InputPublicationStatus {
    /// The worker has been spawned but has not published readiness.
    Starting,
    /// The worker is ready to service source demand.
    Ready,
    /// The worker completed an intentional shutdown.
    Stopped,
    /// The worker exited unexpectedly.
    Failed(Arc<str>),
}

impl InputPublicationStatus {
    const fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed(_))
    }
}

#[derive(Clone)]
pub(crate) struct InputPublicationMonitor {
    status: watch::Receiver<InputPublicationStatus>,
}

impl InputPublicationMonitor {
    pub(crate) fn status(&self) -> InputPublicationStatus {
        self.status.borrow().clone()
    }

    pub(crate) async fn wait_for_terminal(mut self) -> InputPublicationStatus {
        loop {
            let status = self.status();
            if status.is_terminal() {
                return status;
            }
            if self.status.changed().await.is_err() {
                return InputPublicationStatus::Failed(Arc::from(
                    "input publication status channel closed",
                ));
            }
        }
    }
}

pub(crate) struct InputPublicationPump {
    cancel: CancellationToken,
    supervisor: Option<JoinHandle<Result<()>>>,
    reader: InputPublicationReader,
    monitor: InputPublicationMonitor,
}

impl InputPublicationPump {
    pub(crate) async fn start(
        manager: InputManager,
        demands: InputPublicationDemandHandle,
    ) -> Result<Self> {
        let reader = InputPublicationReader::new(
            manager.input_graph_handle(),
            manager.screen_publication_hub(),
            demands.native_execution_policy(),
        );
        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (status_tx, status_rx) = watch::channel(InputPublicationStatus::Starting);
        let worker_cancel = cancel.clone();
        let worker_reader = reader.clone();
        let supervisor = tokio::spawn(async move {
            let worker_status = status_tx.clone();
            let worker = tokio::spawn(run_pump(
                manager,
                worker_reader,
                demands,
                worker_cancel,
                worker_status,
                ready_tx,
            ));
            let worker = AbortOnDropTask::new(worker);
            match worker.join().await {
                Ok(()) => {
                    status_tx.send_replace(InputPublicationStatus::Stopped);
                    Ok(())
                }
                Err(error) => {
                    let message: Arc<str> =
                        Arc::from(format!("input publication worker terminated: {error}"));
                    status_tx.send_replace(InputPublicationStatus::Failed(Arc::clone(&message)));
                    Err(anyhow!(message.to_string()))
                }
            }
        });
        let monitor = InputPublicationMonitor { status: status_rx };

        if ready_rx.await.is_err() {
            supervisor.abort();
            let _ = supervisor.await;
            return Err(match monitor.status() {
                InputPublicationStatus::Failed(message) => anyhow!(message.to_string()),
                status => anyhow!("input publication pump stopped during startup: {status:?}"),
            });
        }
        if monitor.status() != InputPublicationStatus::Ready {
            supervisor.abort();
            let _ = supervisor.await;
            return Err(anyhow!(
                "input publication pump did not reach readiness: {:?}",
                monitor.status()
            ));
        }
        info!("input publication pump started");

        Ok(Self {
            cancel,
            supervisor: Some(supervisor),
            reader,
            monitor,
        })
    }

    pub(crate) fn reader(&self) -> InputPublicationReader {
        self.reader.clone()
    }

    pub(crate) fn monitor(&self) -> InputPublicationMonitor {
        self.monitor.clone()
    }

    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        self.cancel.cancel();
        let Some(mut supervisor) = self.supervisor.take() else {
            return Ok(());
        };

        if let Ok(joined) = timeout(STOP_TIMEOUT, &mut supervisor).await {
            joined.context("input publication supervisor task panicked")??;
        } else {
            supervisor.abort();
            let _ = timeout(STOP_TIMEOUT, &mut supervisor).await;
            return Err(anyhow!(
                "input publication pump exceeded its bounded shutdown deadline"
            ));
        }
        info!("input publication pump stopped");
        Ok(())
    }
}

impl Drop for InputPublicationPump {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(supervisor) = &self.supervisor {
            supervisor.abort();
        }
    }
}

struct AbortOnDropTask<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortOnDropTask<T> {
    const fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn join(mut self) -> std::result::Result<T, tokio::task::JoinError> {
        let result = self
            .handle
            .as_mut()
            .expect("abort-on-drop task retains its join handle")
            .await;
        self.handle = None;
        result
    }

    fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }
}

impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

type ExactScreenTransitionKey = (InputPublicationDemandRevision, u64, u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExactScreenTransitionPurpose {
    ApplyDemand,
    RetireForRetry,
}

fn exact_screen_failure_retry_at(
    purpose: ExactScreenTransitionPurpose,
    committed_plan_is_empty: bool,
    now: Instant,
) -> Instant {
    if purpose == ExactScreenTransitionPurpose::ApplyDemand && !committed_plan_is_empty {
        now
    } else {
        now + EXACT_PLAN_UNAVAILABLE_RETRY_INTERVAL
    }
}

struct ExactScreenTransitionTask {
    key: ExactScreenTransitionKey,
    purpose: ExactScreenTransitionPurpose,
    task: AbortOnDropTask<Result<ExactScreenTransitionOutcome>>,
}

enum ExactScreenTransitionOutcome {
    Completed(Option<CommittedScreenPublicationTransition>),
}

impl ExactScreenTransitionTask {
    fn spawn(
        manager: InputManager,
        reader: InputPublicationReader,
        demands: InputPublicationDemandHandle,
        demand: ScreenPublicationDemandSnapshot,
        source_resolution_revision: u64,
        purpose: ExactScreenTransitionPurpose,
    ) -> Self {
        let key = (
            demand.revision(),
            demand.graph_generation().get(),
            source_resolution_revision,
        );
        let task = AbortOnDropTask::new(tokio::spawn(run_exact_screen_transition(
            manager, reader, demands, demand,
        )));
        Self { key, purpose, task }
    }

    fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    async fn join(
        self,
    ) -> std::result::Result<Result<ExactScreenTransitionOutcome>, tokio::task::JoinError> {
        self.task.join().await
    }
}

async fn run_exact_screen_transition(
    manager: InputManager,
    reader: InputPublicationReader,
    demands: InputPublicationDemandHandle,
    demand: ScreenPublicationDemandSnapshot,
) -> Result<ExactScreenTransitionOutcome> {
    let revision = demand.revision();
    let graph_generation = demand.graph_generation().get();
    if demands.snapshot().revision() != revision
        || reader.graph_snapshot().generation() != graph_generation
    {
        return Ok(ExactScreenTransitionOutcome::Completed(None));
    }
    let preparation = loop {
        let preparation = manager.try_begin_screen_publication_transition_if(&demand, || {
            demands.snapshot().revision() == revision
                && reader.graph_snapshot().generation() == graph_generation
        });
        let mut demand_changes = demands.subscribe_revision();
        let mut graph_changes = reader.graph.subscribe_generation();
        match preparation {
            TryInputManagerIntent::Busy => {
                tokio::select! {
                    () = manager.wait_for_lifecycle_release_after_busy() => {}
                    _ = demand_changes.changed() => {
                        return Ok(ExactScreenTransitionOutcome::Completed(None));
                    }
                    _ = graph_changes.changed() => {
                        return Ok(ExactScreenTransitionOutcome::Completed(None));
                    }
                }
            }
            TryInputManagerIntent::Stale => {
                return Ok(ExactScreenTransitionOutcome::Completed(None));
            }
            TryInputManagerIntent::Applied(preparation) => {
                break preparation.context("exact screen publication plan was rejected")?;
            }
        }
    };
    let Some(preparation) = preparation else {
        return Ok(ExactScreenTransitionOutcome::Completed(None));
    };
    let mut prepared = Some(
        preparation
            .await_workers()
            .await
            .context("exact screen publication worker preparation failed")?,
    );
    if demands.snapshot().revision() != revision
        || reader.graph_snapshot().generation() != graph_generation
    {
        return Ok(ExactScreenTransitionOutcome::Completed(None));
    }
    #[cfg(test)]
    demands.wait_at_exact_screen_commit_test_hook().await;
    let committed = loop {
        let mut demand_changes = demands.subscribe_revision();
        let mut graph_changes = reader.graph.subscribe_generation();
        match demands.try_commit_if_revision(revision, || {
            manager.try_commit_screen_publication_transition_if(&mut prepared, revision, || {
                reader.graph_snapshot().generation() == graph_generation
            })
        }) {
            TryDemandCommit::Busy => {
                tokio::select! {
                    () = demands.wait_for_revision_gate_release_after_busy() => {}
                    _ = demand_changes.changed() => {
                        return Ok(ExactScreenTransitionOutcome::Completed(None));
                    }
                    _ = graph_changes.changed() => {
                        return Ok(ExactScreenTransitionOutcome::Completed(None));
                    }
                }
            }
            TryDemandCommit::Stale => {
                return Ok(ExactScreenTransitionOutcome::Completed(None));
            }
            TryDemandCommit::Committed(TryInputManagerIntent::Busy) => {
                tokio::select! {
                    () = manager.wait_for_lifecycle_release_after_busy() => {}
                    _ = demand_changes.changed() => {
                        return Ok(ExactScreenTransitionOutcome::Completed(None));
                    }
                    _ = graph_changes.changed() => {
                        return Ok(ExactScreenTransitionOutcome::Completed(None));
                    }
                }
            }
            TryDemandCommit::Committed(TryInputManagerIntent::Stale) => {
                return Ok(ExactScreenTransitionOutcome::Completed(None));
            }
            TryDemandCommit::Committed(TryInputManagerIntent::Applied(committed)) => {
                break committed;
            }
        }
    };
    committed
        .context("exact screen publication plan commit failed")
        .map(|committed| ExactScreenTransitionOutcome::Completed(Some(committed)))
}

async fn run_pump(
    manager: InputManager,
    reader: InputPublicationReader,
    demands: InputPublicationDemandHandle,
    cancel: CancellationToken,
    status: watch::Sender<InputPublicationStatus>,
    ready: oneshot::Sender<()>,
) {
    status.send_replace(InputPublicationStatus::Ready);
    let _ = ready.send(());

    let mut schedule = InputPublicationSchedule::default();
    let mut capture_demand = CaptureDemandState::default();
    let mut applied_exact_screen = None;
    let mut exact_screen_retry = None;
    let mut exact_screen_recovery = None;
    let mut exact_screen_failure_streak: u64 = 0;
    let mut exact_screen_transition: Option<ExactScreenTransitionTask> = None;
    let mut applied_renderer_execution = None;
    let mut publication_retirements = VecDeque::new();
    let mut worker_retirement_tasks = JoinSet::new();
    let mut due_sources = Vec::with_capacity(SOURCE_KINDS.len());
    let mut graph_changes = reader.graph.subscribe_generation();
    let mut demand_changes = demands.subscribe_revision();
    loop {
        reap_screen_publication_retirements(&mut publication_retirements);
        while let Some(result) = worker_retirement_tasks.try_join_next() {
            if let Err(error) = result {
                tracing::warn!(%error, "screen publication retirement task terminated");
            }
        }
        if exact_screen_transition
            .as_ref()
            .is_some_and(ExactScreenTransitionTask::is_finished)
        {
            let current_demand = demands.snapshot();
            let current_graph = reader.graph_snapshot();
            let current_source_revision = match manager
                .try_screen_publication_resolution_revision_if(|| {
                    demands.snapshot().revision() == current_demand.revision()
                        && reader.graph_snapshot().generation() == current_graph.generation()
                }) {
                TryInputManagerIntent::Busy => {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = manager.wait_for_lifecycle_release_after_busy() => {}
                        _ = demand_changes.changed() => {}
                        _ = graph_changes.changed() => {}
                    }
                    continue;
                }
                TryInputManagerIntent::Stale => continue,
                TryInputManagerIntent::Applied(revision) => revision,
            };
            let transition = exact_screen_transition
                .take()
                .expect("finished exact screen transition remains present");
            let transition_key = transition.key;
            let transition_purpose = transition.purpose;
            let current_key = (
                current_demand.revision(),
                current_graph.generation(),
                current_source_revision,
            );
            match transition.join().await {
                Ok(Ok(ExactScreenTransitionOutcome::Completed(committed))) => {
                    if exact_screen_failure_streak > 0 {
                        info!(
                            suppressed_failures = exact_screen_failure_streak,
                            "exact screen publication recovered"
                        );
                        exact_screen_failure_streak = 0;
                    }
                    if transition_key == current_key {
                        match transition_purpose {
                            ExactScreenTransitionPurpose::ApplyDemand => {
                                applied_exact_screen = Some(transition_key);
                                exact_screen_retry = None;
                                exact_screen_recovery = None;
                            }
                            ExactScreenTransitionPurpose::RetireForRetry => {
                                applied_exact_screen = None;
                                exact_screen_retry = Some((transition_key, Instant::now()));
                                exact_screen_recovery = Some(transition_key);
                            }
                        }
                    }
                    if let Some(committed) = committed {
                        let (committed, worker_retirements) = committed.into_parts();
                        for (source, retirement) in worker_retirements {
                            worker_retirement_tasks.spawn(async move {
                                if let Err(error) = retirement.complete().await {
                                    tracing::warn!(%source, %error, "screen source retirement failed");
                                }
                            });
                        }
                        let (_, retirement) = committed.into_parts();
                        publication_retirements.push_back(retirement);
                        reap_screen_publication_retirements(&mut publication_retirements);
                    }
                }
                Ok(Err(error)) => {
                    // The same doomed attempt repeats until the source
                    // changes; one warning per streak keeps the log honest
                    // without flooding it at the retry cadence.
                    if exact_screen_failure_streak == 0 {
                        tracing::warn!(
                            error = format!("{error:#}"),
                            "exact screen publication transition failed"
                        );
                    } else if exact_screen_failure_streak.is_multiple_of(60) {
                        tracing::warn!(
                            error = format!("{error:#}"),
                            suppressed_failures = exact_screen_failure_streak,
                            "exact screen publication still failing"
                        );
                    }
                    exact_screen_failure_streak = exact_screen_failure_streak.saturating_add(1);
                    if transition_key == current_key {
                        applied_exact_screen = None;
                        exact_screen_recovery = Some(transition_key);
                        let committed_plan_is_empty =
                            reader.screen_publications.committed_state().branch_count() == 0;
                        let retry_at = exact_screen_failure_retry_at(
                            transition_purpose,
                            committed_plan_is_empty,
                            Instant::now(),
                        );
                        exact_screen_retry = Some((transition_key, retry_at));
                    }
                }
                Err(error) => {
                    if exact_screen_failure_streak == 0 {
                        tracing::warn!(%error, "exact screen publication transition task terminated");
                    }
                    exact_screen_failure_streak = exact_screen_failure_streak.saturating_add(1);
                    if transition_key == current_key {
                        applied_exact_screen = None;
                        exact_screen_recovery = Some(transition_key);
                        let committed_plan_is_empty =
                            reader.screen_publications.committed_state().branch_count() == 0;
                        exact_screen_retry = Some((
                            transition_key,
                            exact_screen_failure_retry_at(
                                transition_purpose,
                                committed_plan_is_empty,
                                Instant::now(),
                            ),
                        ));
                    }
                }
            }
        }
        let demand = demands.snapshot();
        let desired_capture = demand.capture_demand();
        let mut graph = reader.graph_snapshot();
        if !capture_demand.is_current(graph.generation(), desired_capture) {
            let reconcile = capture_demand.reconcile(&manager, desired_capture, || {
                demands.snapshot().revision() == demand.revision()
            });
            match reconcile {
                CaptureDemandReconcile::Applied => {}
                CaptureDemandReconcile::Busy => {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = manager.wait_for_lifecycle_release_after_busy() => {}
                        _ = demand_changes.changed() => {}
                        _ = graph_changes.changed() => {}
                    }
                    continue;
                }
                CaptureDemandReconcile::Stale => continue,
            }
            graph = reader.graph_snapshot();
        }
        let renderer_execution = demand.screen_renderer_execution();
        let renderer_execution_key = (graph.generation(), renderer_execution);
        if applied_renderer_execution != Some(renderer_execution_key) {
            manager.set_screen_renderer_execution_state(renderer_execution);
            applied_renderer_execution = Some(renderer_execution_key);
        }
        let lifecycle_current = capture_demand.is_current(graph.generation(), desired_capture);
        let source_resolution_revision = match manager
            .try_screen_publication_resolution_revision_if(|| {
                demands.snapshot().revision() == demand.revision()
                    && reader.graph_snapshot().generation() == graph.generation()
            }) {
            TryInputManagerIntent::Busy => {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = manager.wait_for_lifecycle_release_after_busy() => {}
                    _ = demand_changes.changed() => {}
                    _ = graph_changes.changed() => {}
                }
                continue;
            }
            TryInputManagerIntent::Stale => continue,
            TryInputManagerIntent::Applied(revision) => revision,
        };
        let exact_screen_key = (
            demand.revision(),
            graph.generation(),
            source_resolution_revision,
        );
        if exact_screen_transition
            .as_ref()
            .is_some_and(|transition| transition.key != exact_screen_key)
        {
            exact_screen_transition = None;
        }
        let exact_screen_retry_due =
            exact_screen_retry.is_none_or(|(retry_key, retry_at): (_, Instant)| {
                retry_key != exact_screen_key || Instant::now() >= retry_at
            });
        let recovery_active = exact_screen_recovery.is_some();
        let committed_exact_plan_is_empty =
            reader.screen_publications.committed_state().branch_count() == 0;
        let recovery_waits_for_retirement = recovery_active
            && committed_exact_plan_is_empty
            && (!publication_retirements.is_empty() || !worker_retirement_tasks.is_empty());
        if applied_exact_screen != Some(exact_screen_key)
            && exact_screen_retry_due
            && !recovery_waits_for_retirement
            && exact_screen_transition.is_none()
        {
            let purpose = if recovery_active && !committed_exact_plan_is_empty {
                ExactScreenTransitionPurpose::RetireForRetry
            } else {
                ExactScreenTransitionPurpose::ApplyDemand
            };
            let exact_demand = match purpose {
                ExactScreenTransitionPurpose::ApplyDemand => {
                    demand.exact_screen_demand(graph.generation())
                }
                ExactScreenTransitionPurpose::RetireForRetry => {
                    demand.exact_screen_retirement_demand(graph.generation())
                }
            };
            exact_screen_transition = Some(ExactScreenTransitionTask::spawn(
                manager.clone(),
                reader.clone(),
                demands.clone(),
                exact_demand,
                source_resolution_revision,
                purpose,
            ));
        }
        let active_demand = demand_for_active_sources(&graph, &demand);
        let now = Instant::now();
        schedule.synchronize(&active_demand, now);

        if demand.max_requested_hz() == 0 && lifecycle_current {
            let retry_at = exact_screen_retry
                .filter(|(retry_key, _)| *retry_key == exact_screen_key)
                .map_or_else(
                    || now + LIFECYCLE_PROBE_INTERVAL,
                    |(_, retry_at)| retry_at.min(now + LIFECYCLE_PROBE_INTERVAL),
                );
            if exact_screen_retry.is_some()
                || exact_screen_transition.is_some()
                || !publication_retirements.is_empty()
            {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = demand_changes.changed() => {}
                    _ = graph_changes.changed() => {}
                    () = tokio::time::sleep_until(TokioInstant::from_std(retry_at)) => {}
                }
            } else {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = demand_changes.changed() => {}
                    _ = graph_changes.changed() => {}
                }
            }
            continue;
        }

        if demand.max_requested_hz() == 0 {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = demand_changes.changed() => {}
                _ = graph_changes.changed() => {}
                () = tokio::time::sleep(LIFECYCLE_PROBE_INTERVAL) => {}
            }
            continue;
        }

        if active_demand.max_requested_hz() == 0 {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = demand_changes.changed() => {}
                _ = graph_changes.changed() => {}
                () = tokio::time::sleep(LIFECYCLE_PROBE_INTERVAL) => {}
            }
            continue;
        }

        if !schedule.is_due(now) {
            let lifecycle_probe = now.checked_add(LIFECYCLE_PROBE_INTERVAL).unwrap_or(now);
            let wake_at = schedule
                .next_deadline()
                .map_or(lifecycle_probe, |deadline| deadline.min(lifecycle_probe));
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = demand_changes.changed() => {}
                _ = graph_changes.changed() => {}
                () = tokio::time::sleep_until(TokioInstant::from_std(wake_at)) => {}
            }
            continue;
        }

        if demands.snapshot().revision() != demand.revision() {
            continue;
        }
        let mut committed_schedule = schedule.clone();
        committed_schedule.collect_due(Instant::now(), &mut due_sources);
        let sampling = manager.try_sample_source_kinds_if(&due_sources, || {
            demands.snapshot().revision() == demand.revision()
                && reader.graph_snapshot().generation() == graph.generation()
        });
        match sampling {
            TryInputManagerIntent::Applied(()) => schedule = committed_schedule,
            TryInputManagerIntent::Busy => {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = manager.wait_for_lifecycle_release_after_busy() => {}
                    _ = demand_changes.changed() => {}
                    _ = graph_changes.changed() => {}
                }
            }
            TryInputManagerIntent::Stale => continue,
        }
    }

    let inactive_capture = CaptureDemand::new(false, ScreenCaptureDemand::Inactive, false);
    match capture_demand.reconcile(&manager, inactive_capture, || true) {
        CaptureDemandReconcile::Applied => {}
        CaptureDemandReconcile::Busy => {
            debug!("input publication shutdown deferred capture release to source retirement");
        }
        CaptureDemandReconcile::Stale => {
            debug!("input publication shutdown rejected an unexpectedly stale capture release");
        }
    }
    debug!("input publication worker exited");
}

fn reap_screen_publication_retirements(retirements: &mut VecDeque<ScreenPublicationRetirement>) {
    let pending = retirements.len();
    for _ in 0..pending {
        let retirement = retirements
            .pop_front()
            .expect("screen retirement count was captured before this pass");
        if let Err(retirement) = retirement.try_reclaim() {
            retirements.push_back(retirement);
        }
    }
}

fn demand_for_active_sources(
    graph: &InputGraphSnapshot,
    demand: &InputPublicationDemandSnapshot,
) -> InputPublicationCadence {
    let now = Instant::now();
    graph
        .slots()
        .iter()
        .fold(InputPublicationCadence::default(), |active_demand, slot| {
            let status = slot.status().availability_at(now);
            if status.retired
                || !(status.configured && status.consented && status.demanded)
                || !matches!(
                    status.state,
                    SourceState::Starting | SourceState::Live | SourceState::Degraded
                )
            {
                return active_demand;
            }
            active_demand.with_source(status.kind, demand.requested_hz(status.kind))
        })
}

#[derive(Clone, Copy, Debug, Default)]
struct SourceCadence {
    requested_hz: u32,
    last_sample_at: Option<Instant>,
    next_sample_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
struct InputPublicationSchedule {
    sources: [SourceCadence; SOURCE_KINDS.len()],
}

impl InputPublicationSchedule {
    fn synchronize(&mut self, demand: &InputPublicationCadence, now: Instant) {
        for source in SOURCE_KINDS {
            let cadence = &mut self.sources[source_kind_index(source)];
            let requested_hz = demand.requested_hz(source);
            if requested_hz == 0 {
                *cadence = SourceCadence::default();
            } else if cadence.requested_hz != requested_hz {
                cadence.requested_hz = requested_hz;
                cadence.next_sample_at =
                    Some(cadence.last_sample_at.map_or(now, |last_sample_at| {
                        let deadline = last_sample_at
                            .checked_add(cadence_interval(requested_hz))
                            .unwrap_or(now);
                        deadline.max(now)
                    }));
            }
        }
    }

    fn collect_due(&mut self, now: Instant, output: &mut Vec<(SourceKind, f32)>) {
        output.clear();
        for source in SOURCE_KINDS {
            let cadence = &mut self.sources[source_kind_index(source)];
            let Some(next_sample_at) = cadence.next_sample_at else {
                continue;
            };
            if next_sample_at > now {
                continue;
            }
            let interval = cadence_interval(cadence.requested_hz);
            let delta_secs = cadence
                .last_sample_at
                .map_or(interval.as_secs_f32(), |previous| {
                    now.saturating_duration_since(previous).as_secs_f32()
                });
            output.push((source, delta_secs));
            cadence.last_sample_at = Some(now);
            cadence.next_sample_at = Some(next_deadline(next_sample_at, interval, now));
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.sources
            .iter()
            .filter_map(|cadence| cadence.next_sample_at)
            .min()
    }

    fn is_due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| deadline <= now)
    }
}

const fn source_kind_index(source: SourceKind) -> usize {
    match source {
        SourceKind::Audio => 0,
        SourceKind::Screen => 1,
        SourceKind::Interaction => 2,
        SourceKind::Media => 3,
        SourceKind::Network => 4,
        SourceKind::Sensors => 5,
    }
}

fn cadence_interval(requested_hz: u32) -> Duration {
    let nanos = 1_000_000_000_u64.div_ceil(u64::from(requested_hz));
    Duration::from_nanos(nanos)
}

fn next_deadline(scheduled: Instant, interval: Duration, now: Instant) -> Instant {
    let next = scheduled.checked_add(interval).unwrap_or(now);
    if next > now {
        next
    } else {
        now.checked_add(interval).unwrap_or(now)
    }
}

#[cfg(test)]
mod tests;
