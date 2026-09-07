use super::{
    CaptureActivity, CaptureBackend, CaptureBackendHandles, CaptureCommandEndpoint,
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureExactRuntimeCollection, CaptureExactRuntimeOwner,
    CaptureExactRuntimeStore, CaptureExactState, CaptureOwnedSource, CapturePublicationFence,
    CapturePublicationSource, CaptureRetirementCause, CaptureSession, CaptureSessionAuthority,
    CaptureSessionAuthoritySequencer, CaptureSessionDeadline, CaptureSessionExit,
    CaptureSessionReadiness, CaptureSessionSet, CaptureSessionTransaction, CaptureSourceShell,
    CaptureSuccessorPolicy, CaptureSuccessorPreparationError, CaptureWorkerCommand,
    ReservedCaptureSessionAuthority, ScreenCaptureAdapter, ScreenCaptureAdapterAssembly,
    VersionedCaptureSettings, begin_capture_exact_retirement, begin_capture_settings_adoption,
    exact_preparation_abort, finish_removed_capture_exact_source,
    preflight_capture_exact_scope_bytes, reap_capture_exact_runtimes,
};
use crate::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CpuReductionExecutor, ExactBoxList, InputPublicationDemandRevision,
    PhysicalOrigin, PixelExtent, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenByteAdmissionCoordinator, ScreenCaptureBackend,
    ScreenCaptureDemand, ScreenCommittedState, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenPlanBuilder, ScreenProcessingProfile, ScreenProcessingProfileConfig,
    ScreenPublicationExecutorRequest, ScreenPublicationHub, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenPublicationSlotPolicy, ScreenResourceApi,
    ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy, ScreenWorkerBinding,
    ScreenWorkerExactLedgerBuilder, SourceScale,
};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

fn exact_scope_test_source() -> ResolvedScreenSource {
    let extent = PixelExtent::new(17, 11).expect("test extent is non-empty");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new("synthetic:adapter-accounting")
                .expect("test source id is non-empty"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new(
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent,
                extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("test geometry is valid"),
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn exact_scope_test_demand(
    source: &ResolvedScreenSource,
    requested_hz: NonZeroU32,
) -> ResolvedScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: NonZeroU32::new(13).expect("test columns are non-zero"),
                rows: NonZeroU32::new(7).expect("test rows are non-zero"),
            },
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                NonZeroU32::new(17),
                NonZeroU32::new(11),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        requested_hz,
    )
    .resolve_with_color_capabilities(
        source,
        CpuReductionExecutor::new(
            std::num::NonZeroUsize::MIN,
            NonZeroU32::new(3).expect("test batch size is non-zero"),
        )
        .expect("test CPU executor builds")
        .capabilities(),
    )
    .expect("test branch resolves")
}

fn exact_scope_test_plan() -> (
    ScreenPlanBuilder,
    crate::input::screen::PreparingScreenPlan,
    ResolvedScreenSource,
    ScreenByteAdmissionCoordinator,
) {
    let source = exact_scope_test_source();
    let coordinator = ScreenByteAdmissionCoordinator::default();
    let mut builder = ScreenPlanBuilder::with_publication_slots_and_admission(
        ScreenPublicationSlotPolicy::default(),
        coordinator.clone(),
    );
    let preparing = builder
        .prepare(
            [exact_scope_test_demand(
                &source,
                NonZeroU32::new(60).expect("test cadence is non-zero"),
            )],
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan prepares");
    (builder, preparing, source, coordinator)
}

fn exact_scope_test_ledger() -> (
    ScreenWorkerExactLedgerBuilder,
    ScreenByteAdmissionCoordinator,
    crate::input::screen::PreparingScreenPlan,
) {
    let (_builder, mut preparing, source, coordinator) = exact_scope_test_plan();
    let ticket = preparing
        .worker_ticket(&source.epoch().source_id)
        .expect("test source owns one worker ticket");
    let ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("test ledger metadata prepares");
    (ledger, coordinator, preparing)
}

fn exact_scope_minimum_token(
    ticket: crate::input::screen::ScreenWorkerPreparationTicket,
) -> (
    crate::input::screen::ScreenPreparedWorkerToken,
    Box<[crate::input::screen::ScreenResourceLifetime]>,
) {
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("test ledger metadata prepares");
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, bytes) in reports {
        ledger
            .report(&name, bytes)
            .expect("test required minimum reports");
    }
    ledger
        .finish()
        .expect("test exact ledger finishes")
        .into_parts()
}

#[test]
fn exact_scope_preflight_consumes_modeled_bytes_before_reserving_excess() {
    let (mut ledger, coordinator, _preparing) = exact_scope_test_ledger();
    let baseline = coordinator.snapshot().reserved_bytes();
    let mut minimum_remaining = 128;

    preflight_capture_exact_scope_bytes(&mut ledger, &mut minimum_remaining, 48)
        .expect("modeled bytes preflight");
    assert_eq!(minimum_remaining, 80);
    assert_eq!(coordinator.snapshot().reserved_bytes(), baseline);

    preflight_capture_exact_scope_bytes(&mut ledger, &mut minimum_remaining, 80)
        .expect("exact remaining minimum preflights");
    assert_eq!(minimum_remaining, 0);
    assert_eq!(coordinator.snapshot().reserved_bytes(), baseline);

    preflight_capture_exact_scope_bytes(&mut ledger, &mut minimum_remaining, 32)
        .expect("first excess reserves");
    preflight_capture_exact_scope_bytes(&mut ledger, &mut minimum_remaining, 16)
        .expect("second excess reserves");
    assert_eq!(coordinator.snapshot().reserved_bytes(), baseline + 48);

    coordinator
        .try_set_capacity(ScreenAdmissionCapacity::new(baseline + 48, baseline + 48))
        .expect("test capacity fences current reservations");
    minimum_remaining = 1;
    assert!(preflight_capture_exact_scope_bytes(&mut ledger, &mut minimum_remaining, 2).is_err());
    assert_eq!(minimum_remaining, 0);
    assert_eq!(coordinator.snapshot().reserved_bytes(), baseline + 48);
}

#[test]
fn removed_source_finalization_rejects_live_work_and_commits_only_full_removal() {
    let (_builder, mut added_preparing, source, _coordinator) = exact_scope_test_plan();
    let added_ticket = added_preparing
        .worker_ticket(&source.epoch().source_id)
        .expect("added source owns one worker ticket");
    assert!(finish_removed_capture_exact_source(added_ticket).is_err());

    let (mut builder, mut initial_preparing, source, _coordinator) = exact_scope_test_plan();
    let source_id = source.epoch().source_id.clone();
    let initial_revision = InputPublicationDemandRevision::new(1);
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let initial_ticket = initial_preparing
        .worker_ticket(&source_id)
        .expect("initial source owns one worker ticket");
    let (initial_token, initial_lifetimes) = exact_scope_minimum_token(initial_ticket);
    initial_preparing
        .acknowledge(initial_token)
        .expect("initial token belongs to its plan");
    let initial_armed = initial_preparing
        .arm(
            builder.current().generation(),
            initial_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("initial plan arms: {}", failure.error()));
    let initial_committed = builder
        .commit(initial_armed, initial_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("initial plan commits: {}", failure.error()));
    initial_committed
        .into_parts()
        .1
        .try_reclaim()
        .expect("initial plan has no retired publication resources");

    let retained_revision = InputPublicationDemandRevision::new(2);
    let mut retained_preparing = builder
        .prepare(
            [exact_scope_test_demand(
                &source,
                NonZeroU32::new(30).expect("test cadence is non-zero"),
            )],
            retained_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("retained source plan prepares");
    let retained_ticket = retained_preparing
        .worker_ticket(&source_id)
        .expect("retained source owns one worker ticket");
    assert!(finish_removed_capture_exact_source(retained_ticket).is_err());
    drop(retained_preparing);

    let removal_revision = InputPublicationDemandRevision::new(3);
    let mut removal_preparing = builder
        .prepare(
            std::iter::empty::<ResolvedScreenBranchDemand>(),
            removal_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("removed source plan prepares");
    let removal_ticket = removal_preparing
        .worker_ticket(&source_id)
        .expect("removed source owns one worker ticket");
    assert!(removal_ticket.source_delta().added_branches().is_empty());
    assert!(removal_ticket.source_delta().retained_branches().is_empty());
    assert!(!removal_ticket.source_delta().removed_branches().is_empty());
    let removal_token = finish_removed_capture_exact_source(removal_ticket)
        .expect("full source removal produces an empty exact token");
    assert!(removal_token.exact_ledger().resources().is_empty());
    removal_preparing
        .acknowledge(removal_token)
        .expect("removal token belongs to its plan");
    let removal_armed = removal_preparing
        .arm(
            builder.current().generation(),
            removal_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("removal plan arms: {}", failure.error()));
    assert!(
        removal_armed
            .candidate_state()
            .runtime_binding(&source_id)
            .is_none()
    );
    let removed = builder
        .commit(removal_armed, removal_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("removal plan commits: {}", failure.error()));
    assert!(removed.plan().branches().is_empty());
    drop((removed, initial_lifetimes));
}

#[derive(Clone)]
struct FakeExactEndpoint {
    commands: Arc<Mutex<Vec<CaptureExactCommand>>>,
    wakes: Arc<AtomicUsize>,
    authority: Arc<AtomicU64>,
    reject: bool,
}

struct FakeSession {
    authority: CaptureSessionAuthority,
    endpoint: FakeExactEndpoint,
    finished: Arc<AtomicBool>,
    aborts: Arc<AtomicUsize>,
    wakes: Arc<AtomicUsize>,
    finishes: Arc<AtomicUsize>,
    detaches: Arc<AtomicUsize>,
    starts: Arc<AtomicUsize>,
    start_prerequisite: Option<Arc<AtomicBool>>,
    events: Option<Arc<Mutex<Vec<&'static str>>>>,
}

impl FakeSession {
    fn new(generation: u64) -> Self {
        let endpoint = FakeExactEndpoint::default();
        endpoint.authority.store(generation, Ordering::Release);
        Self {
            authority: CaptureSessionAuthority::new(generation),
            endpoint,
            finished: Arc::new(AtomicBool::new(false)),
            aborts: Arc::default(),
            wakes: Arc::default(),
            finishes: Arc::default(),
            detaches: Arc::default(),
            starts: Arc::default(),
            start_prerequisite: None,
            events: None,
        }
    }
}

impl CaptureSessionExit for CaptureSessionAuthority {
    fn failure(&self) -> Option<String> {
        None
    }
}

impl CaptureSession for FakeSession {
    type Exit = CaptureSessionAuthority;
    type ExactEndpoint = FakeExactEndpoint;

    const SUCCESSOR_POLICY: CaptureSuccessorPolicy = CaptureSuccessorPolicy::AllowOverlap;

    fn authority(&self) -> CaptureSessionAuthority {
        self.authority
    }

    fn exact_endpoint(&self) -> Self::ExactEndpoint {
        self.endpoint.clone()
    }

    fn abort(&self) {
        if let Some(events) = self.events.as_ref() {
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("abort");
        }
        self.aborts.fetch_add(1, Ordering::Relaxed);
    }

    fn wake(&self) {
        if let Some(events) = self.events.as_ref() {
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("wake");
        }
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }

    fn start(&self) {
        if let Some(prerequisite) = self.start_prerequisite.as_ref() {
            assert!(prerequisite.load(Ordering::Acquire));
        }
        if let Some(events) = self.events.as_ref() {
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("start");
        }
        self.starts.fetch_add(1, Ordering::Relaxed);
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn finish(self) -> Self::Exit {
        self.finishes.fetch_add(1, Ordering::Relaxed);
        self.authority
    }

    fn detach(self) {
        self.detaches.fetch_add(1, Ordering::Relaxed);
    }
}

struct ExclusiveFakeSession(FakeSession);

impl CaptureSession for ExclusiveFakeSession {
    type Exit = CaptureSessionAuthority;
    type ExactEndpoint = FakeExactEndpoint;

    const SUCCESSOR_POLICY: CaptureSuccessorPolicy = CaptureSuccessorPolicy::WaitForRetirement;

    fn authority(&self) -> CaptureSessionAuthority {
        self.0.authority()
    }

    fn exact_endpoint(&self) -> Self::ExactEndpoint {
        self.0.exact_endpoint()
    }

    fn abort(&self) {
        self.0.abort();
    }

    fn wake(&self) {
        self.0.wake();
    }

    fn start(&self) {
        self.0.start();
    }

    fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    fn finish(self) -> Self::Exit {
        self.0.finish()
    }

    fn detach(self) {
        self.0.detach();
    }
}

struct FakeReadiness {
    result: Result<(), &'static str>,
    waits: Arc<AtomicUsize>,
}

struct DropProbe(Arc<AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl FakeReadiness {
    fn ready() -> Self {
        Self {
            result: Ok(()),
            waits: Arc::default(),
        }
    }

    fn failed(message: &'static str) -> Self {
        Self {
            result: Err(message),
            waits: Arc::default(),
        }
    }
}

impl CaptureSessionReadiness for FakeReadiness {
    fn wait(self, deadline: CaptureSessionDeadline) -> anyhow::Result<()> {
        self.waits.fetch_add(1, Ordering::Relaxed);
        if deadline.remaining().is_zero() {
            anyhow::bail!("capture readiness deadline elapsed");
        }
        self.result.map_err(anyhow::Error::msg)
    }
}

#[derive(Default)]
struct FakeCaptureBackend {
    publication_resolutions: AtomicUsize,
    publication_source_incarnation: AtomicU64,
    publication_settings_address: AtomicUsize,
    fail_publication_resolution: AtomicBool,
}

impl CaptureBackend for FakeCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = (FakeSession, FakeReadiness);
    type SettingsConfig = ();
    type ExactState = CaptureExactPublicationShared<FakeSource, FakeOwnedSource>;
    type ActivityFence = FakeFence;
    type ActivityEpoch = FakeEpoch;
    type AuthorityCommitCheckpoint<'a> = CaptureBackendHandles<'a, Self>;

    const NAME: &'static str = "fake capture";
    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn resolve_publication_branch(
        &self,
        settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        source: &<Self::ExactState as CaptureExactState>::Source,
        _demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        self.publication_resolutions.fetch_add(1, Ordering::Relaxed);
        self.publication_source_incarnation
            .store(source.incarnation, Ordering::Relaxed);
        self.publication_settings_address
            .store(std::ptr::from_ref(settings).addr(), Ordering::Relaxed);
        if self.fail_publication_resolution.load(Ordering::Relaxed) {
            anyhow::bail!("injected publication resolution failure");
        }
        Ok(None)
    }

    fn spawn_worker(
        &self,
        (session, readiness): Self::SpawnRequest,
        _handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        Ok(CaptureSessionTransaction::new(
            session,
            readiness,
            reservation,
        ))
    }

    fn prepare_authority_commit<'a>(
        &'a self,
        handles: CaptureBackendHandles<'a, Self>,
        _reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>> {
        Some(handles)
    }

    fn commit_authority(
        reservation: ReservedCaptureSessionAuthority,
        checkpoint: Self::AuthorityCommitCheckpoint<'_>,
    ) {
        drop(
            checkpoint
                .exact_state()
                .activate_reserved_authority(reservation)
                .expect("reserved fake capture authority remains current"),
        );
    }

    fn retire_authority(
        &self,
        handles: CaptureBackendHandles<'_, Self>,
        authority: CaptureSessionAuthority,
        _cause: CaptureRetirementCause,
    ) {
        drop(
            handles
                .exact_state()
                .retire_authority_if_current(authority)
                .expect("fake capture retirement authority reserves"),
        );
    }
}

#[derive(Default)]
struct ExclusiveCaptureBackend {
    spawns: Arc<AtomicUsize>,
}

impl CaptureBackend for ExclusiveCaptureBackend {
    type Worker = ExclusiveFakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = (ExclusiveFakeSession, FakeReadiness);
    type SettingsConfig = ();
    type ExactState = CaptureExactPublicationShared<FakeSource, FakeOwnedSource>;
    type ActivityFence = FakeFence;
    type ActivityEpoch = FakeEpoch;
    type AuthorityCommitCheckpoint<'a> = CaptureBackendHandles<'a, Self>;

    const NAME: &'static str = "fake capture";
    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn resolve_publication_branch(
        &self,
        _settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        _source: &<Self::ExactState as CaptureExactState>::Source,
        _demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        Ok(None)
    }

    fn spawn_worker(
        &self,
        (session, readiness): Self::SpawnRequest,
        _handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        self.spawns.fetch_add(1, Ordering::Relaxed);
        Ok(CaptureSessionTransaction::new(
            session,
            readiness,
            reservation,
        ))
    }

    fn prepare_authority_commit<'a>(
        &'a self,
        handles: CaptureBackendHandles<'a, Self>,
        _reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>> {
        Some(handles)
    }

    fn commit_authority(
        reservation: ReservedCaptureSessionAuthority,
        checkpoint: Self::AuthorityCommitCheckpoint<'_>,
    ) {
        drop(
            checkpoint
                .exact_state()
                .activate_reserved_authority(reservation)
                .expect("reserved exclusive capture authority remains current"),
        );
    }

    fn retire_authority(
        &self,
        handles: CaptureBackendHandles<'_, Self>,
        authority: CaptureSessionAuthority,
        _cause: CaptureRetirementCause,
    ) {
        drop(
            handles
                .exact_state()
                .retire_authority_if_current(authority)
                .expect("exclusive capture retirement authority reserves"),
        );
    }
}

struct StatefulCaptureBackend {
    compatibility: Weak<Mutex<CaptureActivity<FakeFence, FakeEpoch>>>,
    exact: Weak<CaptureExactPublicationShared<FakeSource, FakeOwnedSource>>,
    requests: Arc<Mutex<Vec<u64>>>,
    authority_commit_enabled: Arc<AtomicBool>,
    authority_commit_events: Arc<Mutex<Vec<&'static str>>>,
}

struct StatefulAuthorityCommitCheckpoint<'a> {
    handles: CaptureBackendHandles<'a, StatefulCaptureBackend>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl CaptureBackend for StatefulCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = (u64, FakeSession, FakeReadiness);
    type SettingsConfig = ();
    type ExactState = CaptureExactPublicationShared<FakeSource, FakeOwnedSource>;
    type ActivityFence = FakeFence;
    type ActivityEpoch = FakeEpoch;
    type AuthorityCommitCheckpoint<'a> = StatefulAuthorityCommitCheckpoint<'a>;

    const NAME: &'static str = "fake capture";
    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn resolve_publication_branch(
        &self,
        _settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        _source: &<Self::ExactState as CaptureExactState>::Source,
        _demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        Ok(None)
    }

    fn spawn_worker(
        &self,
        (request, session, readiness): Self::SpawnRequest,
        handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        let compatibility = handles.activity_handle();
        let exact = handles.exact_state_handle();
        assert!(Arc::ptr_eq(
            &compatibility,
            &self
                .compatibility
                .upgrade()
                .expect("adapter compatibility handle remains alive")
        ));
        assert!(Arc::ptr_eq(
            &exact,
            &self
                .exact
                .upgrade()
                .expect("adapter exact handle remains alive")
        ));
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request);
        Ok(CaptureSessionTransaction::new(
            session,
            readiness,
            reservation,
        ))
    }

    fn prepare_authority_commit<'a>(
        &'a self,
        handles: CaptureBackendHandles<'a, Self>,
        _reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>> {
        self.authority_commit_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push("checkpoint");
        self.authority_commit_enabled
            .load(Ordering::Acquire)
            .then(|| StatefulAuthorityCommitCheckpoint {
                handles,
                events: Arc::clone(&self.authority_commit_events),
            })
    }

    fn commit_authority(
        reservation: ReservedCaptureSessionAuthority,
        checkpoint: Self::AuthorityCommitCheckpoint<'_>,
    ) {
        checkpoint
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push("commit");
        drop(
            checkpoint
                .handles
                .exact_state()
                .activate_reserved_authority(reservation)
                .expect("reserved stateful capture authority remains current"),
        );
    }

    fn retire_authority(
        &self,
        _handles: CaptureBackendHandles<'_, Self>,
        _authority: CaptureSessionAuthority,
        cause: CaptureRetirementCause,
    ) {
        let event = match cause {
            CaptureRetirementCause::ActiveStop => "retire_active",
            CaptureRetirementCause::ObservedExit => "retire_exit",
            CaptureRetirementCause::ExclusiveSettlement => "retire_settled",
        };
        self.authority_commit_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

struct StatefulAdapterFixture {
    adapter: ScreenCaptureAdapter<StatefulCaptureBackend>,
    exact: Arc<CaptureExactPublicationShared<FakeSource, FakeOwnedSource>>,
    requests: Arc<Mutex<Vec<u64>>>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

fn stateful_adapter(authority_commit_enabled: bool) -> StatefulAdapterFixture {
    let exact = Arc::new(CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default());
    let assembly =
        ScreenCaptureAdapterAssembly::<StatefulCaptureBackend>::new(Arc::clone(&exact), ());
    let compatibility = assembly.handles().activity_handle();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let authority_commit_events = Arc::new(Mutex::new(Vec::new()));
    let adapter = assembly.finish(StatefulCaptureBackend {
        compatibility: Arc::downgrade(&compatibility),
        exact: Arc::downgrade(&exact),
        requests: Arc::clone(&requests),
        authority_commit_enabled: Arc::new(AtomicBool::new(authority_commit_enabled)),
        authority_commit_events: Arc::clone(&authority_commit_events),
    });
    StatefulAdapterFixture {
        adapter,
        exact,
        requests,
        events: authority_commit_events,
    }
}

#[derive(Default)]
struct FailingCaptureBackend;

impl CaptureBackend for FailingCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = ();
    type SettingsConfig = ();
    type ExactState = CaptureExactPublicationShared<FakeSource, FakeOwnedSource>;
    type ActivityFence = FakeFence;
    type ActivityEpoch = FakeEpoch;
    type AuthorityCommitCheckpoint<'a> = ();

    const NAME: &'static str = "fake capture";
    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn resolve_publication_branch(
        &self,
        _settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        _source: &<Self::ExactState as CaptureExactState>::Source,
        _demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        Ok(None)
    }

    fn spawn_worker(
        &self,
        (): Self::SpawnRequest,
        _handles: CaptureBackendHandles<'_, Self>,
        _reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        anyhow::bail!("injected backend spawn failure")
    }

    fn prepare_authority_commit<'a>(
        &'a self,
        _handles: CaptureBackendHandles<'a, Self>,
        _reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>> {
        Some(())
    }

    fn commit_authority(
        _reservation: ReservedCaptureSessionAuthority,
        (): Self::AuthorityCommitCheckpoint<'_>,
    ) {
    }

    fn retire_authority(
        &self,
        _handles: CaptureBackendHandles<'_, Self>,
        _authority: CaptureSessionAuthority,
        _cause: CaptureRetirementCause,
    ) {
    }
}

struct DropOrderExactState {
    common: CaptureExactPublicationShared<FakeSource, FakeOwnedSource>,
    settings_dropped: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

impl CaptureExactState for DropOrderExactState {
    type Source = FakeSource;
    type OwnedSource = FakeOwnedSource;

    fn common(&self) -> &CaptureExactPublicationShared<Self::Source, Self::OwnedSource> {
        &self.common
    }
}

impl Drop for DropOrderExactState {
    fn drop(&mut self) {
        assert!(self.settings_dropped.load(Ordering::Acquire));
        self.dropped.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
struct SettingsDropProbe {
    backend: Arc<AtomicBool>,
    settings: Arc<AtomicBool>,
}

impl Drop for SettingsDropProbe {
    fn drop(&mut self) {
        assert!(self.backend.load(Ordering::Acquire));
        self.settings.store(true, Ordering::Release);
    }
}

struct DropOrderCaptureBackend {
    session_detaches: Arc<AtomicUsize>,
    backend_dropped: Arc<AtomicBool>,
}

impl Drop for DropOrderCaptureBackend {
    fn drop(&mut self) {
        assert_eq!(self.session_detaches.load(Ordering::Acquire), 1);
        self.backend_dropped.store(true, Ordering::Release);
    }
}

impl CaptureBackend for DropOrderCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = (FakeSession, FakeReadiness);
    type SettingsConfig = SettingsDropProbe;
    type ExactState = DropOrderExactState;
    type ActivityFence = FakeFence;
    type ActivityEpoch = FakeEpoch;
    type AuthorityCommitCheckpoint<'a> = ();

    const NAME: &'static str = "fake capture";
    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn resolve_publication_branch(
        &self,
        _settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        _source: &<Self::ExactState as CaptureExactState>::Source,
        _demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        Ok(None)
    }

    fn spawn_worker(
        &self,
        (session, readiness): Self::SpawnRequest,
        _handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        Ok(CaptureSessionTransaction::new(
            session,
            readiness,
            reservation,
        ))
    }

    fn prepare_authority_commit<'a>(
        &'a self,
        _handles: CaptureBackendHandles<'a, Self>,
        _reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>> {
        Some(())
    }

    fn commit_authority(
        _reservation: ReservedCaptureSessionAuthority,
        (): Self::AuthorityCommitCheckpoint<'_>,
    ) {
    }

    fn retire_authority(
        &self,
        _handles: CaptureBackendHandles<'_, Self>,
        _authority: CaptureSessionAuthority,
        _cause: CaptureRetirementCause,
    ) {
    }
}

fn readiness_deadline() -> CaptureSessionDeadline {
    CaptureSessionDeadline::after(Duration::from_secs(1))
}

fn fake_registered_demand() -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::new(
                ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Rgba8),
            )),
        ),
        NonZeroU32::new(60).expect("test cadence is nonzero"),
    )
}

#[test]
fn backend_worker_factory_pairs_authority_and_waits_once() {
    let adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    let waits = Arc::new(AtomicUsize::new(0));
    let readiness = FakeReadiness {
        result: Ok(()),
        waits: Arc::clone(&waits),
    };

    let prepared = adapter
        .prepare_successor((FakeSession::new(1), readiness))
        .expect("backend worker prepares");

    assert_eq!(prepared.authority().generation(), 1);
    assert_eq!(waits.load(Ordering::Relaxed), 1);
}

#[test]
fn stateful_backend_receives_adapter_handles_and_fresh_spawn_requests() {
    let StatefulAdapterFixture {
        adapter, requests, ..
    } = stateful_adapter(true);

    for (request, generation) in [(17, 1), (29, 2)] {
        let prepared = adapter
            .prepare_successor((
                request,
                FakeSession::new(generation),
                FakeReadiness::ready(),
            ))
            .expect("stateful backend worker prepares");
        drop(prepared);
    }

    assert_eq!(
        requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[17, 29]
    );
}

#[test]
fn adapter_commits_backend_authority_before_starting_candidate() {
    let StatefulAdapterFixture {
        mut adapter,
        exact,
        events,
        ..
    } = stateful_adapter(true);
    let mut candidate = FakeSession::new(1);
    candidate.events = Some(Arc::clone(&events));
    let starts = Arc::clone(&candidate.starts);
    let prepared = adapter
        .prepare_worker(
            (17, candidate, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("stateful backend authority reserves"),
        )
        .expect("stateful backend worker prepares");

    let authority = adapter
        .commit_worker(prepared)
        .unwrap_or_else(|_| panic!("stateful backend worker commits"));

    assert_eq!(exact.current_authority(), Some(authority));
    assert_eq!(starts.load(Ordering::Acquire), 1);
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["checkpoint", "commit", "start"]
    );
}

#[test]
fn adapter_checkpoint_rejection_preserves_predecessor() {
    let StatefulAdapterFixture {
        mut adapter,
        exact,
        events,
        ..
    } = stateful_adapter(false);
    let prior_reservation = adapter
        .reserve_exact_authority()
        .expect("prior authority reserves");
    let prior_authority = prior_reservation.authority();
    drop(
        exact
            .activate_reserved_authority(prior_reservation)
            .expect("prior authority activates"),
    );
    let prior = FakeSession::new(prior_authority.generation());
    let prior_aborts = Arc::clone(&prior.aborts);
    assert!(adapter.install_worker_for_test(prior).is_ok());
    let candidate_reservation = adapter
        .reserve_exact_authority()
        .expect("candidate authority reserves");
    let candidate = FakeSession::new(candidate_reservation.authority().generation());
    let candidate_detaches = Arc::clone(&candidate.detaches);
    let prepared = adapter
        .prepare_worker(
            (29, candidate, FakeReadiness::ready()),
            candidate_reservation,
        )
        .expect("candidate worker prepares");

    let result = adapter.commit_worker(prepared);

    assert!(result.is_err());
    drop(result);
    assert_eq!(prior_aborts.load(Ordering::Acquire), 0);
    assert_eq!(candidate_detaches.load(Ordering::Acquire), 1);
    assert_eq!(exact.current_authority(), Some(prior_authority));
    assert_eq!(
        adapter.active_worker().map(CaptureSession::authority),
        Some(prior_authority)
    );
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["checkpoint"]
    );
}

#[test]
fn adapter_readiness_failure_skips_backend_commit_hooks() {
    let StatefulAdapterFixture {
        adapter, events, ..
    } = stateful_adapter(true);
    let reservation = adapter
        .reserve_exact_authority()
        .expect("candidate authority reserves");
    let candidate = FakeSession::new(reservation.authority().generation());

    assert!(
        adapter
            .prepare_worker(
                (41, candidate, FakeReadiness::failed("not ready")),
                reservation
            )
            .is_err()
    );
    assert!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
}

#[test]
fn active_retirement_aborts_wakes_then_runs_backend_policy_once() {
    let StatefulAdapterFixture {
        mut adapter,
        events,
        ..
    } = stateful_adapter(true);
    let mut session = FakeSession::new(1);
    session.events = Some(Arc::clone(&events));
    let prepared = adapter
        .prepare_worker(
            (17, session, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("retirement authority reserves"),
        )
        .expect("retirement worker prepares");
    adapter
        .commit_worker(prepared)
        .unwrap_or_else(|_| panic!("retirement worker commits"));
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    assert!(adapter.retire_active_worker());
    assert!(!adapter.retire_active_worker());

    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["abort", "wake", "retire_active"]
    );
}

#[test]
fn observed_exit_runs_backend_policy_after_status_checkpoint() {
    let StatefulAdapterFixture {
        mut adapter,
        events,
        ..
    } = stateful_adapter(true);
    let session = FakeSession::new(1);
    let finished = Arc::clone(&session.finished);
    let prepared = adapter
        .prepare_worker(
            (17, session, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("exit authority reserves"),
        )
        .expect("exit worker prepares");
    adapter
        .commit_worker(prepared)
        .unwrap_or_else(|_| panic!("exit worker commits"));
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    finished.store(true, Ordering::Release);

    let (authority, _) = adapter
        .take_finished_active_worker()
        .expect("finished worker exits");
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push("status");
    adapter.retire_finished_worker(authority);

    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["status", "retire_exit"]
    );
}

#[test]
fn successor_overlap_does_not_run_active_stop_policy() {
    let StatefulAdapterFixture {
        mut adapter,
        events,
        ..
    } = stateful_adapter(true);
    let mut prior = FakeSession::new(1);
    prior.events = Some(Arc::clone(&events));
    let prior = adapter
        .prepare_worker(
            (17, prior, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("prior authority reserves"),
        )
        .expect("prior worker prepares");
    adapter
        .commit_worker(prior)
        .unwrap_or_else(|_| panic!("prior worker commits"));
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    let successor = adapter
        .prepare_worker(
            (29, FakeSession::new(2), FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("successor authority reserves"),
        )
        .expect("successor worker prepares");

    adapter
        .commit_worker(successor)
        .unwrap_or_else(|_| panic!("successor worker commits"));

    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["checkpoint", "abort", "wake", "commit"]
    );
}

#[test]
fn exclusive_settlement_takes_the_active_worker_without_abort_and_retires_exact_only() {
    let StatefulAdapterFixture {
        mut adapter,
        events,
        ..
    } = stateful_adapter(true);
    let mut session = FakeSession::new(1);
    session.events = Some(Arc::clone(&events));
    let prepared = adapter
        .prepare_worker(
            (17, session, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("settlement authority reserves"),
        )
        .expect("settlement worker prepares");
    adapter
        .commit_worker(prepared)
        .unwrap_or_else(|_| panic!("settlement worker commits"));
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    let taken = adapter
        .take_active_worker_for_settlement()
        .expect("active worker is taken for settlement");
    assert!(adapter.active_worker().is_none());
    assert!(adapter.can_install_successor());
    assert!(adapter.take_active_worker_for_settlement().is_none());
    let authority = taken.authority();
    adapter.retire_settled_worker(authority);

    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["retire_settled"]
    );
    drop(taken);
}

#[test]
fn adapter_drop_runs_active_retirement_policy_before_field_teardown() {
    let StatefulAdapterFixture {
        mut adapter,
        events,
        ..
    } = stateful_adapter(true);
    let mut session = FakeSession::new(1);
    session.events = Some(Arc::clone(&events));
    let detaches = Arc::clone(&session.detaches);
    let prepared = adapter
        .prepare_worker(
            (17, session, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("drop authority reserves"),
        )
        .expect("drop worker prepares");
    adapter
        .commit_worker(prepared)
        .unwrap_or_else(|_| panic!("drop worker commits"));
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    drop(adapter);

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(&events[..3], &["abort", "wake", "retire_active"]);
    assert_eq!(detaches.load(Ordering::Acquire), 1);
}

#[test]
fn backend_worker_factory_burns_failed_spawn_and_cleans_failed_readiness() {
    let failing_adapter = ScreenCaptureAdapter::<FailingCaptureBackend>::default();
    assert!(matches!(
        failing_adapter.prepare_successor(()),
        Err(CaptureSuccessorPreparationError::Worker(_))
    ));
    let next_reservation = failing_adapter
        .reserve_exact_authority()
        .expect("readiness authority reserves after failed spawn");
    assert_eq!(next_reservation.authority().generation(), 2);

    let candidate = FakeSession::new(1);
    let aborts = Arc::clone(&candidate.aborts);
    let wakes = Arc::clone(&candidate.wakes);
    let detaches = Arc::clone(&candidate.detaches);
    let starts = Arc::clone(&candidate.starts);
    let waits = Arc::new(AtomicUsize::new(0));
    let readiness = FakeReadiness {
        result: Err("injected readiness failure"),
        waits: Arc::clone(&waits),
    };

    let adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    assert!(adapter.prepare_successor((candidate, readiness)).is_err());
    let next_reservation = adapter
        .reserve_exact_authority()
        .expect("authority reserves after failed readiness");
    assert_eq!(next_reservation.authority().generation(), 2);
    assert_eq!(waits.load(Ordering::Relaxed), 1);
    assert_eq!(aborts.load(Ordering::Relaxed), 1);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert_eq!(detaches.load(Ordering::Relaxed), 1);
    assert_eq!(starts.load(Ordering::Relaxed), 0);
}

#[test]
fn spawn_successor_commits_observes_exit_before_retirement_and_reaps_retirees() {
    let StatefulAdapterFixture {
        mut adapter,
        events,
        requests,
        ..
    } = stateful_adapter(true);
    let first = FakeSession::new(1);
    let first_finished = Arc::clone(&first.finished);
    let authority = adapter
        .spawn_successor((41, first, FakeReadiness::ready()))
        .expect("successor spawns through the shared choreography");
    assert_eq!(authority, CaptureSessionAuthority::new(1));
    assert_eq!(
        requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[41]
    );
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["checkpoint", "commit"]
    );
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    assert!(adapter.observe_exit(|_, _| ()).is_none());
    first_finished.store(true, Ordering::Release);
    let observed = adapter.observe_exit(|authority, exit| {
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push("status");
        assert_eq!(authority, exit);
        authority.generation()
    });
    assert_eq!(observed, Some(1));
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &["status", "retire_exit"]
    );
    events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    let second = FakeSession::new(2);
    let second_finished = Arc::clone(&second.finished);
    adapter
        .spawn_successor((43, second, FakeReadiness::ready()))
        .expect("replacement spawns after the observed exit");
    assert_eq!(adapter.retiring_worker_count(), 0);
    second_finished.store(true, Ordering::Release);
    assert!(adapter.shutdown());
    assert!(!adapter.shutdown());
    assert_eq!(adapter.retiring_worker_count(), 0);
    assert!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ends_with(&["retire_active"])
    );
}

#[test]
fn successor_preparation_rejects_exclusive_overlap_before_reserving_or_spawning() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let assembly = ScreenCaptureAdapterAssembly::<ExclusiveCaptureBackend>::new(
        Arc::new(CaptureExactPublicationShared::default()),
        (),
    );
    let mut adapter = assembly.finish(ExclusiveCaptureBackend {
        spawns: Arc::clone(&spawns),
    });
    let predecessor = FakeSession::new(1);
    let predecessor_finished = Arc::clone(&predecessor.finished);
    let predecessor = adapter
        .prepare_successor((ExclusiveFakeSession(predecessor), FakeReadiness::ready()))
        .expect("exclusive predecessor prepares");
    adapter
        .commit_worker(predecessor)
        .unwrap_or_else(|_| panic!("exclusive predecessor commits"));

    assert!(matches!(
        adapter.prepare_successor((
            ExclusiveFakeSession(FakeSession::new(2)),
            FakeReadiness::ready()
        )),
        Err(CaptureSuccessorPreparationError::Unavailable)
    ));
    assert_eq!(spawns.load(Ordering::Relaxed), 1);

    assert!(adapter.retire_active_worker());
    predecessor_finished.store(true, Ordering::Release);
    adapter.reap_finished_workers(|_, _| {});
    let successor = adapter
        .prepare_successor((
            ExclusiveFakeSession(FakeSession::new(3)),
            FakeReadiness::ready(),
        ))
        .expect("exclusive successor prepares without skipping authority");

    assert_eq!(successor.authority().generation(), 3);
    assert_eq!(spawns.load(Ordering::Relaxed), 2);
}

#[test]
fn adapter_and_backend_handles_share_one_settings_and_exact_state() {
    let exact = Arc::new(CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default());
    let assembly = ScreenCaptureAdapterAssembly::<FakeCaptureBackend>::new(Arc::clone(&exact), ());
    let backend_settings = assembly.handles().settings_handle();
    let adapter = assembly.finish(FakeCaptureBackend::default());
    let adapter_settings = adapter.settings_handle();
    assert!(Arc::ptr_eq(&backend_settings, &adapter_settings));
    assert!(std::ptr::eq(adapter.settings(), adapter_settings.as_ref()));
    assert_eq!(Arc::strong_count(&exact), 2);
    let activity = adapter.activity_handle();
    assert!(std::ptr::eq(adapter.activity(), activity.as_ref()));
    assert_eq!(Arc::strong_count(&activity), 2);

    let epoch = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    {
        let mut worker_activity = activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(worker_activity.activate(epoch).is_ok());
    }
    assert_eq!(
        adapter
            .activity()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active(),
        Some(&epoch)
    );

    for _ in 0..3 {
        let borrowed = adapter.exact_state();
        assert!(std::ptr::eq(borrowed, exact.as_ref()));
        assert_eq!(Arc::strong_count(&exact), 2);
    }

    let backend_handle = adapter.exact_state_handle();
    assert!(Arc::ptr_eq(&backend_handle, &exact));
    let reservation = adapter
        .reserve_exact_authority()
        .expect("adapter authority reserves");
    let authority = reservation.authority();
    drop(
        backend_handle
            .activate_reserved_authority(reservation)
            .expect("backend handle activates adapter reservation"),
    );

    let source = FakeSource {
        id: CaptureSourceId::new("fake:adapter").expect("test source id is valid"),
        incarnation: 1,
    };
    assert!(backend_handle.replace_source_if_current(authority, Some(source.clone())));
    assert_eq!(adapter.exact_source(), Some(source.clone()));
    assert!(adapter.owns_exact_source(&source.id));

    let hub = Arc::new(ScreenPublicationHub::new(
        ScreenPublicationSlotPolicy::default(),
    ));
    adapter.install_publication_hub(Arc::clone(&hub));
    assert!(Arc::ptr_eq(
        &backend_handle.hub().expect("adapter hub remains visible"),
        &hub
    ));
    let revision = adapter.exact_resolution_revision();
    adapter.advance_exact_resolution_revision();
    assert_eq!(backend_handle.resolution_revision(), revision + 1);
}

#[test]
fn adapter_resolves_publication_against_current_source_and_owned_settings() {
    let adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    let demand = fake_registered_demand();

    assert!(
        adapter
            .resolve_exact_publication_branch(&demand)
            .expect("missing exact source is not an error")
            .is_none()
    );
    assert_eq!(
        adapter
            .backend()
            .publication_resolutions
            .load(Ordering::Relaxed),
        0
    );

    let reservation = adapter
        .reserve_exact_authority()
        .expect("publication authority reserves");
    let authority = reservation.authority();
    drop(
        adapter
            .exact_state()
            .activate_reserved_authority(reservation)
            .expect("publication authority activates"),
    );

    for incarnation in [7, 11] {
        assert!(
            adapter.exact_state().replace_source_if_current(
                authority,
                Some(FakeSource {
                    id: CaptureSourceId::new(format!("fake:resolution:{incarnation}"))
                        .expect("test source id is valid"),
                    incarnation,
                }),
            )
        );
        assert!(
            adapter
                .resolve_exact_publication_branch(&demand)
                .expect("fake publication branch resolves")
                .is_none()
        );
        assert_eq!(
            adapter
                .backend()
                .publication_source_incarnation
                .load(Ordering::Relaxed),
            incarnation
        );
    }

    assert_eq!(
        adapter
            .backend()
            .publication_resolutions
            .load(Ordering::Relaxed),
        2
    );
    assert_eq!(
        adapter
            .backend()
            .publication_settings_address
            .load(Ordering::Relaxed),
        std::ptr::from_ref(adapter.settings()).addr()
    );

    adapter
        .backend()
        .fail_publication_resolution
        .store(true, Ordering::Relaxed);
    let error = adapter
        .resolve_exact_publication_branch(&demand)
        .expect_err("backend resolution failure propagates");
    assert_eq!(error.to_string(), "injected publication resolution failure");
}

#[test]
fn adapter_detaches_sessions_before_releasing_exact_state() {
    let detaches = Arc::new(AtomicUsize::new(0));
    let backend_dropped = Arc::new(AtomicBool::new(false));
    let settings_dropped = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let exact = Arc::new(DropOrderExactState {
        common: CaptureExactPublicationShared::default(),
        settings_dropped: Arc::clone(&settings_dropped),
        dropped: Arc::clone(&dropped),
    });
    let mut adapter = ScreenCaptureAdapterAssembly::<DropOrderCaptureBackend>::new(
        Arc::clone(&exact),
        SettingsDropProbe {
            backend: Arc::clone(&backend_dropped),
            settings: Arc::clone(&settings_dropped),
        },
    )
    .finish(DropOrderCaptureBackend {
        session_detaches: Arc::clone(&detaches),
        backend_dropped: Arc::clone(&backend_dropped),
    });
    let epoch = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    {
        let mut activity = adapter
            .activity()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(activity.activate(epoch).is_ok());
    }
    drop(exact);
    let mut session = FakeSession::new(1);
    session.detaches = Arc::clone(&detaches);
    assert!(adapter.install_worker_for_test(session).is_ok());

    drop(adapter);

    assert_eq!(detaches.load(Ordering::Acquire), 1);
    assert!(backend_dropped.load(Ordering::Acquire));
    assert!(settings_dropped.load(Ordering::Acquire));
    assert!(dropped.load(Ordering::Acquire));
}

fn reservation(generation: u64) -> ReservedCaptureSessionAuthority {
    let sequencer = CaptureSessionAuthoritySequencer::default();
    let mut reservation = None;
    for _ in 0..generation {
        reservation = Some(sequencer.reserve().expect("test authority reserves"));
    }
    reservation.expect("test authority generation is nonzero")
}

#[test]
fn session_transaction_readiness_failure_preserves_prior_and_detaches_candidate() {
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(FakeSession::new(1)).is_ok());
    let candidate = FakeSession::new(2);
    let aborts = Arc::clone(&candidate.aborts);
    let wakes = Arc::clone(&candidate.wakes);
    let detaches = Arc::clone(&candidate.detaches);

    let result = CaptureSessionTransaction::new(
        candidate,
        FakeReadiness::failed("not ready"),
        reservation(2),
    )
    .prepare(readiness_deadline());

    assert!(result.is_err());
    assert_eq!(aborts.load(Ordering::Relaxed), 1);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert_eq!(detaches.load(Ordering::Relaxed), 1);
    assert_eq!(
        sessions.active().map(CaptureSession::authority),
        Some(CaptureSessionAuthority::new(1))
    );
}

#[test]
fn session_transaction_candidate_exit_before_commit_preserves_prior() {
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(FakeSession::new(1)).is_ok());
    let candidate = FakeSession::new(2);
    candidate.finished.store(true, Ordering::Release);
    let detaches = Arc::clone(&candidate.detaches);

    let result = CaptureSessionTransaction::new(candidate, FakeReadiness::ready(), reservation(2))
        .prepare(readiness_deadline());

    assert!(result.is_err());
    assert_eq!(detaches.load(Ordering::Relaxed), 1);
    assert_eq!(
        sessions.active().map(CaptureSession::authority),
        Some(CaptureSessionAuthority::new(1))
    );
}

#[test]
fn session_transaction_commits_checkpoint_retirement_authority_and_start_in_order() {
    let prior = FakeSession::new(1);
    let prior_aborts = Arc::clone(&prior.aborts);
    let displaced_dropped = Arc::new(AtomicBool::new(false));
    let mut candidate = FakeSession::new(2);
    candidate.start_prerequisite = Some(Arc::clone(&displaced_dropped));
    let candidate_starts = Arc::clone(&candidate.starts);
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(prior).is_ok());
    let prepared =
        CaptureSessionTransaction::new(candidate, FakeReadiness::ready(), reservation(2))
            .prepare(readiness_deadline())
            .expect("candidate becomes ready");

    let commit = prepared
        .commit_into(
            &mut sessions,
            |_| {
                Some({
                    assert_eq!(prior_aborts.load(Ordering::Relaxed), 0);
                    "checkpoint"
                })
            },
            |reservation, checkpoint| {
                let authority = reservation.authority();
                assert_eq!(authority, CaptureSessionAuthority::new(2));
                assert_eq!(checkpoint, "checkpoint");
                assert_eq!(prior_aborts.load(Ordering::Relaxed), 1);
                assert_eq!(candidate_starts.load(Ordering::Relaxed), 0);
                DropProbe(Arc::clone(&displaced_dropped))
            },
        )
        .unwrap_or_else(|_| panic!("ready overlapping candidate commits"));

    assert_eq!(commit.authority(), CaptureSessionAuthority::new(2));
    assert!(displaced_dropped.load(Ordering::Acquire));
    assert_eq!(candidate_starts.load(Ordering::Relaxed), 1);
    assert_eq!(
        sessions.active().map(CaptureSession::authority),
        Some(CaptureSessionAuthority::new(2))
    );
}

#[test]
fn worker_command_envelope_preserves_fifo_and_payload_identity() {
    #[derive(Debug, PartialEq, Eq)]
    enum BackendCommand {
        Sentinel(u64),
    }

    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(CaptureWorkerCommand::Exact(CaptureExactCommand::Reap {
        authority: CaptureSessionAuthority::new(7),
        completion: None,
    }))
    .expect("exact command enters the shared envelope");
    tx.send(CaptureWorkerCommand::Backend(BackendCommand::Sentinel(11)))
        .expect("backend command enters the shared envelope");

    let CaptureWorkerCommand::Exact(CaptureExactCommand::Reap {
        authority,
        completion: None,
    }) = rx.recv().expect("exact command remains first")
    else {
        panic!("first command must retain the exact payload");
    };
    assert_eq!(authority, CaptureSessionAuthority::new(7));
    let CaptureWorkerCommand::Backend(backend) = rx.recv().expect("backend command remains second")
    else {
        panic!("second command must retain the backend payload");
    };
    assert_eq!(backend, BackendCommand::Sentinel(11));
}

#[test]
fn shared_command_endpoint_stamps_live_authority_wakes_and_rejects_dead_workers() {
    enum BackendCommand {}

    let (tx, rx) = std::sync::mpsc::channel::<CaptureWorkerCommand<BackendCommand>>();
    let generation = Arc::new(AtomicU64::new(3));
    let wakes = Arc::new(AtomicUsize::new(0));
    let endpoint = {
        let wakes = Arc::clone(&wakes);
        CaptureCommandEndpoint::new("shared capture", Arc::clone(&generation), tx).with_wake(
            move || {
                wakes.fetch_add(1, Ordering::AcqRel);
            },
        )
    };
    let clone = endpoint.clone();

    assert_eq!(endpoint.source_name(), "shared capture");
    assert_eq!(endpoint.authority(), CaptureSessionAuthority::new(3));
    generation.store(9, Ordering::Release);
    assert_eq!(clone.authority(), CaptureSessionAuthority::new(9));

    let retirement = begin_capture_exact_retirement(&endpoint);
    assert_eq!(wakes.load(Ordering::Acquire), 1);
    let CaptureWorkerCommand::Exact(CaptureExactCommand::Reap {
        authority,
        completion: Some(completion),
    }) = rx.recv().expect("retirement enters the shared envelope")
    else {
        panic!("retirement must carry the live authority");
    };
    assert_eq!(authority, CaptureSessionAuthority::new(9));
    let _ = completion.send(Ok(()));
    pollster::block_on(retirement.complete()).expect("retirement completes once acknowledged");

    drop(rx);
    assert!(
        clone
            .send_exact(CaptureExactCommand::Reap {
                authority: CaptureSessionAuthority::new(9),
                completion: None,
            })
            .is_err()
    );
    let error = pollster::block_on(begin_capture_exact_retirement(&clone).complete())
        .expect_err("a dead worker rejects retirement");
    assert!(
        error
            .to_string()
            .contains("shared capture worker rejected exact publication retirement"),
        "{error}"
    );
}

#[tokio::test]
async fn session_transaction_candidate_exact_commands_are_hidden_until_commit() {
    let mut adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    assert!(adapter.begin_exact_retirement().is_none());
    let candidate = FakeSession::new(1);
    let endpoint = candidate.endpoint.clone();
    let prepared = adapter
        .prepare_worker(
            (candidate, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("fake adapter authority reserves"),
        )
        .expect("candidate becomes ready");
    assert!(adapter.begin_exact_retirement().is_none());

    adapter
        .commit_worker(prepared)
        .unwrap_or_else(|_| panic!("ready candidate commits"));

    let retirement = adapter
        .begin_exact_retirement()
        .expect("committed worker accepts exact retirement");
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 1);
    let command = endpoint
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .expect("retirement reaches the committed endpoint");
    let CaptureExactCommand::Reap {
        authority,
        completion: Some(completion),
    } = command
    else {
        panic!("adapter retirement enqueues a completing reap command");
    };
    assert_eq!(authority, CaptureSessionAuthority::new(1));
    completion
        .send(Ok(()))
        .expect("test retirement receiver remains live");
    retirement
        .complete()
        .await
        .expect("committed worker completes exact retirement");
}

#[tokio::test]
async fn exact_commands_target_only_the_committed_successor_endpoint() {
    let mut adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    let predecessor = FakeSession::new(1);
    let predecessor_endpoint = predecessor.endpoint.clone();
    let predecessor = adapter
        .prepare_worker(
            (predecessor, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("predecessor authority reserves"),
        )
        .expect("predecessor becomes ready");
    adapter
        .commit_worker(predecessor)
        .unwrap_or_else(|_| panic!("predecessor commits"));

    let successor = FakeSession::new(2);
    let successor_endpoint = successor.endpoint.clone();
    let successor = adapter
        .prepare_worker(
            (successor, FakeReadiness::ready()),
            adapter
                .reserve_exact_authority()
                .expect("successor authority reserves"),
        )
        .expect("successor becomes ready");
    adapter
        .commit_worker(successor)
        .unwrap_or_else(|_| panic!("successor commits"));

    let retirement = adapter
        .begin_exact_retirement()
        .expect("successor accepts exact retirement");
    assert!(
        predecessor_endpoint
            .commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    assert_eq!(predecessor_endpoint.wakes.load(Ordering::Relaxed), 0);
    assert_eq!(successor_endpoint.wakes.load(Ordering::Relaxed), 1);
    let command = successor_endpoint
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .expect("retirement reaches the successor endpoint");
    let CaptureExactCommand::Reap {
        authority,
        completion: Some(completion),
    } = command
    else {
        panic!("successor retirement enqueues a completing reap command");
    };
    assert_eq!(authority, CaptureSessionAuthority::new(2));
    completion
        .send(Ok(()))
        .expect("successor retirement receiver remains live");
    retirement
        .complete()
        .await
        .expect("successor completes exact retirement");
}

#[test]
fn session_transaction_checkpoint_rejection_preserves_prior() {
    let prior = FakeSession::new(1);
    let prior_aborts = Arc::clone(&prior.aborts);
    let candidate = FakeSession::new(2);
    let candidate_detaches = Arc::clone(&candidate.detaches);
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(prior).is_ok());
    let prepared =
        CaptureSessionTransaction::new(candidate, FakeReadiness::ready(), reservation(2))
            .prepare(readiness_deadline())
            .expect("candidate becomes ready");

    let result = prepared.commit_into(&mut sessions, |_| None::<()>, |_, ()| ());

    assert!(result.is_err());
    drop(result);
    assert_eq!(prior_aborts.load(Ordering::Relaxed), 0);
    assert_eq!(candidate_detaches.load(Ordering::Relaxed), 1);
    assert_eq!(
        sessions.active().map(CaptureSession::authority),
        Some(CaptureSessionAuthority::new(1))
    );
}

#[test]
fn session_set_drop_aborts_wakes_and_detaches_without_finishing() {
    let session = FakeSession::new(1);
    let aborts = Arc::clone(&session.aborts);
    let wakes = Arc::clone(&session.wakes);
    let finishes = Arc::clone(&session.finishes);
    let detaches = Arc::clone(&session.detaches);
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(session).is_ok());

    drop(sessions);

    assert_eq!(aborts.load(Ordering::Relaxed), 1);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert_eq!(finishes.load(Ordering::Relaxed), 0);
    assert_eq!(detaches.load(Ordering::Relaxed), 1);
}

#[test]
fn finished_session_is_finished_exactly_once() {
    let session = FakeSession::new(1);
    let finished = Arc::clone(&session.finished);
    let finishes = Arc::clone(&session.finishes);
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(session).is_ok());
    finished.store(true, Ordering::Release);

    assert_eq!(
        sessions.take_finished_active(),
        Some((
            CaptureSessionAuthority::new(1),
            CaptureSessionAuthority::new(1)
        ))
    );
    assert!(sessions.take_finished_active().is_none());
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
}

#[test]
fn retired_session_exit_cannot_remove_the_successor() {
    let first = FakeSession::new(1);
    let first_finished = Arc::clone(&first.finished);
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(first).is_ok());
    assert_eq!(
        sessions.retire_active(),
        Some(CaptureSessionAuthority::new(1))
    );
    assert!(sessions.install(FakeSession::new(2)).is_ok());
    first_finished.store(true, Ordering::Release);

    let mut exits = Vec::new();
    sessions.reap_finished(|authority, exit| exits.push((authority, exit)));

    assert_eq!(
        exits,
        [(
            CaptureSessionAuthority::new(1),
            CaptureSessionAuthority::new(1)
        )]
    );
    assert_eq!(
        sessions.active().map(CaptureSession::authority),
        Some(CaptureSessionAuthority::new(2))
    );
}

#[test]
fn reap_callbacks_run_after_session_ownership_is_released() {
    let first = FakeSession::new(1);
    let first_finished = Arc::clone(&first.finished);
    let mut sessions = CaptureSessionSet::default();
    assert!(sessions.install(first).is_ok());
    sessions.retire_active();
    first_finished.store(true, Ordering::Release);
    let publication = Mutex::new(false);

    sessions.reap_finished(|_, _| {
        *publication
            .lock()
            .expect("publication lock remains available while reaping") = true;
    });

    assert!(
        *publication
            .lock()
            .expect("publication lock remains healthy")
    );
}

#[test]
fn successor_overlap_policy_is_statically_enforced() {
    let mut exclusive = CaptureSessionSet::default();
    assert!(
        exclusive
            .install(ExclusiveFakeSession(FakeSession::new(1)))
            .is_ok()
    );
    exclusive.retire_active();
    assert!(!exclusive.can_install_successor());
    assert!(
        exclusive
            .install(ExclusiveFakeSession(FakeSession::new(2)))
            .is_err()
    );

    let mut overlapping = CaptureSessionSet::default();
    assert!(overlapping.install(FakeSession::new(1)).is_ok());
    overlapping.retire_active();
    assert!(overlapping.can_install_successor());
}

#[test]
fn steady_state_access_does_not_grow_retirement_storage() {
    let mut adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    assert!(adapter.install_worker_for_test(FakeSession::new(1)).is_ok());
    let capacity = adapter.retiring_worker_capacity();

    for _ in 0..100 {
        assert!(adapter.active_worker().is_some());
        assert_eq!(adapter.retiring_worker_count(), 0);
        assert!(adapter.can_prepare_successor());
        assert!(!adapter.can_install_successor());
    }

    assert_eq!(adapter.retiring_worker_capacity(), capacity);
}

impl Default for FakeExactEndpoint {
    fn default() -> Self {
        Self {
            commands: Arc::default(),
            wakes: Arc::default(),
            authority: Arc::new(AtomicU64::new(1)),
            reject: false,
        }
    }
}

impl CaptureExactCommandEndpoint for FakeExactEndpoint {
    fn source_name(&self) -> &'static str {
        "fake capture"
    }

    fn authority(&self) -> CaptureSessionAuthority {
        CaptureSessionAuthority::new(self.authority.load(Ordering::Acquire))
    }

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected> {
        if self.reject {
            return Err(CaptureExactCommandRejected);
        }
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command);
        Ok(())
    }

    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn preparation_abort_reaps_the_authority_that_started_the_transaction() {
    let endpoint = FakeExactEndpoint::default();
    let original_authority = endpoint.authority();
    let cancelled = Arc::new(AtomicBool::new(false));
    let abort =
        exact_preparation_abort(endpoint.clone(), original_authority, Arc::clone(&cancelled));

    endpoint.authority.store(2, Ordering::Release);
    abort();

    assert!(cancelled.load(Ordering::Acquire));
    let command = endpoint
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .expect("preparation abort enqueues one exact command");
    let CaptureExactCommand::Reap {
        authority,
        completion: None,
    } = command
    else {
        panic!("preparation abort enqueues a noncompleting reap command");
    };
    assert_eq!(authority, original_authority);
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 1);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FakeSource {
    id: CaptureSourceId,
    incarnation: u64,
}

impl CapturePublicationSource for FakeSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.id
    }
}

struct FakeOwnedSource {
    id: CaptureSourceId,
}

impl CaptureOwnedSource for FakeOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.id
    }

    fn belongs_to_authority(&self, _authority: &ScreenCommittedState) -> bool {
        false
    }
}

struct ProbeRuntime {
    source: FakeSource,
    binding: ScreenWorkerBinding,
}

impl CaptureExactRuntimeOwner for ProbeRuntime {
    type Source = FakeSource;

    const BACKEND_NAME: &'static str = "probe capture";
    const ABORTED_BINDING_ERROR: &'static str = "probe binding aborted";

    fn source(&self) -> &Self::Source {
        &self.source
    }

    fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }

    fn bind_routes(&mut self, _authority: &ScreenCommittedState) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn is_bound(&self) -> bool {
        false
    }
}

struct RetainProbeStore {
    runtime_count: usize,
    retain_calls: usize,
}

impl CaptureExactRuntimeCollection<ProbeRuntime> for RetainProbeStore {
    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut ProbeRuntime>
    where
        ProbeRuntime: 'a,
    {
        std::iter::empty()
    }
}

impl CaptureExactRuntimeStore<ProbeRuntime> for RetainProbeStore {
    type Prepared = ProbeRuntime;

    fn prepare(runtime: ProbeRuntime) -> Self::Prepared {
        runtime
    }

    fn install(&mut self, _prepared: Self::Prepared) {
        self.runtime_count += 1;
    }

    fn retain(&mut self, _retain: impl FnMut(&ProbeRuntime) -> bool) {
        self.retain_calls += 1;
        self.runtime_count = 0;
    }
}

struct ReentrantOwnedSource {
    id: CaptureSourceId,
    state: Weak<CaptureExactPublicationShared<FakeSource, Self>>,
    dropped: Arc<AtomicBool>,
}

struct OrderedDrop {
    id: u64,
    drops: Arc<Mutex<Vec<u64>>>,
}

impl Drop for OrderedDrop {
    fn drop(&mut self) {
        self.drops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(self.id);
    }
}

impl CaptureOwnedSource for ReentrantOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.id
    }

    fn belongs_to_authority(&self, _authority: &ScreenCommittedState) -> bool {
        false
    }
}

impl Drop for ReentrantOwnedSource {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let _ = state.source();
        }
        self.dropped.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeEpoch {
    source: u64,
    activity: u64,
    session: u64,
    topology: u64,
    resource: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FakeFence {
    source: u64,
    activity: u64,
}

struct SettingsCloneProbe(Arc<AtomicUsize>);

impl Clone for SettingsCloneProbe {
    fn clone(&self) -> Self {
        self.0.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(&self.0))
    }
}

#[test]
fn settings_adoption_rendezvous_commits_declines_and_reports_absent_peers() {
    // Commit: the worker sees the payload only after the source commits.
    let (adoption, adopter) = begin_capture_settings_adoption::<u32, &'static str>(7);
    let worker = std::thread::spawn(move || {
        adoption.rendezvous(None).map(|committed| {
            let (prepared, done) = committed.into_parts();
            let _ = done.send("applied");
            prepared
        })
    });
    adopter
        .wait_ready(Duration::from_secs(5))
        .expect("worker reaches the rendezvous");
    assert!(adopter.commit());
    assert_eq!(adopter.wait_done(), Ok("applied"));
    assert_eq!(worker.join().expect("worker thread completes"), Some(7));

    // Decline: dropping the adopter after readiness leaves the payload unapplied.
    let (adoption, adopter) = begin_capture_settings_adoption::<u32, ()>(9);
    let worker = std::thread::spawn(move || adoption.rendezvous(None).map(|c| c.into_parts().0));
    adopter
        .wait_ready(Duration::from_secs(5))
        .expect("worker reaches the rendezvous");
    drop(adopter);
    assert_eq!(worker.join().expect("worker thread completes"), None);

    // Timeout: a silent source declines once the worker's decision window closes.
    let (adoption, adopter) = begin_capture_settings_adoption::<u32, ()>(11);
    assert!(adoption.rendezvous(Some(Duration::ZERO)).is_none());
    assert!(!adopter.commit());
    assert!(adopter.wait_done().is_err());

    // Absent worker: a dropped adoption is visible at every adopter step.
    let (adoption, adopter) = begin_capture_settings_adoption::<u32, ()>(13);
    drop(adoption);
    assert_eq!(
        adopter.wait_ready(Duration::ZERO),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    );
    assert!(!adopter.commit());
}

#[test]
fn source_shell_status_choreography_follows_the_activity_edge() {
    let mut shell = CaptureSourceShell::new(
        ScreenCaptureAdapter::<FakeCaptureBackend>::default(),
        crate::input::SourceStatusReporter::new(
            "shell:test",
            crate::input::SourceKind::Screen,
            "fake",
            true,
            true,
            false,
        ),
        crate::input::status::SourceSessionSlot::new(),
    );
    assert!(!shell.running);
    assert!(shell.status_session.load().is_none());

    // Not running: activation records policy but opens no session.
    shell
        .begin_demand_status(false, true)
        .expect("policy accepts activation");
    assert!(shell.status_session.load().is_none());

    // Running under a manager-bound graph: the activation edge opens a
    // session (each one needs a strictly newer graph generation),
    // deactivation clears it.
    shell.running = true;
    shell.status.set_source_graph_generation(1);
    shell
        .begin_demand_status(false, true)
        .expect("policy accepts activation");
    assert!(shell.status_session.load().is_some());
    shell
        .begin_demand_status(true, true)
        .expect("steady active demand keeps the session");
    assert!(shell.status_session.load().is_some());
    shell
        .begin_demand_status(true, false)
        .expect("policy accepts deactivation");
    assert!(shell.status_session.load().is_none());

    // Rollback after a failed activation restores the previous (inactive) side.
    shell.status.set_source_graph_generation(2);
    shell
        .begin_demand_status(false, true)
        .expect("policy accepts activation");
    assert!(shell.status_session.load().is_some());
    shell
        .rollback_demand_status(false)
        .expect("rollback to inactive");
    assert!(shell.status_session.load().is_none());

    // Rollback after a failed deactivation reopens the active session.
    shell.status.set_source_graph_generation(3);
    shell
        .begin_demand_status(false, true)
        .expect("policy accepts activation");
    shell
        .begin_demand_status(true, false)
        .expect("policy accepts deactivation");
    shell.status.set_source_graph_generation(4);
    shell
        .rollback_demand_status(true)
        .expect("rollback to active");
    assert!(shell.status_session.load().is_some());

    shell.begin_stop();
    assert!(!shell.running);
    assert!(shell.status_session.load().is_none());
}

#[test]
fn versioned_settings_reads_demand_without_cloning_config() {
    let clones = Arc::new(AtomicUsize::new(0));
    let active = ScreenCaptureDemand::active();
    let settings = VersionedCaptureSettings::new(SettingsCloneProbe(Arc::clone(&clones)), active);

    assert_eq!(settings.demand(), active);
    assert_eq!(clones.load(Ordering::Relaxed), 0);

    let _snapshot = settings.snapshot();
    assert_eq!(clones.load(Ordering::Relaxed), 1);
}

#[test]
fn versioned_settings_commit_one_coherent_config_and_demand_snapshot() {
    let settings =
        VersionedCaptureSettings::new(String::from("initial"), ScreenCaptureDemand::Inactive);

    assert_eq!(settings.revision(), 0);
    let initial = settings.snapshot();
    assert_eq!(initial.config, "initial");
    assert_eq!(initial.demand, ScreenCaptureDemand::Inactive);

    let active = ScreenCaptureDemand::active();
    let mut values = settings.lock();
    values.config_mut().clone_from(&String::from("committed"));
    assert_eq!(values.config(), "committed");
    *values.demand_mut() = active;
    assert_eq!(values.commit(), 1);

    let committed = settings.snapshot();
    assert_eq!(committed.config, "committed");
    assert_eq!(committed.demand, active);
    assert_eq!(settings.commit_revision(), 2);
    assert_eq!(settings.bump_revision(), 3);
    assert_eq!(settings.revision(), 3);

    settings.lock_config().clone_from(&String::from("direct"));
    assert_eq!(settings.lock_config().as_str(), "direct");
    *settings.lock_demand() = ScreenCaptureDemand::Inactive;
    assert_eq!(*settings.lock_demand(), ScreenCaptureDemand::Inactive);
}

impl CapturePublicationFence<FakeEpoch> for FakeFence {
    fn admits(&self, epoch: &FakeEpoch) -> bool {
        epoch.source == self.source && epoch.activity == self.activity
    }
}

#[test]
fn activity_requires_current_source_and_activity_before_reactivation() {
    let previous = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    let current = FakeEpoch {
        source: 1,
        activity: 1,
        ..previous
    };
    let mut activity = CaptureActivity::<FakeFence, _>::default();

    assert!(activity.activate(previous).is_ok());
    assert_eq!(activity.active(), Some(&previous));
    let displaced = activity.replace_fence(FakeFence {
        source: 1,
        activity: 1,
    });
    assert_eq!(displaced, Some(previous));

    assert!(activity.activate(previous).is_err());
    assert!(activity.active().is_none());
    assert!(activity.activate(current).is_ok());
    assert_eq!(activity.active(), Some(&current));
    assert!(matches!(activity.activate(current), Ok(None)));
    assert_eq!(activity.clear(), Some(current));
    assert!(activity.active().is_none());
}

#[test]
fn exact_publication_state_versions_sources_and_reaps_unowned_incarnations() {
    let state = CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default();
    let first_reservation = state.reserve_authority().expect("first authority reserves");
    let first_authority = first_reservation.authority();
    let first = FakeSource {
        id: CaptureSourceId::new("fake:first").expect("test source id is valid"),
        incarnation: 1,
    };
    let replacement = FakeSource {
        id: CaptureSourceId::new("fake:replacement").expect("test source id is valid"),
        incarnation: 2,
    };

    assert_eq!(state.resolution_revision(), 0);
    drop(
        state
            .activate_reserved_authority(first_reservation)
            .expect("first authority activates"),
    );
    assert!(state.is_current_authority(first_authority));
    state.replace_source_if_current(first_authority, Some(first.clone()));
    assert_eq!(state.resolution_revision(), 1);
    state.replace_source_if_current(first_authority, Some(first.clone()));
    assert_eq!(state.resolution_revision(), 1);
    assert!(state.owns_source(&first.id));

    assert!(state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: first.id.clone(),
        }),
    ));
    assert_eq!(state.owned_source_count(), 1);
    assert!(state.retain_owned_sources_if_current(first_authority, |source| source.id == first.id));
    state.replace_source_if_current(first_authority, Some(replacement.clone()));
    assert_eq!(state.resolution_revision(), 2);
    assert!(state.owns_source(&first.id));
    assert!(state.owns_source(&replacement.id));

    let hub = Arc::new(ScreenPublicationHub::new(
        ScreenPublicationSlotPolicy::default(),
    ));
    state.install_hub(Arc::clone(&hub));
    assert!(Arc::ptr_eq(
        &state.hub().expect("installed hub remains visible"),
        &hub
    ));
    assert!(state.reap_owned_sources_if_current(first_authority));
    assert!(!state.owns_source(&first.id));
    assert!(state.owns_source(&replacement.id));
    state.replace_source_if_current(first_authority, None);
    assert_eq!(state.resolution_revision(), 3);
    assert!(!state.owns_source(&replacement.id));
    assert!(state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: replacement.id.clone(),
        }),
    ));
    assert_eq!(state.owned_source_count(), 1);
    assert!(state.clear_owned_sources_if_current(first_authority));
    assert_eq!(state.owned_source_count(), 0);
    assert!(!state.owns_source(&replacement.id));

    state.replace_source_if_current(first_authority, Some(first.clone()));
    assert_eq!(state.resolution_revision(), 4);
    let successor_reservation = state
        .reserve_authority()
        .expect("successor authority reserves");
    let successor_authority = successor_reservation.authority();
    drop(
        state
            .activate_reserved_authority(successor_reservation)
            .expect("successor authority activates"),
    );
    assert!(state.is_current_authority(successor_authority));
    assert_eq!(state.resolution_revision(), 5);
    assert!(!state.replace_source_if_current(first_authority, Some(replacement.clone())));
    assert!(!state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: replacement.id.clone(),
        }),
    ));
    assert!(!state.clear_owned_sources_if_current(first_authority));
    assert_eq!(state.resolution_revision(), 5);
    assert!(state.source().is_none());
    assert_eq!(state.owned_source_count(), 0);
}

#[test]
fn authority_reservations_burn_on_drop_and_stale_commits_cannot_regress() {
    let sequencer = CaptureSessionAuthoritySequencer::default();
    let foreign = CaptureSessionAuthoritySequencer::default();
    let foreign_reservation = foreign.reserve().expect("foreign authority reserves");
    assert!(sequencer.commit(foreign_reservation).is_err());
    {
        let burned = sequencer.reserve().expect("first authority reserves");
        assert_eq!(burned.authority().generation(), 1);
    }
    let stale = sequencer.reserve().expect("second authority reserves");
    let successor = sequencer.reserve().expect("third authority reserves");

    assert_eq!(
        sequencer
            .commit(successor)
            .expect("newest authority commits")
            .generation(),
        3
    );
    assert!(sequencer.commit(stale).is_err());
    assert_eq!(
        sequencer
            .current()
            .expect("committed authority remains current")
            .generation(),
        3
    );
}

#[test]
fn exact_authority_retirement_is_conditional_and_does_not_consume_on_stale_retry() {
    let state = CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default();
    let reservation = state.reserve_authority().expect("first authority reserves");
    let authority = reservation.authority();
    drop(
        state
            .activate_reserved_authority(reservation)
            .expect("first authority activates"),
    );
    let source = FakeSource {
        id: CaptureSourceId::new("fake:retirement").expect("test source id is valid"),
        incarnation: 1,
    };
    assert!(state.replace_source_if_current(authority, Some(source)));

    let retirement = state
        .retire_authority_if_current(authority)
        .expect("retirement authority reserves")
        .expect("current authority retires");
    let replacement = retirement.replacement();
    assert_eq!(replacement.generation(), 2);
    assert!(state.source().is_none());
    assert!(!state.replace_source_if_current(authority, None));
    assert!(
        state
            .retire_authority_if_current(authority)
            .expect("stale retirement is not exhaustion")
            .is_none()
    );
    let following = state
        .reserve_authority()
        .expect("stale retirement consumed no authority");
    assert_eq!(following.authority().generation(), 3);
    drop((retirement, following));
}

#[test]
fn exact_authority_retirement_linearizes_against_source_installation() {
    for incarnation in 1..=32 {
        let state =
            Arc::new(CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default());
        let reservation = state.reserve_authority().expect("authority reserves");
        let authority = reservation.authority();
        drop(
            state
                .activate_reserved_authority(reservation)
                .expect("authority activates"),
        );
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let installing_state = Arc::clone(&state);
        let installing_barrier = Arc::clone(&barrier);
        let install = std::thread::spawn(move || {
            let source = FakeSource {
                id: CaptureSourceId::new(format!("fake:race:{incarnation}"))
                    .expect("test source id is valid"),
                incarnation,
            };
            installing_barrier.wait();
            installing_state.replace_source_if_current(authority, Some(source))
        });
        let retiring_state = Arc::clone(&state);
        let retiring_barrier = Arc::clone(&barrier);
        let retire = std::thread::spawn(move || {
            retiring_barrier.wait();
            retiring_state
                .retire_authority_if_current(authority)
                .expect("retirement authority reserves")
                .expect("current authority retires")
        });
        barrier.wait();
        let _installed = install.join().expect("source installer exits");
        let retirement = retire.join().expect("authority retiree exits");

        assert!(state.source().is_none());
        assert_eq!(state.current_authority(), Some(retirement.replacement()));
        drop(retirement);
    }
}

#[test]
fn authority_displacement_drops_owned_sources_after_releasing_ledger_locks() {
    let state = Arc::new(CaptureExactPublicationShared::<
        FakeSource,
        ReentrantOwnedSource,
    >::default());
    let first_reservation = state.reserve_authority().expect("first authority reserves");
    let first_authority = first_reservation.authority();
    drop(
        state
            .activate_reserved_authority(first_reservation)
            .expect("first authority activates"),
    );
    let dropped = Arc::new(AtomicBool::new(false));
    assert!(state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(ReentrantOwnedSource {
            id: CaptureSourceId::new("fake:reentrant").expect("test source id is valid"),
            state: Arc::downgrade(&state),
            dropped: Arc::clone(&dropped),
        }),
    ));

    let successor_reservation = state
        .reserve_authority()
        .expect("successor authority reserves");
    let successor_authority = successor_reservation.authority();
    let displaced = state
        .activate_reserved_authority(successor_reservation)
        .expect("successor authority activates");
    assert!(!dropped.load(Ordering::Acquire));
    drop(displaced);
    assert!(dropped.load(Ordering::Acquire));
    assert!(state.is_current_authority(successor_authority));
}

#[test]
fn extracted_nodes_keep_their_original_destruction_order() {
    let drops = Arc::new(Mutex::new(Vec::new()));
    let mut nodes = ExactBoxList::default();
    for id in [1, 2, 3] {
        nodes.push_boxed(ExactBoxList::boxed_node(OrderedDrop {
            id,
            drops: Arc::clone(&drops),
        }));
    }

    let extracted = nodes.extract_if(|_| true);
    drop(extracted);

    assert_eq!(
        *drops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [3, 2, 1]
    );
}

#[test]
fn stale_reap_leaves_successor_owned_sources_and_runtimes_intact() {
    let state = CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default();
    let stale_authority = state
        .reserve_authority()
        .expect("stale authority reserves")
        .authority();
    let successor_reservation = state
        .reserve_authority()
        .expect("successor authority reserves");
    let successor_authority = successor_reservation.authority();
    drop(
        state
            .activate_reserved_authority(successor_reservation)
            .expect("successor authority activates"),
    );
    assert!(state.register_owned_source_if_current(
        successor_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: CaptureSourceId::new("fake:successor").expect("test source id is valid"),
        }),
    ));
    let mut runtimes = RetainProbeStore {
        runtime_count: 1,
        retain_calls: 0,
    };

    reap_capture_exact_runtimes(stale_authority, &mut runtimes, &state);

    assert_eq!(state.owned_source_count(), 1);
    assert_eq!(runtimes.runtime_count, 1);
    assert_eq!(runtimes.retain_calls, 0);

    reap_capture_exact_runtimes(successor_authority, &mut runtimes, &state);
    assert_eq!(state.owned_source_count(), 0);
    assert_eq!(runtimes.runtime_count, 0);
    assert_eq!(runtimes.retain_calls, 1);
}

#[tokio::test]
async fn exact_retirement_wakes_the_endpoint_and_reports_completion() {
    let endpoint = FakeExactEndpoint::default();
    let retirement = begin_capture_exact_retirement(&endpoint);
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 1);
    let command = endpoint
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .expect("retirement enqueues one exact command");
    let CaptureExactCommand::Reap {
        completion: Some(completion),
        ..
    } = command
    else {
        panic!("retirement enqueues a completing reap command");
    };
    completion
        .send(Ok(()))
        .expect("test retirement receiver remains live");
    retirement
        .complete()
        .await
        .expect("completed exact retirement succeeds");
}

#[tokio::test]
async fn rejected_exact_retirement_fails_without_waking_the_endpoint() {
    let endpoint = FakeExactEndpoint {
        reject: true,
        ..FakeExactEndpoint::default()
    };
    let error = begin_capture_exact_retirement(&endpoint)
        .complete()
        .await
        .expect_err("rejected exact retirement fails");
    assert!(error.to_string().contains("fake capture worker rejected"));
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 0);
}
