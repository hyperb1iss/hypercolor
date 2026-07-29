use std::num::{NonZeroU32, NonZeroU64};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CommittedScreenPlan, InputPublicationDemandRevision, PhysicalOrigin,
    PixelExtent, PlatformGpuApi, PlatformGpuSurface, PreparedScreenPublication,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenBranchPayload, ScreenCaptureBackend, ScreenCapturePlan,
    ScreenColorTransformCapabilities, ScreenCursorCapabilities, ScreenExactResource,
    ScreenExactResourceLedger, ScreenExtentRequest, ScreenGpuSurfacePayload,
    ScreenInputGraphGeneration, ScreenLiveBranchReceipt, ScreenPayloadKind, ScreenPlanBuilder,
    ScreenPlanError, ScreenProcessingProfile, ScreenPublicationColorimetry,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError,
    ScreenPublicationKind, ScreenPublicationMetadata, ScreenPublicationRequest,
    ScreenPublicationResidency, ScreenPublicationSlotPolicy, ScreenResourceApi,
    ScreenResourceLifetime, ScreenSourceReflection, ScreenSourceSelector, ScreenSurfacePayload,
    ScreenWorkerBinding, SourceScale,
};

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test values are non-zero")
}

fn pixel_extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extents are non-empty")
}

fn source_id() -> CaptureSourceId {
    CaptureSourceId::new("writable-display").expect("test source id is non-empty")
}

fn source(width: u32, height: u32) -> ResolvedScreenSource {
    let extent = pixel_extent(width, height);
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    let config = ResolvedScreenSourceConfig::new_with_cursor_capabilities(
        geometry,
        extent,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        CaptureColorimetry::SRGB,
        ScreenCursorCapabilities::clean_with_separate_cursor(),
        ScreenBackendResourceIdentity::new(
            ScreenCaptureBackend::Synthetic,
            ScreenResourceApi::Cpu,
            1,
            1,
        ),
    );
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source_id(),
            topology_generation: 1,
            session_generation: 1,
        },
        config,
    )
}

fn gpu_source(width: u32, height: u32) -> ResolvedScreenSource {
    let extent = pixel_extent(width, height);
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    let config = ResolvedScreenSourceConfig::new_with_cursor_capabilities(
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
    );
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source_id(),
            topology_generation: 1,
            session_generation: 1,
        },
        config,
    )
}

fn demand(
    source: &ResolvedScreenSource,
    kind: ScreenPublicationKind,
    requested_hz: u32,
) -> ResolvedScreenBranchDemand {
    let registered = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        non_zero(requested_hz),
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
        .expect("test demand resolves")
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
    let ledger = ScreenExactResourceLedger::try_new(resources)?;
    Ok((ledger, lifetimes))
}

fn commit(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> ScreenCapturePlan {
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revisions remain representable");
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
    reclaim(committed)
}

fn reclaim(committed: CommittedScreenPlan) -> ScreenCapturePlan {
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved retired pools reclaim immediately");
    plan
}

fn binding(builder: &ScreenPlanBuilder) -> ScreenWorkerBinding {
    builder
        .committed_state()
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == &source_id())
        .cloned()
        .expect("committed source has a worker binding")
}

fn intent(
    descriptor: &ResolvedScreenPublicationDescriptor,
    binding: &ScreenWorkerBinding,
    native_sequence: u64,
) -> ScreenPublicationMetadata {
    let now = Instant::now();
    ScreenPublicationMetadata::try_intent(
        descriptor.source_epoch().clone(),
        binding.plan_generation(),
        NonZeroU64::new(native_sequence).expect("test sequence is non-zero"),
        now,
        now + Duration::from_secs(1),
    )
    .expect("test timeline is valid")
}

fn finalize(
    hub: &ScreenPublicationHub,
    prepared: PreparedScreenPublication,
) -> Result<ScreenLiveBranchReceipt, ScreenPublicationHubError> {
    hub.finalize_writable_publication(prepared, Instant::now(), ScreenPublicationHealth::Healthy)
}

struct Fixture {
    builder: ScreenPlanBuilder,
    hub: Arc<ScreenPublicationHub>,
    source: ResolvedScreenSource,
    descriptor: ResolvedScreenPublicationDescriptor,
}

impl Fixture {
    fn new(
        width: u32,
        height: u32,
        kind: ScreenPublicationKind,
        slot_policy: ScreenPublicationSlotPolicy,
        requested_hz: u32,
    ) -> Self {
        let source = source(width, height);
        let demand = demand(&source, kind, requested_hz);
        let descriptor = demand.descriptor().clone();
        let mut builder = ScreenPlanBuilder::with_publication_slots(slot_policy);
        let hub = builder.publication_hub();
        commit(&mut builder, [demand]);
        Self {
            builder,
            hub,
            source,
            descriptor,
        }
    }
}

#[test]
fn writable_surface_slots_preserve_last_good_and_reuse_exact_bytes() {
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 1)
        .expect("two publication slots are valid");
    let fixture = Fixture::new(2, 2, ScreenPublicationKind::Surface, policy, 60);
    let binding = binding(&fixture.builder);
    let publisher = fixture
        .hub
        .publisher(&fixture.descriptor, &binding)
        .expect("active worker owns the branch");
    let lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("active branch has a lease");

    assert!(matches!(
        fixture.hub.prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Zones,
            &intent(&fixture.descriptor, &binding, 1),
        ),
        Err(ScreenPublicationHubError::PayloadKindMismatch { .. })
    ));

    let mut first = fixture
        .hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &binding, 1),
        )
        .expect("first slot reserves");
    assert!(matches!(
        first.zone_colors_mut(),
        Err(ScreenPublicationHubError::PayloadKindMismatch { .. })
    ));
    let first_bytes = first
        .surface_pixels_mut()
        .expect("surface reservation exposes bytes");
    assert_eq!(first_bytes.len(), 2 * 2 * 4);
    let first_address = first_bytes.as_ptr();
    first_bytes.fill(0x11);
    let first_receipt = finalize(&fixture.hub, first).expect("first direct write publishes");
    drop(first_receipt);
    let first_last_good = lease.read().expect("first write becomes last-good");

    let rollback = (|| -> Result<(), &'static str> {
        let mut reservation = fixture
            .hub
            .prepare_writable_publication(
                &publisher,
                ScreenPayloadKind::Surface,
                &intent(&fixture.descriptor, &binding, 2),
            )
            .map_err(|_| "reservation failed")?;
        reservation
            .surface_pixels_mut()
            .map_err(|_| "surface borrow failed")?
            .fill(0x22);
        Err("reducer failed")
    })();
    assert_eq!(rollback, Err("reducer failed"));
    assert!(Arc::ptr_eq(
        &lease.read().expect("rollback retains last-good"),
        &first_last_good
    ));
    drop(first_last_good);

    let mut second = fixture
        .hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &binding, 2),
        )
        .expect("rolled-back slot returns to the pool");
    second
        .surface_pixels_mut()
        .expect("second reservation exposes bytes")
        .fill(0x33);
    let second_receipt = finalize(&fixture.hub, second).expect("second direct write publishes");
    drop(second_receipt);

    let mut third = fixture
        .hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &binding, 3),
        )
        .expect("superseded first slot becomes reusable");
    let third_bytes = third
        .surface_pixels_mut()
        .expect("third reservation exposes bytes");
    assert_eq!(third_bytes.as_ptr(), first_address);
    third_bytes.fill(0x44);
    finalize(&fixture.hub, third).expect("reused bytes publish without allocation");
    let latest = lease.read().expect("third write is last-good");
    let ScreenBranchPayload::Surface(surface) = latest.payload() else {
        panic!("surface branch publishes surface payloads");
    };
    assert_eq!(surface.pixels(), &[0x44; 16]);
}

#[test]
fn gpu_surface_publications_retain_native_ownership_and_reject_cpu_substitution() {
    let source = gpu_source(2, 2);
    let resolved = demand(&source, ScreenPublicationKind::Surface, 60);
    let descriptor = resolved.descriptor().clone();
    let mut builder = ScreenPlanBuilder::default();
    let hub = builder.publication_hub();
    commit(&mut builder, [resolved]);
    let binding = binding(&builder);
    let publisher = hub
        .publisher(&descriptor, &binding)
        .expect("GPU worker owns the exact branch");
    let colorimetry =
        ScreenPublicationColorimetry::new(descriptor.physical().color_pipeline().output());
    let surface = PlatformGpuSurface::new(
        PlatformGpuApi::Direct3d11,
        41,
        pixel_extent(2, 2),
        CapturePixelFormat::Rgba8,
        Arc::new(String::from("shared-d3d11-texture")),
    )
    .expect("test GPU surface has a stable non-zero identity");
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
    .expect("test timeline is valid");
    hub.publish(
        &publisher,
        ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(colorimetry, &surface)),
        &metadata,
    )
    .expect("native GPU surface publishes without readback");

    let publication = hub
        .lease(&descriptor)
        .expect("GPU branch remains committed")
        .read()
        .expect("GPU branch has a last-good publication");
    assert_eq!(
        publication.residency(),
        ScreenPublicationResidency::PlatformGpu(PlatformGpuApi::Direct3d11)
    );
    let ScreenBranchPayload::GpuSurface(payload) = publication.payload() else {
        panic!("GPU source Surface branches retain opaque GPU payloads");
    };
    assert_eq!(payload.surface().handle_id(), 41);
    assert_eq!(
        payload
            .surface()
            .owner::<String>()
            .expect("native owner type remains recoverable")
            .as_str(),
        "shared-d3d11-texture"
    );

    assert!(matches!(
        hub.prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&descriptor, &binding, 2),
        ),
        Err(ScreenPublicationHubError::ResidencyMismatch {
            expected: ScreenPublicationResidency::PlatformGpu(PlatformGpuApi::Direct3d11),
            observed: ScreenPublicationResidency::Cpu,
        })
    ));
    let cpu_pixels = [0_u8; 16];
    let cpu_payload = ScreenBranchPayload::Surface(
        ScreenSurfacePayload::try_new(
            pixel_extent(2, 2),
            CapturePixelFormat::Rgba8,
            colorimetry,
            &cpu_pixels,
        )
        .expect("CPU payload is shape-valid"),
    );
    assert!(matches!(
        hub.publish(
            &publisher,
            cpu_payload,
            &ScreenPublicationMetadata::try_new(
                descriptor.source_epoch().clone(),
                binding.plan_generation(),
                NonZeroU64::new(2).expect("test sequence is non-zero"),
                now,
                now,
                now + Duration::from_secs(1),
                ScreenPublicationHealth::Degraded,
            )
            .expect("fallback timeline is valid"),
        ),
        Err(ScreenPublicationHubError::ResidencyMismatch { .. })
    ));
    assert_eq!(
        hub.lease(&descriptor)
            .expect("GPU branch remains committed")
            .read()
            .expect("rejected CPU substitution preserves last-good")
            .native_sequence(),
        NonZeroU64::MIN
    );
}

#[test]
fn writable_reservation_is_panic_safe_and_reader_pressure_is_bounded() {
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 1)
        .expect("two publication slots are valid");
    let fixture = Fixture::new(1, 1, ScreenPublicationKind::Surface, policy, 60);
    let binding = binding(&fixture.builder);
    let publisher = fixture
        .hub
        .publisher(&fixture.descriptor, &binding)
        .expect("active worker owns the branch");
    let lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("active branch has a lease");
    let mut first = fixture
        .hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &binding, 1),
        )
        .expect("first slot reserves");
    first
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .copy_from_slice(&[1, 2, 3, 4]);
    finalize(&fixture.hub, first).expect("first slot publishes");
    let held_first = lease.read().expect("reader retains first publication");

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut reservation = fixture
            .hub
            .prepare_writable_publication(
                &publisher,
                ScreenPayloadKind::Surface,
                &intent(&fixture.descriptor, &binding, 2),
            )
            .expect("second slot reserves before reducer panic");
        reservation
            .surface_pixels_mut()
            .expect("surface bytes are writable")
            .fill(9);
        panic!("synthetic reducer panic");
    }));
    assert!(panic.is_err());
    assert!(Arc::ptr_eq(
        &lease.read().expect("panic retains last-good"),
        &held_first
    ));

    let mut second = fixture
        .hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &binding, 2),
        )
        .expect("panic returns its slot");
    second
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .fill(8);
    let held_second = finalize(&fixture.hub, second).expect("second slot publishes");
    assert!(matches!(
        fixture.hub.prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &binding, 3),
        ),
        Err(ScreenPublicationHubError::PublicationPressure { admitted_slots: 2 })
    ));
    assert!(Arc::ptr_eq(
        &lease.read().expect("pressure retains last-good"),
        held_second.publication()
    ));
    drop(held_first);
    drop(
        fixture
            .hub
            .prepare_writable_publication(
                &publisher,
                ScreenPayloadKind::Surface,
                &intent(&fixture.descriptor, &binding, 3),
            )
            .expect("releasing the reader restores fixed pool capacity"),
    );
}

#[test]
fn writable_zone_slots_have_exact_shape_and_publish_immutable_colors() {
    let policy = ScreenPublicationSlotPolicy::default();
    let fixture = Fixture::new(
        3840,
        2160,
        ScreenPublicationKind::Zones {
            columns: non_zero(7),
            rows: non_zero(5),
        },
        policy,
        60,
    );
    let binding = binding(&fixture.builder);
    let publisher = fixture
        .hub
        .publisher(&fixture.descriptor, &binding)
        .expect("active worker owns the zone branch");
    let mut prepared = fixture
        .hub
        .prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Zones,
            &intent(&fixture.descriptor, &binding, 1),
        )
        .expect("zone slot reserves");
    assert!(matches!(
        prepared.surface_pixels_mut(),
        Err(ScreenPublicationHubError::PayloadKindMismatch { .. })
    ));
    let colors = prepared
        .zone_colors_mut()
        .expect("zone reservation exposes row-major colors");
    assert_eq!(colors.len(), 7 * 5);
    for (index, color) in colors.iter_mut().enumerate() {
        let value = u8::try_from(index).expect("test grid values fit u8");
        *color = [value, value.saturating_add(1), value.saturating_add(2)];
    }
    finalize(&fixture.hub, prepared).expect("zone colors publish");
    let publication = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("zone branch has a lease")
        .read()
        .expect("zone branch is live");
    let ScreenBranchPayload::Zones(zones) = publication.payload() else {
        panic!("zone branch publishes zone payloads");
    };
    assert_eq!(zones.columns(), non_zero(7));
    assert_eq!(zones.rows(), non_zero(5));
    assert_eq!(zones.colors()[34], [34, 35, 36]);
}

#[test]
fn writable_finalize_rejects_stale_worker_and_returns_slot() {
    let policy = ScreenPublicationSlotPolicy::default();
    let mut fixture = Fixture::new(2, 2, ScreenPublicationKind::Surface, policy, 30);
    let old_binding = binding(&fixture.builder);
    let old_publisher = fixture
        .hub
        .publisher(&fixture.descriptor, &old_binding)
        .expect("initial worker owns the branch");
    let mut prepared = fixture
        .hub
        .prepare_writable_publication(
            &old_publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &old_binding, 1),
        )
        .expect("old worker reserves a slot");
    prepared
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .fill(0x55);

    commit(
        &mut fixture.builder,
        [demand(&fixture.source, ScreenPublicationKind::Surface, 60)],
    );
    assert!(matches!(
        finalize(&fixture.hub, prepared),
        Err(ScreenPublicationHubError::PublisherStale { .. })
    ));
    let lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("rebound branch remains active");
    assert!(lease.read().is_none());

    let new_binding = binding(&fixture.builder);
    let new_publisher = fixture
        .hub
        .publisher(&fixture.descriptor, &new_binding)
        .expect("replacement worker owns the branch");
    let mut replacement = fixture
        .hub
        .prepare_writable_publication(
            &new_publisher,
            ScreenPayloadKind::Surface,
            &intent(&fixture.descriptor, &new_binding, 1),
        )
        .expect("stale finalize returned its slot");
    replacement
        .surface_pixels_mut()
        .expect("replacement surface is writable")
        .fill(0x66);
    finalize(&fixture.hub, replacement).expect("replacement worker publishes");
    assert_eq!(
        lease
            .read()
            .expect("replacement becomes last-good")
            .native_sequence()
            .get(),
        1
    );
}

#[test]
fn writable_completion_stamps_delivery_time_and_rejects_expired_work() {
    let fixture = Fixture::new(
        1,
        1,
        ScreenPublicationKind::Surface,
        ScreenPublicationSlotPolicy::default(),
        60,
    );
    let binding = binding(&fixture.builder);
    let publisher = fixture
        .hub
        .publisher(&fixture.descriptor, &binding)
        .expect("active worker owns the branch");
    let lease = fixture
        .hub
        .lease(&fixture.descriptor)
        .expect("active branch has a lease");
    let captured_at = Instant::now();
    let freshness_deadline = captured_at + Duration::from_millis(100);
    let intent = |sequence| {
        ScreenPublicationMetadata::try_intent(
            fixture.descriptor.source_epoch().clone(),
            binding.plan_generation(),
            NonZeroU64::new(sequence).expect("test sequence is non-zero"),
            captured_at,
            freshness_deadline,
        )
        .expect("test intent timeline is valid")
    };

    let completed_metadata = ScreenPublicationMetadata::try_new(
        fixture.descriptor.source_epoch().clone(),
        binding.plan_generation(),
        NonZeroU64::MIN,
        captured_at,
        captured_at,
        freshness_deadline,
        ScreenPublicationHealth::Healthy,
    )
    .expect("completed metadata remains available to copy callers");
    assert!(matches!(
        fixture.hub.prepare_writable_publication(
            &publisher,
            ScreenPayloadKind::Surface,
            &completed_metadata,
        ),
        Err(ScreenPublicationHubError::PublicationCompletionAlreadySet)
    ));

    let mut first = fixture
        .hub
        .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &intent(1))
        .expect("first writable intent reserves");
    first
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .fill(1);
    let published_at = captured_at + Duration::from_millis(40);
    fixture
        .hub
        .finalize_writable_publication(first, published_at, ScreenPublicationHealth::Degraded)
        .expect("completion before the deadline publishes");
    let first_last_good = lease.read().expect("first completion becomes last-good");
    assert_eq!(first_last_good.published_at(), published_at);
    assert_eq!(first_last_good.health(), ScreenPublicationHealth::Degraded);

    let mut missing = fixture
        .hub
        .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &intent(2))
        .expect("second writable intent reserves");
    missing
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .fill(2);
    assert!(matches!(
        fixture.hub.finalize_publication(missing),
        Err(ScreenPublicationHubError::PublicationCompletionMissing)
    ));
    assert!(Arc::ptr_eq(
        &lease.read().expect("missing completion retains last-good"),
        &first_last_good
    ));

    let mut expired = fixture
        .hub
        .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &intent(2))
        .expect("missing completion returned its slot");
    expired
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .fill(3);
    assert!(matches!(
        fixture.hub.finalize_writable_publication(
            expired,
            freshness_deadline + Duration::from_nanos(1),
            ScreenPublicationHealth::Healthy,
        ),
        Err(ScreenPublicationHubError::PublicationFreshnessExpired)
    ));
    assert!(Arc::ptr_eq(
        &lease.read().expect("expired completion retains last-good"),
        &first_last_good
    ));

    let mut replacement = fixture
        .hub
        .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &intent(2))
        .expect("expired completion returned its slot");
    replacement
        .surface_pixels_mut()
        .expect("surface bytes are writable")
        .fill(4);
    let replacement_published_at = captured_at + Duration::from_millis(80);
    fixture
        .hub
        .finalize_writable_publication(
            replacement,
            replacement_published_at,
            ScreenPublicationHealth::Healthy,
        )
        .expect("returned slot accepts a fresh replacement");
    let replacement = lease.read().expect("replacement becomes last-good");
    assert_eq!(replacement.native_sequence().get(), 2);
    assert_eq!(replacement.published_at(), replacement_published_at);
}

#[test]
fn eight_k_slots_are_admitted_by_exact_bytes_without_resolution_caps() {
    const WIDTH: u64 = 7_680;
    const HEIGHT: u64 = 4_320;
    const PIXEL_BYTES: u64 = 4;
    let source = source(
        u32::try_from(WIDTH).expect("8K width fits u32"),
        u32::try_from(HEIGHT).expect("8K height fits u32"),
    );
    let demand = demand(&source, ScreenPublicationKind::Surface, 60);
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 2)
        .expect("three publication slots are valid");
    let mut builder = ScreenPlanBuilder::with_publication_slots(policy);
    let revision: InputPublicationDemandRevision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision advances");
    let preparing = builder
        .prepare(
            [demand],
            None,
            revision,
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("8K exact bytes fit the admitted capacity");
    let bytes_per_slot = WIDTH * HEIGHT * PIXEL_BYTES;
    let candidate = preparing.admission().candidate();
    assert_eq!(candidate.physical_plane_bytes(), bytes_per_slot);
    assert_eq!(candidate.publication_retention_bytes(), bytes_per_slot);
    assert_eq!(
        candidate.publication_subscriber_slot_bytes(),
        bytes_per_slot * 2
    );
    assert_eq!(
        candidate.publication_retention_bytes() + candidate.publication_subscriber_slot_bytes(),
        bytes_per_slot * u64::from(policy.total_slots())
    );
    assert_eq!(
        candidate.total_bytes(),
        bytes_per_slot * (u64::from(policy.total_slots()) + 1)
    );
    let _abort = preparing.abort();
}
