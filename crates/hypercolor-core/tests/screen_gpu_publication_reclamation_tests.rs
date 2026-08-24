use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::consumer::{CaptureEpoch, CaptureSourceId, PixelExtent};
use hypercolor_core::input::screen::implementer::{
    CaptureColorimetry, CaptureGeometry, CapturePixelFormat, CaptureRotation, PhysicalOrigin,
    PlatformGpuApi, PlatformGpuSurface, ScreenBranchPayload, ScreenBranchPublisher,
    ScreenGpuSurfacePayload, ScreenLiveBranchReceipt, ScreenNativeWorkPayload,
    ScreenPublicationColorimetry, ScreenPublicationHealth, ScreenPublicationMetadata,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerLedgerBuildError, SourceScale,
};
use hypercolor_core::input::screen::planner::{
    BoundScreenNativeTargetPreparation, CommittedScreenPlan, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenCapturePlan,
    ScreenColorTransformCapabilities, ScreenCursorCapabilities, ScreenExactResource,
    ScreenExecutorColorCapabilities, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativePreparationPayload,
    ScreenNativeRetentionQuote, ScreenNativeTargetBindingError, ScreenNativeTargetPreparation,
    ScreenNativeTargetPreparer, ScreenNativeTargetResourceError, ScreenPhysicalGpuDeviceIdentity,
    ScreenPlanBuilder, ScreenProcessingProfile, ScreenProcessingProfileConfig,
    ScreenPublicationExecutor, ScreenPublicationExecutorRequest, ScreenPublicationHub,
    ScreenPublicationHubError, ScreenPublicationKind, ScreenPublicationRequest,
    ScreenPublicationSlotPolicy, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenSourceReflection, ScreenSourceSelector,
    ScreenWorkerBinding,
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
                ScreenCaptureBackend::DesktopDuplication,
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
    demand_for_target_profile_extent(
        source,
        target,
        requested_extent,
        Arc::new(ScreenProcessingProfile::default()),
    )
}

fn demand_for_target_profile_extent(
    source: &ResolvedScreenSource,
    target: ScreenNativeExecutionTarget,
    requested_extent: ScreenExtentRequest,
    profile: Arc<ScreenProcessingProfile>,
) -> ResolvedScreenBranchDemand {
    let registered = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::SourceNative(target),
            requested_extent,
            ScreenAspectPolicy::Contain,
            profile,
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
    ticket: hypercolor_core::input::screen::ScreenWorkerPreparationTicket,
) -> anyhow::Result<(
    hypercolor_core::input::screen::ScreenPreparedWorkerToken,
    Box<[ScreenResourceLifetime]>,
    Option<BoundScreenNativeTargetPreparation>,
)> {
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
    let has_native_runtime = ledger
        .ticket()
        .required_minimums()
        .iter()
        .any(|minimum| minimum.name().as_ref() == "worker-runtime-total");
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    let admitted_target = if has_native_runtime {
        let descriptor = ledger
            .ticket()
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
            .expect("native fixture candidate has a native descriptor")
            .clone();
        let ScreenPublicationExecutor::SourceNative(target) = descriptor.executor() else {
            unreachable!("native descriptor selected above")
        };
        let preparation = ledger.prepare_native_target(
            target,
            &descriptor,
            &ScreenNativePreparationPayload::new(
                &descriptor,
                ledger.ticket().plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            "native-target-test",
            "worker-runtime-total",
        )?;
        ledger.preflight_additional_bytes(CAPTURE_PLAN_RETAINED_BYTES)?;
        ledger.report_scoped(
            "capture-plan-test",
            "worker-runtime-total",
            CAPTURE_PLAN_RETAINED_BYTES,
        )?;
        Some(preparation)
    } else {
        None
    };
    for (name, bytes) in reports {
        ledger.report(&name, bytes)?;
    }
    let (token, lifetimes) = ledger.finish()?.into_parts();
    let target = admitted_target
        .map(|preparation| {
            let lifetime = lifetimes
                .iter()
                .find(|lifetime| lifetime.resource().name().as_ref() == "native-target-test")
                .cloned()
                .expect("native target lifetime is present after exact acknowledgement");
            preparation.bind(lifetime)
        })
        .transpose()?;
    Ok((token, lifetimes, target))
}

fn commit_candidate(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> (
    CommittedScreenPlan,
    Vec<Box<[ScreenResourceLifetime]>>,
    Vec<BoundScreenNativeTargetPreparation>,
) {
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision advances");
    let mut preparing = builder
        .prepare(
            demands,
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan prepares");
    let mut worker_lifetimes = Vec::new();
    let mut native_targets = Vec::new();
    for required_source in preparing.required_sources().to_vec() {
        let ticket = preparing
            .worker_ticket(&required_source)
            .expect("required source has a worker ticket");
        let (token, lifetimes, native_target) =
            exact_ledger(ticket).expect("exact ledger is valid");
        preparing
            .acknowledge(token)
            .expect("worker token belongs to the candidate");
        worker_lifetimes.push(lifetimes);
        native_targets.extend(native_target);
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
    (committed, worker_lifetimes, native_targets)
}

fn worker_ticket_for(
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> hypercolor_core::input::screen::ScreenWorkerPreparationTicket {
    let mut builder = ScreenPlanBuilder::new();
    let mut preparing = builder
        .prepare(
            demands,
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
    BoundScreenNativeTargetPreparation,
) {
    let (committed, worker_lifetimes, native_targets) = commit_candidate(builder, [demand]);
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
    let target = native_targets
        .into_iter()
        .next()
        .expect("initial native target is bound");
    (plan, target_lifetime, capture_lifetime, target)
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
    let surface = fixture.bind_native_surface(surface);
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
    target_preparation: Option<BoundScreenNativeTargetPreparation>,
}

impl Fixture {
    fn new(slot_policy: ScreenPublicationSlotPolicy) -> Self {
        let source = source();
        let demand = demand(&source);
        let descriptor = demand.descriptor().clone();
        let mut builder = ScreenPlanBuilder::with_publication_slots(slot_policy);
        let hub = builder.publication_hub();
        let (_, target_lifetime, capture_lifetime, target_preparation) =
            commit_initial(&mut builder, demand);
        Self {
            builder,
            hub,
            descriptor,
            target_lifetime: Some(target_lifetime),
            capture_lifetime: Some(capture_lifetime),
            target_preparation: Some(target_preparation),
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

    fn bind_native_surface(&self, surface: PlatformGpuSurface) -> PlatformGpuSurface {
        self.target_preparation
            .as_ref()
            .expect("fixture retains its admitted native target")
            .retain_on_surface_with_capture_allocation(
                surface,
                self.capture_lifetime
                    .as_ref()
                    .expect("fixture retains capture-plan accounting")
                    .clone(),
            )
            .expect("capture and target allocations belong to one worker")
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
    let surface = fixture.bind_native_surface(gpu_surface_with_owner(1, owner));
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
    let pressured_surface = fixture.bind_native_surface(pressured_surface);
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
    let abandoned_surface = fixture.bind_native_surface(abandoned_surface);
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
    let rejected_surface = fixture.bind_native_surface(rejected_surface);
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

struct SharedTargetPreparer {
    exclusive_bytes: u64,
    shared_bytes: u64,
}

impl ScreenNativeTargetPreparer for SharedTargetPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(self.exclusive_bytes)
    }

    fn quote_retention(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeRetentionQuote> {
        Ok(ScreenNativeRetentionQuote::split(
            self.exclusive_bytes,
            self.shared_bytes,
        ))
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            ScreenNativeRetentionQuote::split(self.exclusive_bytes, self.shared_bytes),
        ))
    }
}

fn smoothing_profile() -> Arc<ScreenProcessingProfile> {
    Arc::new(ScreenProcessingProfile::new(
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(80),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..ScreenProcessingProfileConfig::default()
        },
    ))
}

#[test]
fn equal_native_physical_work_retains_one_shared_allocation() {
    const EXCLUSIVE_BYTES: u64 = 17;
    const SHARED_BYTES: u64 = 101;

    let source = source();
    let target = native_target_with(
        81,
        Arc::new(SharedTargetPreparer {
            exclusive_bytes: EXCLUSIVE_BYTES,
            shared_bytes: SHARED_BYTES,
        }),
    );
    let first = demand_for_target(&source, target.clone());
    let second = demand_for_target_profile_extent(
        &source,
        target.clone(),
        ScreenExtentRequest::Native,
        smoothing_profile(),
    );
    assert_ne!(first.descriptor(), second.descriptor());
    assert_eq!(
        first.descriptor().physical(),
        second.descriptor().physical()
    );

    let ticket = worker_ticket_for([first.clone(), second.clone()]);
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)
        .expect("shared native ledger metadata prepares");
    let first_admitted = ledger
        .prepare_native_target(
            &target,
            first.descriptor(),
            &ScreenNativePreparationPayload::new(
                first.descriptor(),
                ledger.ticket().plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            "native-shared-first",
            "worker-runtime-total",
        )
        .expect("first shared native route prepares");
    let second_admitted = ledger
        .prepare_native_target(
            &target,
            second.descriptor(),
            &ScreenNativePreparationPayload::new(
                second.descriptor(),
                ledger.ticket().plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            "native-shared-second",
            "worker-runtime-total",
        )
        .expect("second shared native route reuses physical admission");
    let shared_name = first_admitted
        .shared_resource_name()
        .cloned()
        .expect("split quote names one shared physical allocation");
    assert_eq!(second_admitted.shared_resource_name(), Some(&shared_name));

    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, bytes) in reports {
        ledger
            .report(&name, bytes)
            .expect("required shared native scope reports");
    }
    let (_, lifetimes) = ledger
        .finish()
        .expect("shared native ledger finishes")
        .into_parts();
    let shared_lifetimes = lifetimes
        .iter()
        .filter(|lifetime| lifetime.resource().name() == &shared_name)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(shared_lifetimes.len(), 1);
    assert_eq!(shared_lifetimes[0].resource().bytes(), SHARED_BYTES);

    for (admitted, name) in [
        (first_admitted, "native-shared-first"),
        (second_admitted, "native-shared-second"),
    ] {
        let exclusive = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name().as_ref() == name)
            .cloned()
            .expect("branch-exclusive native lifetime exists");
        let bound = admitted
            .bind_with_shared(exclusive, Some(shared_lifetimes[0].clone()))
            .expect("branch binds the shared physical lifetime");
        assert_eq!(bound.allocation().retained_bytes(), EXCLUSIVE_BYTES);
        assert_eq!(
            bound
                .shared_physical_allocation()
                .expect("bound route retains shared physical admission")
                .retained_bytes(),
            SHARED_BYTES
        );
        let (surface, _) = gpu_surface(91);
        let surface = bound.retain_on_surface(surface);
        assert_eq!(
            surface
                .shared_resource_lifetime()
                .expect("published surface retains the shared physical lifetime")
                .resource()
                .name(),
            &shared_name
        );
    }
}

struct ConflictingSharedTargetPreparer {
    quotes: AtomicUsize,
    first_shared_bytes: u64,
    second_shared_bytes: u64,
}

impl ScreenNativeTargetPreparer for ConflictingSharedTargetPreparer {
    fn quote_retained_bytes(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<u64> {
        Ok(17)
    }

    fn quote_retention(
        &self,
        _descriptor: &ResolvedScreenPublicationDescriptor,
        _platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeRetentionQuote> {
        let shared_bytes = if self.quotes.fetch_add(1, Ordering::Relaxed) == 0 {
            self.first_shared_bytes
        } else {
            self.second_shared_bytes
        };
        Ok(ScreenNativeRetentionQuote::split(17, shared_bytes))
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> anyhow::Result<ScreenNativeTargetPreparation> {
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            ScreenNativeRetentionQuote::split(17, self.first_shared_bytes),
        ))
    }
}

fn conflicting_shared_quote_error(
    first_shared_bytes: u64,
    second_shared_bytes: u64,
) -> ScreenWorkerLedgerBuildError {
    let source = source();
    let target = native_target_with(
        82,
        Arc::new(ConflictingSharedTargetPreparer {
            quotes: AtomicUsize::new(0),
            first_shared_bytes,
            second_shared_bytes,
        }),
    );
    let first = demand_for_target(&source, target.clone());
    let second = demand_for_target_profile_extent(
        &source,
        target.clone(),
        ScreenExtentRequest::Native,
        smoothing_profile(),
    );
    let ticket = worker_ticket_for([first.clone(), second.clone()]);
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)
        .expect("conflicting shared quote ledger prepares");
    ledger
        .prepare_native_target(
            &target,
            first.descriptor(),
            &ScreenNativePreparationPayload::new(
                first.descriptor(),
                ledger.ticket().plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            "native-conflict-first",
            "worker-runtime-total",
        )
        .expect("first shared quote establishes the physical charge");
    ledger
        .prepare_native_target(
            &target,
            second.descriptor(),
            &ScreenNativePreparationPayload::new(
                second.descriptor(),
                ledger.ticket().plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            "native-conflict-second",
            "worker-runtime-total",
        )
        .expect_err("equal physical work cannot change its shared byte quote")
        .downcast::<ScreenWorkerLedgerBuildError>()
        .expect("conflicting shared retention returns its typed ledger error")
}

#[test]
fn equal_native_physical_work_rejects_conflicting_shared_quotes() {
    assert!(matches!(
        conflicting_shared_quote_error(101, 102),
        ScreenWorkerLedgerBuildError::ConflictingNativeSharedRetention {
            expected: 101,
            observed: 102,
        }
    ));
}

#[test]
fn equal_native_physical_work_rejects_zero_then_shared_quotes() {
    assert!(matches!(
        conflicting_shared_quote_error(0, 101),
        ScreenWorkerLedgerBuildError::ConflictingNativeSharedRetention {
            expected: 0,
            observed: 101,
        }
    ));
}

#[test]
fn native_target_bindings_require_installed_admission_and_exact_identity() {
    let source = source();
    let first_target = native_target();
    let second_target = native_target_with(2, native_target_support::preparer());
    let first_demand = demand_for_target(&source, first_target.clone());
    let second_demand = demand_for_target(&source, second_target.clone());
    let first_descriptor = first_demand.descriptor().clone();
    let second_descriptor = second_demand.descriptor().clone();

    let premature_ticket = worker_ticket_for([first_demand.clone()]);
    let mut premature = ScreenWorkerExactLedgerBuilder::new(premature_ticket)
        .expect("premature admission prepares");
    let first_platform = ScreenNativePreparationPayload::new(
        &first_descriptor,
        premature.ticket().plan_generation(),
        Arc::new(RendererTargetPayload),
    );
    let unbound = ScreenNativeTargetPreparation::new(
        ScreenNativePreparationPayload::new(
            &first_descriptor,
            premature.ticket().plan_generation(),
            Arc::new(RendererTargetPayload),
        ),
        native_target_support::RETAINED_BYTES,
    );
    assert!(matches!(
        unbound.exact_resource("unbound", "worker-runtime-total"),
        Err(ScreenNativeTargetResourceError::TargetIdentityMissing)
    ));
    let generic_resource = ScreenExactResource::try_new_scoped(
        "generic-worker",
        "worker-runtime-total",
        ScreenResourceKind::WorkerAdditional,
        native_target_support::RETAINED_BYTES,
    )
    .expect("generic worker resource is valid");
    let uninstalled_lifetime = premature
        .ticket()
        .bind_resource_lifetime(&generic_resource)
        .expect("generic lifetime belongs to the ticket");
    let premature_target = premature
        .prepare_native_target(
            &first_target,
            &first_descriptor,
            &first_platform,
            "native-premature",
            "worker-runtime-total",
        )
        .expect("target prepares under a dedicated lease");
    assert!(matches!(
        premature_target.bind(uninstalled_lifetime),
        Err(ScreenNativeTargetBindingError::AdmissionLeaseMismatch)
    ));

    let ticket = worker_ticket_for([first_demand, second_demand]);
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("native binding admission prepares");
    let first_platform = ScreenNativePreparationPayload::new(
        &first_descriptor,
        ledger.ticket().plan_generation(),
        Arc::new(RendererTargetPayload),
    );
    let first = ledger
        .prepare_native_target(
            &first_target,
            &first_descriptor,
            &first_platform,
            "native-route-first",
            "worker-runtime-total",
        )
        .expect("first route prepares");
    let second = ledger
        .prepare_native_target(
            &second_target,
            &second_descriptor,
            &ScreenNativePreparationPayload::new(
                &second_descriptor,
                ledger.ticket().plan_generation(),
                Arc::new(RendererTargetPayload),
            ),
            "native-route-second",
            "worker-runtime-total",
        )
        .expect("second route prepares");
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, bytes) in reports {
        ledger
            .report(&name, bytes)
            .expect("required native scope reports");
    }
    let (_, lifetimes) = ledger
        .finish()
        .expect("native ledger finishes")
        .into_parts();
    let first_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "native-route-first")
        .cloned()
        .expect("first target lifetime installs");
    let second_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "native-route-second")
        .cloned()
        .expect("second target lifetime installs");
    let first = first
        .bind(first_lifetime)
        .expect("first target binds to its installed lease");
    let second = second
        .bind(second_lifetime)
        .expect("second target binds to its installed lease");
    assert_eq!(first.target_id(), first_target.id());
    assert_eq!(second.target_id(), second_target.id());
}

#[test]
fn reader_held_gpu_surface_retains_capture_and_renderer_bytes_after_plan_retirement() {
    let mut fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let branch_lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("GPU branch has a lease");
    let preparation = fixture
        .target_preparation
        .take()
        .expect("fixture retains its admitted native target");
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
    let renderer_payload = surface
        .retained_owner::<native_target_support::TestNativeTargetPayload>()
        .expect("fixture target retains renderer payload");
    let renderer_payload_weak = renderer_payload.downgrade();
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
    drop((receipt, surface, preparation));
    let held = branch_lease
        .read()
        .expect("reader retains the renderer-bound publication");

    let (committed, _, _) = commit_candidate(&mut fixture.builder, std::iter::empty());
    let (_, retirement) = committed.into_parts();
    drop(fixture.target_preparation.take());
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
            .retained_owner::<native_target_support::TestNativeTargetPayload>()
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
    let retirement = retirement
        .try_reclaim()
        .expect_err("typed owner access retains exact admission after the surface drops");
    drop(renderer_payload);
    retirement
        .try_reclaim()
        .expect("retirement completes after the final owner guard drops");
    assert_eq!(fixture.hub.pending_retired_bytes(), 0);
    assert!(owner.upgrade().is_none());
    assert!(renderer_payload_weak.upgrade().is_none());
}

#[test]
fn native_publications_reject_missing_capture_and_substituted_worker_lifetimes() {
    let mut fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let target = fixture
        .target_preparation
        .take()
        .expect("fixture retains its admitted native target");
    let (surface, _) = gpu_surface(1);
    let target_only = target.retain_on_surface(surface);
    let metadata = metadata(&fixture.descriptor, &publisher, 1);
    assert!(matches!(
        fixture.hub.publish(
            &publisher,
            ScreenBranchPayload::NativeWork(ScreenNativeWorkPayload::new(
                fixture.colorimetry(),
                &target_only,
            )),
            &metadata,
        ),
        Err(ScreenPublicationHubError::NativeCaptureLifetimeMismatch)
    ));
    assert!(
        fixture
            .hub
            .lease(&fixture.descriptor)
            .expect("native branch remains committed")
            .read()
            .is_none()
    );

    let mut substitute = Fixture::new(ScreenPublicationSlotPolicy::default());
    let substitute_target = substitute
        .target_preparation
        .take()
        .expect("substitute fixture retains its admitted target");
    let (surface, _) = gpu_surface(2);
    let substituted = substitute_target
        .retain_on_surface_with_capture_allocation(
            surface,
            substitute
                .capture_lifetime
                .as_ref()
                .expect("substitute fixture retains capture accounting")
                .clone(),
        )
        .expect("substitute target and capture belong together");
    assert!(matches!(
        fixture.hub.publish(
            &publisher,
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                fixture.colorimetry(),
                &substituted,
            )),
            &metadata,
        ),
        Err(ScreenPublicationHubError::NativeTargetLifetimeMismatch)
    ));
    assert!(
        fixture
            .hub
            .lease(&fixture.descriptor)
            .expect("native branch remains committed")
            .read()
            .is_none()
    );
}

#[test]
fn retirement_releases_unread_latest_gpu_payload_before_stale_publisher_drops() {
    let mut fixture = Fixture::new(ScreenPublicationSlotPolicy::default());
    let publisher = fixture.publisher();
    let (owner, receipt) = publish_gpu(&fixture, &publisher, 1);
    drop(receipt);

    let (committed, _, _) = commit_candidate(&mut fixture.builder, std::iter::empty());
    let (_, retirement) = committed.into_parts();
    drop(fixture.target_preparation.take());
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
