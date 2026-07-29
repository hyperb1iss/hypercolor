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
    ScreenExactResourceLedger, ScreenExtentRequest, ScreenGpuSurfacePayload,
    ScreenInputGraphGeneration, ScreenLiveBranchReceipt, ScreenPlanBuilder, ScreenPlanError,
    ScreenProcessingProfile, ScreenPublicationColorimetry, ScreenPublicationHealth,
    ScreenPublicationHub, ScreenPublicationHubError, ScreenPublicationKind,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenPublicationSlotPolicy,
    ScreenResourceApi, ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector,
    ScreenWorkerBinding, SourceScale,
};

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test values are non-zero")
}

fn extent() -> PixelExtent {
    PixelExtent::new(2, 2).expect("test extent is non-empty")
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
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::WindowsDesktopDuplication,
                ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
                1,
                1,
            ),
        ),
    )
}

fn demand(source: &ResolvedScreenSource) -> ResolvedScreenBranchDemand {
    let registered = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        non_zero(60),
    );
    registered
        .resolve_with_color_capabilities(
            source,
            ScreenColorTransformCapabilities::new(
                true,
                true,
                true,
                registered
                    .request()
                    .processing_profile()
                    .algorithm_revision(),
            ),
        )
        .expect("GPU test demand resolves")
}

fn exact_ledger(
    ticket: &hypercolor_core::input::screen::ScreenWorkerPreparationTicket,
) -> Result<(ScreenExactResourceLedger, Vec<ScreenResourceLifetime>), ScreenPlanError> {
    let resources = ticket
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
    let lifetimes = resources
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((ScreenExactResourceLedger::try_new(resources)?, lifetimes))
}

fn commit_candidate(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> CommittedScreenPlan {
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
    drop(worker_lifetimes);
    committed
}

fn commit_initial(
    builder: &mut ScreenPlanBuilder,
    demand: ResolvedScreenBranchDemand,
) -> ScreenCapturePlan {
    let (plan, retirement) = commit_candidate(builder, [demand]).into_parts();
    retirement
        .try_reclaim()
        .expect("empty predecessor retires immediately");
    plan
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
}

impl Fixture {
    fn new(slot_policy: ScreenPublicationSlotPolicy) -> Self {
        let source = source();
        let demand = demand(&source);
        let descriptor = demand.descriptor().clone();
        let mut builder = ScreenPlanBuilder::with_publication_slots(slot_policy);
        let hub = builder.publication_hub();
        commit_initial(&mut builder, demand);
        Self {
            builder,
            hub,
            descriptor,
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

#[test]
fn retirement_releases_unread_latest_gpu_payload_before_stale_publisher_drops() {
    let mut fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let (owner, receipt) = publish_gpu(&fixture, &publisher, 1);
    drop(receipt);

    let (_, retirement) = commit_candidate(&mut fixture.builder, std::iter::empty()).into_parts();
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
