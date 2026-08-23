use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use arc_swap::ArcSwap;
use hypercolor_core::input::screen::{
    CommittedScreenPublicationTransition, InputPublicationDemandRevision, LedToneMapCalibration,
    PixelExtent, RegisteredScreenBranchDemand, ScreenAspectPolicy, ScreenBranchLease,
    ScreenCaptureDemand, ScreenExtentRequest, ScreenHdrPolicy, ScreenInputGraphGeneration,
    ScreenNativeExecutionTarget, ScreenPlanGeneration, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationDemandSnapshot,
    ScreenPublicationExecutorRequest, ScreenPublicationHub, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenPublicationRetirement, ScreenSourceSelector,
    ScreenToneMapOperator, ScreenToneMapPolicy, ScreenUpscalePolicy,
};
use hypercolor_core::input::{
    InputGraphHandle, InputGraphSnapshot, InputManager, SourceKind, SourceState,
};
use hypercolor_types::sensor::SystemSnapshot;
use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant as TokioInstant, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use super::capture_demand::{CaptureDemand, CaptureDemandState};

const STOP_TIMEOUT: Duration = Duration::from_secs(1);
// Staleness probe for a committed exact screen plan; capped by how long a
// source-internal retirement may starve the plan before it re-plans.
const COMMITMENT_PROBE_INTERVAL: Duration = Duration::from_secs(1);
const LIFECYCLE_PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// Retry cadence for an exact plan that failed with no committed plan to
/// replace: the source itself must change (session, consent, capacity)
/// before another attempt can succeed, and nothing graph-visible signals
/// that yet, so this is a bounded liveness probe rather than a recovery
/// path. Spec 74 folds the source resolution revision into the pump key,
/// which turns this into an event-driven re-arm.
const EXACT_PLAN_UNAVAILABLE_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const SOURCE_KINDS: [SourceKind; 5] = [
    SourceKind::Audio,
    SourceKind::Screen,
    SourceKind::Interaction,
    SourceKind::Media,
    SourceKind::Network,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputScreenBranchDemand {
    branch: RegisteredScreenBranchDemand,
    legacy_extent: PixelExtent,
}

impl InputScreenBranchDemand {
    /// Pair one exact unresolved branch with its temporary legacy projection.
    #[must_use]
    pub const fn new(branch: RegisteredScreenBranchDemand, legacy_extent: PixelExtent) -> Self {
        Self {
            branch,
            legacy_extent,
        }
    }

    /// Exact unresolved branch preserved by the authoritative registry.
    #[must_use]
    pub const fn branch(&self) -> &RegisteredScreenBranchDemand {
        &self.branch
    }

    /// Concrete extent consumed only by the legacy single-surface adapter.
    #[must_use]
    pub const fn legacy_extent(&self) -> PixelExtent {
        self.legacy_extent
    }

    fn surface(
        requested_hz: u32,
        requested_extent: PixelExtent,
        executor: ScreenPublicationExecutorRequest,
    ) -> Option<Self> {
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
        let request = ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            executor,
            extent,
            ScreenAspectPolicy::Contain,
            Arc::new(profile),
        );
        Some(Self::new(
            RegisteredScreenBranchDemand::new(request, requested_hz),
            requested_extent,
        ))
    }

    const fn requested_hz(&self) -> u32 {
        self.branch.requested_hz().get()
    }

    fn same_publication_request(&self, other: &Self) -> bool {
        self.branch.request() == other.branch.request()
            && self.branch.requested_hz() == other.branch.requested_hz()
            && self.legacy_extent == other.legacy_extent
    }
}

impl InputPublicationDemand {
    pub(crate) fn same_publication_request(&self, other: &Self) -> bool {
        self.audio == other.audio
            && self.interaction == other.interaction
            && self.media == other.media
            && self.network == other.network
            && self.screen.len() == other.screen.len()
            && self
                .screen
                .iter()
                .zip(other.screen.iter())
                .all(|(left, right)| left.same_publication_request(right))
    }

    /// Request the same rate for every typed source.
    #[must_use]
    pub fn all_sources(requested_hz: u32, screen_extent: PixelExtent) -> Self {
        Self {
            audio: requested_hz,
            screen: InputScreenBranchDemand::surface(
                requested_hz,
                screen_extent,
                ScreenPublicationExecutorRequest::Cpu,
            )
            .into_iter()
            .collect::<Vec<_>>()
            .into(),
            interaction: requested_hz,
            media: requested_hz,
            network: requested_hz,
        }
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
        }
        self
    }

    /// Set screen publication cadence and extent together.
    #[must_use]
    pub fn with_screen(mut self, requested_hz: u32, requested_extent: PixelExtent) -> Self {
        self.screen = InputScreenBranchDemand::surface(
            requested_hz,
            requested_extent,
            ScreenPublicationExecutorRequest::Cpu,
        )
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
        self.screen = branches.into_iter().collect::<Vec<_>>().into();
        self
    }

    #[cfg(test)]
    pub(crate) fn screen_requested_extent(&self) -> Option<PixelExtent> {
        self.screen
            .first()
            .map(InputScreenBranchDemand::legacy_extent)
    }

    pub(crate) fn requested_hz(&self, source: SourceKind) -> u32 {
        match source {
            SourceKind::Audio => self.audio,
            SourceKind::Screen => self
                .screen
                .iter()
                .map(InputScreenBranchDemand::requested_hz)
                .max()
                .unwrap_or(0),
            SourceKind::Interaction => self.interaction,
            SourceKind::Media => self.media,
            SourceKind::Network => self.network,
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

#[derive(Clone, Debug, Default)]
struct InputPublicationDemandSnapshot {
    entries: Arc<[InputPublicationDemandEntry]>,
    cadence: InputPublicationCadence,
    screen_branches: Arc<[RegisteredScreenBranchDemand]>,
    compatibility_screen_extent: Option<PixelExtent>,
    compatibility_surface: Option<RegisteredScreenBranchDemand>,
    compatibility_zones: Option<RegisteredScreenBranchDemand>,
    revision: InputPublicationDemandRevision,
}

impl InputPublicationDemandSnapshot {
    fn from_entries(
        entries: Vec<InputPublicationDemandEntry>,
        revision: InputPublicationDemandRevision,
    ) -> Self {
        let mut cadence = InputPublicationCadence::default();
        let mut screen_branches = Vec::new();
        let mut compatibility_screen: Option<(
            &InputPublicationDemandEntry,
            &InputScreenBranchDemand,
        )> = None;
        let mut compatibility_surface = None;
        let mut compatibility_zones = None;
        for entry in &entries {
            cadence.merge_demand(&entry.demand);
            for screen in entry.demand.screen.iter() {
                screen_branches.push(screen.branch.clone());
                if compatibility_screen
                    .as_ref()
                    .is_none_or(|(selected_entry, selected_screen)| {
                        compatibility_screen_precedes(
                            entry,
                            screen,
                            selected_entry,
                            selected_screen,
                        )
                    })
                {
                    compatibility_screen = Some((entry, screen));
                }
                let selected = match screen.branch.request().kind() {
                    ScreenPublicationKind::Surface => &mut compatibility_surface,
                    ScreenPublicationKind::Zones { .. } => &mut compatibility_zones,
                };
                if selected.as_ref().is_none_or(
                    |(selected_entry, selected_screen): &(
                        &InputPublicationDemandEntry,
                        &InputScreenBranchDemand,
                    )| {
                        compatibility_screen_precedes(
                            entry,
                            screen,
                            selected_entry,
                            selected_screen,
                        )
                    },
                ) {
                    *selected = Some((entry, screen));
                }
            }
        }
        let compatibility_screen_extent =
            compatibility_screen.map(|(_, screen)| screen.legacy_extent);
        let compatibility_surface = compatibility_surface.map(|(_, screen)| screen.branch.clone());
        let compatibility_zones = compatibility_zones.map(|(_, screen)| screen.branch.clone());
        Self {
            entries: entries.into(),
            cadence,
            screen_branches: screen_branches.into(),
            compatibility_screen_extent,
            compatibility_surface,
            compatibility_zones,
            revision,
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

    fn capture_demand(&self) -> CaptureDemand {
        let screen = self
            .compatibility_screen_extent
            .map_or(ScreenCaptureDemand::Inactive, ScreenCaptureDemand::active);
        CaptureDemand::new(
            self.requested_hz(SourceKind::Audio) > 0,
            screen,
            self.requested_hz(SourceKind::Interaction) > 0,
        )
    }

    const fn revision(&self) -> InputPublicationDemandRevision {
        self.revision
    }

    fn exact_screen_demand(&self, graph_generation: u64) -> ScreenPublicationDemandSnapshot {
        ScreenPublicationDemandSnapshot::try_new(
            self.revision,
            ScreenInputGraphGeneration::new(graph_generation),
            Arc::clone(&self.screen_branches),
            self.compatibility_surface.clone(),
            self.compatibility_zones.clone(),
        )
        .expect("compatibility branches originate from the exact demand snapshot")
    }

    fn exact_screen_retirement_demand(
        &self,
        graph_generation: u64,
    ) -> ScreenPublicationDemandSnapshot {
        ScreenPublicationDemandSnapshot::try_new(
            self.revision,
            ScreenInputGraphGeneration::new(graph_generation),
            Arc::default(),
            None,
            None,
        )
        .expect("an empty exact screen demand has no compatibility branches")
    }
}

fn compatibility_screen_precedes(
    candidate: &InputPublicationDemandEntry,
    candidate_screen: &InputScreenBranchDemand,
    selected: &InputPublicationDemandEntry,
    selected_screen: &InputScreenBranchDemand,
) -> bool {
    let candidate_rank = consumer_rank(candidate.consumer);
    let selected_rank = consumer_rank(selected.consumer);
    if candidate_rank != selected_rank {
        return candidate_rank < selected_rank;
    }
    let candidate_kind = publication_kind_rank(candidate_screen.branch.request().kind());
    let selected_kind = publication_kind_rank(selected_screen.branch.request().kind());
    if candidate_kind != selected_kind {
        return candidate_kind < selected_kind;
    }
    let candidate_pixels = u64::from(candidate_screen.legacy_extent.width())
        * u64::from(candidate_screen.legacy_extent.height());
    let selected_pixels = u64::from(selected_screen.legacy_extent.width())
        * u64::from(selected_screen.legacy_extent.height());
    candidate_pixels > selected_pixels
        || (candidate_pixels == selected_pixels
            && candidate_screen.requested_hz() > selected_screen.requested_hz())
        || (candidate_pixels == selected_pixels
            && candidate_screen.requested_hz() == selected_screen.requested_hz()
            && candidate.id < selected.id)
}

const fn publication_kind_rank(kind: ScreenPublicationKind) -> u8 {
    match kind {
        ScreenPublicationKind::Surface => 0,
        ScreenPublicationKind::Zones { .. } => 1,
    }
}

const fn consumer_rank(consumer: InputPublicationConsumer) -> u8 {
    match consumer {
        InputPublicationConsumer::Authoritative => 0,
        InputPublicationConsumer::Preview => 1,
        InputPublicationConsumer::PassiveStream => 2,
        InputPublicationConsumer::Diagnostic => 3,
    }
}

struct InputPublicationDemandRegistry {
    next_id: AtomicU64,
    latest: ArcSwap<InputPublicationDemandSnapshot>,
    revision_tx: watch::Sender<InputPublicationDemandRevision>,
    revision_gate: SyncMutex<()>,
    #[cfg(test)]
    commit_test_hook: SyncMutex<Option<ExactScreenCommitTestHook>>,
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
    #[must_use]
    pub fn new() -> Self {
        let (revision_tx, _) = watch::channel(InputPublicationDemandRevision::default());
        Self {
            registry: Arc::new(InputPublicationDemandRegistry {
                next_id: AtomicU64::new(1),
                latest: ArcSwap::from_pointee(InputPublicationDemandSnapshot::default()),
                revision_tx,
                revision_gate: SyncMutex::new(()),
                #[cfg(test)]
                commit_test_hook: SyncMutex::new(None),
            }),
        }
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

    pub(crate) fn is_active(&self) -> bool {
        self.snapshot().max_requested_hz() > 0
    }

    fn snapshot(&self) -> Arc<InputPublicationDemandSnapshot> {
        self.registry.latest.load_full()
    }

    fn subscribe_revision(&self) -> watch::Receiver<InputPublicationDemandRevision> {
        self.registry.revision_tx.subscribe()
    }

    fn commit_if_revision<T>(
        &self,
        expected: InputPublicationDemandRevision,
        commit: impl FnOnce() -> T,
    ) -> Option<T> {
        let _revision_guard = self
            .registry
            .revision_gate
            .lock()
            .expect("input publication revision gate is healthy");
        (self.registry.latest.load().revision() == expected).then(commit)
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
        Self::new()
    }
}

impl InputPublicationDemandRegistry {
    fn update_entries(&self, update: impl Fn(&mut Vec<InputPublicationDemandEntry>)) {
        let _revision_guard = self
            .revision_gate
            .lock()
            .expect("input publication revision gate is healthy");
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
                entries, revision,
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
    sensors: Option<watch::Receiver<Arc<SystemSnapshot>>>,
    #[allow(
        dead_code,
        reason = "screen publication leases are optional for pump consumers"
    )]
    screen_publications: Arc<ScreenPublicationHub>,
}

impl InputPublicationReader {
    fn new(
        graph: InputGraphHandle,
        sensors: Option<watch::Receiver<Arc<SystemSnapshot>>>,
        screen_publications: Arc<ScreenPublicationHub>,
    ) -> Self {
        Self {
            graph,
            sensors,
            screen_publications,
        }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self::new(
            InputGraphHandle::default(),
            None,
            hypercolor_core::input::screen::ScreenPlanBuilder::new().publication_hub(),
        )
    }

    pub(crate) fn graph_snapshot(&self) -> Arc<InputGraphSnapshot> {
        self.graph.snapshot()
    }

    pub(crate) fn latest_sensor_snapshot(&self) -> Option<Arc<SystemSnapshot>> {
        self.sensors
            .as_ref()
            .map(|receiver| Arc::clone(&receiver.borrow()))
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
        self.screen_publications
            .observe_matching_lease(|descriptor| {
                descriptor.kind() == ScreenPublicationKind::Surface
                    && descriptor.geometry().output_extent() == extent
                    && match (target, descriptor.requested_executor()) {
                        (
                            Some(target),
                            ScreenPublicationExecutorRequest::SourceNative(selected),
                        ) => selected.id() == target.id(),
                        (None, ScreenPublicationExecutorRequest::Cpu) => true,
                        _ => false,
                    }
            })
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
        manager: Arc<Mutex<InputManager>>,
        demands: InputPublicationDemandHandle,
    ) -> Result<Self> {
        let reader = {
            let manager = manager.lock().await;
            InputPublicationReader::new(
                manager.input_graph_handle(),
                manager.sensor_snapshot_receiver(),
                manager.screen_publication_hub(),
            )
        };
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

type ExactScreenTransitionKey = (InputPublicationDemandRevision, u64);

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
    task: AbortOnDropTask<Result<Option<CommittedScreenPublicationTransition>>>,
}

impl ExactScreenTransitionTask {
    fn spawn(
        manager: Arc<Mutex<InputManager>>,
        reader: InputPublicationReader,
        demands: InputPublicationDemandHandle,
        demand: ScreenPublicationDemandSnapshot,
        purpose: ExactScreenTransitionPurpose,
    ) -> Self {
        let key = (demand.revision(), demand.graph_generation().get());
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
    ) -> std::result::Result<
        Result<Option<CommittedScreenPublicationTransition>>,
        tokio::task::JoinError,
    > {
        self.task.join().await
    }
}

async fn run_exact_screen_transition(
    manager: Arc<Mutex<InputManager>>,
    reader: InputPublicationReader,
    demands: InputPublicationDemandHandle,
    demand: ScreenPublicationDemandSnapshot,
) -> Result<Option<CommittedScreenPublicationTransition>> {
    let revision = demand.revision();
    let graph_generation = demand.graph_generation().get();
    let mut input_manager = manager.lock().await;
    if demands.snapshot().revision() != revision
        || reader.graph_snapshot().generation() != graph_generation
    {
        return Ok(None);
    }
    let preparation = input_manager
        .begin_screen_publication_transition(demand)
        .context("exact screen publication plan was rejected")?;
    drop(input_manager);
    let Some(preparation) = preparation else {
        return Ok(None);
    };
    let prepared = preparation
        .await_workers()
        .await
        .context("exact screen publication worker preparation failed")?;
    if demands.snapshot().revision() != revision
        || reader.graph_snapshot().generation() != graph_generation
    {
        return Ok(None);
    }
    let mut input_manager = manager.lock().await;
    if demands.snapshot().revision() != revision
        || reader.graph_snapshot().generation() != graph_generation
    {
        return Ok(None);
    }
    #[cfg(test)]
    demands.wait_at_exact_screen_commit_test_hook().await;
    let Some(committed) = demands.commit_if_revision(revision, || {
        input_manager.commit_screen_publication_transition(prepared, revision)
    }) else {
        return Ok(None);
    };
    committed
        .context("exact screen publication plan commit failed")
        .map(Some)
}

async fn run_pump(
    manager: Arc<Mutex<InputManager>>,
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
    let mut publication_retirements = VecDeque::new();
    let mut worker_retirement_tasks = JoinSet::new();
    let mut due_sources = Vec::with_capacity(SOURCE_KINDS.len());
    let mut last_commitment_probe = Instant::now();
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
            let transition = exact_screen_transition
                .take()
                .expect("finished exact screen transition remains present");
            let transition_key = transition.key;
            let transition_purpose = transition.purpose;
            let current_key = (
                demands.snapshot().revision(),
                reader.graph_snapshot().generation(),
            );
            match transition.join().await {
                Ok(Ok(committed)) => {
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
            let manager_lock = manager.lock();
            tokio::pin!(manager_lock);
            let mut input_manager = tokio::select! {
                () = cancel.cancelled() => break,
                _ = demand_changes.changed() => continue,
                manager = &mut manager_lock => manager,
            };
            if demands.snapshot().revision() != demand.revision() {
                continue;
            }
            capture_demand.reconcile(&mut input_manager, desired_capture);
            drop(input_manager);
            graph = reader.graph_snapshot();
        }
        let lifecycle_current = capture_demand.is_current(graph.generation(), desired_capture);
        let exact_screen_key = (demand.revision(), graph.generation());
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
                Arc::clone(&manager),
                reader.clone(),
                demands.clone(),
                exact_demand,
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

        let manager_lock = manager.lock();
        tokio::pin!(manager_lock);
        let mut manager = tokio::select! {
            () = cancel.cancelled() => break,
            _ = demand_changes.changed() => continue,
            _ = graph_changes.changed() => continue,
            manager = &mut manager_lock => manager,
        };
        if demands.snapshot().revision() != demand.revision() {
            continue;
        }
        // A screen source can retire its publication source without any
        // demand or graph change (a capture worker restart does exactly
        // this), leaving a committed plan with no producer behind it.
        // Piggyback a throttled currency probe on the sampling lock so a
        // starved plan gets re-planned instead of trusted forever.
        if applied_exact_screen.is_some()
            && last_commitment_probe.elapsed() >= COMMITMENT_PROBE_INTERVAL
        {
            last_commitment_probe = Instant::now();
            if !manager.screen_publication_commitment_is_current() {
                applied_exact_screen = None;
            }
        }
        schedule.collect_due(Instant::now(), &mut due_sources);
        manager.sample_source_kinds(&due_sources);
    }

    // Cancellation can win the wake-up race after the authoritative renderer
    // clears its demand, so shutdown must explicitly release capture hardware.
    let mut manager = manager.lock().await;
    capture_demand.reconcile(
        &mut manager,
        CaptureDemand::new(false, ScreenCaptureDemand::Inactive, false),
    );
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

#[derive(Debug, Default)]
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
