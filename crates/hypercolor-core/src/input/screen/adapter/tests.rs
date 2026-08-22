use super::{
    CaptureBackend, CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureExactRuntimeCollection, CaptureExactRuntimeOwner,
    CaptureExactRuntimeStore, CaptureExactState, CaptureOwnedSource, CapturePublication,
    CapturePublicationFence, CapturePublicationSource, CaptureSession, CaptureSessionAuthority,
    CaptureSessionAuthoritySequencer, CaptureSessionDeadline, CaptureSessionReadiness,
    CaptureSessionSet, CaptureSessionTransaction, CaptureSuccessorPolicy,
    ReservedCaptureSessionAuthority, ScreenCaptureAdapter, VersionedCaptureSettings,
    begin_capture_exact_retirement, exact_preparation_abort, reap_capture_exact_runtimes,
};
use crate::input::screen::{
    CaptureSourceId, ExactBoxList, PixelExtent, ScreenCaptureDemand, ScreenCommittedState,
    ScreenPublicationHub, ScreenPublicationSlotPolicy, ScreenWorkerBinding,
};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

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
}

impl FakeSession {
    fn new(generation: u64) -> Self {
        Self {
            authority: CaptureSessionAuthority::new(generation),
            endpoint: FakeExactEndpoint::default(),
            finished: Arc::new(AtomicBool::new(false)),
            aborts: Arc::default(),
            wakes: Arc::default(),
            finishes: Arc::default(),
            detaches: Arc::default(),
            starts: Arc::default(),
            start_prerequisite: None,
        }
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
        self.aborts.fetch_add(1, Ordering::Relaxed);
    }

    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }

    fn start(&self) {
        if let Some(prerequisite) = self.start_prerequisite.as_ref() {
            assert!(prerequisite.load(Ordering::Acquire));
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

struct FakeCaptureBackend;

impl CaptureBackend for FakeCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = (FakeSession, FakeReadiness);
    type ExactState = CaptureExactPublicationShared<FakeSource, FakeOwnedSource>;
    type CompatibilityFence = FakeFence;
    type CompatibilityEpoch = FakeEpoch;
    type CompatibilityValue = &'static str;

    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn spawn_worker(
        (session, readiness): Self::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        Ok(CaptureSessionTransaction::new(
            session,
            readiness,
            reservation,
        ))
    }
}

struct FailingCaptureBackend;

impl CaptureBackend for FailingCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = ();
    type ExactState = CaptureExactPublicationShared<FakeSource, FakeOwnedSource>;
    type CompatibilityFence = FakeFence;
    type CompatibilityEpoch = FakeEpoch;
    type CompatibilityValue = &'static str;

    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn spawn_worker(
        (): Self::SpawnRequest,
        _reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        anyhow::bail!("injected backend spawn failure")
    }
}

struct DropOrderExactState {
    common: CaptureExactPublicationShared<FakeSource, FakeOwnedSource>,
    session_detaches: Arc<AtomicUsize>,
    compatibility_dropped: Arc<AtomicBool>,
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
        assert_eq!(self.session_detaches.load(Ordering::Acquire), 1);
        assert!(self.compatibility_dropped.load(Ordering::Acquire));
        self.dropped.store(true, Ordering::Release);
    }
}

struct CompatibilityDropProbe(Arc<AtomicBool>);

impl Drop for CompatibilityDropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct DropOrderCaptureBackend;

impl CaptureBackend for DropOrderCaptureBackend {
    type Worker = FakeSession;
    type Readiness = FakeReadiness;
    type SpawnRequest = (FakeSession, FakeReadiness);
    type ExactState = DropOrderExactState;
    type CompatibilityFence = FakeFence;
    type CompatibilityEpoch = FakeEpoch;
    type CompatibilityValue = CompatibilityDropProbe;

    const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

    fn spawn_worker(
        (session, readiness): Self::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>> {
        Ok(CaptureSessionTransaction::new(
            session,
            readiness,
            reservation,
        ))
    }
}

fn readiness_deadline() -> CaptureSessionDeadline {
    CaptureSessionDeadline::after(Duration::from_secs(1))
}

#[test]
fn backend_worker_factory_pairs_authority_and_waits_once() {
    let sequencer = CaptureSessionAuthoritySequencer::default();
    let adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    let reservation = sequencer.reserve().expect("backend authority reserves");
    let waits = Arc::new(AtomicUsize::new(0));
    let readiness = FakeReadiness {
        result: Ok(()),
        waits: Arc::clone(&waits),
    };

    let prepared = adapter
        .prepare_worker((FakeSession::new(1), readiness), reservation)
        .expect("backend worker prepares");

    assert_eq!(prepared.authority().generation(), 1);
    assert_eq!(waits.load(Ordering::Relaxed), 1);
}

#[test]
fn backend_worker_factory_burns_failed_spawn_and_cleans_failed_readiness() {
    let sequencer = CaptureSessionAuthoritySequencer::default();
    let failing_adapter = ScreenCaptureAdapter::<FailingCaptureBackend>::default();
    let failed_spawn = sequencer
        .reserve()
        .expect("failed spawn authority reserves");
    assert!(failing_adapter.prepare_worker((), failed_spawn).is_err());
    let readiness_reservation = sequencer
        .reserve()
        .expect("readiness authority reserves after failed spawn");
    assert_eq!(readiness_reservation.authority().generation(), 2);

    let candidate = FakeSession::new(2);
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
    assert!(
        adapter
            .prepare_worker((candidate, readiness), readiness_reservation)
            .is_err()
    );
    assert_eq!(waits.load(Ordering::Relaxed), 1);
    assert_eq!(aborts.load(Ordering::Relaxed), 1);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert_eq!(detaches.load(Ordering::Relaxed), 1);
    assert_eq!(starts.load(Ordering::Relaxed), 0);
}

#[test]
fn adapter_and_backend_handles_share_one_exact_state() {
    let exact = Arc::new(CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default());
    let adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::new(Arc::clone(&exact));
    assert_eq!(Arc::strong_count(&exact), 2);
    let compatibility = adapter.compatibility_publication_handle();
    assert!(std::ptr::eq(
        adapter.compatibility_publication(),
        compatibility.as_ref()
    ));
    assert_eq!(Arc::strong_count(&compatibility), 2);

    let epoch = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    {
        let mut worker_publication = compatibility
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(worker_publication.activate(epoch).is_ok());
        assert!(worker_publication.publish(&epoch, "worker frame").is_ok());
    }
    let sampled = adapter
        .compatibility_publication()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .snapshot()
        .expect("adapter samples the worker publication");
    assert_eq!(sampled.epoch, epoch);
    assert_eq!(sampled.value, "worker frame");

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
fn adapter_detaches_sessions_before_releasing_exact_state() {
    let detaches = Arc::new(AtomicUsize::new(0));
    let compatibility_dropped = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let exact = Arc::new(DropOrderExactState {
        common: CaptureExactPublicationShared::default(),
        session_detaches: Arc::clone(&detaches),
        compatibility_dropped: Arc::clone(&compatibility_dropped),
        dropped: Arc::clone(&dropped),
    });
    let mut adapter = ScreenCaptureAdapter::<DropOrderCaptureBackend>::new(Arc::clone(&exact));
    let epoch = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    {
        let mut publication = adapter
            .compatibility_publication()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(publication.activate(epoch).is_ok());
        assert!(
            publication
                .publish(
                    &epoch,
                    CompatibilityDropProbe(Arc::clone(&compatibility_dropped)),
                )
                .is_ok()
        );
    }
    drop(exact);
    let mut session = FakeSession::new(1);
    session.detaches = Arc::clone(&detaches);
    assert!(adapter.install_worker_for_test(session).is_ok());

    drop(adapter);

    assert_eq!(detaches.load(Ordering::Acquire), 1);
    assert!(compatibility_dropped.load(Ordering::Acquire));
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
fn session_transaction_candidate_endpoint_is_hidden_until_commit() {
    let mut adapter = ScreenCaptureAdapter::<FakeCaptureBackend>::default();
    assert!(adapter.active_exact_endpoint().is_none());
    let prepared = adapter
        .prepare_worker(
            (FakeSession::new(1), FakeReadiness::ready()),
            reservation(1),
        )
        .expect("candidate becomes ready");
    assert!(adapter.active_exact_endpoint().is_none());

    adapter
        .commit_worker(prepared, |_| Some(()), |_, ()| ())
        .unwrap_or_else(|_| panic!("ready candidate commits"));

    assert!(adapter.active_exact_endpoint().is_some());
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
    const SOURCE_NAME: &'static str = "fake capture";

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

#[test]
fn versioned_settings_commit_one_coherent_config_and_demand_snapshot() {
    let settings =
        VersionedCaptureSettings::new(String::from("initial"), ScreenCaptureDemand::Inactive);

    assert_eq!(settings.revision(), 0);
    let initial = settings.snapshot();
    assert_eq!(initial.config, "initial");
    assert_eq!(initial.demand, ScreenCaptureDemand::Inactive);

    let active = ScreenCaptureDemand::active(
        PixelExtent::new(64, 32).expect("test capture extent is nonzero"),
    );
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
    assert_eq!(
        *settings
            .try_lock_demand()
            .expect("demand lock should be available"),
        ScreenCaptureDemand::Inactive
    );
}

impl CapturePublicationFence<FakeEpoch> for FakeFence {
    fn admits(&self, epoch: &FakeEpoch) -> bool {
        epoch.source == self.source && epoch.activity == self.activity
    }
}

#[test]
fn publication_fences_every_epoch_dimension_and_keeps_only_the_latest_value() {
    let initial = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    let mut publication = CapturePublication::<FakeFence, _, _>::default();

    assert_eq!(publication.fence(), &FakeFence::default());
    assert!(
        publication
            .replace_fence_if_changed(FakeFence::default())
            .is_none()
    );
    assert!(publication.activate(initial).is_ok());
    assert!(publication.is_active(&initial));
    assert!(publication.publish(&initial, "first").is_ok());
    assert!(publication.publish(&initial, "latest").is_ok());
    assert_eq!(publication.latest(), Some(&"latest"));
    assert_eq!(
        publication
            .snapshot()
            .expect("latest publication remains visible")
            .revision,
        2
    );

    for stale in [
        FakeEpoch {
            source: 1,
            ..initial
        },
        FakeEpoch {
            activity: 1,
            ..initial
        },
        FakeEpoch {
            session: 4,
            ..initial
        },
        FakeEpoch {
            topology: 5,
            ..initial
        },
        FakeEpoch {
            resource: 6,
            ..initial
        },
    ] {
        assert!(publication.publish(&stale, "stale").is_err());
        assert_eq!(publication.latest(), Some(&"latest"));
    }
}

#[test]
fn publication_requires_current_source_and_activity_before_reactivation() {
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
    let mut publication = CapturePublication::<FakeFence, _, _>::default();

    assert!(publication.activate(previous).is_ok());
    assert!(publication.publish(&previous, "previous").is_ok());
    let checkpoint = publication.checkpoint();
    let displaced = publication.replace_fence(FakeFence {
        source: 1,
        activity: 1,
    });
    assert_eq!(displaced.latest, Some("previous"));

    assert!(publication.activate(previous).is_err());
    assert!(publication.publish(&previous, "late").is_err());
    assert!(publication.active().is_none());
    assert!(publication.latest().is_none());
    assert!(publication.activate(current).is_ok());
    assert!(matches!(
        publication.restore_checkpoint(Some(&current), checkpoint),
        Ok(None)
    ));
    let snapshot = publication.snapshot().expect("restored value is visible");
    assert_eq!(snapshot.epoch, current);
    assert_eq!(snapshot.revision, 2);
    assert_eq!(snapshot.value, "previous");
    assert!(publication.publish(&current, "current").is_ok());
    assert_eq!(publication.latest(), Some(&"current"));
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
