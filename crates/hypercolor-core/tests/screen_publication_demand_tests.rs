use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CpuReductionExecutor, InputPublicationDemandRevision, PhysicalOrigin,
    PixelExtent, RegisteredScreenBranchDemand, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAspectPolicy, ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenExtentRequest,
    ScreenInputGraphGeneration, ScreenProcessingProfile, ScreenPublicationDemandError,
    ScreenPublicationDemandSnapshot, ScreenPublicationExecutorRequest, ScreenPublicationHub,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenPublicationTransitionError,
    ScreenResourceApi, ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector,
    ScreenUpscalePolicy, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceScale,
};
use hypercolor_core::input::{InputData, InputManager, InputSource};
use tokio::sync::Barrier;

fn branch(kind: ScreenPublicationKind) -> RegisteredScreenBranchDemand {
    branch_for(ScreenSourceSelector::Configured, kind)
}

fn branch_for(
    selector: ScreenSourceSelector,
    kind: ScreenPublicationKind,
) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            selector,
            kind,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                NonZeroU32::new(7_680),
                NonZeroU32::new(4_320),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        NonZeroU32::new(144).expect("test cadence is non-zero"),
    )
}

#[test]
fn demand_snapshot_preserves_independent_arbitrary_resolution_branches() {
    let surface = branch(ScreenPublicationKind::Surface);
    let zones = branch(ScreenPublicationKind::Zones {
        columns: NonZeroU32::new(127).expect("test grid is non-zero"),
        rows: NonZeroU32::new(71).expect("test grid is non-zero"),
    });
    let branches: Arc<[_]> = [surface.clone(), zones.clone()].into();
    let snapshot = ScreenPublicationDemandSnapshot::try_new(
        InputPublicationDemandRevision::new(19),
        ScreenInputGraphGeneration::new(23),
        branches,
        Some(surface.clone()),
        Some(zones.clone()),
    )
    .expect("exact branches form a valid snapshot");

    assert_eq!(snapshot.revision().get(), 19);
    assert_eq!(snapshot.graph_generation().get(), 23);
    assert_eq!(snapshot.branches().as_ref(), &[surface, zones]);
    assert!(matches!(
        snapshot
            .compatibility_surface()
            .expect("surface compatibility is retained")
            .request()
            .kind(),
        ScreenPublicationKind::Surface
    ));
    assert!(matches!(
        snapshot
            .compatibility_zones()
            .expect("zones compatibility is retained")
            .request()
            .kind(),
        ScreenPublicationKind::Zones { .. }
    ));
    assert!(!snapshot.is_empty());
}

#[test]
fn demand_snapshot_rejects_unregistered_or_mistyped_compatibility() {
    let surface = branch(ScreenPublicationKind::Surface);
    let zones = branch(ScreenPublicationKind::Zones {
        columns: NonZeroU32::MIN,
        rows: NonZeroU32::MIN,
    });

    assert_eq!(
        ScreenPublicationDemandSnapshot::try_new(
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            Arc::from([surface.clone()]),
            Some(zones.clone()),
            None,
        ),
        Err(ScreenPublicationDemandError::CompatibilityKindMismatch)
    );
    assert_eq!(
        ScreenPublicationDemandSnapshot::try_new(
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            Arc::from([surface]),
            None,
            Some(zones),
        ),
        Err(ScreenPublicationDemandError::CompatibilityBranchMissing)
    );
}

struct RuntimeAllocation {
    binding: ScreenWorkerBinding,
    _lifetimes: Vec<ScreenResourceLifetime>,
}

#[derive(Default)]
struct ExactWorkerState {
    preparations: AtomicUsize,
    aborts: AtomicUsize,
    retirements: AtomicUsize,
    fail_preparation: AtomicBool,
    allocations: Mutex<Vec<RuntimeAllocation>>,
}

impl ExactWorkerState {
    fn reap(&self, state: ScreenWorkerBindingState) {
        self.allocations
            .lock()
            .expect("runtime allocation mutex is healthy")
            .retain(|allocation| allocation.binding.state() != state);
    }
}

struct ExactDemandProbe {
    source: ResolvedScreenSource,
    hub: Arc<Mutex<Option<Arc<ScreenPublicationHub>>>>,
    worker: Arc<ExactWorkerState>,
    preparation_barrier: Option<Arc<Barrier>>,
}

impl ExactDemandProbe {
    fn new(
        hub: Arc<Mutex<Option<Arc<ScreenPublicationHub>>>>,
        worker: Arc<ExactWorkerState>,
    ) -> Self {
        let source_id =
            CaptureSourceId::new("synthetic:coordinator").expect("test source id is non-empty");
        Self::for_source(hub, worker, ScreenSourceSelector::Configured, source_id)
    }

    fn for_source(
        hub: Arc<Mutex<Option<Arc<ScreenPublicationHub>>>>,
        worker: Arc<ExactWorkerState>,
        selector: ScreenSourceSelector,
        source_id: CaptureSourceId,
    ) -> Self {
        let extent = PixelExtent::new(7_680, 4_320).expect("test extent is non-empty");
        let geometry = CaptureGeometry::new(
            PhysicalOrigin::default(),
            extent,
            extent,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        )
        .expect("test geometry is valid");
        Self {
            source: ResolvedScreenSource::new(
                selector,
                CaptureEpoch {
                    source_id,
                    topology_generation: 3,
                    session_generation: 5,
                },
                ResolvedScreenSourceConfig::new(
                    geometry,
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
            ),
            hub,
            worker,
            preparation_barrier: None,
        }
    }

    fn with_preparation_barrier(mut self, barrier: Arc<Barrier>) -> Self {
        self.preparation_barrier = Some(barrier);
        self
    }
}

impl InputSource for ExactDemandProbe {
    fn name(&self) -> &'static str {
        "exact_demand_probe"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) {}

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        true
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_publication_hub(&mut self, hub: Arc<ScreenPublicationHub>) {
        *self.hub.lock().expect("probe hub mutex is healthy") = Some(hub);
    }

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<hypercolor_core::input::screen::ResolvedScreenBranchDemand>> {
        if demand.request().selector() != self.source.selector() {
            return Ok(None);
        }
        let capabilities = CpuReductionExecutor::new(NonZeroUsize::MIN, NonZeroU32::MIN)
            .expect("test CPU reducer builds")
            .capabilities();
        Ok(Some(demand.resolve_with_color_capabilities(
            &self.source,
            capabilities,
        )?))
    }

    fn owns_screen_publication_source(&self, source_id: &CaptureSourceId) -> bool {
        self.source.epoch().source_id == *source_id
    }

    fn begin_screen_publication_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        let worker = Arc::clone(&self.worker);
        let abort_worker = Arc::clone(&self.worker);
        let preparation_barrier = self.preparation_barrier.clone();
        Ok(ScreenWorkerPreparation::with_abort(
            async move {
                if let Some(barrier) = preparation_barrier {
                    barrier.wait().await;
                }
                worker.preparations.fetch_add(1, Ordering::AcqRel);
                let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
                let reports = ledger
                    .ticket()
                    .required_minimums()
                    .iter()
                    .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
                    .collect::<Vec<_>>();
                for (name, bytes) in reports {
                    ledger.report(&name, bytes)?;
                }
                let exact = ledger.finish()?;
                let binding = exact.token().binding().clone();
                let (token, lifetimes) = exact.into_parts();
                worker
                    .allocations
                    .lock()
                    .expect("runtime allocation mutex is healthy")
                    .push(RuntimeAllocation {
                        binding,
                        _lifetimes: lifetimes,
                    });
                if worker.fail_preparation.load(Ordering::Acquire) {
                    anyhow::bail!("injected exact worker failure");
                }
                Ok(token)
            },
            move || {
                abort_worker.aborts.fetch_add(1, Ordering::AcqRel);
                abort_worker.reap(ScreenWorkerBindingState::Aborted);
            },
        ))
    }

    fn begin_screen_publication_retirement(&mut self) -> Option<ScreenWorkerRetirement> {
        let worker = Arc::clone(&self.worker);
        Some(ScreenWorkerRetirement::new(async move {
            worker.retirements.fetch_add(1, Ordering::AcqRel);
            worker.reap(ScreenWorkerBindingState::Retired);
            Ok(())
        }))
    }
}

struct PassiveSource;

impl InputSource for PassiveSource {
    fn name(&self) -> &'static str {
        "passive"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) {}

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        true
    }
}

fn demand(
    manager: &InputManager,
    revision: u64,
    branches: impl IntoIterator<Item = RegisteredScreenBranchDemand>,
) -> ScreenPublicationDemandSnapshot {
    let branches = branches.into_iter().collect::<Vec<_>>();
    let compatibility_surface = branches
        .iter()
        .find(|branch| matches!(branch.request().kind(), ScreenPublicationKind::Surface))
        .cloned();
    ScreenPublicationDemandSnapshot::try_new(
        InputPublicationDemandRevision::new(revision),
        ScreenInputGraphGeneration::new(manager.source_graph_generation()),
        branches.into(),
        compatibility_surface,
        None,
    )
    .expect("test demand is valid")
}

async fn finish_retirements(
    transition: hypercolor_core::input::screen::CommittedScreenPublicationTransition,
) -> hypercolor_core::input::screen::CommittedScreenPlan {
    let (committed, retirements) = transition.into_parts();
    for (_, retirement) in retirements {
        retirement
            .complete()
            .await
            .expect("test worker retirement completes");
    }
    committed
}

fn manager_fixture() -> (
    InputManager,
    Arc<ScreenPublicationHub>,
    Arc<ExactWorkerState>,
) {
    let attached_hub = Arc::new(Mutex::new(None));
    let worker = Arc::new(ExactWorkerState::default());
    let mut manager = InputManager::new();
    let stable_hub = manager.screen_publication_hub();
    manager.add_source(Box::new(ExactDemandProbe::new(
        Arc::clone(&attached_hub),
        Arc::clone(&worker),
    )));
    let source_hub = attached_hub
        .lock()
        .expect("probe hub mutex is healthy")
        .clone()
        .expect("screen source receives a hub");
    assert!(Arc::ptr_eq(&stable_hub, &source_hub));
    assert!(Arc::ptr_eq(&stable_hub, &manager.screen_publication_hub()));
    (manager, stable_hub, worker)
}

#[tokio::test]
async fn manager_commits_exact_plan_once_through_detached_worker_preparation() {
    let (mut manager, hub, worker) = manager_fixture();
    let demand = demand(&manager, 5, [branch(ScreenPublicationKind::Surface)]);
    let preparation = manager
        .begin_screen_publication_transition(demand.clone())
        .expect("exact plan resolves")
        .expect("new exact plan requires preparation");
    assert_eq!(worker.preparations.load(Ordering::Acquire), 0);

    let prepared = preparation
        .await_workers()
        .await
        .expect("worker acknowledges exact resources");
    assert_eq!(worker.preparations.load(Ordering::Acquire), 1);
    let committed = manager
        .commit_screen_publication_transition(prepared, demand.revision())
        .expect("fenced exact plan commits");
    let committed = finish_retirements(committed).await;
    assert_eq!(committed.plan().branches().len(), 1);
    assert_eq!(hub.committed_state().branch_count(), 1);
    assert_eq!(worker.aborts.load(Ordering::Acquire), 0);
    assert!(
        manager
            .begin_screen_publication_transition(demand)
            .expect("equal demand remains valid")
            .is_none()
    );
    let (_, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("first commit retires no visible resources");
}

#[tokio::test]
async fn independent_source_workers_prepare_concurrently() {
    let first_id = CaptureSourceId::new("synthetic:first").expect("test source id is non-empty");
    let second_id = CaptureSourceId::new("synthetic:second").expect("test source id is non-empty");
    let first_worker = Arc::new(ExactWorkerState::default());
    let second_worker = Arc::new(ExactWorkerState::default());
    let barrier = Arc::new(Barrier::new(2));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(
        ExactDemandProbe::for_source(
            Arc::new(Mutex::new(None)),
            Arc::clone(&first_worker),
            ScreenSourceSelector::Exact(first_id.clone()),
            first_id.clone(),
        )
        .with_preparation_barrier(Arc::clone(&barrier)),
    ));
    manager.add_source(Box::new(
        ExactDemandProbe::for_source(
            Arc::new(Mutex::new(None)),
            Arc::clone(&second_worker),
            ScreenSourceSelector::Exact(second_id.clone()),
            second_id.clone(),
        )
        .with_preparation_barrier(barrier),
    ));
    let exact = demand(
        &manager,
        9,
        [
            branch_for(
                ScreenSourceSelector::Exact(first_id),
                ScreenPublicationKind::Surface,
            ),
            branch_for(
                ScreenSourceSelector::Exact(second_id),
                ScreenPublicationKind::Surface,
            ),
        ],
    );
    let preparation = manager
        .begin_screen_publication_transition(exact.clone())
        .expect("both exact sources resolve")
        .expect("multi-source plan requires preparation");
    let prepared = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        preparation.await_workers(),
    )
    .await
    .expect("independent workers must reach the barrier concurrently")
    .expect("both workers acknowledge exact resources");

    assert_eq!(first_worker.preparations.load(Ordering::Acquire), 1);
    assert_eq!(second_worker.preparations.load(Ordering::Acquire), 1);
    let committed = manager
        .commit_screen_publication_transition(prepared, exact.revision())
        .expect("multi-source exact plan commits");
    let committed = finish_retirements(committed).await;
    assert_eq!(committed.plan().branches().len(), 2);
}

#[tokio::test]
async fn demand_race_aborts_candidate_and_preserves_committed_authority() {
    let (mut manager, hub, worker) = manager_fixture();
    let demand = demand(&manager, 5, [branch(ScreenPublicationKind::Surface)]);
    let before = hub.committed_state();
    let prepared = manager
        .begin_screen_publication_transition(demand.clone())
        .expect("exact plan resolves")
        .expect("new exact plan prepares")
        .await_workers()
        .await
        .expect("worker acknowledges exact resources");
    let failure = manager
        .commit_screen_publication_transition(
            prepared,
            InputPublicationDemandRevision::new(demand.revision().get() + 1),
        )
        .expect_err("newer demand revision rejects stale preparation");

    assert!(matches!(
        failure.error(),
        ScreenPublicationTransitionError::Plan(
            hypercolor_core::input::screen::ScreenPlanError::DemandRevisionConflict { .. }
        )
    ));
    assert!(Arc::ptr_eq(&before, &hub.committed_state()));
    assert_eq!(failure.abort().active_plan().generation().get(), 0);
    drop(failure);
    assert_eq!(worker.aborts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn graph_race_aborts_candidate_and_preserves_committed_authority() {
    let (mut manager, hub, worker) = manager_fixture();
    let demand = demand(&manager, 5, [branch(ScreenPublicationKind::Surface)]);
    let before = hub.committed_state();
    let prepared = manager
        .begin_screen_publication_transition(demand.clone())
        .expect("exact plan resolves")
        .expect("new exact plan prepares")
        .await_workers()
        .await
        .expect("worker acknowledges exact resources");
    manager.add_source(Box::new(PassiveSource));
    let failure = manager
        .commit_screen_publication_transition(prepared, demand.revision())
        .expect_err("new graph generation rejects stale preparation");

    assert!(matches!(
        failure.error(),
        ScreenPublicationTransitionError::Plan(
            hypercolor_core::input::screen::ScreenPlanError::BaseGraphGenerationConflict { .. }
        )
    ));
    assert!(Arc::ptr_eq(&before, &hub.committed_state()));
    drop(failure);
    assert_eq!(worker.aborts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn worker_failure_aborts_candidate_before_manager_commit() {
    let (mut manager, hub, worker) = manager_fixture();
    worker.fail_preparation.store(true, Ordering::Release);
    let demand = demand(&manager, 5, [branch(ScreenPublicationKind::Surface)]);
    let before = hub.committed_state();
    let failure = manager
        .begin_screen_publication_transition(demand)
        .expect("exact plan resolves")
        .expect("new exact plan prepares")
        .await_workers()
        .await
        .expect_err("injected worker failure rejects candidate");

    assert!(matches!(
        failure.error(),
        ScreenPublicationTransitionError::WorkerPreparationFailed { .. }
    ));
    assert!(Arc::ptr_eq(&before, &hub.committed_state()));
    assert_eq!(worker.aborts.load(Ordering::Acquire), 1);
    assert!(
        worker
            .allocations
            .lock()
            .expect("runtime allocation mutex is healthy")
            .is_empty()
    );
}

#[test]
fn explicit_abort_cancels_unpolled_workers_and_preserves_active_plan() {
    let (mut manager, hub, worker) = manager_fixture();
    let demand = demand(&manager, 5, [branch(ScreenPublicationKind::Surface)]);
    let before = hub.committed_state();
    let abort = manager
        .begin_screen_publication_transition(demand)
        .expect("exact plan resolves")
        .expect("new exact plan prepares")
        .abort();

    assert!(Arc::ptr_eq(&before, &hub.committed_state()));
    assert_eq!(abort.active_plan().generation().get(), 0);
    assert_eq!(worker.preparations.load(Ordering::Acquire), 0);
    assert_eq!(worker.aborts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn empty_demand_retires_worker_and_reclaims_after_reader_release() {
    let (mut manager, hub, worker) = manager_fixture();
    let active = demand(&manager, 5, [branch(ScreenPublicationKind::Surface)]);
    let prepared = manager
        .begin_screen_publication_transition(active.clone())
        .expect("active exact plan resolves")
        .expect("active exact plan prepares")
        .await_workers()
        .await
        .expect("active worker acknowledges");
    let committed = manager
        .commit_screen_publication_transition(prepared, active.revision())
        .expect("active exact plan commits");
    let committed = finish_retirements(committed).await;
    let descriptor = committed.plan().branches()[0].descriptor().clone();
    let lease = hub.lease(&descriptor).expect("active branch leases");
    let (_, first_retirement) = committed.into_parts();
    first_retirement
        .try_reclaim()
        .expect("first commit retires no visible resources");

    let empty = demand(&manager, 6, []);
    let prepared = manager
        .begin_screen_publication_transition(empty.clone())
        .expect("empty exact plan resolves")
        .expect("empty exact plan prepares retirement")
        .await_workers()
        .await
        .expect("removal worker acknowledges");
    let committed = manager
        .commit_screen_publication_transition(prepared, empty.revision())
        .expect("empty exact plan commits");
    let committed = finish_retirements(committed).await;
    let (plan, retirement) = committed.into_parts();

    assert!(plan.branches().is_empty());
    assert!(worker.retirements.load(Ordering::Acquire) >= 2);
    assert!(
        worker
            .allocations
            .lock()
            .expect("runtime allocation mutex is healthy")
            .is_empty()
    );
    let retirement = retirement
        .try_reclaim()
        .expect_err("active reader retains retired branch storage");
    drop(lease);
    retirement
        .try_reclaim()
        .expect("retired storage reclaims after reader release");
}
