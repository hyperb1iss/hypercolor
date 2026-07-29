use std::cmp::Ordering;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    ArmedScreenPlan, CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureEpoch,
    CaptureGeometry, CaptureLuminanceContext, CapturePixelFormat, CapturePositiveScalar,
    CaptureRotation, CaptureSourceId, CaptureTransferFunction, CommittedScreenPlan,
    InputPublicationDemandRevision, KnownCaptureColorimetry, PhysicalOrigin, PixelExtent,
    PixelRect, PlatformGpuApi, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenPublicationDescriptor, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenBranchDeliveryLifecycle, ScreenBranchPayload, ScreenCaptureBackend, ScreenCapturePlan,
    ScreenColorTransformCapabilities, ScreenColorTuning, ScreenCompatibilitySelection,
    ScreenContentBarsPolicy, ScreenContinuityError, ScreenCursorCapabilities, ScreenCursorPolicy,
    ScreenExactResource, ScreenExactResourceLedger, ScreenExtentRequest, ScreenGridPolicy,
    ScreenHdrPolicy, ScreenInputGraphGeneration, ScreenLetterboxFill, ScreenPlanBuilder,
    ScreenPlanError, ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenProfileScalar,
    ScreenPublicationColorimetry, ScreenPublicationError, ScreenPublicationFreshness,
    ScreenPublicationHealth, ScreenPublicationHubError, ScreenPublicationKind,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenPublicationSlotPolicy,
    ScreenReductionFilter, ScreenResourceApi, ScreenResourceKind, ScreenResourceLifetime,
    ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenSourceReflection, ScreenSourceSelector,
    ScreenSurfacePayload, ScreenTargetColorimetry, ScreenToneMapOperator, ScreenToneMapPolicy,
    ScreenUnknownColorPolicy, ScreenUpscalePolicy, ScreenWorkerBinding, ScreenWorkerBindingState,
    ScreenWorkerPreparationTicket, ScreenZonesPayload, SourceScale,
};

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test values are non-zero")
}

fn pixel_extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extents are non-empty")
}

fn known_colorimetry(
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
) -> KnownCaptureColorimetry {
    KnownCaptureColorimetry::try_new(
        color_space,
        transfer_function,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("test colorimetry is complete")
}

fn colorimetry(
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
) -> CaptureColorimetry {
    CaptureColorimetry::from_known(known_colorimetry(color_space, transfer_function))
}

fn luminance(reference_white: f32, peak: f32) -> CaptureLuminanceContext {
    CaptureLuminanceContext::new(
        CapturePositiveScalar::try_new(reference_white).expect("test white is positive"),
        CapturePositiveScalar::try_new(peak).expect("test peak is positive"),
    )
    .expect("test luminance is ordered")
}

fn source_id(value: &str) -> CaptureSourceId {
    CaptureSourceId::new(value).expect("test source ids are non-empty")
}

fn capture_geometry(
    origin: PhysicalOrigin,
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
    source_scale: SourceScale,
) -> CaptureGeometry {
    CaptureGeometry::new(
        origin,
        native_extent,
        storage_extent,
        rotation,
        crop,
        source_scale,
    )
    .expect("test capture geometry is valid")
}

#[derive(Clone)]
struct SourceConfigParts {
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    pixel_format: CapturePixelFormat,
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
    resources: ScreenBackendResourceIdentity,
}

impl SourceConfigParts {
    fn build(self) -> ResolvedScreenSourceConfig {
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            self.geometry,
            self.logical_extent,
            self.reflection,
            self.pixel_format,
            colorimetry(self.color_space, self.transfer_function),
            ScreenCursorCapabilities::clean_with_separate_cursor(),
            self.resources,
        )
    }
}

fn source_config_parts(width: u32, height: u32) -> SourceConfigParts {
    let extent = pixel_extent(width, height);
    SourceConfigParts {
        geometry: capture_geometry(
            PhysicalOrigin::default(),
            extent,
            extent,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        ),
        logical_extent: extent,
        reflection: ScreenSourceReflection::None,
        pixel_format: CapturePixelFormat::Rgba8,
        color_space: CaptureColorSpace::Srgb,
        transfer_function: CaptureTransferFunction::Srgb,
        resources: ScreenBackendResourceIdentity::new(
            ScreenCaptureBackend::Synthetic,
            ScreenResourceApi::Cpu,
            1,
            1,
        ),
    }
}

fn resolved_source(
    selector: ScreenSourceSelector,
    id: &str,
    width: u32,
    height: u32,
) -> ResolvedScreenSource {
    resolved_source_with_config(selector, id, 1, 1, source_config_parts(width, height))
}

fn resolved_source_with_config(
    selector: ScreenSourceSelector,
    id: &str,
    topology_generation: u64,
    session_generation: u64,
    config: SourceConfigParts,
) -> ResolvedScreenSource {
    ResolvedScreenSource::new(
        selector,
        CaptureEpoch {
            source_id: source_id(id),
            topology_generation,
            session_generation,
        },
        config.build(),
    )
}

fn profile(config: ScreenProcessingProfileConfig) -> Arc<ScreenProcessingProfile> {
    Arc::new(ScreenProcessingProfile::new(config))
}

fn default_profile() -> Arc<ScreenProcessingProfile> {
    profile(ScreenProcessingProfileConfig::default())
}

fn registered(
    selector: ScreenSourceSelector,
    kind: ScreenPublicationKind,
    extent: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
    processing_profile: Arc<ScreenProcessingProfile>,
    requested_hz: u32,
) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(selector, kind, extent, aspect, processing_profile),
        non_zero(requested_hz),
    )
}

fn resolve(
    demand: &RegisteredScreenBranchDemand,
    source: &ResolvedScreenSource,
) -> ResolvedScreenBranchDemand {
    demand
        .resolve_with_color_capabilities(
            source,
            ScreenColorTransformCapabilities::new(
                true,
                true,
                true,
                demand.request().processing_profile().algorithm_revision(),
            ),
        )
        .expect("test publication request resolves")
}

fn output_extent(descriptor: &ResolvedScreenPublicationDescriptor) -> PixelExtent {
    descriptor.geometry().output_extent()
}

struct BoundExactResources {
    ledger: ScreenExactResourceLedger,
    lifetimes: Vec<ScreenResourceLifetime>,
}

impl BoundExactResources {
    fn acknowledge(
        self,
        ticket: &ScreenWorkerPreparationTicket,
    ) -> Result<hypercolor_core::input::screen::ScreenPreparedWorkerToken, ScreenPlanError> {
        let Self { ledger, lifetimes } = self;
        ticket.acknowledge(ledger, &lifetimes)
    }
}

fn exact_resources(
    ticket: &ScreenWorkerPreparationTicket,
) -> Result<BoundExactResources, ScreenPlanError> {
    bind_resources(
        ticket,
        ticket.required_minimums().iter().map(|expected| {
            ScreenExactResource::try_new(
                Arc::clone(expected.name()),
                expected.resource(),
                expected.minimum_bytes(),
            )
            .expect("ticket resource names are valid")
        }),
    )
}

fn bind_resources(
    ticket: &ScreenWorkerPreparationTicket,
    resources: impl IntoIterator<Item = ScreenExactResource>,
) -> Result<BoundExactResources, ScreenPlanError> {
    bind_ledger(ticket, ScreenExactResourceLedger::try_new(resources)?)
}

fn bind_ledger(
    ticket: &ScreenWorkerPreparationTicket,
    ledger: ScreenExactResourceLedger,
) -> Result<BoundExactResources, ScreenPlanError> {
    let lifetimes = ledger
        .resources()
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BoundExactResources { ledger, lifetimes })
}

fn required_scope(
    ticket: &ScreenWorkerPreparationTicket,
    resource: ScreenResourceKind,
) -> Arc<str> {
    Arc::clone(
        ticket
            .required_minimums()
            .iter()
            .find(|minimum| minimum.resource() == resource)
            .expect("ticket contains the requested accounting scope")
            .name(),
    )
}

fn next_demand_revision(builder: &ScreenPlanBuilder) -> InputPublicationDemandRevision {
    builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revisions remain representable")
}

fn binding_for(builder: &ScreenPlanBuilder, source: &CaptureSourceId) -> ScreenWorkerBinding {
    builder
        .committed_state()
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == source)
        .cloned()
        .expect("committed source has a worker binding")
}

fn descriptor_colorimetry(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> ScreenPublicationColorimetry {
    ScreenPublicationColorimetry::new(descriptor.physical().color_pipeline().output())
}

fn surface_payload<'a>(
    descriptor: &ResolvedScreenPublicationDescriptor,
    pixels: &'a [u8],
) -> ScreenBranchPayload<'a> {
    ScreenBranchPayload::Surface(
        ScreenSurfacePayload::try_new(
            output_extent(descriptor),
            descriptor.physical().target_pixel_format(),
            descriptor_colorimetry(descriptor),
            pixels,
        )
        .expect("test surface payload exactly matches its descriptor"),
    )
}

fn zones_payload<'a>(
    descriptor: &ResolvedScreenPublicationDescriptor,
    colors: &'a [[u8; 3]],
) -> ScreenBranchPayload<'a> {
    let ScreenPublicationKind::Zones { columns, rows } = descriptor.kind() else {
        panic!("test zones payload requires a zones descriptor");
    };
    ScreenBranchPayload::Zones(
        ScreenZonesPayload::try_new(columns, rows, descriptor_colorimetry(descriptor), colors)
            .expect("test zones payload exactly matches its descriptor"),
    )
}

fn publication_metadata(
    descriptor: &ResolvedScreenPublicationDescriptor,
    binding: &ScreenWorkerBinding,
    native_sequence: u64,
    captured_at: Instant,
    published_at: Instant,
    freshness_deadline: Instant,
    health: ScreenPublicationHealth,
) -> ScreenPublicationMetadata {
    ScreenPublicationMetadata::try_new(
        descriptor.source_epoch().clone(),
        binding.plan_generation(),
        NonZeroU64::new(native_sequence).expect("test native sequence is non-zero"),
        captured_at,
        published_at,
        freshness_deadline,
        health,
    )
    .expect("test publication metadata has a valid timeline")
}

fn commit_demands(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
    compatibility_descriptor: Option<&ResolvedScreenPublicationDescriptor>,
) -> Result<ScreenCapturePlan, ScreenPlanError> {
    let compatibility = compatibility_descriptor
        .map(|descriptor| ScreenCompatibilitySelection::try_new(descriptor.clone(), None))
        .transpose()?;
    commit_demands_with_compatibility(builder, demands, compatibility.as_ref())
}

fn commit_demands_with_compatibility(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
    compatibility: Option<&ScreenCompatibilitySelection>,
) -> Result<ScreenCapturePlan, ScreenPlanError> {
    commit_demands_outcome(builder, demands, compatibility).map(reclaim_committed)
}

fn commit_demands_with_retirement(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> Result<CommittedScreenPlan, ScreenPlanError> {
    commit_demands_outcome(builder, demands, None)
}

fn commit_demands_outcome(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
    compatibility: Option<&ScreenCompatibilitySelection>,
) -> Result<CommittedScreenPlan, ScreenPlanError> {
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = next_demand_revision(builder);
    let mut preparing = builder.prepare(
        demands,
        compatibility,
        demand_revision,
        graph_generation,
        ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
    )?;
    let required_sources = preparing.required_sources().to_vec();
    for source_id in required_sources {
        let ticket = preparing.worker_ticket(&source_id)?;
        let token = exact_resources(&ticket)?.acknowledge(&ticket)?;
        preparing.acknowledge(token)?;
    }
    let armed = preparing
        .arm(
            builder.current().generation(),
            demand_revision,
            graph_generation,
        )
        .map_err(|failure| failure.error().clone())?;
    builder
        .commit(armed, demand_revision, graph_generation)
        .map_err(|failure| failure.error().clone())
}

fn reclaim_committed(committed: CommittedScreenPlan) -> ScreenCapturePlan {
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved retired test pools reclaim immediately");
    plan
}

fn arm_demands(
    builder: &mut ScreenPlanBuilder,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
    graph_generation: ScreenInputGraphGeneration,
) -> ArmedScreenPlan {
    let demand_revision = next_demand_revision(builder);
    let mut preparing = builder
        .prepare(
            demands,
            None,
            demand_revision,
            graph_generation,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("candidate prepares");
    let required_sources = preparing.required_sources().to_vec();
    for source_id in required_sources {
        let ticket = preparing
            .worker_ticket(&source_id)
            .expect("required worker ticket is issued");
        let token = exact_resources(&ticket)
            .expect("ticket contract is representable")
            .acknowledge(&ticket)
            .expect("exact ledger covers the ticket");
        preparing
            .acknowledge(token)
            .expect("bound worker token is accepted");
    }
    preparing
        .arm(
            builder.current().generation(),
            demand_revision,
            graph_generation,
        )
        .expect("fully acknowledged candidate arms")
}

#[test]
fn ultrawide_and_portrait_requests_never_form_a_synthetic_union() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 3840, 2160);
    let ultrawide = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(5120)),
            Some(non_zero(720)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        60,
    );
    let portrait = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(1920)),
            Some(non_zero(2160)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        30,
    );

    let mut builder = ScreenPlanBuilder::new();
    let plan = commit_demands(
        &mut builder,
        [resolve(&ultrawide, &source), resolve(&portrait, &source)],
        None,
    )
    .expect("independent plan resolves");
    let outputs: Vec<_> = plan
        .branches()
        .iter()
        .map(|branch| output_extent(branch.descriptor()))
        .collect();

    assert_eq!(plan.branches().len(), 2);
    assert!(outputs.contains(&pixel_extent(5120, 720)));
    assert!(outputs.contains(&pixel_extent(1920, 2160)));
    assert!(!outputs.contains(&pixel_extent(5120, 2160)));
}

#[test]
fn selectors_are_control_inputs_while_exact_epochs_are_publication_identity() {
    let configured = resolved_source(ScreenSourceSelector::Configured, "same-display", 1920, 1080);
    let primary = resolved_source(ScreenSourceSelector::Primary, "same-display", 1920, 1080);
    let configured_demand = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        30,
    );
    let primary_demand = registered(
        ScreenSourceSelector::Primary,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        60,
    );

    let mut builder = ScreenPlanBuilder::new();
    let plan = commit_demands(
        &mut builder,
        [
            resolve(&configured_demand, &configured),
            resolve(&primary_demand, &primary),
        ],
        None,
    )
    .expect("equal resolved epochs group");

    assert_eq!(plan.branches().len(), 1);
    assert_eq!(plan.branches()[0].requested_hz(), non_zero(60));

    let different = resolved_source(ScreenSourceSelector::Primary, "other-display", 1920, 1080);
    let mut independent_builder = ScreenPlanBuilder::new();
    let independent = commit_demands(
        &mut independent_builder,
        [
            resolve(&configured_demand, &configured),
            resolve(&primary_demand, &different),
        ],
        None,
    )
    .expect("different exact source ids remain independent");
    assert_eq!(independent.branches().len(), 2);
}

#[test]
fn native_and_bounded_resolution_handle_each_axis_and_explicit_upscale() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 1920, 1080);
    let cases = [
        (
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            pixel_extent(1920, 1080),
        ),
        (
            ScreenExtentRequest::bounded(Some(non_zero(1280)), None, ScreenUpscalePolicy::Never),
            ScreenAspectPolicy::Contain,
            pixel_extent(1280, 720),
        ),
        (
            ScreenExtentRequest::bounded(None, Some(non_zero(720)), ScreenUpscalePolicy::Never),
            ScreenAspectPolicy::Contain,
            pixel_extent(1280, 720),
        ),
        (
            ScreenExtentRequest::bounded(
                Some(non_zero(1000)),
                Some(non_zero(1000)),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            pixel_extent(1000, 562),
        ),
        (
            ScreenExtentRequest::bounded(
                Some(non_zero(3840)),
                Some(non_zero(2160)),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            pixel_extent(1920, 1080),
        ),
        (
            ScreenExtentRequest::bounded(Some(non_zero(3840)), None, ScreenUpscalePolicy::Allow),
            ScreenAspectPolicy::Contain,
            pixel_extent(3840, 2160),
        ),
    ];

    for (extent, aspect, expected) in cases {
        let demand = registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            extent,
            aspect,
            default_profile(),
            30,
        );
        let descriptor = resolve(&demand, &source);
        assert_eq!(output_extent(descriptor.descriptor()), expected);
    }

    assert_eq!(
        ScreenExtentRequest::bounded(None, None, ScreenUpscalePolicy::Allow),
        ScreenExtentRequest::Native
    );
}

#[test]
fn extent_requests_are_structurally_canonical_for_every_axis_combination() {
    for upscale in [ScreenUpscalePolicy::Never, ScreenUpscalePolicy::Allow] {
        let native = ScreenExtentRequest::bounded(None, None, upscale);
        assert_eq!(native, ScreenExtentRequest::Native);
        assert_eq!(native.bounded_extent(), None);

        for (max_width, max_height) in [
            (Some(non_zero(1)), None),
            (None, Some(non_zero(u32::MAX))),
            (Some(non_zero(u32::MAX)), Some(non_zero(1))),
        ] {
            let request = ScreenExtentRequest::bounded(max_width, max_height, upscale);
            let bounds = request
                .bounded_extent()
                .expect("at least one bound is structurally present");
            assert_eq!(bounds.max_width(), max_width);
            assert_eq!(bounds.max_height(), max_height);
            assert_eq!(bounds.upscale(), upscale);
        }
    }
}

#[test]
fn cover_resolves_exact_outputs_and_never_upscales() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 1920, 1080);
    let allowed = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(1000)),
            Some(non_zero(1000)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        30,
    );
    let never = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(4000)),
            Some(non_zero(4000)),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        30,
    );

    let allowed = resolve(&allowed, &source);
    let allowed_geometry = allowed.descriptor().geometry();
    assert_eq!(allowed_geometry.output_extent(), pixel_extent(1000, 1000));
    let allowed_region = allowed_geometry.source_region();
    assert_eq!(allowed_region.x().numerator(), 420);
    assert_eq!(allowed_region.x().denominator().get(), 1);
    assert_eq!(allowed_region.width().numerator(), 1080);
    assert_eq!(allowed_region.width().denominator().get(), 1);
    assert_eq!(allowed_region.height().numerator(), 1080);
    assert_eq!(allowed_region.height().denominator().get(), 1);

    let never = resolve(&never, &source);
    let never_geometry = never.descriptor().geometry();
    assert_eq!(never_geometry.output_extent(), pixel_extent(1080, 1080));
    assert!(never_geometry.output_extent().width() <= source.logical_extent().width());
    assert!(never_geometry.output_extent().height() <= source.logical_extent().height());
}

#[test]
fn cover_preserves_an_exact_subpixel_window_for_odd_geometry() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-odd", 4, 3);
    let demand = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(3)),
            Some(non_zero(2)),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        30,
    );
    let resolved = resolve(&demand, &source);
    let geometry = resolved.descriptor().geometry();
    let region = geometry.source_region();

    assert_eq!(geometry.output_extent(), pixel_extent(3, 2));
    assert_eq!(
        (region.x().numerator(), region.x().denominator().get()),
        (0, 1)
    );
    assert_eq!(
        (region.y().numerator(), region.y().denominator().get()),
        (1, 6)
    );
    assert_eq!(
        (
            region.width().numerator(),
            region.width().denominator().get()
        ),
        (4, 1)
    );
    assert_eq!(
        (
            region.height().numerator(),
            region.height().denominator().get()
        ),
        (8, 3)
    );
}

#[test]
fn surface_and_zone_branches_are_independent_without_grid_driven_upscale() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 3840, 2160);
    let extent = ScreenExtentRequest::bounded(
        Some(non_zero(320)),
        Some(non_zero(180)),
        ScreenUpscalePolicy::Never,
    );
    let surface = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        extent,
        ScreenAspectPolicy::Contain,
        default_profile(),
        60,
    );
    let zones = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Zones {
            columns: non_zero(10_000),
            rows: non_zero(10_000),
        },
        extent,
        ScreenAspectPolicy::Contain,
        default_profile(),
        20,
    );
    let finer_zones = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Zones {
            columns: non_zero(20_000),
            rows: non_zero(20_000),
        },
        extent,
        ScreenAspectPolicy::Contain,
        default_profile(),
        20,
    );
    let mut builder = ScreenPlanBuilder::new();
    let plan = commit_demands(
        &mut builder,
        [
            resolve(&surface, &source),
            resolve(&zones, &source),
            resolve(&finer_zones, &source),
        ],
        None,
    )
    .expect("surface and zones resolve independently");

    assert_eq!(plan.branches().len(), 3);
    assert!(
        plan.branches()
            .iter()
            .all(|branch| output_extent(branch.descriptor()) == pixel_extent(320, 180))
    );
}

#[test]
fn every_processing_profile_field_participates_in_exact_grouping() {
    let scalar = |value| ScreenProfileScalar::try_new(value).expect("finite test scalar");
    let configs = vec![
        ScreenProcessingProfileConfig::default(),
        ScreenProcessingProfileConfig {
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: scalar(0.01),
            },
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: scalar(0.02),
            },
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            letterbox_fill: ScreenLetterboxFill::Transparent,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            letterbox_fill: ScreenLetterboxFill::Solid([1, 0, 0, u8::MAX]),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(80),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(81),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(80),
                scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
                    threshold: scalar(0.25),
                },
            },
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(80),
                scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
                    threshold: scalar(0.5),
                },
            },
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(1.1, 1.0, 1.0).expect("finite tuning"),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(1.0, 1.1, 1.0).expect("finite tuning"),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(1.0, 1.0, 1.1).expect("finite tuning"),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            cursor: ScreenCursorPolicy::Include,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            grid: ScreenGridPolicy::PointSample,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Bilinear,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            target_pixel_format: CapturePixelFormat::Bgra8,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            target_colorimetry: ScreenTargetColorimetry::ConvertTo(known_colorimetry(
                CaptureColorSpace::DisplayP3,
                CaptureTransferFunction::Srgb,
            )),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            unknown_color: ScreenUnknownColorPolicy::Assume(known_colorimetry(
                CaptureColorSpace::DisplayP3,
                CaptureTransferFunction::Srgb,
            )),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::new(
                ScreenToneMapOperator::Bt2390Eetf,
                luminance(100.0, 100.0),
            )),
            ..ScreenProcessingProfileConfig::default()
        },
        ScreenProcessingProfileConfig {
            algorithm_revision: non_zero(2),
            ..ScreenProcessingProfileConfig::default()
        },
    ];

    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 1920, 1080);
    let demands = configs
        .into_iter()
        .map(|config| {
            resolve(
                &registered(
                    ScreenSourceSelector::Configured,
                    ScreenPublicationKind::Surface,
                    ScreenExtentRequest::Native,
                    ScreenAspectPolicy::Contain,
                    profile(config),
                    30,
                ),
                &source,
            )
        })
        .collect::<Vec<_>>();
    let expected_count = demands.len();
    let mut builder = ScreenPlanBuilder::new();
    let plan = commit_demands(&mut builder, demands, None).expect("profiles remain independent");

    assert_eq!(plan.branches().len(), expected_count);
}

#[test]
fn profile_scalars_reject_non_finite_values_and_canonicalize_zero() {
    assert_eq!(
        ScreenProfileScalar::try_new(0.0).expect("positive zero is finite"),
        ScreenProfileScalar::try_new(-0.0).expect("negative zero is finite")
    );
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            ScreenProfileScalar::try_new(invalid),
            Err(ScreenPublicationError::NonFiniteProfileScalar)
        );
    }
    assert_eq!(
        ScreenColorTuning::try_new(1.0, f32::NAN, 1.0),
        Err(ScreenPublicationError::NonFiniteProfileScalar)
    );
}

#[test]
fn complete_source_and_output_fields_prevent_false_sharing() {
    let base = source_config_parts(1920, 1080);
    let base_geometry = base.geometry;
    let base_source = resolved_source_with_config(
        ScreenSourceSelector::Configured,
        "display-a",
        1,
        1,
        base.clone(),
    );
    let mut source_variants = vec![
        resolved_source_with_config(
            ScreenSourceSelector::Configured,
            "display-b",
            1,
            1,
            base.clone(),
        ),
        resolved_source_with_config(
            ScreenSourceSelector::Configured,
            "display-a",
            2,
            1,
            base.clone(),
        ),
        resolved_source_with_config(
            ScreenSourceSelector::Configured,
            "display-a",
            1,
            2,
            base.clone(),
        ),
    ];

    let mut native_width = base.clone();
    native_width.geometry = capture_geometry(
        base_geometry.origin(),
        pixel_extent(1921, 1080),
        base_geometry.storage_extent(),
        base_geometry.rotation(),
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let mut native_height = base.clone();
    native_height.geometry = capture_geometry(
        base_geometry.origin(),
        pixel_extent(1920, 1081),
        base_geometry.storage_extent(),
        base_geometry.rotation(),
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let mut storage_width = base.clone();
    storage_width.geometry = capture_geometry(
        base_geometry.origin(),
        base_geometry.native_extent(),
        pixel_extent(1919, 1080),
        base_geometry.rotation(),
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let mut storage_height = base.clone();
    storage_height.geometry = capture_geometry(
        base_geometry.origin(),
        base_geometry.native_extent(),
        pixel_extent(1920, 1079),
        base_geometry.rotation(),
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let mut origin_x = base.clone();
    origin_x.geometry = capture_geometry(
        PhysicalOrigin { x: 1, y: 0 },
        base_geometry.native_extent(),
        base_geometry.storage_extent(),
        base_geometry.rotation(),
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let mut origin_y = base.clone();
    origin_y.geometry = capture_geometry(
        PhysicalOrigin { x: 0, y: -1 },
        base_geometry.native_extent(),
        base_geometry.storage_extent(),
        base_geometry.rotation(),
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let mut rotation = base.clone();
    rotation.geometry = capture_geometry(
        base_geometry.origin(),
        base_geometry.native_extent(),
        base_geometry.storage_extent(),
        CaptureRotation::Clockwise90,
        base_geometry.crop(),
        base_geometry.source_scale(),
    );
    let crop_geometry = |crop| {
        capture_geometry(
            base_geometry.origin(),
            base_geometry.native_extent(),
            base_geometry.storage_extent(),
            base_geometry.rotation(),
            Some(crop),
            base_geometry.source_scale(),
        )
    };
    let mut crop_presence = base.clone();
    crop_presence.geometry = crop_geometry(PixelRect::new(0, 0, 1900, 1000).expect("test crop"));
    let mut crop_x = base.clone();
    crop_x.geometry = crop_geometry(PixelRect::new(1, 0, 1900, 1000).expect("test crop"));
    let mut crop_y = base.clone();
    crop_y.geometry = crop_geometry(PixelRect::new(0, 1, 1900, 1000).expect("test crop"));
    let mut crop_width = base.clone();
    crop_width.geometry = crop_geometry(PixelRect::new(0, 0, 1901, 1000).expect("test crop"));
    let mut crop_height = base.clone();
    crop_height.geometry = crop_geometry(PixelRect::new(0, 0, 1900, 1001).expect("test crop"));
    let mut scale_numerator = base.clone();
    scale_numerator.geometry = capture_geometry(
        base_geometry.origin(),
        base_geometry.native_extent(),
        base_geometry.storage_extent(),
        base_geometry.rotation(),
        base_geometry.crop(),
        SourceScale::new(2, 1).expect("test scale is valid"),
    );
    let mut scale_denominator = base.clone();
    scale_denominator.geometry = capture_geometry(
        base_geometry.origin(),
        base_geometry.native_extent(),
        base_geometry.storage_extent(),
        base_geometry.rotation(),
        base_geometry.crop(),
        SourceScale::new(1, 2).expect("test scale is valid"),
    );
    let mut logical_width = base.clone();
    logical_width.logical_extent = pixel_extent(1921, 1080);
    let mut logical_height = base.clone();
    logical_height.logical_extent = pixel_extent(1920, 1081);
    let mut reflection = base.clone();
    reflection.reflection = ScreenSourceReflection::Horizontal;
    let mut pixel_format = base.clone();
    pixel_format.pixel_format = CapturePixelFormat::Bgra8;
    let mut color_space = base.clone();
    color_space.color_space = CaptureColorSpace::DisplayP3;
    let mut transfer_function = base.clone();
    transfer_function.transfer_function = CaptureTransferFunction::Linear;
    let mut backend = base.clone();
    backend.resources = ScreenBackendResourceIdentity::new(
        ScreenCaptureBackend::WaylandPipeWire,
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        1,
        1,
    );
    let mut api = base.clone();
    api.resources = ScreenBackendResourceIdentity::new(
        ScreenCaptureBackend::WindowsDesktopDuplication,
        ScreenResourceApi::Cpu,
        1,
        1,
    );
    let mut platform_api = base.clone();
    platform_api.resources = ScreenBackendResourceIdentity::new(
        ScreenCaptureBackend::WindowsDesktopDuplication,
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Vulkan),
        1,
        1,
    );
    let mut device_generation = base.clone();
    device_generation.resources = ScreenBackendResourceIdentity::new(
        ScreenCaptureBackend::WindowsDesktopDuplication,
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        2,
        1,
    );
    let mut resource_generation = base.clone();
    resource_generation.resources = ScreenBackendResourceIdentity::new(
        ScreenCaptureBackend::WindowsDesktopDuplication,
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        1,
        2,
    );

    source_variants.extend(
        [
            native_width,
            native_height,
            storage_width,
            storage_height,
            origin_x,
            origin_y,
            rotation,
            crop_presence,
            crop_x,
            crop_y,
            crop_width,
            crop_height,
            scale_numerator,
            scale_denominator,
            logical_width,
            logical_height,
            reflection,
            pixel_format,
            color_space,
            transfer_function,
            backend,
            api,
            platform_api,
            device_generation,
            resource_generation,
        ]
        .into_iter()
        .map(|config| {
            resolved_source_with_config(ScreenSourceSelector::Configured, "display-a", 1, 1, config)
        }),
    );
    let base_request = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(640)),
            Some(non_zero(480)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        30,
    );
    let mut demands = vec![resolve(&base_request, &base_source)];
    demands.extend(
        source_variants
            .iter()
            .map(|source| resolve(&base_request, source)),
    );
    demands.push(resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(16),
                rows: non_zero(9),
            },
            base_request.request().extent(),
            base_request.request().aspect(),
            default_profile(),
            30,
        ),
        &base_source,
    ));
    demands.push(resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            base_request.request().extent(),
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &base_source,
    ));

    for (index, left) in demands.iter().enumerate() {
        for right in demands.iter().skip(index + 1) {
            assert_ne!(left.descriptor(), right.descriptor());
            assert_ne!(left.descriptor().cmp(right.descriptor()), Ordering::Equal);
        }
    }

    let expected_count = demands.len();
    let mut builder = ScreenPlanBuilder::new();
    let plan = commit_demands(&mut builder, demands, None)
        .expect("complete descriptors remain independent");
    assert_eq!(plan.branches().len(), expected_count);
    assert!(plan.branches().iter().any(|branch| {
        branch.descriptor().aspect() == ScreenAspectPolicy::Cover
            && output_extent(branch.descriptor()) == pixel_extent(640, 480)
    }));
    assert!(plan.branches().iter().any(|branch| {
        branch.descriptor().aspect() == ScreenAspectPolicy::Contain
            && output_extent(branch.descriptor()) == pixel_extent(640, 360)
    }));
}

#[test]
fn grouped_matrix_matches_independent_resolution_in_deterministic_order() {
    let sources = [
        resolved_source(ScreenSourceSelector::Configured, "display-a", 3440, 1440),
        resolved_source(ScreenSourceSelector::Primary, "display-b", 1080, 1920),
    ];
    let extents = [
        ScreenExtentRequest::Native,
        ScreenExtentRequest::bounded(Some(non_zero(1280)), None, ScreenUpscalePolicy::Never),
        ScreenExtentRequest::bounded(None, Some(non_zero(720)), ScreenUpscalePolicy::Allow),
        ScreenExtentRequest::bounded(
            Some(non_zero(1024)),
            Some(non_zero(1024)),
            ScreenUpscalePolicy::Never,
        ),
    ];
    let kinds = [
        ScreenPublicationKind::Surface,
        ScreenPublicationKind::Zones {
            columns: non_zero(32),
            rows: non_zero(18),
        },
    ];
    let aspects = [ScreenAspectPolicy::Contain, ScreenAspectPolicy::Cover];
    let alternate_config = ScreenProcessingProfileConfig {
        cursor: ScreenCursorPolicy::Include,
        ..ScreenProcessingProfileConfig::default()
    };
    let profiles = [default_profile(), profile(alternate_config)];

    let mut demands = Vec::new();
    for source in &sources {
        for extent in extents {
            for kind in kinds {
                for aspect in aspects {
                    for processing_profile in &profiles {
                        let logical = registered(
                            source.selector().clone(),
                            kind,
                            extent,
                            aspect,
                            Arc::clone(processing_profile),
                            20,
                        );
                        let resolved = resolve(&logical, source);
                        demands.push(resolved.clone());

                        let duplicate = registered(
                            source.selector().clone(),
                            kind,
                            extent,
                            aspect,
                            Arc::clone(processing_profile),
                            60,
                        );
                        demands.push(resolve(&duplicate, source));
                    }
                }
            }
        }
    }

    let mut expected: Vec<_> = demands
        .iter()
        .map(|demand| demand.descriptor().clone())
        .collect();
    expected.sort_unstable();
    expected.dedup();

    let mut forward_builder = ScreenPlanBuilder::new();
    let forward = commit_demands(&mut forward_builder, demands.clone(), None)
        .expect("forward matrix resolves");
    demands.reverse();
    let mut reverse_builder = ScreenPlanBuilder::new();
    let reverse =
        commit_demands(&mut reverse_builder, demands, None).expect("reverse matrix resolves");
    let actual: Vec<_> = forward
        .branches()
        .iter()
        .map(|branch| branch.descriptor().clone())
        .collect();

    assert_eq!(actual, expected);
    assert_eq!(forward, reverse);
    assert!(
        forward
            .branches()
            .iter()
            .all(|branch| branch.requested_hz() == non_zero(60))
    );
}

#[test]
fn physical_reductions_share_only_complete_compatible_work() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 640, 360);
    let tuned_config = ScreenProcessingProfileConfig {
        tuning: ScreenColorTuning::try_new(1.25, 1.0, 1.0).expect("test tuning is finite"),
        ..ScreenProcessingProfileConfig::default()
    };
    let cursor_config = ScreenProcessingProfileConfig {
        cursor: ScreenCursorPolicy::Include,
        ..ScreenProcessingProfileConfig::default()
    };
    let filter_config = ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Bilinear,
        ..ScreenProcessingProfileConfig::default()
    };
    let demands = [
        registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(16),
                rows: non_zero(9),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            profile(tuned_config),
            60,
        ),
        registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            profile(cursor_config),
            20,
        ),
        registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            profile(filter_config),
            10,
        ),
    ]
    .map(|demand| resolve(&demand, &source));
    let mut builder = ScreenPlanBuilder::new();
    let plan = commit_demands(&mut builder, demands, None).expect("compatible work groups");

    assert_eq!(plan.branches().len(), 4);
    assert_eq!(plan.physical_reductions().len(), 3);
    let shared = plan
        .physical_reductions()
        .iter()
        .find(|reduction| reduction.branch_indices().len() == 2)
        .expect("surface and post-analysis zone branch share reduction");
    let shared_branches = shared
        .branch_indices()
        .iter()
        .map(|index| plan.branches()[*index].descriptor())
        .collect::<Vec<_>>();
    assert_ne!(shared_branches[0], shared_branches[1]);
    assert_eq!(shared_branches[0].physical(), shared_branches[1].physical());
    assert_eq!(shared.requested_hz(), non_zero(60));
    assert!(plan.physical_reductions().iter().any(|reduction| {
        reduction.descriptor().cursor() == ScreenCursorPolicy::Include
            && reduction.branch_indices().len() == 1
    }));
    assert!(plan.physical_reductions().iter().any(|reduction| {
        reduction.descriptor().reduction_filter() == ScreenReductionFilter::Bilinear
            && reduction.branch_indices().len() == 1
    }));
}

#[test]
fn admission_counts_shared_physical_work_and_writable_publication_slots() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    let revision = next_demand_revision(&builder);
    let preparing = builder
        .prepare(
            [surface, zones],
            None,
            revision,
            ScreenInputGraphGeneration::new(7),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("small shared plan is admitted");
    let ledger = preparing.admission().candidate();

    assert_eq!(preparing.candidate_plan().physical_reductions().len(), 1);
    assert_eq!(ledger.physical_pixels(), 12);
    assert_eq!(ledger.physical_row_stride_bytes(), 16);
    assert_eq!(ledger.physical_plane_bytes(), 48);
    assert_eq!(ledger.publication_retention_bytes(), 60);
    assert_eq!(ledger.publication_subscriber_slot_bytes(), 120);
    assert_eq!(ledger.total_bytes(), 228);
    assert_eq!(preparing.admission().active().total_bytes(), 0);
    assert_eq!(preparing.admission().staged(), ledger);
    assert_eq!(preparing.admission().overlap(), ledger);
}

#[test]
fn gpu_surface_admission_does_not_reserve_cpu_publication_planes() {
    let mut config = source_config_parts(7680, 4320);
    config.resources = ScreenBackendResourceIdentity::new(
        ScreenCaptureBackend::WindowsDesktopDuplication,
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
        9,
        17,
    );
    let source = resolved_source_with_config(
        ScreenSourceSelector::Configured,
        "gpu-display",
        3,
        5,
        config,
    );
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    let revision = next_demand_revision(&builder);
    let preparing = builder
        .prepare(
            [surface],
            None,
            revision,
            ScreenInputGraphGeneration::new(11),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("8K GPU publication is admitted by checked resources");
    let ledger = preparing.admission().candidate();

    assert_eq!(ledger.publication_retention_bytes(), 0);
    assert_eq!(ledger.publication_subscriber_slot_bytes(), 0);
    assert_eq!(ledger.physical_plane_bytes(), 0);
    assert_eq!(ledger.total_bytes(), 0);
}

#[test]
fn unchanged_and_replacement_admission_use_exact_transition_overlap() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    commit_demands(&mut builder, [surface.clone()], None).expect("surface becomes active");

    let revision = next_demand_revision(&builder);
    let unchanged = builder
        .prepare(
            [surface],
            None,
            revision,
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("unchanged plan reuses every resource");
    assert_eq!(unchanged.admission().active().total_bytes(), 144);
    assert_eq!(unchanged.admission().candidate().total_bytes(), 144);
    assert_eq!(unchanged.admission().active().physical_plane_bytes(), 0);
    assert_eq!(unchanged.admission().candidate().physical_plane_bytes(), 0);
    assert_eq!(unchanged.admission().staged().total_bytes(), 0);
    assert_eq!(unchanged.admission().overlap().total_bytes(), 144);
    assert_eq!(
        unchanged.admission().active().publication_retention_bytes(),
        48
    );
    assert_eq!(
        unchanged
            .admission()
            .active()
            .publication_subscriber_slot_bytes(),
        96
    );
    assert!(unchanged.required_sources().is_empty());

    let revision = next_demand_revision(&builder);
    let replacement = builder
        .prepare(
            [zones],
            None,
            revision,
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("replacement shares the physical reduction");
    assert_eq!(replacement.admission().active().total_bytes(), 144);
    assert_eq!(replacement.admission().candidate().total_bytes(), 84);
    assert_eq!(replacement.admission().staged().total_bytes(), 84);
    assert_eq!(replacement.admission().staged().physical_plane_bytes(), 48);
    assert_eq!(replacement.admission().overlap().total_bytes(), 228);
    assert_eq!(
        replacement
            .admission()
            .staged()
            .publication_retention_bytes(),
        12
    );
    assert_eq!(
        replacement
            .admission()
            .staged()
            .publication_subscriber_slot_bytes(),
        24
    );
}

#[test]
fn admission_reports_explicit_budget_backend_and_arithmetic_failures() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    for (capacity, expected_resource) in [
        (
            ScreenAdmissionCapacity::new(143, u64::MAX),
            ScreenResourceKind::ByteBudget,
        ),
        (
            ScreenAdmissionCapacity::new(u64::MAX, 143),
            ScreenResourceKind::BackendCapacity,
        ),
    ] {
        let mut builder = ScreenPlanBuilder::new();
        let revision = next_demand_revision(&builder);
        let error = builder
            .prepare(
                [surface.clone()],
                None,
                revision,
                ScreenInputGraphGeneration::new(1),
                capacity,
            )
            .expect_err("144-byte admitted plan exceeds the explicit capacity");
        match error {
            ScreenPlanError::ResourceExhausted {
                descriptor,
                resource,
                requested,
                available,
            } => {
                assert_eq!(descriptor.as_ref(), surface.descriptor());
                assert_eq!(resource, expected_resource);
                assert_eq!(requested, 144);
                assert_eq!(available, 143);
            }
            other => panic!("unexpected admission error: {other:?}"),
        }
    }

    let maximum = resolved_source(
        ScreenSourceSelector::Configured,
        "maximum-display",
        u32::MAX,
        u32::MAX,
    );
    let maximum_surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &maximum,
    );
    let mut builder = ScreenPlanBuilder::new();
    let revision = next_demand_revision(&builder);
    let error = builder
        .prepare(
            [maximum_surface.clone()],
            None,
            revision,
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect_err("u32::MAX squared RGBA cannot fit the byte ledger");
    match error {
        ScreenPlanError::ArithmeticOverflow {
            descriptor,
            resource,
            requested,
            available,
        } => {
            assert_eq!(descriptor.as_ref(), maximum_surface.descriptor());
            assert_eq!(resource, ScreenResourceKind::PublicationRetention);
            assert_eq!(requested, u128::from(u32::MAX) * u128::from(u32::MAX) * 4);
            assert_eq!(available, u64::MAX);
        }
        other => panic!("unexpected maximum-resolution error: {other:?}"),
    }
}

#[test]
fn preparation_wait_arm_and_abort_never_replace_the_active_plan() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    commit_demands(&mut builder, [surface], None).expect("surface becomes active");
    let active = builder.current().clone();
    let graph = ScreenInputGraphGeneration::new(17);
    let revision = next_demand_revision(&builder);
    let preparing = builder
        .prepare(
            [zones],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("replacement prepares");
    assert_eq!(preparing.active_plan(), active.as_ref());
    assert_eq!(builder.current(), active);

    let awaiting = preparing.await_backend();
    assert_eq!(awaiting.active_plan(), active.as_ref());
    assert_eq!(builder.current(), active);
    let preparing = awaiting.backend_ready();
    let failure = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect_err("worker acknowledgement is required before arming");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::MissingWorkerAcknowledgement {
            source_id: observed_source,
        } if observed_source == &source_id("display-a")
    ));
    let abort = failure.into_preparing().abort();
    assert_eq!(abort.active_plan(), active.as_ref());
    assert!(abort.prepared_tokens().is_empty());
    assert_eq!(builder.current(), active);
}

#[test]
fn worker_tokens_are_candidate_transaction_source_and_nonce_bound() {
    let source_a = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let source_b = resolved_source(ScreenSourceSelector::Configured, "display-b", 4, 3);
    let request = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        60,
    );
    let demand_a = resolve(&request, &source_a);
    let demand_b = resolve(&request, &source_b);
    let graph = ScreenInputGraphGeneration::new(1);
    let capacity = ScreenAdmissionCapacity::new(u64::MAX, u64::MAX);

    let mut builder = ScreenPlanBuilder::new();
    let revision = next_demand_revision(&builder);
    let mut target = builder
        .prepare([demand_a.clone()], None, revision, graph, capacity)
        .expect("target prepares");
    let mut foreign_candidate = builder
        .prepare([demand_a, demand_b], None, revision, graph, capacity)
        .expect("foreign candidate prepares");
    assert_eq!(
        target.candidate_plan().generation(),
        foreign_candidate.candidate_plan().generation()
    );
    assert_ne!(target.transaction_id(), foreign_candidate.transaction_id());

    let target_ticket = target
        .worker_ticket(&source_id("display-a"))
        .expect("target ticket is issued");
    assert!(target_ticket.worker_nonce().get() > 0);
    assert!(matches!(
        target.worker_ticket(&source_id("display-a")),
        Err(ScreenPlanError::WorkerTicketAlreadyIssued { .. })
    ));
    let foreign_ticket = foreign_candidate
        .worker_ticket(&source_id("display-a"))
        .expect("same-source foreign candidate ticket is issued");
    let foreign_token = exact_resources(&foreign_ticket)
        .expect("foreign contract is representable")
        .acknowledge(&foreign_ticket)
        .expect("foreign exact ledger covers its ticket");
    assert!(matches!(
        target.acknowledge(foreign_token),
        Err(ScreenPlanError::WorkerCandidateMismatch { .. })
    ));
    let target_token = exact_resources(&target_ticket)
        .expect("target contract is representable")
        .acknowledge(&target_ticket)
        .expect("target exact ledger covers its ticket");
    target
        .acknowledge(target_token)
        .expect("issued candidate capability and nonce are accepted");
}

#[test]
fn resource_lifetimes_are_exact_and_ticket_bound_without_arming_on_failure() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let graph = ScreenInputGraphGeneration::new(1);
    let capacity = ScreenAdmissionCapacity::new(u64::MAX, u64::MAX);
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let revision = next_demand_revision(&builder);
    let mut target = builder
        .prepare([demand.clone()], None, revision, graph, capacity)
        .expect("target prepares");
    let mut foreign = builder
        .prepare([demand], None, revision, graph, capacity)
        .expect("foreign candidate prepares");
    let ticket = target
        .worker_ticket(&source_id("display-a"))
        .expect("target ticket is issued");
    let foreign_ticket = foreign
        .worker_ticket(&source_id("display-a"))
        .expect("foreign ticket is issued");
    let bound = exact_resources(&ticket).expect("target resources bind");
    let last = bound
        .lifetimes
        .len()
        .checked_sub(1)
        .expect("surface ticket has required resources");

    assert!(matches!(
        ticket.acknowledge(bound.ledger.clone(), &bound.lifetimes[..last]),
        Err(ScreenPlanError::MissingResourceLifetime { .. })
    ));

    let mut duplicate = bound.lifetimes.clone();
    duplicate.push(bound.lifetimes[0].clone());
    assert!(matches!(
        ticket.acknowledge(bound.ledger.clone(), &duplicate),
        Err(ScreenPlanError::DuplicateResourceLifetime { .. })
    ));

    let mut foreign_lifetimes = bound.lifetimes.clone();
    foreign_lifetimes[0] = foreign_ticket
        .bind_resource_lifetime(bound.lifetimes[0].resource())
        .expect("foreign allocation lifetime binds");
    assert!(matches!(
        ticket.acknowledge(bound.ledger.clone(), &foreign_lifetimes),
        Err(ScreenPlanError::ResourceLifetimeTicketMismatch { .. })
    ));

    let expected = &bound.ledger.resources()[0];
    let mismatched = ScreenExactResource::try_new_scoped(
        Arc::clone(expected.name()),
        Arc::clone(expected.accounting_scope()),
        expected.resource(),
        expected.bytes() + 1,
    )
    .expect("mismatched lifetime description is structurally valid");
    let mut mismatched_lifetimes = bound.lifetimes.clone();
    mismatched_lifetimes[0] = ticket
        .bind_resource_lifetime(&mismatched)
        .expect("mismatched allocation lifetime binds");
    assert!(matches!(
        ticket.acknowledge(bound.ledger.clone(), &mismatched_lifetimes),
        Err(ScreenPlanError::ResourceLifetimeAccountingMismatch { .. })
    ));

    let unexpected = ScreenExactResource::try_new_scoped(
        "unexpected-allocation",
        Arc::clone(expected.accounting_scope()),
        ScreenResourceKind::WorkerAdditional,
        1,
    )
    .expect("unexpected resource is structurally valid");
    let mut unexpected_lifetimes = bound.lifetimes.clone();
    unexpected_lifetimes.push(
        ticket
            .bind_resource_lifetime(&unexpected)
            .expect("unexpected allocation lifetime binds"),
    );
    assert!(matches!(
        ticket.acknowledge(bound.ledger, &unexpected_lifetimes),
        Err(ScreenPlanError::UnexpectedResourceLifetime { .. })
    ));
    assert_eq!(hub.pending_retired_bytes(), 0);
}

#[test]
fn exact_worker_attestation_requires_minimums_and_counts_disjoint_extras() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            profile(ScreenProcessingProfileConfig {
                tuning: ScreenColorTuning::try_new(1.1, 1.0, 1.0).expect("test tuning is finite"),
                ..ScreenProcessingProfileConfig::default()
            }),
            60,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [surface],
            None,
            revision,
            ScreenInputGraphGeneration::new(3),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("candidate prepares");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("worker ticket is issued");
    let backend_scope = required_scope(&ticket, ScreenResourceKind::BackendAllocation);
    let profile_scope = required_scope(&ticket, ScreenResourceKind::ProcessingProfileState);
    assert_eq!(ticket.required_minimums().len(), 4);
    for required_kind in [
        ScreenResourceKind::BackendAllocation,
        ScreenResourceKind::ApiAllocation,
        ScreenResourceKind::ProcessingProfileState,
    ] {
        assert!(
            ticket
                .required_minimums()
                .iter()
                .any(|minimum| minimum.resource() == required_kind)
        );
    }

    let omitted = ScreenExactResourceLedger::try_new([]).expect("empty ledger is representable");
    assert!(matches!(
        bind_ledger(&ticket, omitted)
            .expect("empty resource set binds")
            .acknowledge(&ticket),
        Err(ScreenPlanError::MissingExactResource { .. })
    ));

    let first = &ticket.required_minimums()[0];
    let partial = ScreenExactResourceLedger::try_new([ScreenExactResource::try_new(
        Arc::clone(first.name()),
        first.resource(),
        first.minimum_bytes(),
    )
    .expect("ticket name is valid")])
    .expect("partial ledger is representable");
    assert!(matches!(
        bind_ledger(&ticket, partial)
            .expect("partial resource set binds")
            .acknowledge(&ticket),
        Err(ScreenPlanError::MissingExactResource { .. })
    ));

    for omitted_kind in [
        ScreenResourceKind::BackendAllocation,
        ScreenResourceKind::ApiAllocation,
        ScreenResourceKind::ProcessingProfileState,
    ] {
        let omitted_domain = ScreenExactResourceLedger::try_new(
            ticket
                .required_minimums()
                .iter()
                .filter(|minimum| minimum.resource() != omitted_kind)
                .map(|minimum| {
                    ScreenExactResource::try_new(
                        Arc::clone(minimum.name()),
                        minimum.resource(),
                        minimum.minimum_bytes(),
                    )
                    .expect("ticket name is valid")
                }),
        )
        .expect("domain-omitted ledger is representable");
        assert!(matches!(
            bind_ledger(&ticket, omitted_domain)
                .expect("domain-omitted resource set binds")
                .acknowledge(&ticket),
            Err(ScreenPlanError::MissingExactResource { resource, .. })
                if resource == omitted_kind
        ));
    }

    let duplicate = ScreenExactResourceLedger::try_new(
        ticket
            .required_minimums()
            .iter()
            .map(|minimum| {
                ScreenExactResource::try_new(
                    Arc::clone(minimum.name()),
                    minimum.resource(),
                    minimum.minimum_bytes(),
                )
                .expect("ticket name is valid")
            })
            .chain([
                ScreenExactResource::try_new_scoped(
                    "worker-extra-buffer",
                    Arc::clone(&backend_scope),
                    ScreenResourceKind::WorkerAdditional,
                    7,
                )
                .expect("extra name is valid"),
                ScreenExactResource::try_new_scoped(
                    "worker-extra-buffer",
                    Arc::clone(&backend_scope),
                    ScreenResourceKind::WorkerAdditional,
                    9,
                )
                .expect("extra name is valid"),
            ]),
    );
    assert!(matches!(
        duplicate,
        Err(ScreenPlanError::DuplicateExactResourceName { .. })
    ));

    let unknown_scope = ScreenExactResourceLedger::try_new(
        ticket
            .required_minimums()
            .iter()
            .map(|minimum| {
                ScreenExactResource::try_new(
                    Arc::clone(minimum.name()),
                    minimum.resource(),
                    minimum.minimum_bytes(),
                )
                .expect("ticket name is valid")
            })
            .chain([ScreenExactResource::try_new_scoped(
                "orphan-worker-buffer",
                "absent-accounting-domain",
                ScreenResourceKind::WorkerAdditional,
                1,
            )
            .expect("extra resource is structurally valid")]),
    )
    .expect("unknown scope remains structurally representable");
    assert!(matches!(
        bind_ledger(&ticket, unknown_scope)
            .expect("unknown-scope resource set binds")
            .acknowledge(&ticket),
        Err(ScreenPlanError::UnknownExactResourceScope { .. })
    ));

    let with_extras = ScreenExactResourceLedger::try_new(
        ticket
            .required_minimums()
            .iter()
            .map(|minimum| {
                ScreenExactResource::try_new(
                    Arc::clone(minimum.name()),
                    minimum.resource(),
                    minimum.minimum_bytes(),
                )
                .expect("resource name is valid")
            })
            .chain([
                ScreenExactResource::try_new_scoped(
                    "backend-query-pool",
                    Arc::clone(&backend_scope),
                    ScreenResourceKind::BackendAllocation,
                    7,
                )
                .expect("backend extra name is valid"),
                ScreenExactResource::try_new_scoped(
                    "profile-history-ring",
                    Arc::clone(&profile_scope),
                    ScreenResourceKind::ProcessingProfileState,
                    5,
                )
                .expect("profile extra name is valid"),
            ]),
    )
    .expect("complete ledger with disjoint extras is representable");
    let with_extras = bind_ledger(&ticket, with_extras)
        .expect("complete resource set binds")
        .acknowledge(&ticket)
        .expect("unique additional accounting entries are accepted");
    let staged = preparing.admission().staged();
    let staged_worker_bytes = staged.total_bytes()
        - staged.publication_retention_bytes()
        - staged.publication_subscriber_slot_bytes();
    assert_eq!(
        staged_worker_bytes
            + staged.publication_retention_bytes()
            + staged.publication_subscriber_slot_bytes(),
        staged.total_bytes()
    );
    assert_eq!(
        with_extras.exact_ledger().total_bytes(),
        staged_worker_bytes + 12
    );

    let wrong_kind =
        ScreenExactResourceLedger::try_new(ticket.required_minimums().iter().enumerate().map(
            |(index, expected)| {
                ScreenExactResource::try_new(
                    Arc::clone(expected.name()),
                    if index == 0 {
                        ScreenResourceKind::ZoneGrid
                    } else {
                        expected.resource()
                    },
                    expected.minimum_bytes(),
                )
                .expect("resource name is valid")
            },
        ))
        .expect("wrong-kind ledger is structurally representable");
    assert!(matches!(
        bind_ledger(&ticket, wrong_kind)
            .expect("wrong-kind resource set binds")
            .acknowledge(&ticket),
        Err(ScreenPlanError::ExactResourceKindMismatch { .. })
    ));

    let understated_name = Arc::clone(
        ticket
            .required_minimums()
            .iter()
            .find(|minimum| minimum.minimum_bytes() > 0)
            .expect("surface ticket has a non-zero allocation minimum")
            .name(),
    );
    let understated =
        ScreenExactResourceLedger::try_new(ticket.required_minimums().iter().map(|expected| {
            ScreenExactResource::try_new(
                Arc::clone(expected.name()),
                expected.resource(),
                expected.minimum_bytes() - u64::from(expected.name() == &understated_name),
            )
            .expect("resource name is valid")
        }))
        .expect("understated ledger is structurally representable");
    assert!(matches!(
        bind_ledger(&ticket, understated)
            .expect("understated resource set binds")
            .acknowledge(&ticket),
        Err(ScreenPlanError::UnderstatedExactResource { .. })
    ));

    let exact = exact_resources(&ticket)
        .expect("ticket contract is representable")
        .acknowledge(&ticket)
        .expect("exact coverage is accepted");
    assert_eq!(exact.exact_ledger().total_bytes(), staged_worker_bytes);
    let overestimated =
        ScreenExactResourceLedger::try_new(ticket.required_minimums().iter().map(|expected| {
            ScreenExactResource::try_new(
                Arc::clone(expected.name()),
                expected.resource(),
                expected.minimum_bytes() + 1,
            )
            .expect("resource name is valid")
        }))
        .expect("overestimated ledger is representable");
    assert!(
        bind_ledger(&ticket, overestimated)
            .expect("overestimated resource set binds")
            .acknowledge(&ticket)
            .is_ok()
    );
    preparing
        .acknowledge(with_extras)
        .expect("issued exhaustive token is accepted by its preparation");
}

#[test]
fn exact_worker_ledgers_gate_arming_and_survive_explicit_abort() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    commit_demands(&mut builder, [surface], None).expect("surface becomes active");
    let active = builder.current().clone();
    let graph = ScreenInputGraphGeneration::new(9);
    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [zones],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(243, 243),
        )
        .expect("planned overlap fits capacity");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("worker ticket is issued");
    let profile_scope = required_scope(&ticket, ScreenResourceKind::ProcessingProfileState);
    let ledger = ScreenExactResourceLedger::try_new(
        ticket
            .required_minimums()
            .iter()
            .map(|expected| {
                ScreenExactResource::try_new(
                    Arc::clone(expected.name()),
                    expected.resource(),
                    expected.minimum_bytes(),
                )
                .expect("ticket resource names are valid")
            })
            .chain([ScreenExactResource::try_new_scoped(
                "worker-staging-overhead",
                profile_scope,
                ScreenResourceKind::WorkerAdditional,
                16,
            )
            .expect("extra resource name is valid")]),
    )
    .expect("worker ledger with disjoint staging overhead is representable");
    let token = bind_ledger(&ticket, ledger)
        .expect("worker resources bind to the ticket")
        .acknowledge(&ticket)
        .expect("required minimums and extra overhead are exhaustive");
    preparing
        .acknowledge(token)
        .expect("bound worker token is accepted");
    let failure = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect_err("active plus exact worker allocation exceeds capacity");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::ResourceExhausted {
            resource: ScreenResourceKind::ByteBudget,
            requested: 244,
            available: 243,
            ..
        }
    ));
    let abort = failure.into_preparing().abort();
    assert_eq!(abort.active_plan(), active.as_ref());
    assert_eq!(abort.prepared_tokens().len(), 1);
    assert_eq!(abort.prepared_tokens()[0].exact_ledger().total_bytes(), 64);
    assert_eq!(builder.current(), active);

    let overflow = ScreenExactResourceLedger::try_new([
        ScreenExactResource::try_new("first", ScreenResourceKind::PhysicalPlane, u64::MAX)
            .expect("name is valid"),
        ScreenExactResource::try_new("second", ScreenResourceKind::ZoneOutput, 1)
            .expect("name is valid"),
    ]);
    assert!(matches!(
        overflow,
        Err(ScreenPlanError::ExactLedgerOverflow { resource }) if resource.as_ref() == "second"
    ));
}

#[test]
fn committed_exact_resources_drive_future_overlap_and_retention() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            profile(ScreenProcessingProfileConfig {
                tuning: ScreenColorTuning::try_new(1.1, 1.0, 1.0).expect("test tuning is finite"),
                ..ScreenProcessingProfileConfig::default()
            }),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let graph = ScreenInputGraphGeneration::new(14);
    let mut builder = ScreenPlanBuilder::new();
    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [surface.clone()],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("surface candidate prepares");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("surface worker ticket is issued");
    let backend_scope = required_scope(&ticket, ScreenResourceKind::BackendAllocation);
    let surface_ledger = ScreenExactResourceLedger::try_new(
        ticket
            .required_minimums()
            .iter()
            .map(|minimum| {
                ScreenExactResource::try_new(
                    Arc::clone(minimum.name()),
                    minimum.resource(),
                    minimum.minimum_bytes(),
                )
                .expect("ticket resource names are valid")
            })
            .chain([ScreenExactResource::try_new_scoped(
                "backend-retained-extra",
                backend_scope,
                ScreenResourceKind::WorkerAdditional,
                50,
            )
            .expect("backend extra is valid")]),
    )
    .expect("surface exact ledger is representable");
    let token = bind_ledger(&ticket, surface_ledger)
        .expect("surface resources bind to the ticket")
        .acknowledge(&ticket)
        .expect("surface exact ledger covers its contract");
    preparing
        .acknowledge(token)
        .expect("surface worker token is accepted");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect("surface candidate arms");
    let committed = builder
        .commit(armed, revision, graph)
        .expect("surface candidate commits");
    reclaim_committed(committed);
    assert_eq!(builder.retained_exact_bytes(), 98);

    let revision = next_demand_revision(&builder);
    let failure = builder
        .prepare(
            [zones.clone()],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(270, 270),
        )
        .expect_err("retained exact bytes plus staged zones exceed capacity");
    assert!(matches!(
        failure,
        ScreenPlanError::ResourceExhausted {
            resource: ScreenResourceKind::ByteBudget,
            requested: 278,
            available: 270,
            ..
        }
    ));
    assert_eq!(builder.retained_exact_bytes(), 98);

    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [zones],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("zones replacement prepares");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("zones worker ticket is issued");
    let token = exact_resources(&ticket)
        .expect("zones ledger is representable")
        .acknowledge(&ticket)
        .expect("zones exact ledger covers its contract");
    preparing
        .acknowledge(token)
        .expect("zones worker token is accepted");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect("zones candidate arms");
    let committed = builder
        .commit(armed, revision, graph)
        .expect("zones candidate commits");
    reclaim_committed(committed);
    assert_eq!(builder.retained_exact_bytes(), 98);

    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [surface],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("abort candidate prepares");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("abort worker ticket is issued");
    let token = exact_resources(&ticket)
        .expect("abort ledger is representable")
        .acknowledge(&ticket)
        .expect("abort ledger covers its contract");
    preparing
        .acknowledge(token)
        .expect("abort worker token is accepted");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect("abort candidate arms");
    let abort = armed.abort();
    assert_eq!(abort.active_plan(), builder.current().as_ref());
    assert_eq!(builder.retained_exact_bytes(), 98);
}

#[test]
fn same_scope_worker_allocations_retire_independently_by_identity() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let graph = ScreenInputGraphGeneration::new(15);
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [surface],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("surface candidate prepares");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("surface worker ticket is issued");
    let backend_scope = required_scope(&ticket, ScreenResourceKind::BackendAllocation);
    let bound = bind_resources(
        &ticket,
        ticket
            .required_minimums()
            .iter()
            .map(|minimum| {
                ScreenExactResource::try_new(
                    Arc::clone(minimum.name()),
                    minimum.resource(),
                    minimum.minimum_bytes(),
                )
                .expect("ticket resource names are valid")
            })
            .chain([
                ScreenExactResource::try_new_scoped(
                    "stale-seven-byte-allocation",
                    Arc::clone(&backend_scope),
                    ScreenResourceKind::WorkerAdditional,
                    7,
                )
                .expect("seven-byte allocation is valid"),
                ScreenExactResource::try_new_scoped(
                    "released-eleven-byte-allocation",
                    backend_scope,
                    ScreenResourceKind::WorkerAdditional,
                    11,
                )
                .expect("eleven-byte allocation is valid"),
            ]),
    )
    .expect("same-scope worker allocations bind");
    let allocation_count = bound.lifetimes.len();
    let stale_lifetime = bound
        .lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().bytes() == 7)
        .expect("seven-byte lifetime exists")
        .clone();
    let token = bound
        .acknowledge(&ticket)
        .expect("same-scope resources exactly cover the ticket");
    preparing
        .acknowledge(token)
        .expect("worker token is accepted");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect("surface candidate arms");
    reclaim_committed(
        builder
            .commit(armed, revision, graph)
            .expect("surface candidate commits"),
    );

    let retired_resource_bytes = builder.retained_exact_bytes();
    let committed = commit_demands_with_retirement(&mut builder, std::iter::empty())
        .expect("surface removal commits");
    let (_, retirement) = committed.into_parts();
    assert_eq!(retirement.resource_count(), allocation_count);
    assert_eq!(retirement.pending_bytes(), 144 + retired_resource_bytes);

    let retirement = retirement
        .try_reclaim()
        .expect_err("one stale job still owns the seven-byte allocation");
    assert_eq!(retirement.branch_count(), 0);
    assert_eq!(retirement.resource_count(), 1);
    assert_eq!(retirement.pending_bytes(), 7);
    assert_eq!(hub.pending_retired_bytes(), 7);
    drop(retirement);
    assert_eq!(hub.pending_retired_bytes(), 7);

    let pressure_revision = next_demand_revision(&builder);
    assert!(matches!(
        builder.prepare(
            std::iter::empty(),
            None,
            pressure_revision,
            graph,
            ScreenAdmissionCapacity::new(6, 6),
        ),
        Err(ScreenPlanError::RetirementPressure {
            requested: 7,
            available: 6,
            ..
        })
    ));
    drop(stale_lifetime);
    assert_eq!(hub.pending_retired_bytes(), 0);

    let recovered_revision = next_demand_revision(&builder);
    let preparing = builder
        .prepare(
            std::iter::empty(),
            None,
            recovered_revision,
            graph,
            ScreenAdmissionCapacity::new(0, 0),
        )
        .expect("capacity recovers after the stale allocation owner releases");
    let _abort = preparing.abort();
}

#[test]
fn arm_and_commit_recheck_plan_and_graph_generation_fences() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    commit_demands(&mut builder, [surface], None).expect("surface becomes active");
    let hub = builder.publication_hub();
    let active = builder.current().clone();
    let graph = ScreenInputGraphGeneration::new(11);
    let revision = next_demand_revision(&builder);

    let preparing = builder
        .prepare(
            [zones.clone()],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("candidate prepares");
    let failure = preparing
        .arm(
            ScreenPlanBuilder::new().current().generation(),
            revision,
            graph,
        )
        .expect_err("stale plan generation cannot arm");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::BasePlanGenerationConflict { .. }
    ));
    assert_eq!(
        failure.into_preparing().abort().active_plan(),
        active.as_ref()
    );

    let preparing = builder
        .prepare(
            [zones.clone()],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("candidate prepares");
    let failure = preparing
        .arm(
            builder.current().generation(),
            revision,
            ScreenInputGraphGeneration::new(12),
        )
        .expect_err("changed graph cannot arm");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::BaseGraphGenerationConflict { .. }
    ));
    assert_eq!(
        failure.into_preparing().abort().active_plan(),
        active.as_ref()
    );

    let preparing = builder
        .prepare(
            [zones.clone()],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("candidate prepares");
    let changed_revision = revision.next().expect("revision advances");
    let failure = preparing
        .arm(builder.current().generation(), changed_revision, graph)
        .expect_err("changed demand revision cannot arm");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::DemandRevisionConflict { .. }
    ));
    assert_eq!(
        failure.into_preparing().abort().active_plan(),
        active.as_ref()
    );

    let mut preparing = builder
        .prepare(
            [zones],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("candidate prepares");
    let ticket = preparing
        .worker_ticket(&source_id("display-a"))
        .expect("worker ticket is issued");
    let token = exact_resources(&ticket)
        .expect("ticket contract is representable")
        .acknowledge(&ticket)
        .expect("exact ledger covers the ticket");
    preparing
        .acknowledge(token)
        .expect("worker token is accepted");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect("candidate arms");
    assert_eq!(builder.current(), active);
    let failure = builder
        .commit(armed, revision, ScreenInputGraphGeneration::new(12))
        .expect_err("changed graph cannot commit");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::BaseGraphGenerationConflict { .. }
    ));
    assert_eq!(failure.into_armed().abort().active_plan(), active.as_ref());
    assert_eq!(builder.current(), active);
    assert_eq!(hub.pending_retired_bytes(), 0);
}

#[test]
fn concurrent_armed_candidates_cannot_commit_over_a_newer_plan() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones_two_by_two = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let zones_one_by_one = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(1),
                rows: non_zero(1),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            20,
        ),
        &source,
    );
    let mut builder = ScreenPlanBuilder::new();
    commit_demands(&mut builder, [surface], None).expect("surface becomes active");
    let original = builder.current().clone();
    let graph = ScreenInputGraphGeneration::new(21);
    let first = arm_demands(&mut builder, [zones_two_by_two], graph);
    let stale = arm_demands(&mut builder, [zones_one_by_one], graph);
    assert_eq!(first.active_plan(), original.as_ref());
    assert_eq!(stale.active_plan(), original.as_ref());
    let first_revision = first.candidate_plan().demand_revision();
    let stale_revision = stale.candidate_plan().demand_revision();

    let committed = builder
        .commit(first, first_revision, graph)
        .expect("first armed candidate commits");
    let failure = builder
        .commit(stale, stale_revision, graph)
        .expect_err("second candidate is fenced by its stale base plan");
    assert!(matches!(
        failure.error(),
        ScreenPlanError::BaseCommittedStateConflict {
            expected_plan,
            observed_plan,
            ..
        } if *expected_plan == original.generation()
            && *observed_plan == committed.plan().generation()
    ));
    let abort = failure.into_armed().abort();
    assert_eq!(abort.active_plan(), original.as_ref());
    assert_eq!(builder.current().as_ref(), committed.plan());
    let (_, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("stale candidate no longer pins retired storage");
}

#[test]
fn continuity_transition_retains_old_until_exact_new_branch_is_live() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let old_demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let old = old_demand.descriptor().clone();
    let new_demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let new = new_demand.descriptor().clone();
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [old_demand.clone()], None).expect("old branch commits");
    let old_binding = binding_for(&builder, &source_id("display-a"));
    let old_publisher = hub
        .publisher(&old, &old_binding)
        .expect("old publisher is issued");
    let now = Instant::now();
    let old_pixels = [1_u8; 48];
    let old_metadata = publication_metadata(
        &old,
        &old_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let old_receipt = hub
        .publish(
            &old_publisher,
            surface_payload(&old, &old_pixels),
            &old_metadata,
        )
        .expect("old branch becomes live");
    let old_branch_lease = hub.lease(&old).expect("old branch lease is issued");
    let old_snapshot = old_branch_lease
        .read()
        .expect("old publication is readable");
    let old_lease = hub
        .continuity_lease(&old)
        .expect("continuity starts from hub-proven liveness");
    assert!(Arc::ptr_eq(old_lease.publication(), &old_snapshot));

    let overlap = commit_demands(&mut builder, [old_demand, new_demand.clone()], None)
        .expect("overlap plan contains old and new");
    assert_eq!(hub.generation(), overlap.generation());
    let stale_pixels = [5_u8; 48];
    assert!(matches!(
        hub.publish(
            &old_publisher,
            surface_payload(&old, &stale_pixels),
            &old_metadata,
        ),
        Err(ScreenPublicationHubError::PublisherStale { .. })
    ));
    assert!(matches!(
        hub.continuity_lease(&new),
        Err(ScreenPublicationHubError::BranchPending { .. })
    ));

    let overlap_binding = binding_for(&builder, &source_id("display-a"));
    let old_overlap_publisher = hub
        .publisher(&old, &overlap_binding)
        .expect("overlap generation issues an old publisher");
    let overlap_pixels = [9_u8; 48];
    let overlap_metadata = publication_metadata(
        &old,
        &overlap_binding,
        2,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let old_overlap_receipt = hub
        .publish(
            &old_overlap_publisher,
            surface_payload(&old, &overlap_pixels),
            &overlap_metadata,
        )
        .expect("old branch remains live in overlap");
    let transition = old_lease
        .stage(&new, &hub)
        .expect("committed overlap acquires the pending staged branch");
    assert_eq!(transition.active_descriptor(), &old);
    assert_eq!(transition.staged_descriptor(), &new);
    assert_eq!(transition.overlap_generation(), overlap.generation());
    let failure = transition
        .activate(&old_overlap_receipt)
        .expect_err("same-generation old receipt cannot impersonate staged liveness");
    assert_eq!(
        failure.error(),
        ScreenContinuityError::StagedReceiptMismatch
    );
    let old_lease = failure.into_transition().abort();
    assert_eq!(old_lease.descriptor(), &old);
    assert!(Arc::ptr_eq(old_lease.publication(), &old_snapshot));

    let failure = old_lease
        .stage(&new, &hub)
        .expect("staged branch remains acquired")
        .activate(&old_receipt)
        .expect_err("receipt from the pre-overlap catalog is fenced");
    assert_eq!(
        failure.error(),
        ScreenContinuityError::OverlapGenerationMismatch
    );
    let old_lease = failure.into_transition().abort();
    assert!(Arc::ptr_eq(old_lease.publication(), &old_snapshot));

    let retirement_lease = hub
        .continuity_lease(&old)
        .expect("second old lease retains real storage through retirement");
    let new_publisher = hub
        .publisher(&new, &overlap_binding)
        .expect("new publisher is issued");
    let first_zone_colors = [[13_u8, 14, 15]; 4];
    let first_zone_metadata = publication_metadata(
        &new,
        &overlap_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let superseded_new_receipt = hub
        .publish(
            &new_publisher,
            zones_payload(&new, &first_zone_colors),
            &first_zone_metadata,
        )
        .expect("staged branch publishes a live non-empty epoch");
    let second_zone_colors = [[16_u8, 17, 18]; 4];
    let second_zone_metadata = publication_metadata(
        &new,
        &overlap_binding,
        2,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let new_receipt = hub
        .publish(
            &new_publisher,
            zones_payload(&new, &second_zone_colors),
            &second_zone_metadata,
        )
        .expect("newer staged publication supersedes its receipt");
    assert!(new_receipt.publication().publication_epoch().get() > 0);
    assert!(new_receipt.publication().native_sequence().get() > 0);
    let failure = old_lease
        .stage(&new, &hub)
        .expect("staged branch is acquired before release")
        .activate(&superseded_new_receipt)
        .expect_err("superseded publication receipt cannot prove current liveness");
    assert_eq!(failure.error(), ScreenContinuityError::StagedBranchNotLive);
    let old_lease = failure.into_transition().abort();
    assert!(Arc::ptr_eq(old_lease.publication(), &old_snapshot));
    let activated = old_lease
        .stage(&new, &hub)
        .expect("staged branch remains acquired after failed activation")
        .activate(&new_receipt)
        .expect("opaque staged live receipt authorizes activation");
    assert_eq!(activated.descriptor(), &new);
    assert!(Arc::ptr_eq(
        activated.publication(),
        new_receipt.publication()
    ));

    let committed = commit_demands_with_retirement(&mut builder, [new_demand])
        .expect("old branch retires after switch");
    let (_, retirement) = committed.into_parts();
    assert!(matches!(
        hub.lease(&old),
        Err(ScreenPublicationHubError::BranchMissing { .. })
    ));
    assert!(old_branch_lease.read().is_none());
    let ScreenBranchPayload::Surface(payload) = old_snapshot.payload() else {
        panic!("old surface snapshot retains surface storage");
    };
    assert_eq!(payload.pixels(), &old_pixels);
    let failure = retirement_lease
        .stage(&new, &hub)
        .expect_err("overlap must retain the active branch");
    assert_eq!(failure.error(), ScreenContinuityError::ActiveBranchMissing);
    assert_eq!(failure.into_lease().descriptor(), &old);
    let retirement = retirement
        .try_reclaim()
        .expect_err("old continuity readers keep retired storage charged");
    drop((
        old_publisher,
        old_receipt,
        old_branch_lease,
        old_snapshot,
        old_overlap_publisher,
        old_overlap_receipt,
        superseded_new_receipt,
        new_receipt,
        activated,
    ));
    retirement
        .try_reclaim()
        .expect("retired continuity storage reclaims after readers release");
}

#[test]
fn continuity_rejects_a_staged_branch_committed_away_before_activation() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let active_demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let staged_demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let active_descriptor = active_demand.descriptor().clone();
    let staged_descriptor = staged_demand.descriptor().clone();
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [active_demand.clone()], None).expect("active branch commits");
    let active_binding = binding_for(&builder, &source_id("display-a"));
    let active_publisher = hub
        .publisher(&active_descriptor, &active_binding)
        .expect("active publisher is issued");
    let now = Instant::now();
    let active_pixels = [3_u8; 48];
    let active_metadata = publication_metadata(
        &active_descriptor,
        &active_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let active_receipt = hub
        .publish(
            &active_publisher,
            surface_payload(&active_descriptor, &active_pixels),
            &active_metadata,
        )
        .expect("active branch becomes live");
    let active_lease = hub
        .continuity_lease(&active_descriptor)
        .expect("active continuity lease is issued");

    commit_demands(&mut builder, [active_demand.clone(), staged_demand], None)
        .expect("overlap snapshot commits");
    let overlap_binding = binding_for(&builder, &source_id("display-a"));
    let staged_publisher = hub
        .publisher(&staged_descriptor, &overlap_binding)
        .expect("staged publisher is issued");
    let staged_colors = [[7_u8, 8, 9]; 4];
    let staged_metadata = publication_metadata(
        &staged_descriptor,
        &overlap_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let staged_receipt = hub
        .publish(
            &staged_publisher,
            zones_payload(&staged_descriptor, &staged_colors),
            &staged_metadata,
        )
        .expect("staged branch becomes live");
    let transition = active_lease
        .stage(&staged_descriptor, &hub)
        .expect("one stable overlap snapshot contains both entries");

    let committed = commit_demands_with_retirement(&mut builder, [active_demand])
        .expect("staged branch is committed away");
    let (_, retirement) = committed.into_parts();
    let failure = transition
        .activate(&staged_receipt)
        .expect_err("a receipt from a stale overlap cannot activate");
    assert_eq!(
        failure.error(),
        ScreenContinuityError::OverlapGenerationMismatch
    );
    let active_lease = failure.into_transition().abort();
    assert_eq!(active_lease.descriptor(), &active_descriptor);

    let fresh_active_lease = hub
        .continuity_lease(&active_descriptor)
        .expect("active branch remains live after staged retirement");
    let failure = fresh_active_lease
        .stage(&staged_descriptor, &hub)
        .expect_err("stable post-commit authority no longer contains staged");
    assert_eq!(failure.error(), ScreenContinuityError::StagedBranchMissing);
    let fresh_active_lease = failure.into_lease();

    drop((
        active_publisher,
        active_receipt,
        active_lease,
        fresh_active_lease,
        staged_publisher,
        staged_receipt,
    ));
    retirement
        .try_reclaim()
        .expect("stale overlap storage reclaims after its observers release");
}

#[test]
fn cadence_rebind_and_source_removal_switch_one_atomic_authority() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface_30 = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let surface_60 = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let descriptor = surface_30.descriptor().clone();
    let source = source_id("display-a");
    let graph = ScreenInputGraphGeneration::new(31);
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [surface_30], None).expect("initial cadence commits");
    let old_state = builder.committed_state();
    let old_binding = binding_for(&builder, &source);
    assert_eq!(old_state.plan().generation(), old_binding.plan_generation());
    assert_eq!(
        old_state.plan().demand_revision(),
        old_binding.demand_revision()
    );
    let old_publisher = hub
        .publisher(&descriptor, &old_binding)
        .expect("initial worker owns the durable branch");
    let lease = hub.lease(&descriptor).expect("durable branch lease exists");
    assert_eq!(old_binding.state(), ScreenWorkerBindingState::Active);
    assert_eq!(
        lease.delivery_state(Instant::now()).lifecycle(),
        ScreenBranchDeliveryLifecycle::Pending
    );

    let revision = next_demand_revision(&builder);
    let mut preparing = builder
        .prepare(
            [surface_60],
            None,
            revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("cadence-only source change prepares");
    assert_eq!(preparing.required_sources(), std::slice::from_ref(&source));
    let ticket = preparing
        .worker_ticket(&source)
        .expect("cadence-only change requires fresh worker authority");
    assert!(ticket.required_minimums().is_empty());
    let token = exact_resources(&ticket)
        .expect("empty delta ledger is exact")
        .acknowledge(&ticket)
        .expect("cadence worker attests its empty allocation delta");
    let candidate_binding = token.binding().clone();
    assert_eq!(
        candidate_binding.state(),
        ScreenWorkerBindingState::Prepared
    );
    preparing
        .acknowledge(token)
        .expect("candidate accepts its exact worker token");
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .expect("cadence candidate arms");
    assert_eq!(candidate_binding.state(), ScreenWorkerBindingState::Armed);
    let candidate_state = Arc::clone(armed.candidate_state());
    assert_eq!(
        candidate_state.plan().generation(),
        candidate_binding.plan_generation()
    );
    assert_eq!(
        candidate_state.plan().demand_revision(),
        candidate_binding.demand_revision()
    );
    assert_eq!(
        candidate_state.worker_bindings()[0].transaction_id(),
        candidate_binding.transaction_id()
    );
    assert_eq!(
        candidate_state.worker_bindings()[0].state(),
        ScreenWorkerBindingState::Armed,
        "an unreachable candidate is ready but has not received commit notification"
    );
    assert!(Arc::ptr_eq(&builder.committed_state(), &old_state));
    assert_eq!(hub.generation(), old_state.plan().generation());
    let committed = builder
        .commit(armed, revision, graph)
        .expect("cadence candidate commits atomically");
    let (_, retirement) = committed.into_parts();
    assert_eq!(retirement.branch_count(), 0);
    assert_eq!(retirement.resource_count(), 0);
    retirement
        .try_reclaim()
        .expect("cadence rebind preserves every allocation identity");
    let committed_state = builder.committed_state();
    assert!(Arc::ptr_eq(&committed_state, &candidate_state));
    assert_eq!(hub.generation(), candidate_state.plan().generation());
    assert_eq!(
        committed_state.worker_bindings()[0].transaction_id(),
        candidate_binding.transaction_id()
    );
    assert_eq!(candidate_binding.state(), ScreenWorkerBindingState::Active);
    assert_eq!(
        committed_state.worker_bindings()[0].state(),
        ScreenWorkerBindingState::Active,
        "the committed snapshot is visible before worker activation is reported"
    );
    assert_eq!(old_binding.state(), ScreenWorkerBindingState::Retired);

    let now = Instant::now();
    let pixels = [7_u8; 48];
    let old_metadata = publication_metadata(
        &descriptor,
        &old_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    assert!(matches!(
        hub.publish(
            &old_publisher,
            surface_payload(&descriptor, &pixels),
            &old_metadata,
        ),
        Err(ScreenPublicationHubError::PublisherStale { .. })
    ));
    let publisher = hub
        .publisher(&descriptor, &candidate_binding)
        .expect("new cadence worker owns the same durable branch");
    let metadata = publication_metadata(
        &descriptor,
        &candidate_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let live_receipt = hub
        .publish(&publisher, surface_payload(&descriptor, &pixels), &metadata)
        .expect("new worker publishes through retained branch storage");
    let weak_publication = Arc::downgrade(live_receipt.publication());
    assert!(lease.read().is_some());

    let removal_revision = next_demand_revision(&builder);
    let retired_resource_bytes = builder.retained_exact_bytes();
    let mut preparing = builder
        .prepare(
            std::iter::empty(),
            None,
            removal_revision,
            graph,
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("full source removal prepares");
    assert_eq!(preparing.required_sources(), std::slice::from_ref(&source));
    let ticket = preparing
        .worker_ticket(&source)
        .expect("source removal still requires an exact worker handoff");
    let token = exact_resources(&ticket)
        .expect("removal ledger is exact")
        .acknowledge(&ticket)
        .expect("removal worker token is accepted");
    let removal_binding = token.binding().clone();
    preparing
        .acknowledge(token)
        .expect("removal candidate accepts its worker token");
    let armed = preparing
        .arm(builder.current().generation(), removal_revision, graph)
        .expect("removal candidate arms");
    assert_eq!(removal_binding.state(), ScreenWorkerBindingState::Armed);
    let committed = builder
        .commit(armed, removal_revision, graph)
        .expect("empty authority commits");
    let (_, retirement) = committed.into_parts();
    assert_eq!(builder.committed_state().branch_count(), 0);
    assert_eq!(candidate_binding.state(), ScreenWorkerBindingState::Retired);
    assert_eq!(removal_binding.state(), ScreenWorkerBindingState::Retired);
    assert!(lease.read().is_none());
    assert_eq!(
        lease.delivery_state(Instant::now()).lifecycle(),
        ScreenBranchDeliveryLifecycle::Retired
    );
    let retired_bytes = 144 + retired_resource_bytes;
    assert!(retirement.resource_count() > 0);
    assert_eq!(retirement.pending_bytes(), retired_bytes);
    assert_eq!(hub.pending_retired_bytes(), retired_bytes);
    drop(retirement);
    assert_eq!(
        hub.pending_retired_bytes(),
        retired_bytes,
        "dropping the handoff cannot uncharge externally owned retired storage"
    );
    let pressure_revision = next_demand_revision(&builder);
    assert!(matches!(
        builder.prepare(
            std::iter::empty(),
            None,
            pressure_revision,
            graph,
            ScreenAdmissionCapacity::new(retired_bytes - 1, retired_bytes - 1),
        ),
        Err(ScreenPlanError::RetirementPressure {
            requested,
            available,
            ..
        }) if requested == retired_bytes && available == retired_bytes - 1
    ));
    drop((
        lease,
        publisher,
        old_publisher,
        committed_state,
        candidate_state,
        old_state,
        live_receipt,
    ));
    assert!(weak_publication.upgrade().is_none());
    assert_eq!(hub.pending_retired_bytes(), 0);
    drop(weak_publication);
}

#[test]
fn retirement_reclaims_ready_entries_without_unaccounting_pinned_payloads() {
    let small_source = resolved_source(ScreenSourceSelector::Configured, "display-small", 1, 1);
    let large_source = resolved_source(ScreenSourceSelector::Configured, "display-large", 4, 3);
    let small = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &small_source,
    );
    let large = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &large_source,
    );
    let small_descriptor = small.descriptor().clone();
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [small, large], None).expect("two branch pools commit");

    let small_binding = binding_for(&builder, &source_id("display-small"));
    let small_publisher = hub
        .publisher(&small_descriptor, &small_binding)
        .expect("small worker owns its branch");
    let now = Instant::now();
    let pixels = [5_u8; 4];
    let metadata = publication_metadata(
        &small_descriptor,
        &small_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let receipt = hub
        .publish(
            &small_publisher,
            surface_payload(&small_descriptor, &pixels),
            &metadata,
        )
        .expect("small branch publishes");
    let pinned_publication = Arc::clone(receipt.publication());
    let weak_publication = Arc::downgrade(&pinned_publication);
    drop((receipt, small_publisher));

    let retired_resource_bytes = builder.retained_exact_bytes();
    let committed = commit_demands_with_retirement(&mut builder, std::iter::empty())
        .expect("both branch pools retire");
    let (_, retirement) = committed.into_parts();
    assert_eq!(retirement.branch_count(), 2);
    assert!(retirement.resource_count() > 0);
    assert_eq!(retirement.pending_bytes(), 156 + retired_resource_bytes);
    assert_eq!(hub.pending_retired_bytes(), 156 + retired_resource_bytes);

    let retirement = retirement
        .try_reclaim()
        .expect_err("the raw small publication still pins one pool");
    assert_eq!(retirement.branch_count(), 1);
    assert_eq!(retirement.resource_count(), 0);
    assert_eq!(retirement.pending_bytes(), 12);
    assert_eq!(
        hub.pending_retired_bytes(),
        12,
        "the unpinned large pool is destroyed and uncharged independently"
    );
    drop(retirement);
    assert_eq!(
        hub.pending_retired_bytes(),
        12,
        "dropping the handle leaves its publication-owned charge intact"
    );

    let pressure_revision = next_demand_revision(&builder);
    assert!(matches!(
        builder.prepare(
            std::iter::empty(),
            None,
            pressure_revision,
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(11, 11),
        ),
        Err(ScreenPlanError::RetirementPressure {
            requested: 12,
            available: 11,
            ..
        })
    ));

    drop(pinned_publication);
    assert!(weak_publication.upgrade().is_none());
    assert_eq!(hub.pending_retired_bytes(), 0);
    drop(weak_publication);

    let recovered_revision = next_demand_revision(&builder);
    let preparing = builder
        .prepare(
            std::iter::empty(),
            None,
            recovered_revision,
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(0, 0),
        )
        .expect("admission recovers as soon as the final payload owner releases");
    let _abort = preparing.abort();
}

#[test]
fn typed_publication_validation_and_fixed_slots_preserve_last_good() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let descriptor = demand.descriptor().clone();
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 2)
        .expect("one retained plus two subscriber slots is valid");
    let mut builder = ScreenPlanBuilder::with_publication_slots(policy);
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [demand], None).expect("surface commits");
    assert_eq!(builder.committed_state().slot_policy().total_slots(), 3);
    let binding = binding_for(&builder, &source_id("display-a"));
    let publisher = hub
        .publisher(&descriptor, &binding)
        .expect("active binding receives a publisher");
    let lease = hub.lease(&descriptor).expect("surface lease exists");
    let now = Instant::now();
    let metadata = |sequence| {
        publication_metadata(
            &descriptor,
            &binding,
            sequence,
            now,
            now,
            now + Duration::from_millis(1),
            ScreenPublicationHealth::Healthy,
        )
    };

    let zone_colors = [[0_u8; 3]; 1];
    let wrong_kind = ScreenBranchPayload::Zones(
        ScreenZonesPayload::try_new(
            NonZeroU32::MIN,
            NonZeroU32::MIN,
            descriptor_colorimetry(&descriptor),
            &zone_colors,
        )
        .expect("one zone is structurally valid"),
    );
    assert!(matches!(
        hub.publish(&publisher, wrong_kind, &metadata(1)),
        Err(ScreenPublicationHubError::PayloadKindMismatch { .. })
    ));
    let tiny_pixels = [0_u8; 4];
    let wrong_extent = ScreenBranchPayload::Surface(
        ScreenSurfacePayload::try_new(
            pixel_extent(1, 1),
            descriptor.physical().target_pixel_format(),
            descriptor_colorimetry(&descriptor),
            &tiny_pixels,
        )
        .expect("one pixel is structurally valid"),
    );
    assert!(matches!(
        hub.publish(&publisher, wrong_extent, &metadata(1)),
        Err(ScreenPublicationHubError::SurfaceExtentMismatch { .. })
    ));
    let pixels = [3_u8; 48];
    let wrong_color = ScreenBranchPayload::Surface(
        ScreenSurfacePayload::try_new(
            output_extent(&descriptor),
            descriptor.physical().target_pixel_format(),
            ScreenPublicationColorimetry::new(colorimetry(
                CaptureColorSpace::DisplayP3,
                CaptureTransferFunction::Linear,
            )),
            &pixels,
        )
        .expect("wrong colorimetry remains structurally valid"),
    );
    assert!(matches!(
        hub.publish(&publisher, wrong_color, &metadata(1)),
        Err(ScreenPublicationHubError::ColorimetryMismatch { .. })
    ));
    let wrong_luminance = ScreenBranchPayload::Surface(
        ScreenSurfacePayload::try_new(
            output_extent(&descriptor),
            descriptor.physical().target_pixel_format(),
            ScreenPublicationColorimetry::new(CaptureColorimetry::from_known(
                KnownCaptureColorimetry::SRGB.with_luminance(luminance(100.0, 100.0)),
            )),
            &pixels,
        )
        .expect("mismatched luminance remains structurally valid"),
    );
    assert!(matches!(
        hub.publish(&publisher, wrong_luminance, &metadata(1)),
        Err(ScreenPublicationHubError::ColorimetryMismatch { .. })
    ));
    let wrong_epoch = ScreenPublicationMetadata::try_new(
        CaptureEpoch {
            source_id: source_id("display-a"),
            topology_generation: 1,
            session_generation: 2,
        },
        binding.plan_generation(),
        NonZeroU64::MIN,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    )
    .expect("wrong epoch metadata is temporally valid");
    assert!(matches!(
        hub.publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &wrong_epoch,
        ),
        Err(ScreenPublicationHubError::SourceEpochMismatch)
    ));
    assert!(matches!(
        ScreenPublicationMetadata::try_new(
            descriptor.source_epoch().clone(),
            binding.plan_generation(),
            NonZeroU64::MIN,
            now + Duration::from_secs(1),
            now,
            now,
            ScreenPublicationHealth::Healthy,
        ),
        Err(ScreenPublicationHubError::InvalidPublicationTimeline)
    ));

    let first = hub
        .publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(1),
        )
        .expect("first admitted slot publishes");
    let first_payload = Arc::clone(first.publication());
    hub.report_delivery_health(&publisher, ScreenPublicationHealth::Degraded)
        .expect("worker reports degradation without replacing last-good");
    assert!(Arc::ptr_eq(
        &lease.read().expect("health update retains payload"),
        &first_payload
    ));
    assert_eq!(
        lease.delivery_state(now).source_health(),
        Some(ScreenPublicationHealth::Degraded)
    );
    let second = hub
        .publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(2),
        )
        .expect("second admitted slot publishes");
    let third = hub
        .publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(3),
        )
        .expect("third admitted slot publishes");
    let last_good = lease.read().expect("third publication is last-good");
    assert!(Arc::ptr_eq(&last_good, third.publication()));
    assert!(matches!(
        hub.publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(4),
        ),
        Err(ScreenPublicationHubError::PublicationPressure { admitted_slots: 3 })
    ));
    assert!(Arc::ptr_eq(
        &lease.read().expect("pressure preserves last-good"),
        &last_good
    ));
    hub.report_delivery_health(&publisher, ScreenPublicationHealth::Failed)
        .expect("worker reports failure without replacing last-good");
    let pressured = lease.delivery_state(now + Duration::from_secs(1));
    assert_eq!(pressured.lifecycle(), ScreenBranchDeliveryLifecycle::Live);
    assert_eq!(
        pressured.freshness(),
        Some(ScreenPublicationFreshness::Stale)
    );
    assert_eq!(
        pressured.source_health(),
        Some(ScreenPublicationHealth::Failed)
    );
    assert!(pressured.last_publish_was_pressured());
    assert_eq!(pressured.pressure_events(), 1);

    drop((first, first_payload));
    assert!(
        lease.delivery_state(now).last_publish_was_pressured(),
        "releasing a slot does not rewrite the last-attempt diagnostic"
    );
    let fourth = hub
        .publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(4),
        )
        .expect("released slot accepts the next publication");
    assert_eq!(fourth.publication().native_sequence().get(), 4);
    assert!(!lease.delivery_state(now).last_publish_was_pressured());
    drop((second, third, fourth));
}

#[test]
fn copied_old_generation_slot_cannot_finalize_after_cadence_rebind() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface_30 = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let surface_60 = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let descriptor = surface_30.descriptor().clone();
    let mut builder = ScreenPlanBuilder::new();
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [surface_30], None).expect("initial cadence commits");
    let old_binding = binding_for(&builder, &source_id("display-a"));
    let old_publisher = hub
        .publisher(&descriptor, &old_binding)
        .expect("old worker owns the branch");
    let lease = hub.lease(&descriptor).expect("branch lease exists");
    let pixels = [29_u8; 48];
    let now = Instant::now();
    let old_metadata = publication_metadata(
        &descriptor,
        &old_binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    let prepared = hub
        .prepare_publication(
            &old_publisher,
            surface_payload(&descriptor, &pixels),
            &old_metadata,
        )
        .expect("old generation reserves and copies outside the commit barrier");
    assert!(lease.read().is_none());

    commit_demands(&mut builder, [surface_60], None).expect("cadence rebind commits");
    assert_eq!(old_binding.state(), ScreenWorkerBindingState::Retired);
    assert!(matches!(
        hub.finalize_publication(prepared),
        Err(ScreenPublicationHubError::PublisherStale { .. })
    ));
    assert!(lease.read().is_none());
    assert_eq!(
        lease.delivery_state(now).lifecycle(),
        ScreenBranchDeliveryLifecycle::Pending
    );
    assert!(matches!(
        hub.report_delivery_health(&old_publisher, ScreenPublicationHealth::Failed),
        Err(ScreenPublicationHubError::PublisherStale { .. })
    ));

    let binding = binding_for(&builder, &source_id("display-a"));
    let publisher = hub
        .publisher(&descriptor, &binding)
        .expect("new cadence worker owns the branch");
    let metadata = publication_metadata(
        &descriptor,
        &binding,
        1,
        now,
        now,
        now + Duration::from_secs(1),
        ScreenPublicationHealth::Healthy,
    );
    hub.publish(&publisher, surface_payload(&descriptor, &pixels), &metadata)
        .expect("stale reservation returned its slot without advancing sequence");
    assert_eq!(
        lease
            .read()
            .expect("new generation becomes live")
            .plan_generation(),
        builder.current().generation()
    );
}

#[test]
fn weak_publication_owner_causes_typed_pressure_without_mutation_panic() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 1, 1);
    let demand = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let descriptor = demand.descriptor().clone();
    let policy = ScreenPublicationSlotPolicy::try_new(NonZeroU32::MIN, 1)
        .expect("two-slot pressure policy is valid");
    let mut builder = ScreenPlanBuilder::with_publication_slots(policy);
    let hub = builder.publication_hub();
    commit_demands(&mut builder, [demand], None).expect("one-pixel surface commits");
    let binding = binding_for(&builder, &source_id("display-a"));
    let publisher = hub
        .publisher(&descriptor, &binding)
        .expect("worker owns one-pixel branch");
    let now = Instant::now();
    let pixels = [7_u8; 4];
    let metadata = |sequence| {
        publication_metadata(
            &descriptor,
            &binding,
            sequence,
            now,
            now,
            now + Duration::from_secs(1),
            ScreenPublicationHealth::Healthy,
        )
    };
    let first = hub
        .publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(1),
        )
        .expect("first slot publishes");
    let weak = Arc::downgrade(first.publication());
    drop(first);
    let second = hub
        .publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(2),
        )
        .expect("second slot publishes while weak owner pins the first");
    drop(second);
    assert!(matches!(
        hub.publish(
            &publisher,
            surface_payload(&descriptor, &pixels),
            &metadata(3),
        ),
        Err(ScreenPublicationHubError::PublicationPressure { admitted_slots: 2 })
    ));
    assert!(weak.upgrade().is_some());
    drop(weak);
    hub.publish(
        &publisher,
        surface_payload(&descriptor, &pixels),
        &metadata(3),
    )
    .expect("released weak owner makes its fixed slot safely reusable");
}

#[test]
fn large_source_set_rebinds_with_canonical_scaling_paths() {
    const SOURCE_COUNT: usize = 256;
    let mut initial = Vec::with_capacity(SOURCE_COUNT);
    let mut faster = Vec::with_capacity(SOURCE_COUNT);
    for index in 0..SOURCE_COUNT {
        let id = format!("display-{index:04}");
        let source = resolved_source(ScreenSourceSelector::Configured, &id, 1, 1);
        initial.push(resolve(
            &registered(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Contain,
                default_profile(),
                30,
            ),
            &source,
        ));
        faster.push(resolve(
            &registered(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Contain,
                default_profile(),
                60,
            ),
            &source,
        ));
    }
    let mut builder = ScreenPlanBuilder::new();
    let initial = commit_demands(&mut builder, initial, None).expect("large initial plan commits");
    assert_eq!(initial.branches().len(), SOURCE_COUNT);
    let faster = commit_demands(&mut builder, faster, None).expect("large cadence rebind commits");
    assert_eq!(faster.branches().len(), SOURCE_COUNT);
    assert_eq!(
        builder.committed_state().worker_bindings().len(),
        SOURCE_COUNT
    );
}

#[test]
fn compatibility_roles_reject_kind_substitution() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 4, 3);
    let surface = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            60,
        ),
        &source,
    );
    let zones = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(2),
                rows: non_zero(2),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    assert_eq!(
        ScreenCompatibilitySelection::try_new(zones.descriptor().clone(), None),
        Err(ScreenPlanError::CompatibilitySurfaceKindMismatch)
    );
    assert_eq!(
        ScreenCompatibilitySelection::try_new(
            surface.descriptor().clone(),
            Some(surface.descriptor().clone()),
        ),
        Err(ScreenPlanError::CompatibilityZonesKindMismatch)
    );
}

#[test]
fn generation_tracks_only_effective_plan_and_mirror_changes() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 1920, 1080);
    let surface_30 = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        30,
    );
    let surface_60 = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        60,
    );
    let zones = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Zones {
            columns: non_zero(16),
            rows: non_zero(9),
        },
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        20,
    );
    let surface_30 = resolve(&surface_30, &source);
    let surface_60 = resolve(&surface_60, &source);
    let zones = resolve(&zones, &source);
    let surface_descriptor = surface_60.descriptor().clone();
    let zones_descriptor = zones.descriptor().clone();
    let mut builder = ScreenPlanBuilder::new();

    let first = commit_demands(&mut builder, [surface_30.clone()], None)
        .expect("first material plan resolves");
    assert_eq!(first.generation().get(), 1);

    let faster = commit_demands(&mut builder, [surface_30.clone(), surface_60.clone()], None)
        .expect("effective cadence change resolves");
    assert_eq!(faster.generation().get(), 2);

    let lower_duplicate = commit_demands(&mut builder, [surface_60.clone(), surface_30], None)
        .expect("registration order and lower duplicate are ineffective");
    assert_eq!(lower_duplicate.generation(), faster.generation());

    let with_zones = commit_demands(
        &mut builder,
        [zones.clone(), surface_60.clone()],
        Some(&surface_descriptor),
    )
    .expect("ordinary compatibility branch resolves");
    assert_eq!(with_zones.generation().get(), 3);
    assert_eq!(with_zones.branches().len(), 2);
    assert_eq!(
        with_zones
            .compatibility_branch()
            .expect("mirror points to a branch")
            .descriptor(),
        &surface_descriptor
    );

    let reordered = commit_demands(
        &mut builder,
        [surface_60.clone(), zones.clone()],
        Some(&surface_descriptor),
    )
    .expect("registration order is ineffective");
    assert_eq!(reordered.generation(), with_zones.generation());

    let compatibility = ScreenCompatibilitySelection::try_new(
        surface_descriptor.clone(),
        Some(zones_descriptor.clone()),
    )
    .expect("surface and zones compatibility roles are valid");
    let remirrored =
        commit_demands_with_compatibility(&mut builder, [surface_60, zones], Some(&compatibility))
            .expect("zones compatibility selection changes independently");
    assert_eq!(remirrored.generation().get(), 4);
    assert_eq!(remirrored.branches().len(), 2);
    assert_eq!(
        remirrored
            .compatibility_branch()
            .expect("mirror points to an ordinary branch")
            .descriptor(),
        &surface_descriptor
    );
    assert_eq!(
        remirrored
            .compatibility_zones_branch()
            .expect("zones mirror points to an ordinary branch")
            .descriptor(),
        &zones_descriptor
    );
}

#[test]
fn compatibility_mirror_rejects_descriptors_absent_from_the_plan() {
    let source = resolved_source(ScreenSourceSelector::Configured, "display-a", 1920, 1080);
    let present = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );
    let absent = resolve(
        &registered(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(16),
                rows: non_zero(9),
            },
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            default_profile(),
            30,
        ),
        &source,
    );

    let compatibility = ScreenCompatibilitySelection::try_new(
        present.descriptor().clone(),
        Some(absent.descriptor().clone()),
    )
    .expect("compatibility roles are structurally valid");
    let mut builder = ScreenPlanBuilder::new();
    assert_eq!(
        commit_demands_with_compatibility(&mut builder, [present], Some(&compatibility)),
        Err(ScreenPlanError::CompatibilityBranchMissing)
    );
}

#[test]
fn resolution_rejects_selector_mismatch_and_checked_geometry_overflow() {
    let configured = resolved_source(ScreenSourceSelector::Configured, "display-a", 1920, 1080);
    let primary_request = registered(
        ScreenSourceSelector::Primary,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        30,
    );
    assert_eq!(
        primary_request.resolve(&configured),
        Err(ScreenPublicationError::SourceSelectorMismatch)
    );

    let exact_id = source_id("display-a");
    let incorrect_exact = resolved_source(
        ScreenSourceSelector::Exact(exact_id.clone()),
        "display-b",
        1920,
        1080,
    );
    let exact_request = registered(
        ScreenSourceSelector::Exact(exact_id),
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        30,
    );
    assert_eq!(
        exact_request.resolve(&incorrect_exact),
        Err(ScreenPublicationError::SourceSelectorMismatch)
    );

    let extreme = resolved_source(
        ScreenSourceSelector::Configured,
        "display-extreme",
        1,
        u32::MAX,
    );
    let overflowing = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(Some(non_zero(u32::MAX)), None, ScreenUpscalePolicy::Allow),
        ScreenAspectPolicy::Contain,
        default_profile(),
        30,
    );
    assert_eq!(
        overflowing.resolve(&extreme),
        Err(ScreenPublicationError::GeometryOverflow)
    );
}

#[test]
fn maximum_native_resolution_has_no_synthetic_product_cap() {
    let source = resolved_source(
        ScreenSourceSelector::Configured,
        "display-max",
        u32::MAX,
        u32::MAX,
    );
    let demand = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        default_profile(),
        1,
    );
    let descriptor = resolve(&demand, &source);
    assert_eq!(
        output_extent(descriptor.descriptor()),
        pixel_extent(u32::MAX, u32::MAX)
    );
}

#[test]
fn cover_geometry_remains_exact_at_u32_extremes() {
    let source = resolved_source(
        ScreenSourceSelector::Configured,
        "display-max-cover",
        u32::MAX,
        u32::MAX - 1,
    );
    let demand = registered(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(u32::MAX - 1)),
            Some(non_zero(u32::MAX)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        default_profile(),
        1,
    );
    let descriptor = resolve(&demand, &source);
    let geometry = descriptor.descriptor().geometry();
    let region = geometry.source_region();
    let maximum = u64::from(u32::MAX);

    assert_eq!(
        geometry.output_extent(),
        pixel_extent(u32::MAX - 1, u32::MAX)
    );
    assert_eq!(region.x().numerator(), maximum * 2 - 1);
    assert_eq!(region.x().denominator().get(), maximum * 2);
    assert_eq!(region.width().numerator(), (maximum - 1) * (maximum - 1));
    assert_eq!(region.width().denominator().get(), maximum);
    assert_eq!(region.height().numerator(), maximum - 1);
    assert_eq!(region.height().denominator().get(), 1);
}
