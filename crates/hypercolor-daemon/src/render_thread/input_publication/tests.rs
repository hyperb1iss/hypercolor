use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CpuReductionExecutor, PhysicalOrigin, PixelExtent,
    RegisteredScreenBranchDemand, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAspectPolicy, ScreenBackendResourceIdentity, ScreenBranchPayload, ScreenCaptureBackend,
    ScreenCaptureDemand, ScreenExtentRequest, ScreenProcessingProfile,
    ScreenPublicationColorimetry, ScreenPublicationExecutorRequest, ScreenPublicationHealth,
    ScreenPublicationHub, ScreenPublicationKind, ScreenPublicationMetadata,
    ScreenPublicationRequest, ScreenResourceApi, ScreenResourceLifetime, ScreenSourceReflection,
    ScreenSourceSelector, ScreenSurfacePayload, ScreenUpscalePolicy, ScreenWorkerBinding,
    ScreenWorkerBindingState, ScreenWorkerExactLedgerBuilder, ScreenWorkerPreparation,
    ScreenWorkerPreparationTicket, ScreenWorkerRetirement, SourceScale,
};
use hypercolor_core::input::{
    InputData, InputManager, InputSource, InteractionSource, InteractionSourceRole,
    ManagedSourceRole, ScreenSource, ScreenSourceRole, SourceKind, SourceRoleBinding,
};
use tokio::sync::Notify;

use super::{
    EXACT_PLAN_UNAVAILABLE_RETRY_INTERVAL, ExactScreenTransitionOutcome,
    ExactScreenTransitionPurpose, InputPublicationCadence, InputPublicationConsumer,
    InputPublicationDemand, InputPublicationDemandHandle, InputPublicationPump,
    InputPublicationReader, InputPublicationSchedule, InputPublicationStatus,
    InputScreenBranchDemand, LIFECYCLE_PROBE_INTERVAL, cadence_interval,
    exact_screen_failure_retry_at, run_exact_screen_transition,
};
use crate::render_thread::producer_queue::ProducerFrame;

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

struct CountingSource {
    samples: Arc<AtomicUsize>,
    capture_active: Arc<AtomicBool>,
    running: bool,
}

struct BlockingStartInteractionSource {
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
    running: bool,
}

struct BlockingCapabilityInteractionSource {
    armed: Arc<AtomicBool>,
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}

struct ScreenDemandSource {
    demand: ScreenCaptureDemand,
    transitions: Arc<StdMutex<Vec<ScreenCaptureDemand>>>,
    source: ResolvedScreenSource,
    runtime: Arc<StdMutex<Vec<ScreenRuntimeAllocation>>>,
    preparation_started: Option<Arc<Notify>>,
    preparation_release: Option<Arc<Notify>>,
    preparation_attempts: Option<Arc<AtomicUsize>>,
    preparation_failures: Option<Arc<AtomicUsize>>,
    retirement_started: Option<Arc<Notify>>,
    retirement_release: Option<Arc<Notify>>,
    running: bool,
}

struct ScreenRuntimeAllocation {
    binding: ScreenWorkerBinding,
    _lifetimes: Box<[ScreenResourceLifetime]>,
}

impl ScreenDemandSource {
    fn new(transitions: Arc<StdMutex<Vec<ScreenCaptureDemand>>>) -> Self {
        let extent = extent(7_680, 4_320);
        let geometry = CaptureGeometry::new(
            PhysicalOrigin::default(),
            extent,
            extent,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        )
        .expect("test screen geometry is valid");
        Self {
            demand: ScreenCaptureDemand::Inactive,
            transitions,
            source: ResolvedScreenSource::new(
                ScreenSourceSelector::Configured,
                CaptureEpoch {
                    source_id: CaptureSourceId::new("synthetic:daemon-screen")
                        .expect("test source id is non-empty"),
                    topology_generation: 1,
                    session_generation: 1,
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
                        1,
                        1,
                    ),
                ),
            ),
            runtime: Arc::new(StdMutex::new(Vec::new())),
            preparation_started: None,
            preparation_release: None,
            preparation_attempts: None,
            preparation_failures: None,
            retirement_started: None,
            retirement_release: None,
            running: false,
        }
    }

    fn with_preparation_gate(mut self, started: Arc<Notify>, release: Arc<Notify>) -> Self {
        self.preparation_started = Some(started);
        self.preparation_release = Some(release);
        self
    }

    fn with_preparation_attempts(mut self, attempts: Arc<AtomicUsize>) -> Self {
        self.preparation_attempts = Some(attempts);
        self
    }

    fn with_recovery_gates(
        mut self,
        preparation_failures: Arc<AtomicUsize>,
        retirement_started: Arc<Notify>,
        retirement_release: Arc<Notify>,
    ) -> Self {
        self.preparation_failures = Some(preparation_failures);
        self.retirement_started = Some(retirement_started);
        self.retirement_release = Some(retirement_release);
        self
    }
}

impl InputSource for ScreenDemandSource {
    fn name(&self) -> &'static str {
        "screen_demand"
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
}

impl SourceRoleBinding for ScreenDemandSource {
    type Role = ScreenSourceRole;
}

impl ScreenSource for ScreenDemandSource {
    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.demand
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        self.demand = demand;
        self.transitions
            .lock()
            .expect("screen demand transition lock")
            .push(demand);
        Ok(())
    }

    fn set_screen_publication_hub(&mut self, _hub: Arc<ScreenPublicationHub>) {}

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<hypercolor_core::input::screen::ResolvedScreenBranchDemand>> {
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
        if let Some(attempts) = &self.preparation_attempts {
            attempts.fetch_add(1, Ordering::AcqRel);
        }
        let runtime = Arc::clone(&self.runtime);
        let abort_runtime = Arc::clone(&self.runtime);
        let preparation_started = self.preparation_started.clone();
        let preparation_release = self.preparation_release.clone();
        let preparation_failures = self.preparation_failures.clone();
        Ok(ScreenWorkerPreparation::with_abort(
            async move {
                if let Some(started) = preparation_started {
                    started.notify_one();
                }
                if let Some(release) = preparation_release {
                    release.notified().await;
                }
                if preparation_failures.as_ref().is_some_and(|failures| {
                    failures
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                            remaining.checked_sub(1)
                        })
                        .is_ok()
                }) {
                    anyhow::bail!("injected exact screen preparation failure");
                }
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
                runtime
                    .lock()
                    .expect("screen runtime lock is healthy")
                    .push(ScreenRuntimeAllocation {
                        binding,
                        _lifetimes: lifetimes,
                    });
                Ok(token)
            },
            move || {
                abort_runtime
                    .lock()
                    .expect("screen runtime lock is healthy")
                    .retain(|allocation| {
                        allocation.binding.state() != ScreenWorkerBindingState::Aborted
                    });
            },
        ))
    }

    fn begin_screen_publication_retirement(&mut self) -> Option<ScreenWorkerRetirement> {
        let runtime = Arc::clone(&self.runtime);
        let should_block = runtime
            .lock()
            .expect("screen runtime lock is healthy")
            .iter()
            .any(|allocation| allocation.binding.state() == ScreenWorkerBindingState::Retired);
        let retirement_started = self.retirement_started.clone();
        let retirement_release = self.retirement_release.clone();
        Some(ScreenWorkerRetirement::new(async move {
            if should_block {
                if let Some(started) = retirement_started {
                    started.notify_one();
                }
                if let Some(release) = retirement_release {
                    release.notified().await;
                }
            }
            runtime
                .lock()
                .expect("screen runtime lock is healthy")
                .retain(|allocation| {
                    allocation.binding.state() != ScreenWorkerBindingState::Retired
                });
            Ok(())
        }))
    }
}

impl CountingSource {
    fn new(samples: Arc<AtomicUsize>) -> Self {
        Self {
            samples,
            capture_active: Arc::new(AtomicBool::new(true)),
            running: false,
        }
    }

    fn with_capture_active(samples: Arc<AtomicUsize>, capture_active: Arc<AtomicBool>) -> Self {
        Self {
            samples,
            capture_active,
            running: false,
        }
    }
}

impl InputSource for CountingSource {
    fn name(&self) -> &'static str {
        "counting"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl SourceRoleBinding for CountingSource {
    type Role = InteractionSourceRole;
}

impl InteractionSource for CountingSource {
    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.capture_active.store(active, Ordering::Release);
        Ok(())
    }
}

impl InputSource for BlockingStartInteractionSource {
    fn name(&self) -> &'static str {
        "blocking_start_interaction"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.entered
            .send(())
            .expect("startup observer should remain connected");
        self.release
            .recv()
            .expect("startup release should remain connected");
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
}

impl SourceRoleBinding for BlockingStartInteractionSource {
    type Role = InteractionSourceRole;
}

impl InteractionSource for BlockingStartInteractionSource {}

impl InputSource for BlockingCapabilityInteractionSource {
    fn name(&self) -> &'static str {
        "blocking_capability_interaction"
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

impl SourceRoleBinding for BlockingCapabilityInteractionSource {
    type Role = InteractionSourceRole;
}

impl InteractionSource for BlockingCapabilityInteractionSource {
    fn set_capability_context(
        &mut self,
        _context: &hypercolor_core::input::SourceCapabilityContext,
    ) -> anyhow::Result<()> {
        if self.armed.load(Ordering::Acquire) {
            self.entered
                .send(())
                .expect("capability observer should remain connected");
            self.release
                .recv()
                .expect("capability release should remain connected");
        }
        Ok(())
    }
}

#[test]
fn demand_snapshot_uses_the_highest_typed_consumer_rate() {
    let demands = InputPublicationDemandHandle::new();
    let _authoritative = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::all_sources(60, extent(640, 480)),
    );
    let _preview = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default().with_source(SourceKind::Interaction, 360),
    );
    let _diagnostic = demands.register(
        InputPublicationConsumer::Diagnostic,
        InputPublicationDemand::default().with_screen(144, extent(5_120, 720)),
    );

    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 360);
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 144);
    assert_eq!(snapshot.requested_hz(SourceKind::Sensors), 60);
    assert_eq!(snapshot.compatibility_screen_extent, Some(extent(640, 480)));
    assert_eq!(snapshot.screen_branches.len(), 2);
    assert!(
        snapshot.screen_branches.iter().all(|branch| {
            branch.request().executor() == &ScreenPublicationExecutorRequest::Cpu
        })
    );
    assert_eq!(
        branch_extent(&snapshot.screen_branches[0]),
        extent(640, 480)
    );
    assert_eq!(
        branch_extent(&snapshot.screen_branches[1]),
        extent(5_120, 720)
    );
    assert_eq!(snapshot.requested_hz(SourceKind::Audio), 60);
}

#[test]
fn incompatible_screen_demands_remain_exact_and_never_form_an_envelope() {
    let demands = InputPublicationDemandHandle::new();
    let ultrawide = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(5_120, 720)),
    );
    let portrait = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default().with_screen(144, extent(1_920, 2_160)),
    );

    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 144);
    assert_eq!(
        snapshot.compatibility_screen_extent,
        Some(extent(5_120, 720))
    );
    assert_eq!(snapshot.screen_branches.len(), 2);
    assert_eq!(
        snapshot
            .screen_branches
            .iter()
            .map(branch_extent)
            .collect::<Vec<_>>(),
        [extent(5_120, 720), extent(1_920, 2_160)]
    );
    assert!(
        !snapshot
            .screen_branches
            .iter()
            .any(|branch| branch_extent(branch) == extent(5_120, 2_160))
    );

    drop(ultrawide);
    let shrunk = demands.snapshot();
    assert_eq!(shrunk.requested_hz(SourceKind::Screen), 144);
    assert_eq!(
        shrunk.compatibility_screen_extent,
        Some(extent(1_920, 2_160))
    );
    assert_eq!(shrunk.screen_branches.len(), 1);
    drop(portrait);
    assert_eq!(demands.snapshot().compatibility_screen_extent, None);
}

#[test]
fn demand_revision_advances_on_register_update_and_release() {
    let demands = InputPublicationDemandHandle::new();
    let initial = demands.snapshot().revision();
    let registration = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default().with_screen(60, extent(1_280, 720)),
    );
    let registered = demands.snapshot().revision();
    assert!(registered > initial);

    registration.update(InputPublicationDemand::default().with_screen(120, extent(3_840, 2_160)));
    let updated = demands.snapshot().revision();
    assert!(updated > registered);

    drop(registration);
    let released = demands.snapshot().revision();
    assert!(released > updated);
}

#[test]
fn exact_screen_failure_retries_retirement_immediately_but_backs_off_when_empty() {
    let now = Instant::now();
    assert_eq!(
        exact_screen_failure_retry_at(ExactScreenTransitionPurpose::ApplyDemand, false, now),
        now
    );
    assert_eq!(
        exact_screen_failure_retry_at(ExactScreenTransitionPurpose::ApplyDemand, true, now),
        now + EXACT_PLAN_UNAVAILABLE_RETRY_INTERVAL
    );
    assert_eq!(
        exact_screen_failure_retry_at(ExactScreenTransitionPurpose::RetireForRetry, false, now),
        now + EXACT_PLAN_UNAVAILABLE_RETRY_INTERVAL
    );
}

#[tokio::test]
async fn demand_revision_wakes_independent_coordinators() {
    let demands = InputPublicationDemandHandle::new();
    let mut pump_revision = demands.subscribe_revision();
    let mut plan_revision = demands.subscribe_revision();
    let _registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(1_920, 1_080)),
    );

    pump_revision
        .changed()
        .await
        .expect("pump revision watch remains open");
    plan_revision
        .changed()
        .await
        .expect("plan revision watch remains open");
    assert_eq!(*pump_revision.borrow(), demands.revision());
    assert_eq!(*plan_revision.borrow(), demands.revision());
}

#[test]
fn delayed_revision_publication_cannot_regress_or_repeat_watch_state() {
    let demands = InputPublicationDemandHandle::new();
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(1_920, 1_080)),
    );
    let stale_revision = demands.revision();
    registration.update(InputPublicationDemand::default().with_screen(120, extent(3_840, 2_160)));
    let current_revision = demands.revision();
    let mut pump_revision = demands.subscribe_revision();
    let mut plan_revision = demands.subscribe_revision();
    pump_revision.borrow_and_update();
    plan_revision.borrow_and_update();

    demands.registry.publish_revision(stale_revision);
    demands.registry.publish_revision(current_revision);

    assert_eq!(demands.revision(), current_revision);
    assert_eq!(*pump_revision.borrow(), current_revision);
    assert_eq!(*plan_revision.borrow(), current_revision);
    assert!(
        !pump_revision
            .has_changed()
            .expect("revision watch remains open")
    );
    assert!(
        !plan_revision
            .has_changed()
            .expect("revision watch remains open")
    );
}

#[test]
fn identical_updates_preserve_revision_and_older_snapshots() {
    let demands = InputPublicationDemandHandle::new();
    let initial_demand = InputPublicationDemand::default().with_screen(60, extent(1_280, 720));
    let registration = demands.register(InputPublicationConsumer::Preview, initial_demand.clone());
    let held = demands.snapshot();

    registration.update(initial_demand);
    let unchanged = demands.snapshot();
    assert_eq!(unchanged.revision(), held.revision());
    assert!(Arc::ptr_eq(&unchanged, &held));

    registration.update(InputPublicationDemand::default().with_screen(120, extent(3_840, 2_160)));
    let updated = demands.snapshot();
    assert!(updated.revision() > held.revision());
    assert_eq!(branch_extent(&held.screen_branches[0]), extent(1_280, 720));
    assert_eq!(
        branch_extent(&updated.screen_branches[0]),
        extent(3_840, 2_160)
    );
}

#[test]
fn one_registration_preserves_every_screen_request_shape() {
    let demands = InputPublicationDemandHandle::new();
    let native = test_screen_branch(
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        30,
        extent(1_920, 1_080),
    );
    let width_only = test_screen_branch(
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(NonZeroU32::new(5_120), None, ScreenUpscalePolicy::Never),
        60,
        extent(5_120, 720),
    );
    let upscaled = test_screen_branch(
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            NonZeroU32::new(7_680),
            NonZeroU32::new(4_320),
            ScreenUpscalePolicy::Allow,
        ),
        120,
        extent(7_680, 4_320),
    );
    let zones = test_screen_branch(
        ScreenPublicationKind::Zones {
            columns: NonZeroU32::new(32).expect("test grid columns are non-zero"),
            rows: NonZeroU32::new(18).expect("test grid rows are non-zero"),
        },
        ScreenExtentRequest::bounded(
            NonZeroU32::new(1_920),
            NonZeroU32::new(1_080),
            ScreenUpscalePolicy::Never,
        ),
        144,
        extent(1_920, 1_080),
    );
    let _registration = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default()
            .with_screen_branches([native, width_only, upscaled, zones]),
    );

    let branches = demands.screen_branches();
    assert_eq!(branches.len(), 4);
    assert_eq!(branches[0].request().extent(), ScreenExtentRequest::Native);
    assert_eq!(
        branches[1].request().extent().bounded_extent(),
        ScreenExtentRequest::bounded(NonZeroU32::new(5_120), None, ScreenUpscalePolicy::Never,)
            .bounded_extent()
    );
    assert_eq!(
        branches[2].request().extent().bounded_extent(),
        ScreenExtentRequest::bounded(
            NonZeroU32::new(7_680),
            NonZeroU32::new(4_320),
            ScreenUpscalePolicy::Allow,
        )
        .bounded_extent()
    );
    assert!(matches!(
        branches[3].request().kind(),
        ScreenPublicationKind::Zones { columns, rows }
            if columns.get() == 32 && rows.get() == 18
    ));
    assert_eq!(demands.requested_hz(SourceKind::Screen), 144);
    assert_eq!(
        demands.snapshot().compatibility_screen_extent,
        Some(extent(7_680, 4_320))
    );
    let exact = demands.snapshot().exact_screen_demand(91);
    assert_eq!(exact.graph_generation().get(), 91);
    assert_eq!(exact.branches().len(), 4);
    assert!(
        exact
            .compatibility_surface()
            .expect("a Surface compatibility branch is selected")
            .request()
            .extent()
            .bounded_extent()
            .is_some()
    );
    assert!(matches!(
        exact
            .compatibility_zones()
            .expect("a Zones compatibility branch is selected")
            .request()
            .kind(),
        ScreenPublicationKind::Zones { .. }
    ));
}

#[test]
fn concurrent_preview_registrations_union_and_drop_independently() {
    let demands = InputPublicationDemandHandle::new();
    let first = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default().with_source(SourceKind::Interaction, 120),
    );
    let second = demands.register(
        InputPublicationConsumer::Preview,
        InputPublicationDemand::default()
            .with_source(SourceKind::Interaction, 240)
            .with_screen(90, extent(1_920, 1_080)),
    );

    assert_eq!(
        demands.registration_count(InputPublicationConsumer::Preview),
        2
    );
    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 240);
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 90);

    drop(second);
    assert_eq!(
        demands.registration_count(InputPublicationConsumer::Preview),
        1
    );
    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 120);
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 0);
    drop(first);
    assert_eq!(demands.snapshot().max_requested_hz(), 0);
}

#[test]
fn authoritative_registration_does_not_blanket_unrequested_source_types() {
    let demands = InputPublicationDemandHandle::new();
    let _authoritative = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(640, 480)),
    );

    let snapshot = demands.snapshot();
    assert_eq!(snapshot.requested_hz(SourceKind::Screen), 60);
    assert_eq!(snapshot.requested_hz(SourceKind::Audio), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Interaction), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Media), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Network), 0);
    assert_eq!(snapshot.requested_hz(SourceKind::Sensors), 0);
}

#[test]
fn cadence_conversion_does_not_cap_large_requests() {
    assert_eq!(cadence_interval(u32::MAX), Duration::from_nanos(1));
    assert_eq!(cadence_interval(240), Duration::from_nanos(4_166_667));
}

#[test]
fn source_cadences_advance_independently_without_catch_up_bursts() {
    let started_at = Instant::now();
    let mut schedule = InputPublicationSchedule::default();
    let demand = InputPublicationCadence::default()
        .with_source(SourceKind::Screen, 20)
        .with_source(SourceKind::Interaction, 120);
    let mut due = Vec::with_capacity(5);

    schedule.synchronize(&demand, started_at);
    schedule.collect_due(started_at, &mut due);
    assert_eq!(
        due.iter().map(|(source, _)| *source).collect::<Vec<_>>(),
        [SourceKind::Screen, SourceKind::Interaction]
    );

    let interaction_tick = started_at + cadence_interval(120);
    schedule.collect_due(interaction_tick, &mut due);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0, SourceKind::Interaction);

    let delayed_tick = started_at + Duration::from_millis(75);
    schedule.collect_due(delayed_tick, &mut due);
    assert_eq!(
        due.iter().map(|(source, _)| *source).collect::<Vec<_>>(),
        [SourceKind::Screen, SourceKind::Interaction]
    );
    assert!(due[0].1 > due[1].1);

    schedule.collect_due(delayed_tick, &mut due);
    assert!(due.is_empty(), "missed intervals must coalesce, not burst");
}

#[test]
fn source_reactivation_starts_with_one_cadence_window() {
    let started_at = Instant::now();
    let interval = cadence_interval(20);
    let mut schedule = InputPublicationSchedule::default();
    let active = InputPublicationCadence::default().with_source(SourceKind::Screen, 20);
    let mut due = Vec::with_capacity(5);

    schedule.synchronize(&active, started_at);
    schedule.collect_due(started_at, &mut due);
    assert_eq!(due, [(SourceKind::Screen, interval.as_secs_f32())]);

    schedule.synchronize(&InputPublicationCadence::default(), started_at + interval);
    let resumed_at = started_at + Duration::from_secs(30);
    schedule.synchronize(&active, resumed_at);
    schedule.collect_due(resumed_at, &mut due);

    assert_eq!(due, [(SourceKind::Screen, interval.as_secs_f32())]);
}

#[tokio::test]
async fn pump_waits_for_live_demand_then_samples_without_render_frames() {
    let samples = Arc::new(AtomicUsize::new(0));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CountingSource::new(Arc::clone(&samples)),
        )))
        .expect("counting source should register");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let _demand = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::all_sources(60, extent(640, 480)),
    );
    let mut pump = InputPublicationPump::start(manager.clone(), demands)
        .await
        .expect("publication pump should start");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(samples.load(Ordering::Relaxed), 0);

    manager.start_all().expect("counting source should start");
    tokio::time::timeout(Duration::from_millis(500), async {
        while samples.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pump should sample a newly live source");

    assert_eq!(pump.monitor().status(), InputPublicationStatus::Ready);
    pump.shutdown()
        .await
        .expect("publication pump should stop cleanly");
    assert_eq!(pump.monitor().status(), InputPublicationStatus::Stopped);
}

#[tokio::test]
async fn dropping_the_pump_aborts_its_worker() {
    let samples = Arc::new(AtomicUsize::new(0));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CountingSource::new(Arc::clone(&samples)),
        )))
        .expect("counting source should register");
    manager.start_all().expect("counting source should start");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let _demand = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::all_sources(120, extent(640, 480)),
    );
    let pump = InputPublicationPump::start(manager, demands)
        .await
        .expect("publication pump should start");
    tokio::time::timeout(Duration::from_millis(500), async {
        while samples.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pump should publish before being dropped");

    drop(pump);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let after_drop = samples.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(samples.load(Ordering::Relaxed), after_drop);
}

#[tokio::test]
async fn pump_sleeps_with_zero_typed_demand() {
    let samples = Arc::new(AtomicUsize::new(0));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CountingSource::new(Arc::clone(&samples)),
        )))
        .expect("counting source should register");
    manager.start_all().expect("counting source should start");
    let mut pump = InputPublicationPump::start(manager, InputPublicationDemandHandle::new())
        .await
        .expect("publication pump should start");

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(samples.load(Ordering::Relaxed), 0);
    pump.shutdown()
        .await
        .expect("publication pump should stop cleanly");
}

#[tokio::test]
async fn zero_demand_graph_change_shuts_down_new_source() {
    let manager = InputManager::new();
    let mut pump =
        InputPublicationPump::start(manager.clone(), InputPublicationDemandHandle::new())
            .await
            .expect("publication pump should start");
    tokio::time::sleep(Duration::from_millis(25)).await;
    let capture_active = Arc::new(AtomicBool::new(true));
    let mut source = CountingSource::with_capture_active(
        Arc::new(AtomicUsize::new(0)),
        Arc::clone(&capture_active),
    );
    source.start().expect("counting source should start");

    manager
        .add_source(ManagedSourceRole::interaction(Box::new(source)))
        .expect("prestarted counting source should register");

    tokio::time::timeout(Duration::from_millis(500), async {
        while capture_active.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("zero aggregate demand should stop a newly registered source");
    pump.shutdown()
        .await
        .expect("publication pump should stop cleanly");
}

#[tokio::test]
async fn aggregate_demand_owns_interaction_lifecycle_and_cadence() {
    let samples = Arc::new(AtomicUsize::new(0));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CountingSource::new(Arc::clone(&samples)),
        )))
        .expect("counting source should register");
    manager.start_all().expect("counting source should start");
    let graph = manager.input_graph_handle();
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump should start");

    wait_for_interaction_demand(&graph, false).await;
    let lease = demands.register(
        InputPublicationConsumer::PassiveStream,
        InputPublicationDemand::default().with_source(SourceKind::Interaction, 120),
    );
    wait_for_interaction_demand(&graph, true).await;
    tokio::time::timeout(Duration::from_millis(500), async {
        while samples.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("active passive demand should drive publication cadence");

    drop(lease);
    wait_for_interaction_demand(&graph, false).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let after_release = samples.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(samples.load(Ordering::Relaxed), after_release);

    pump.shutdown()
        .await
        .expect("publication pump should stop cleanly");
}

#[tokio::test]
async fn pump_propagates_screen_extent_changes_even_while_cadence_stays_active() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(
            ScreenDemandSource::new(Arc::clone(&transitions)),
        )))
        .expect("screen demand source should register");
    manager.start_all().expect("screen source starts");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let large = ScreenCaptureDemand::active(extent(5_120, 2_160));
    let small = ScreenCaptureDemand::active(extent(1_280, 720));
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(5_120, 2_160)),
    );
    wait_for_screen_demand(&transitions, large).await;

    registration.update(InputPublicationDemand::default().with_screen(60, extent(1_280, 720)));
    wait_for_screen_demand(&transitions, small).await;

    drop(registration);
    wait_for_screen_demand(&transitions, ScreenCaptureDemand::Inactive).await;
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn pump_propagates_exact_branches_with_revision_and_graph_fences() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(
            ScreenDemandSource::new(Arc::clone(&transitions)),
        )))
        .expect("screen demand source should register");
    manager.start_all().expect("screen source starts");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let publications = pump.reader().screen_publications();
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(144, extent(7_680, 4_320)),
    );

    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let committed = publications.committed_state();
            if committed.branch_count() == 1
                && committed.plan().demand_revision() == demands.revision()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact screen plan should commit");

    let committed = publications.committed_state();
    assert_eq!(committed.plan().branches()[0].requested_hz().get(), 144);
    assert_eq!(
        committed.plan().branches()[0]
            .descriptor()
            .geometry()
            .output_extent(),
        extent(7_680, 4_320)
    );
    assert!(manager.source_graph_generation() > 0);

    drop(registration);
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if publications.committed_state().branch_count() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("empty exact demand should retire committed branches");
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn reader_exposes_exact_cpu_surface_without_legacy_screen_data() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let source = ScreenDemandSource::new(Arc::clone(&transitions));
    let runtime = Arc::clone(&source.runtime);
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(source)))
        .expect("screen demand source should register");
    manager.start_all().expect("screen source starts");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let reader = pump.reader();
    let publications = reader.screen_publications();
    let output_extent = extent(16, 9);
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, output_extent),
    );

    wait_for_exact_extent(&publications, output_extent).await;
    let committed = publications.committed_state();
    let descriptor = committed.plan().branches()[0].descriptor().clone();
    let binding = runtime
        .lock()
        .expect("screen runtime lock is healthy")
        .last()
        .expect("exact runtime allocation exists")
        .binding
        .clone();
    let publisher = publications
        .publisher(&descriptor, &binding)
        .expect("committed branch issues a publisher");
    let pixels = vec![37_u8; 16 * 9 * 4];
    let now = Instant::now();
    let metadata = ScreenPublicationMetadata::try_new(
        descriptor.source_epoch().clone(),
        binding.plan_generation(),
        NonZeroU64::MIN,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    )
    .expect("test publication timeline is valid");
    let payload = ScreenBranchPayload::Surface(
        ScreenSurfacePayload::try_new(
            output_extent,
            descriptor.physical().target_pixel_format(),
            ScreenPublicationColorimetry::new(descriptor.physical().color_pipeline().output()),
            &pixels,
        )
        .expect("test payload matches its descriptor"),
    );
    publications
        .publish(&publisher, payload, &metadata)
        .expect("exact CPU surface publishes");

    let (generation, lease) = reader.screen_observation(None, output_extent);
    assert_eq!(generation, committed.plan().generation());
    let publication = lease
        .expect("CPU route has a lease")
        .read()
        .expect("CPU route reads its publication");
    let frame = ProducerFrame::screen_publication(publication)
        .expect("RGBA exact surface is a producer frame");
    assert_eq!((frame.width(), frame.height()), (16, 9));
    #[cfg(feature = "wgpu")]
    assert_eq!(frame.cpu_rgba_bytes(), Some(pixels.as_slice()));
    let (canvas, retained_surface) = frame
        .into_cpu_render_frame()
        .expect("CPU backend materializes exact publication");
    assert!(retained_surface.is_none());
    assert_eq!(canvas.as_rgba_bytes(), pixels);

    drop(registration);
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn failed_exact_replacement_preserves_retirement_barrier_across_demand_changes() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let preparation_failures = Arc::new(AtomicUsize::new(0));
    let retirement_started = Arc::new(Notify::new());
    let retirement_release = Arc::new(Notify::new());
    let source = ScreenDemandSource::new(Arc::clone(&transitions)).with_recovery_gates(
        Arc::clone(&preparation_failures),
        Arc::clone(&retirement_started),
        Arc::clone(&retirement_release),
    );
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(source)))
        .expect("screen demand source should register");
    manager.start_all().expect("screen source starts");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let publications = pump.reader().screen_publications();
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(1_920, 1_080)),
    );

    wait_for_exact_extent(&publications, extent(1_920, 1_080)).await;
    preparation_failures.store(1, Ordering::Release);
    registration.update(InputPublicationDemand::default().with_screen(60, extent(3_840, 2_160)));

    tokio::time::timeout(Duration::from_millis(500), retirement_started.notified())
        .await
        .expect("failed replacement should commit retirement");
    assert_eq!(publications.committed_state().branch_count(), 0);
    assert_eq!(
        publications.committed_state().plan().demand_revision(),
        demands.revision()
    );
    assert_eq!(
        transitions
            .lock()
            .expect("screen demand transitions remain readable")
            .last(),
        Some(&ScreenCaptureDemand::active(extent(3_840, 2_160)))
    );
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(publications.committed_state().branch_count(), 0);

    registration.update(InputPublicationDemand::default().with_screen(60, extent(5_120, 2_160)));
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(publications.committed_state().branch_count(), 0);

    retirement_release.notify_one();
    wait_for_exact_extent(&publications, extent(5_120, 2_160)).await;

    drop(registration);
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn persistent_exact_failure_uses_bounded_retry_cadence_after_retirement() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let preparation_failures = Arc::new(AtomicUsize::new(3));
    let source = ScreenDemandSource::new(Arc::clone(&transitions)).with_recovery_gates(
        Arc::clone(&preparation_failures),
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
    );
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(source)))
        .expect("screen demand source should register");
    manager.start_all().expect("screen source starts");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let _registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(1_920, 1_080)),
    );

    tokio::time::timeout(Duration::from_millis(500), async {
        while preparation_failures.load(Ordering::Acquire) == 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first exact preparation should fail");
    tokio::time::sleep(LIFECYCLE_PROBE_INTERVAL / 2).await;
    assert_eq!(preparation_failures.load(Ordering::Acquire), 2);

    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn pump_samples_unrelated_sources_while_exact_workers_prepare() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let samples = Arc::new(AtomicUsize::new(0));
    let preparation_started = Arc::new(Notify::new());
    let preparation_release = Arc::new(Notify::new());
    let source = ScreenDemandSource::new(Arc::clone(&transitions)).with_preparation_gate(
        Arc::clone(&preparation_started),
        Arc::clone(&preparation_release),
    );
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(source)))
        .expect("screen demand source should register");
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CountingSource::new(Arc::clone(&samples)),
        )))
        .expect("counting source should register");
    manager.start_all().expect("input sources start");
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let publications = pump.reader().screen_publications();
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default()
            .with_screen(144, extent(7_680, 4_320))
            .with_source(SourceKind::Interaction, 120),
    );

    tokio::time::timeout(Duration::from_millis(500), preparation_started.notified())
        .await
        .expect("worker preparation should begin");
    assert!(manager.source_graph_generation() > 0);
    tokio::time::timeout(Duration::from_millis(500), async {
        while samples.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("gated screen preparation must not stall unrelated input sampling");
    preparation_release.notify_one();

    tokio::time::timeout(Duration::from_millis(500), async {
        while publications.committed_state().branch_count() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("released worker preparation should commit");

    drop(registration);
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_screen_busy_commit_retains_one_preparation_until_lifecycle_release() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let preparation_started = Arc::new(Notify::new());
    let preparation_release = Arc::new(Notify::new());
    let preparation_attempts = Arc::new(AtomicUsize::new(0));
    let armed = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let source = ScreenDemandSource::new(Arc::clone(&transitions))
        .with_preparation_gate(
            Arc::clone(&preparation_started),
            Arc::clone(&preparation_release),
        )
        .with_preparation_attempts(Arc::clone(&preparation_attempts));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(source)))
        .expect("screen demand source should register");
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            BlockingCapabilityInteractionSource {
                armed: Arc::clone(&armed),
                entered: entered_tx,
                release: release_rx,
            },
        )))
        .expect("blocking capability source should register");
    manager.start_all().expect("input sources start");
    let demands = InputPublicationDemandHandle::new();
    let commit_pause = demands.pause_next_exact_screen_commit();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let publications = pump.reader().screen_publications();
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(1_920, 1_080)),
    );

    tokio::time::timeout(Duration::from_millis(500), preparation_started.notified())
        .await
        .expect("worker preparation should begin once");
    assert_eq!(preparation_attempts.load(Ordering::Acquire), 1);
    armed.store(true, Ordering::Release);
    let blocker = {
        let manager = manager.clone();
        std::thread::spawn(move || manager.set_source_capability_identity("held-owner", None, None))
    };
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("capability update should own lifecycle state");
    preparation_release.notify_one();
    commit_pause.wait_until_reached().await;
    commit_pause.release();

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(preparation_attempts.load(Ordering::Acquire), 1);
    assert_eq!(publications.committed_state().branch_count(), 0);

    release_tx
        .send(())
        .expect("capability update should release lifecycle state");
    blocker
        .join()
        .expect("capability thread should finish")
        .expect("capability update should succeed");
    wait_for_exact_extent(&publications, extent(1_920, 1_080)).await;
    assert_eq!(preparation_attempts.load(Ordering::Acquire), 1);

    drop(registration);
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn demand_revision_fence_rejects_update_after_final_snapshot_check() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(
            ScreenDemandSource::new(Arc::clone(&transitions)),
        )))
        .expect("screen demand source should register");
    manager.start_all().expect("screen source starts");
    let reader = InputPublicationReader::new(
        manager.input_graph_handle(),
        manager.screen_publication_hub(),
    );
    let publications = reader.screen_publications();
    let graph_generation = reader.graph_snapshot().generation();
    let manager = manager;
    let demands = InputPublicationDemandHandle::new();
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(7_680, 4_320)),
    );
    let stale = demands.snapshot().exact_screen_demand(graph_generation);
    let stale_revision = stale.revision();
    let commit_pause = demands.pause_next_exact_screen_commit();
    let transition = tokio::spawn(run_exact_screen_transition(
        manager.clone(),
        reader,
        demands.clone(),
        stale,
    ));

    tokio::time::timeout(
        Duration::from_millis(500),
        commit_pause.wait_until_reached(),
    )
    .await
    .expect("stale transition should reach its final commit boundary");
    registration.update(InputPublicationDemand::default().with_screen(60, extent(1_280, 720)));
    assert!(demands.revision() > stale_revision);
    assert_eq!(publications.committed_state().branch_count(), 0);
    commit_pause.release();

    let committed = transition
        .await
        .expect("exact transition task should not panic")
        .expect("stale transition should abort cleanly");
    assert!(matches!(
        committed,
        ExactScreenTransitionOutcome::Completed(None)
    ));
    assert_eq!(publications.committed_state().branch_count(), 0);
}

#[tokio::test]
async fn pump_rejects_a_superseded_demand_while_lifecycle_is_busy() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(
            ScreenDemandSource::new(Arc::clone(&transitions)),
        )))
        .expect("screen demand source should register");
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            BlockingStartInteractionSource {
                entered: entered_tx,
                release: release_rx,
                running: false,
            },
        )))
        .expect("blocking interaction source should register");
    let starter = {
        let manager = manager.clone();
        std::thread::spawn(move || manager.start_all())
    };
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("startup should own lifecycle state");
    let demands = InputPublicationDemandHandle::new();
    let mut pump = InputPublicationPump::start(manager.clone(), demands.clone())
        .await
        .expect("publication pump starts");
    let stale_extent = extent(7_680, 4_320);
    let current_extent = extent(1_280, 720);
    let registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, stale_extent),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        transitions
            .lock()
            .expect("screen transitions lock")
            .is_empty(),
        "busy capture reconciliation must wait without applying demand"
    );
    registration.update(InputPublicationDemand::default().with_screen(60, current_extent));
    release_tx.send(()).expect("startup should resume");
    starter
        .join()
        .expect("startup thread should finish")
        .expect("sources should start");

    wait_for_screen_demand(&transitions, ScreenCaptureDemand::active(current_extent)).await;
    assert!(
        !transitions
            .lock()
            .expect("screen transitions lock")
            .contains(&ScreenCaptureDemand::active(stale_extent))
    );

    drop(registration);
    wait_for_screen_demand(&transitions, ScreenCaptureDemand::Inactive).await;
    pump.shutdown().await.expect("publication pump stops");
}

#[tokio::test]
async fn pump_cancellation_while_lifecycle_is_busy_prevents_late_mutation() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(
            ScreenDemandSource::new(Arc::clone(&transitions)),
        )))
        .expect("screen demand source should register");
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            BlockingStartInteractionSource {
                entered: entered_tx,
                release: release_rx,
                running: false,
            },
        )))
        .expect("blocking interaction source should register");
    let starter = {
        let manager = manager.clone();
        std::thread::spawn(move || manager.start_all())
    };
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("startup should own lifecycle state");
    let demands = InputPublicationDemandHandle::new();
    let _registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default().with_screen(60, extent(1_280, 720)),
    );
    let mut pump = InputPublicationPump::start(manager, demands)
        .await
        .expect("publication pump should start without lifecycle ownership");
    tokio::time::sleep(Duration::from_millis(20)).await;

    tokio::time::timeout(Duration::from_millis(500), pump.shutdown())
        .await
        .expect("pump cancellation must not wait for lifecycle ownership")
        .expect("publication pump should stop");
    release_tx.send(()).expect("startup should resume");
    starter
        .join()
        .expect("startup thread should finish")
        .expect("sources should start");
    tokio::task::yield_now().await;

    assert!(
        transitions
            .lock()
            .expect("screen transitions lock")
            .is_empty(),
        "a cancelled pump must not apply demand after lifecycle release"
    );
}

#[tokio::test]
async fn pump_shutdown_releases_active_capture_demand() {
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let capture_active = Arc::new(AtomicBool::new(false));
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(
            ScreenDemandSource::new(Arc::clone(&transitions)),
        )))
        .expect("screen demand source should register");
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CountingSource::with_capture_active(
                Arc::new(AtomicUsize::new(0)),
                Arc::clone(&capture_active),
            ),
        )))
        .expect("counting source should register");
    manager.start_all().expect("input sources should start");
    let demands = InputPublicationDemandHandle::new();
    let _registration = demands.register(
        InputPublicationConsumer::Authoritative,
        InputPublicationDemand::default()
            .with_screen(60, extent(1_280, 720))
            .with_source(SourceKind::Interaction, 120),
    );
    let mut pump = InputPublicationPump::start(manager, demands)
        .await
        .expect("publication pump should start");

    wait_for_screen_demand(
        &transitions,
        ScreenCaptureDemand::active(extent(1_280, 720)),
    )
    .await;
    tokio::time::timeout(Duration::from_millis(500), async {
        while !capture_active.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("interaction capture should become active");

    pump.shutdown().await.expect("publication pump should stop");

    assert_eq!(
        transitions
            .lock()
            .expect("screen demand transition lock")
            .last()
            .copied(),
        Some(ScreenCaptureDemand::Inactive)
    );
    assert!(!capture_active.load(Ordering::Acquire));
}

async fn wait_for_screen_demand(
    transitions: &StdMutex<Vec<ScreenCaptureDemand>>,
    expected: ScreenCaptureDemand,
) {
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if transitions
                .lock()
                .expect("screen demand transition lock")
                .last()
                .copied()
                == Some(expected)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("screen demand transition should propagate");
}

async fn wait_for_exact_extent(publications: &ScreenPublicationHub, expected: PixelExtent) {
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let committed = publications.committed_state();
            if committed.branch_count() == 1
                && committed.plan().branches()[0]
                    .descriptor()
                    .geometry()
                    .output_extent()
                    == expected
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact screen extent should commit");
}

async fn wait_for_interaction_demand(
    graph: &hypercolor_core::input::InputGraphHandle,
    expected: bool,
) {
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let snapshot = graph.snapshot();
            let demanded = snapshot
                .slots()
                .first()
                .expect("counting source remains registered")
                .status()
                .snapshot()
                .demanded;
            if demanded == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capture lifecycle should follow aggregate demand");
}

fn branch_extent(
    demand: &hypercolor_core::input::screen::RegisteredScreenBranchDemand,
) -> PixelExtent {
    let ScreenExtentRequest::Bounded(bounds) = demand.request().extent() else {
        panic!("compatibility surface demand uses explicit bounds");
    };
    PixelExtent::new(
        bounds
            .max_width()
            .expect("compatibility surface has width")
            .get(),
        bounds
            .max_height()
            .expect("compatibility surface has height")
            .get(),
    )
    .expect("compatibility bounds are non-empty")
}

fn test_screen_branch(
    kind: ScreenPublicationKind,
    extent_request: ScreenExtentRequest,
    requested_hz: u32,
    legacy_extent: PixelExtent,
) -> InputScreenBranchDemand {
    let request = ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        kind,
        ScreenPublicationExecutorRequest::Cpu,
        extent_request,
        ScreenAspectPolicy::Contain,
        Arc::new(ScreenProcessingProfile::default()),
    );
    InputScreenBranchDemand::new(
        RegisteredScreenBranchDemand::new(
            request,
            NonZeroU32::new(requested_hz).expect("test cadence is non-zero"),
        ),
        legacy_extent,
    )
}
