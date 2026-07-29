use std::num::{NonZeroU32, NonZeroU64};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CommittedScreenPlan, PhysicalOrigin, PixelExtent, PlatformGpuApi,
    PlatformGpuSurface, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenPublicationDescriptor, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenBranchPayload, ScreenBranchPublisher, ScreenCaptureBackend, ScreenCapturePlan,
    ScreenColorTransformCapabilities, ScreenCursorCapabilities, ScreenExactResource,
    ScreenExactResourceLedger, ScreenExecutorColorCapabilities, ScreenExtentRequest,
    ScreenGpuSurfacePayload, ScreenInputGraphGeneration, ScreenLiveBranchReceipt,
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativePreparationPayload,
    ScreenNativeTargetBindingError, ScreenNativeTargetPreparation, ScreenNativeTargetResourceError,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanBuilder, ScreenProcessingProfile,
    ScreenPublicationColorimetry, ScreenPublicationExecutor, ScreenPublicationExecutorRequest,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError,
    ScreenPublicationKind, ScreenPublicationMetadata, ScreenPublicationRequest,
    ScreenPublicationSlotPolicy, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerBinding, SourceScale,
};

#[path = "support/native_target.rs"]
mod native_target_support;

const CAPTURE_PLAN_RETAINED_BYTES: u64 = 37;

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test values are non-zero")
}

fn extent() -> PixelExtent {
    PixelExtent::new(2, 2).expect("test extent is non-empty")
}

fn gpu_device() -> ScreenPhysicalGpuDeviceIdentity {
    ScreenPhysicalGpuDeviceIdentity::Direct3dAdapterLuid {
        low_part: 1,
        high_part: 1,
    }
}

fn native_target() -> ScreenNativeExecutionTarget {
    native_target_with(1, native_target_support::preparer())
}

fn native_target_with(
    id: u64,
    preparer: Arc<dyn hypercolor_core::input::screen::ScreenNativeTargetPreparer>,
) -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        ScreenNativeExecutionTargetId::new(
            NonZeroU64::new(id).expect("test target identity is non-zero"),
        ),
        PlatformGpuApi::Direct3d11,
        gpu_device(),
        non_zero(16_384),
        preparer,
    )
}

fn source_id() -> CaptureSourceId {
    CaptureSourceId::new("gpu-reclamation-display").expect("test source id is non-empty")
}

fn source() -> ResolvedScreenSource {
    let extent = extent();
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source_id(),
            topology_generation: 1,
            session_generation: 1,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenCursorCapabilities::clean_with_separate_cursor(),
            ScreenBackendResourceIdentity::new_with_physical_gpu_device(
                ScreenCaptureBackend::WindowsDesktopDuplication,
                ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
                gpu_device(),
                1,
                1,
            ),
        ),
    )
}

fn demand_for_target(
    source: &ResolvedScreenSource,
    target: ScreenNativeExecutionTarget,
) -> ResolvedScreenBranchDemand {
    demand_for_target_extent(source, target, ScreenExtentRequest::Native)
}

fn demand_for_target_extent(
    source: &ResolvedScreenSource,
    target: ScreenNativeExecutionTarget,
    requested_extent: ScreenExtentRequest,
) -> ResolvedScreenBranchDemand {
    let registered = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::SourceNative(target),
            requested_extent,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        non_zero(60),
    );
    let capabilities = ScreenColorTransformCapabilities::new(
        true,
        true,
        true,
        registered
            .request()
            .processing_profile()
            .algorithm_revision(),
    );
    registered
        .resolve_with_executor_capabilities(
            source,
            ScreenExecutorColorCapabilities::new(capabilities, capabilities),
        )
        .expect("GPU test demand resolves")
}

fn demand(source: &ResolvedScreenSource) -> ResolvedScreenBranchDemand {
    demand_for_target(source, native_target())
}

fn exact_ledger(
    ticket: &hypercolor_core::input::screen::ScreenWorkerPreparationTicket,
) -> anyhow::Result<(ScreenExactResourceLedger, Vec<ScreenResourceLifetime>)> {
    let mut resources = ticket
        .required_minimums()
        .iter()
        .map(|minimum| {
            ScreenExactResource::try_new(
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ticket
        .required_minimums()
        .iter()
        .any(|minimum| minimum.name().as_ref() == "worker-runtime-total")
    {
        let descriptor = ticket
            .candidate_plan()
            .branches()
            .iter()
            .map(hypercolor_core::input::screen::ScreenBranchDemand::descriptor)
            .find(|descriptor| {
                matches!(
                    descriptor.executor(),
                    ScreenPublicationExecutor::SourceNative(_)
                )
            })
            .expect("native fixture candidate has a native descriptor");
        let ScreenPublicationExecutor::SourceNative(target) = descriptor.executor() else {
            unreachable!("native descriptor selected above")
        };
        let preparation = target.prepare(
            descriptor,
            &ScreenNativePreparationPayload::new(
                descriptor,
                ticket.plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
        )?;
        resources.push(preparation.exact_resource("native-target-test", "worker-runtime-total")?);
        resources.push(ScreenExactResource::try_new_scoped(
            "capture-plan-test",
            "worker-runtime-total",
            ScreenResourceKind::WorkerAdditional,
            CAPTURE_PLAN_RETAINED_BYTES,
        )?);
    }
    let lifetimes = resources
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((ScreenExactResourceLedger::try_new(resources)?, lifetimes))
}

fn commit_candidate(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> (CommittedScreenPlan, Vec<Vec<ScreenResourceLifetime>>) {
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision advances");
    let mut preparing = builder
        .prepare(
            demands,
            None,
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan prepares");
    let mut worker_lifetimes = Vec::new();
    for required_source in preparing.required_sources().to_vec() {
        let ticket = preparing
            .worker_ticket(&required_source)
            .expect("required source has a worker ticket");
        let (ledger, lifetimes) = exact_ledger(&ticket).expect("exact ledger is valid");
        let token = ticket
            .acknowledge(ledger, &lifetimes)
            .expect("worker resources satisfy the ticket");
        preparing
            .acknowledge(token)
            .expect("worker token belongs to the candidate");
        worker_lifetimes.push(lifetimes);
    }
    let armed = preparing
        .arm(
            builder.current().generation(),
            demand_revision,
            graph_generation,
        )
        .unwrap_or_else(|failure| panic!("test plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, demand_revision, graph_generation)
        .unwrap_or_else(|failure| panic!("test plan commits: {}", failure.error()));
    (committed, worker_lifetimes)
}

fn worker_ticket_for(
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> hypercolor_core::input::screen::ScreenWorkerPreparationTicket {
    let mut builder = ScreenPlanBuilder::new();
    let mut preparing = builder
        .prepare(
            demands,
            None,
            hypercolor_core::input::screen::InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("native binding test plan prepares");
    preparing
        .worker_ticket(&source_id())
        .expect("native binding test source owns a worker ticket")
}

fn commit_initial(
    builder: &mut ScreenPlanBuilder,
    demand: ResolvedScreenBranchDemand,
) -> (
    ScreenCapturePlan,
    ScreenResourceLifetime,
    ScreenResourceLifetime,
) {
    let (committed, worker_lifetimes) = commit_candidate(builder, [demand]);
    let worker_lifetimes = worker_lifetimes.into_iter().flatten().collect::<Vec<_>>();
    let target_lifetime = worker_lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "native-target-test")
        .cloned()
        .expect("initial native target has an exact lifetime");
    let capture_lifetime = worker_lifetimes
        .into_iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "capture-plan-test")
        .expect("initial capture plan has an exact lifetime");
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("empty predecessor retires immediately");
    (plan, target_lifetime, capture_lifetime)
}

fn binding(builder: &ScreenPlanBuilder) -> ScreenWorkerBinding {
    builder
        .committed_state()
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == &source_id())
        .cloned()
        .expect("committed GPU source has a worker binding")
}

fn metadata(
    descriptor: &ResolvedScreenPublicationDescriptor,
    publisher: &ScreenBranchPublisher,
    sequence: u64,
) -> ScreenPublicationMetadata {
    let now = Instant::now();
    ScreenPublicationMetadata::try_new(
        descriptor.source_epoch().clone(),
        publisher.plan_generation(),
        NonZeroU64::new(sequence).expect("test sequence is non-zero"),
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    )
    .expect("test timeline is valid")
}

fn gpu_surface(handle_id: u64) -> (PlatformGpuSurface, Weak<()>) {
    let owner = Arc::new(());
    let weak_owner = Arc::downgrade(&owner);
    (gpu_surface_with_owner(handle_id, owner), weak_owner)
}

fn gpu_surface_with_owner<T>(handle_id: u64, owner: Arc<T>) -> PlatformGpuSurface
where
    T: Send + Sync + 'static,
{
    PlatformGpuSurface::new(
        PlatformGpuApi::Direct3d11,
        handle_id,
        extent(),
        CapturePixelFormat::Rgba8,
        owner,
    )
    .expect("test GPU surface has a non-zero identity")
}

fn publish_gpu(
    fixture: &Fixture,
    publisher: &ScreenBranchPublisher,
    sequence: u64,
) -> (Weak<()>, ScreenLiveBranchReceipt) {
    let (surface, owner) = gpu_surface(sequence);
    let receipt = fixture
        .hub
        .publish(
            publisher,
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                fixture.colorimetry(),
                &surface,
            )),
            &metadata(&fixture.descriptor, publisher, sequence),
        )
        .expect("GPU surface publishes");
    drop(surface);
    (owner, receipt)
}

struct Fixture {
    builder: ScreenPlanBuilder,
    hub: Arc<ScreenPublicationHub>,
    descriptor: ResolvedScreenPublicationDescriptor,
    target_lifetime: Option<ScreenResourceLifetime>,
    capture_lifetime: Option<ScreenResourceLifetime>,
}

impl Fixture {
    fn new(slot_policy: ScreenPublicationSlotPolicy) -> Self {
        let source = source();
        let demand = demand(&source);
        let descriptor = demand.descriptor().clone();
        let mut builder = ScreenPlanBuilder::with_publication_slots(slot_policy);
        let hub = builder.publication_hub();
        let (_, target_lifetime, capture_lifetime) = commit_initial(&mut builder, demand);
        Self {
            builder,
            hub,
            descriptor,
            target_lifetime: Some(target_lifetime),
            capture_lifetime: Some(capture_lifetime),
        }
    }

    fn publisher(&self) -> ScreenBranchPublisher {
        let binding = binding(&self.builder);
        self.hub
            .publisher(&self.descriptor, &binding)
            .expect("GPU worker owns the branch")
    }

    fn colorimetry(&self) -> ScreenPublicationColorimetry {
        ScreenPublicationColorimetry::new(self.descriptor.physical().color_pipeline().output())
    }
}

struct ReentrantBlockingOwner {
    publisher: Arc<ScreenBranchPublisher>,
    events: mpsc::Sender<(&'static str, usize)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl Drop for ReentrantBlockingOwner {
    fn drop(&mut self) {
        let nested_reaped = self.publisher.reap_releasable_gpu_payloads();
        let _ = self.events.send(("reentered", nested_reaped));
        let (released, wake) = self.release.as_ref();
        let mut released = released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released {
            released = wake
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

#[test]
fn latest_gpu_payload_is_never_reaped() {
    let fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let (owner, receipt) = publish_gpu(&fixture, &publisher, 1);
    drop(receipt);

    assert_eq!(publisher.reap_releasable_gpu_payloads(), 0);
    assert!(owner.upgrade().is_some());
    let publication = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("GPU branch has a lease")
        .read()
        .expect("latest GPU publication remains live");
    let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
        panic!("GPU branch retains a GPU payload");
    };
    assert_eq!(payload.surface().handle_id(), 1);
}

#[test]
fn gpu_owner_drop_can_reenter_while_other_publishers_take_the_runtime_lock() {
    let fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = Arc::new(fixture.publisher());
    let (events, observed_events) = mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let owner = Arc::new(ReentrantBlockingOwner {
        publisher: Arc::clone(&publisher),
        events,
        release: Arc::clone(&release),
    });
    let weak_owner = Arc::downgrade(&owner);
    let surface = gpu_surface_with_owner(1, owner);
    let receipt = fixture
        .hub
        .publish(
            publisher.as_ref(),
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                fixture.colorimetry(),
                &surface,
            )),
            &metadata(&fixture.descriptor, publisher.as_ref(), 1),
        )
        .expect("reentrant GPU owner publishes");
    drop((receipt, surface));
    let (_, latest_receipt) = publish_gpu(&fixture, publisher.as_ref(), 2);
    drop(latest_receipt);

    let outer_publisher = Arc::clone(&publisher);
    let (outer_result, observed_outer_result) = mpsc::channel();
    let outer = thread::spawn(move || {
        let _ = outer_result.send(outer_publisher.reap_releasable_gpu_payloads());
    });
    assert_eq!(
        observed_events.recv_timeout(Duration::from_secs(1)),
        Ok(("reentered", 0))
    );

    let probe_publisher = Arc::clone(&publisher);
    let (probe_result, observed_probe_result) = mpsc::channel();
    let probe = thread::spawn(move || {
        let _ = probe_result.send(probe_publisher.reap_releasable_gpu_payloads());
    });
    assert_eq!(
        observed_probe_result.recv_timeout(Duration::from_secs(1)),
        Ok(0)
    );

    let (released, wake) = release.as_ref();
    *released
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    wake.notify_all();
    assert_eq!(
        observed_outer_result.recv_timeout(Duration::from_secs(1)),
        Ok(1)
    );
    outer.join().expect("outer reaper exits");
    probe.join().expect("probe reaper exits");
    assert!(weak_owner.upgrade().is_none());
}

#[test]
fn superseded_gpu_payloads_reap_while_publication_slots_remain_pooled() {
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 1)
        .expect("two publication slots are valid");
    let fixture = Fixture::new(policy);
    let publisher = fixture.publisher();
    let mut slot_addresses = [0_usize; 2];
    let mut previous_owner: Option<Weak<()>> = None;

    for sequence in 1..=8 {
        let (owner, receipt) = publish_gpu(&fixture, &publisher, sequence);
        let address = Arc::as_ptr(receipt.publication()) as usize;
        if sequence <= 2 {
            slot_addresses[usize::try_from(sequence - 1).expect("index fits usize")] = address;
        } else {
            assert!(slot_addresses.contains(&address));
        }
        drop(receipt);

        let expected_releases = usize::from(previous_owner.is_some());
        assert_eq!(publisher.reap_releasable_gpu_payloads(), expected_releases);
        if let Some(previous_owner) = previous_owner.replace(owner.clone()) {
            assert!(previous_owner.upgrade().is_none());
        }
        assert!(owner.upgrade().is_some());
    }
}

#[test]
fn reader_held_gpu_payload_defers_reaping_and_pool_capacity_recovers() {
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 1)
        .expect("two publication slots are valid");
    let fixture = Fixture::new(policy);
    let publisher = fixture.publisher();
    let lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("GPU branch has a lease");
    let (first_owner, first_receipt) = publish_gpu(&fixture, &publisher, 1);
    drop(first_receipt);
    let held_first = lease.read().expect("reader holds the first publication");
    let (second_owner, second_receipt) = publish_gpu(&fixture, &publisher, 2);
    drop(second_receipt);

    assert_eq!(publisher.reap_releasable_gpu_payloads(), 0);
    assert!(first_owner.upgrade().is_some());
    let (pressured_surface, pressured_owner) = gpu_surface(3);
    let pressured = fixture.hub.publish(
        &publisher,
        ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
            fixture.colorimetry(),
            &pressured_surface,
        )),
        &metadata(&fixture.descriptor, &publisher, 3),
    );
    assert!(matches!(
        pressured,
        Err(ScreenPublicationHubError::PublicationPressure { admitted_slots: 2 })
    ));
    drop(pressured_surface);
    assert!(pressured_owner.upgrade().is_none());

    drop(held_first);
    assert_eq!(publisher.reap_releasable_gpu_payloads(), 1);
    assert!(first_owner.upgrade().is_none());
    let (third_owner, third_receipt) = publish_gpu(&fixture, &publisher, 3);
    drop(third_receipt);
    assert_eq!(publisher.reap_releasable_gpu_payloads(), 1);
    assert!(second_owner.upgrade().is_none());
    assert!(third_owner.upgrade().is_some());
}

#[test]
fn abandoned_and_rejected_gpu_staging_releases_native_owners() {
    let fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let (abandoned_surface, abandoned_owner) = gpu_surface(1);
    let abandoned = fixture
        .hub
        .prepare_publication(
            &publisher,
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                fixture.colorimetry(),
                &abandoned_surface,
            )),
            &metadata(&fixture.descriptor, &publisher, 1),
        )
        .expect("GPU staging reserves a pool slot");
    drop(abandoned_surface);
    assert!(abandoned_owner.upgrade().is_some());
    drop(abandoned);
    assert!(abandoned_owner.upgrade().is_none());

    let (latest_owner, latest_receipt) = publish_gpu(&fixture, &publisher, 1);
    drop(latest_receipt);
    let (rejected_surface, rejected_owner) = gpu_surface(2);
    let rejected = fixture
        .hub
        .prepare_publication(
            &publisher,
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                fixture.colorimetry(),
                &rejected_surface,
            )),
            &metadata(&fixture.descriptor, &publisher, 1),
        )
        .expect("duplicate sequence stages before final authority validation");
    drop(rejected_surface);
    assert!(matches!(
        fixture.hub.finalize_publication(rejected),
        Err(ScreenPublicationHubError::NativeSequenceNotMonotonic { .. })
    ));
    assert!(rejected_owner.upgrade().is_none());
    assert_eq!(publisher.reap_releasable_gpu_payloads(), 0);
    assert!(latest_owner.upgrade().is_some());
}

#[derive(Debug)]
struct RendererTargetPayload;

#[test]
fn native_target_bindings_reject_unbound_mismatched_and_swapped_allocations() {
    let source = source();
    let first_target = native_target();
    let second_target = native_target_with(2, native_target_support::preparer());
    let first_demand = demand_for_target(&source, first_target.clone());
    let second_demand = demand_for_target(&source, second_target.clone());
    let first_descriptor = first_demand.descriptor().clone();
    let second_descriptor = second_demand.descriptor().clone();
    let ticket = worker_ticket_for([first_demand, second_demand]);
    let platform = ScreenNativePreparationPayload::new(
        &first_descriptor,
        ticket.plan_generation(),
        Arc::new(RendererTargetPayload),
    );

    let unbound =
        ScreenNativeTargetPreparation::new(platform.clone(), native_target_support::RETAINED_BYTES);
    assert!(matches!(
        unbound.exact_resource("unbound", "worker-runtime-total"),
        Err(ScreenNativeTargetResourceError::TargetIdentityMissing)
    ));
    let generic_worker = ScreenExactResource::try_new_scoped(
        "generic-worker",
        "worker-runtime-total",
        ScreenResourceKind::WorkerAdditional,
        native_target_support::RETAINED_BYTES,
    )
    .expect("generic worker resource is valid");
    let generic_worker_lifetime = ticket
        .bind_resource_lifetime(&generic_worker)
        .expect("generic worker lifetime binds to the ticket");
    assert!(matches!(
        unbound.bind(generic_worker_lifetime),
        Err(ScreenNativeTargetBindingError::TargetIdentityMissing)
    ));

    let wrong_kind = ScreenExactResource::try_new(
        "wrong-kind",
        ScreenResourceKind::ApiAllocation,
        native_target_support::RETAINED_BYTES,
    )
    .expect("API resource is valid exact accounting");
    let wrong_kind_lifetime = ticket
        .bind_resource_lifetime(&wrong_kind)
        .expect("wrong-kind lifetime still belongs to the ticket");
    let preparation = first_target
        .prepare(&first_descriptor, &platform)
        .expect("first target prepares its descriptor");
    assert!(matches!(
        preparation.bind(wrong_kind_lifetime),
        Err(ScreenNativeTargetBindingError::ResourceKindMismatch {
            observed: ScreenResourceKind::ApiAllocation
        })
    ));

    let accounted = first_target
        .prepare(&first_descriptor, &platform)
        .expect("first target prepares its accounted allocation");
    let accounted_resource = accounted
        .exact_resource("native-first-sized", "worker-runtime-total")
        .expect("target-stamped resource builds");
    let accounted_lifetime = ticket
        .bind_resource_lifetime(&accounted_resource)
        .expect("target-stamped lifetime binds to the ticket");
    let smaller_target = native_target_with(
        1,
        native_target_support::preparer_with_bytes(native_target_support::RETAINED_BYTES / 2),
    );
    let smaller = smaller_target
        .prepare(&first_descriptor, &platform)
        .expect("equal-identity target prepares a smaller allocation");
    assert!(matches!(
        smaller.bind(accounted_lifetime),
        Err(ScreenNativeTargetBindingError::RetainedBytesMismatch {
            expected,
            observed
        }) if expected == native_target_support::RETAINED_BYTES / 2
            && observed == native_target_support::RETAINED_BYTES
    ));

    let first_preparation = first_target
        .prepare(&first_descriptor, &platform)
        .expect("first route prepares");
    let first_resource = first_preparation
        .exact_resource("native-route-first", "worker-runtime-total")
        .expect("first route produces its bound resource");
    let first_lifetime = ticket
        .bind_resource_lifetime(&first_resource)
        .expect("first route lifetime binds");
    let second_preparation = second_target
        .prepare(
            &second_descriptor,
            &ScreenNativePreparationPayload::new(
                &second_descriptor,
                ticket.plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
        )
        .expect("second route prepares");
    let second_resource = second_preparation
        .exact_resource("native-route-second", "worker-runtime-total")
        .expect("second route produces its bound resource");
    let second_lifetime = ticket
        .bind_resource_lifetime(&second_resource)
        .expect("second route lifetime binds");
    assert_eq!(first_resource.name().as_ref(), "native-route-first");
    assert_eq!(second_resource.name().as_ref(), "native-route-second");
    assert_eq!(first_resource.bytes(), second_resource.bytes());
    assert!(matches!(
        first_preparation.bind(second_lifetime),
        Err(ScreenNativeTargetBindingError::NativeBindingMismatch)
    ));
    assert!(matches!(
        second_preparation.bind(first_lifetime),
        Err(ScreenNativeTargetBindingError::NativeBindingMismatch)
    ));

    let native_demand = demand_for_target(&source, first_target.clone());
    let compact_demand = demand_for_target_extent(
        &source,
        first_target.clone(),
        ScreenExtentRequest::bounded(
            Some(NonZeroU32::MIN),
            Some(NonZeroU32::MIN),
            hypercolor_core::input::screen::ScreenUpscalePolicy::Never,
        ),
    );
    let native_descriptor = native_demand.descriptor().clone();
    let compact_descriptor = compact_demand.descriptor().clone();
    assert_ne!(native_descriptor, compact_descriptor);
    let descriptor_ticket = worker_ticket_for([native_demand, compact_demand]);
    let native_preparation = first_target
        .prepare(&native_descriptor, &platform)
        .expect("native-size route prepares");
    let native_resource = native_preparation
        .exact_resource("native-size-route", "worker-runtime-total")
        .expect("native-size route produces its bound resource");
    let native_lifetime = descriptor_ticket
        .bind_resource_lifetime(&native_resource)
        .expect("native-size route lifetime binds");
    let compact_preparation = first_target
        .prepare(
            &compact_descriptor,
            &ScreenNativePreparationPayload::new(
                &compact_descriptor,
                descriptor_ticket.plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
        )
        .expect("compact route prepares");
    let compact_resource = compact_preparation
        .exact_resource("compact-route", "worker-runtime-total")
        .expect("compact route produces its bound resource");
    let compact_lifetime = descriptor_ticket
        .bind_resource_lifetime(&compact_resource)
        .expect("compact route lifetime binds");
    assert_eq!(native_resource.bytes(), compact_resource.bytes());
    assert!(matches!(
        native_preparation.bind(compact_lifetime),
        Err(ScreenNativeTargetBindingError::NativeBindingMismatch)
    ));
    assert!(matches!(
        compact_preparation.bind(native_lifetime),
        Err(ScreenNativeTargetBindingError::NativeBindingMismatch)
    ));
}

#[test]
fn reader_held_gpu_surface_retains_capture_and_renderer_bytes_after_plan_retirement() {
    let mut fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let branch_lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("GPU branch has a lease");
    let ScreenPublicationExecutor::SourceNative(target) = fixture.descriptor.executor() else {
        panic!("fixture resolves a native target");
    };
    let renderer_payload = Arc::new(RendererTargetPayload);
    let renderer_payload_weak = Arc::downgrade(&renderer_payload);
    let platform = ScreenNativePreparationPayload::new(
        &fixture.descriptor,
        fixture.hub.committed_state().plan().generation(),
        renderer_payload,
    );
    let preparation = target
        .prepare(&fixture.descriptor, &platform)
        .expect("test renderer prepares the target")
        .bind(
            fixture
                .target_lifetime
                .as_ref()
                .expect("fixture retains target accounting")
                .clone(),
        )
        .expect("renderer bytes bind to their exact lifetime");
    let (surface, owner) = gpu_surface(1);
    let surface = preparation
        .retain_on_surface_with_capture_allocation(
            surface,
            fixture
                .capture_lifetime
                .as_ref()
                .expect("fixture retains capture-plan accounting")
                .clone(),
        )
        .expect("capture and target allocations belong to one worker");
    let receipt = fixture
        .hub
        .publish(
            &publisher,
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                fixture.colorimetry(),
                &surface,
            )),
            &metadata(&fixture.descriptor, &publisher, 1),
        )
        .expect("bound GPU surface publishes");
    drop((receipt, surface, preparation, platform));
    let held = branch_lease
        .read()
        .expect("reader retains the renderer-bound publication");

    let (committed, _) = commit_candidate(&mut fixture.builder, std::iter::empty());
    let (_, retirement) = committed.into_parts();
    drop(fixture.target_lifetime.take());
    drop(fixture.capture_lifetime.take());
    drop((publisher, branch_lease));
    let retirement = retirement
        .try_reclaim()
        .expect_err("reader-held publication defers exact retirement");
    assert_eq!(
        retirement.pending_bytes(),
        native_target_support::RETAINED_BYTES + CAPTURE_PLAN_RETAINED_BYTES
    );
    assert_eq!(
        fixture.hub.pending_retired_bytes(),
        retirement.pending_bytes()
    );
    let ScreenBranchPayload::GpuSurface(payload) = held.payload() else {
        panic!("reader retains a GPU payload");
    };
    assert!(
        payload
            .surface()
            .retained_owner::<RendererTargetPayload>()
            .is_some()
    );
    assert_eq!(
        payload
            .surface()
            .resource_lifetime()
            .expect("surface retains exact accounting")
            .resource()
            .bytes(),
        native_target_support::RETAINED_BYTES
    );
    assert_eq!(
        payload
            .surface()
            .capture_resource_lifetime()
            .expect("surface retains capture-plan accounting")
            .resource()
            .bytes(),
        CAPTURE_PLAN_RETAINED_BYTES
    );
    assert!(owner.upgrade().is_some());
    assert!(renderer_payload_weak.upgrade().is_some());

    drop(held);
    retirement
        .try_reclaim()
        .expect("retirement completes after the final reader drops");
    assert_eq!(fixture.hub.pending_retired_bytes(), 0);
    assert!(owner.upgrade().is_none());
    assert!(renderer_payload_weak.upgrade().is_none());
}

#[test]
fn retirement_releases_unread_latest_gpu_payload_before_stale_publisher_drops() {
    let mut fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let (owner, receipt) = publish_gpu(&fixture, &publisher, 1);
    drop(receipt);

    let (committed, _) = commit_candidate(&mut fixture.builder, std::iter::empty());
    let (_, retirement) = committed.into_parts();
    drop(fixture.target_lifetime.take());
    drop(fixture.capture_lifetime.take());
    assert!(fixture.hub.lease(&fixture.descriptor).is_err());
    assert!(owner.upgrade().is_none());
    let retirement = retirement
        .try_reclaim()
        .expect_err("stale publisher still retains the retired branch");

    drop(publisher);
    retirement
        .try_reclaim()
        .expect("retired pool reclaims after the stale publisher drops");
}
