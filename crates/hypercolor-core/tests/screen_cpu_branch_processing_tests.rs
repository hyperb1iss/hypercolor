//! Stateful CPU logical processing over exact prepared screen branches.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::sector::{LetterboxDetectionError, PreparedLetterboxDetector};
use hypercolor_core::input::screen::smooth::PreparedTemporalSmoother;
use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureEpoch, CaptureGeometry,
    CapturePixelFormat, CaptureRotation, CaptureSourceId, CaptureTransferFunction, ColorTuning,
    CommittedScreenPlan, CpuReductionExecutor, CpuSurfaceMaterializationError,
    CpuZoneMaterializationError, KnownCaptureColorimetry, PhysicalOrigin, PixelExtent,
    PreparedCpuSurfaceMaterializer, PreparedCpuZoneMaterializer, PreparedScreenPublication,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenCapturePlan, ScreenColorTuning,
    ScreenContentBarsPolicy, ScreenExactResource, ScreenExactResourceLedger, ScreenExtentRequest,
    ScreenGridPolicy, ScreenInputGraphGeneration, ScreenLetterboxFill, ScreenPayloadKind,
    ScreenPhysicalReductionDescriptor, ScreenPlanBuilder, ScreenPlanError, ScreenPlanGeneration,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenProfileScalar,
    ScreenPublicationExecutorRequest, ScreenPublicationHub, ScreenPublicationKind,
    ScreenPublicationMetadata, ScreenPublicationRequest, ScreenResourceApi, ScreenResourceLifetime,
    ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenSourceReflection, ScreenSourceSelector,
    ScreenTargetColorimetry, ScreenUpscalePolicy, ScreenWorkerBinding,
    ScreenWorkerPreparationTicket, SourceScale,
};
use hypercolor_types::canvas::SurfaceResourceError;

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn scalar(value: f32) -> ScreenProfileScalar {
    ScreenProfileScalar::try_new(value).expect("test scalar is finite")
}

fn source_id() -> CaptureSourceId {
    CaptureSourceId::new("synthetic:cpu-branch-processing").expect("test source id is non-empty")
}

fn source(source_extent: PixelExtent, colorimetry: CaptureColorimetry) -> ResolvedScreenSource {
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        source_extent,
        source_extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test source geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source_id(),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new(
            geometry,
            source_extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            colorimetry,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn executor() -> CpuReductionExecutor {
    CpuReductionExecutor::new(
        NonZeroUsize::new(2).expect("test worker count is non-zero"),
        non_zero(2),
    )
    .expect("test worker pool builds")
}

fn exact_resources(
    ticket: &ScreenWorkerPreparationTicket,
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

fn commit(
    builder: &mut ScreenPlanBuilder,
    demand: ResolvedScreenBranchDemand,
) -> (ScreenCapturePlan, ScreenWorkerBinding) {
    let graph_generation = ScreenInputGraphGeneration::new(1);
    let demand_revision = builder
        .current()
        .demand_revision()
        .next()
        .expect("test demand revision remains representable");
    let mut preparing = builder
        .prepare(
            [demand],
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
        let (ledger, lifetimes) = exact_resources(&ticket).expect("exact resources bind");
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
    let plan = reclaim(committed);
    let binding = builder
        .committed_state()
        .worker_bindings()
        .iter()
        .find(|binding| binding.source_id() == &source_id())
        .cloned()
        .expect("committed source has a worker binding");
    (plan, binding)
}

fn reclaim(committed: CommittedScreenPlan) -> ScreenCapturePlan {
    let (plan, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved retired pools reclaim immediately");
    plan
}

struct ZoneFixture {
    hub: Arc<ScreenPublicationHub>,
    descriptor: ResolvedScreenPublicationDescriptor,
    physical: ScreenPhysicalReductionDescriptor,
    generation: ScreenPlanGeneration,
    binding: ScreenWorkerBinding,
}

impl ZoneFixture {
    fn new(
        width: u32,
        height: u32,
        columns: u32,
        rows: u32,
        profile: ScreenProcessingProfileConfig,
        colorimetry: CaptureColorimetry,
    ) -> Self {
        let source = source(extent(width, height), colorimetry);
        let executor = executor();
        let demand = RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Zones {
                    columns: non_zero(columns),
                    rows: non_zero(rows),
                },
                ScreenPublicationExecutorRequest::Cpu,
                ScreenExtentRequest::bounded(
                    Some(non_zero(width)),
                    Some(non_zero(height)),
                    ScreenUpscalePolicy::Allow,
                ),
                ScreenAspectPolicy::Cover,
                Arc::new(ScreenProcessingProfile::new(profile)),
            ),
            non_zero(60),
        )
        .resolve_with_color_capabilities(&source, executor.capabilities())
        .expect("test demand resolves");
        let mut builder = ScreenPlanBuilder::new();
        let hub = builder.publication_hub();
        let (plan, binding) = commit(&mut builder, demand);
        let descriptor = plan.branches()[0].descriptor().clone();
        let physical = descriptor.physical().clone();
        Self {
            hub,
            descriptor,
            physical,
            generation: plan.generation(),
            binding,
        }
    }

    fn publication(&self, sequence: u64, captured_at: Instant) -> PreparedScreenPublication {
        let publisher = self
            .hub
            .publisher(&self.descriptor, &self.binding)
            .expect("test publisher is committed");
        let metadata = ScreenPublicationMetadata::try_intent(
            self.descriptor.source_epoch().clone(),
            self.generation,
            NonZeroU64::new(sequence).expect("test sequence is non-zero"),
            captured_at,
            captured_at + Duration::from_secs(2),
        )
        .expect("test publication intent is valid");
        self.hub
            .prepare_writable_publication(
                &publisher,
                hypercolor_core::input::screen::ScreenPayloadKind::Zones,
                &metadata,
            )
            .expect("test publication slot reserves")
    }
}

struct SurfaceFixture {
    hub: Arc<ScreenPublicationHub>,
    descriptor: ResolvedScreenPublicationDescriptor,
    physical: ScreenPhysicalReductionDescriptor,
    generation: ScreenPlanGeneration,
    binding: ScreenWorkerBinding,
}

impl SurfaceFixture {
    fn new(
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
        aspect: ScreenAspectPolicy,
        profile: ScreenProcessingProfileConfig,
    ) -> Self {
        let source = source(
            extent(source_width, source_height),
            CaptureColorimetry::SRGB,
        );
        let executor = executor();
        let demand = RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                ScreenSourceSelector::Configured,
                ScreenPublicationKind::Surface,
                ScreenPublicationExecutorRequest::Cpu,
                ScreenExtentRequest::bounded(
                    Some(non_zero(output_width)),
                    Some(non_zero(output_height)),
                    ScreenUpscalePolicy::Allow,
                ),
                aspect,
                Arc::new(ScreenProcessingProfile::new(profile)),
            ),
            non_zero(60),
        )
        .resolve_with_color_capabilities(&source, executor.capabilities())
        .expect("test Surface demand resolves");
        let mut builder = ScreenPlanBuilder::new();
        let hub = builder.publication_hub();
        let (plan, binding) = commit(&mut builder, demand);
        let descriptor = plan.branches()[0].descriptor().clone();
        let physical = descriptor.physical().clone();
        Self {
            hub,
            descriptor,
            physical,
            generation: plan.generation(),
            binding,
        }
    }

    fn publication(&self, sequence: u64, captured_at: Instant) -> PreparedScreenPublication {
        let publisher = self
            .hub
            .publisher(&self.descriptor, &self.binding)
            .expect("test Surface publisher is committed");
        let metadata = ScreenPublicationMetadata::try_intent(
            self.descriptor.source_epoch().clone(),
            self.generation,
            NonZeroU64::new(sequence).expect("test sequence is non-zero"),
            captured_at,
            captured_at + Duration::from_secs(2),
        )
        .expect("test Surface intent is valid");
        self.hub
            .prepare_writable_publication(&publisher, ScreenPayloadKind::Surface, &metadata)
            .expect("test Surface slot reserves")
    }
}

fn point_profile() -> ScreenProcessingProfileConfig {
    ScreenProcessingProfileConfig {
        grid: ScreenGridPolicy::PointSample,
        ..ScreenProcessingProfileConfig::default()
    }
}

fn rgba_grid(colors: &[[u8; 3]]) -> Vec<u8> {
    colors
        .iter()
        .flat_map(|color| [color[0], color[1], color[2], u8::MAX])
        .collect()
}

fn rgba_row(color: [u8; 4], count: usize) -> Vec<u8> {
    std::iter::repeat_n(color, count).flatten().collect()
}

fn encoded_pixel(color: [u8; 4], pixel_format: CapturePixelFormat) -> [u8; 4] {
    match pixel_format {
        CapturePixelFormat::Rgba8 => color,
        CapturePixelFormat::Bgra8 => [color[2], color[1], color[0], color[3]],
    }
}

fn decoded_pixel(color: [u8; 4], pixel_format: CapturePixelFormat) -> [u8; 3] {
    match pixel_format {
        CapturePixelFormat::Rgba8 => color[..3].try_into().expect("pixel has RGB channels"),
        CapturePixelFormat::Bgra8 => [color[2], color[1], color[0]],
    }
}

fn smoothed_color(
    smoothing: ScreenSmoothingPolicy,
    incoming: [u8; 3],
    elapsed: Duration,
) -> [u8; 3] {
    let mut smoother =
        PreparedTemporalSmoother::try_new(smoothing, 1, 1).expect("reference smoother prepares");
    let mut colors = [[0, 0, 0]];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::ZERO,
            false,
            false,
        )
        .expect("reference baseline stages");
    assert!(smoother.commit_staged());
    colors[0] = incoming;
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            elapsed,
            false,
            false,
        )
        .expect("reference response stages");
    colors[0]
}

#[test]
fn contain_geometry_keeps_explicit_odd_output_placement() {
    let fixture = SurfaceFixture::new(
        4,
        2,
        5,
        5,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
    );
    let geometry = fixture.descriptor.geometry();
    assert_eq!(geometry.output_extent(), extent(5, 5));
    assert_eq!(geometry.content_extent(), extent(5, 2));
    assert_eq!((geometry.content_x(), geometry.content_y()), (0, 1));
    assert_eq!(fixture.physical.reduction_extent(), extent(5, 2));
    assert!(!geometry.content_fills_output());

    let cover = SurfaceFixture::new(
        4,
        2,
        5,
        5,
        ScreenAspectPolicy::Cover,
        ScreenProcessingProfileConfig::default(),
    );
    assert!(cover.descriptor.geometry().content_fills_output());
    assert_eq!(cover.physical.reduction_extent(), extent(5, 5));
}

#[test]
fn surface_letterbox_fill_modes_preserve_content_and_alpha() {
    let red = [255, 0, 0, 255];
    let blue = [0, 0, 255, 128];
    for (fill, expected_top, expected_bottom) in [
        (ScreenLetterboxFill::Transparent, [0, 0, 0, 0], [0, 0, 0, 0]),
        (
            ScreenLetterboxFill::Solid([1, 2, 3, 4]),
            [1, 2, 3, 4],
            [1, 2, 3, 4],
        ),
        (ScreenLetterboxFill::EdgeExtend, red, blue),
    ] {
        let fixture = SurfaceFixture::new(
            4,
            2,
            5,
            5,
            ScreenAspectPolicy::Contain,
            ScreenProcessingProfileConfig {
                letterbox_fill: fill,
                ..ScreenProcessingProfileConfig::default()
            },
        );
        let mut materializer = PreparedCpuSurfaceMaterializer::prepare_stateful(
            &fixture.descriptor,
            fixture.generation,
        )
        .expect("Surface materializer prepares");
        let mut physical = rgba_row(red, 5);
        physical.extend(rgba_row(blue, 5));
        let now = Instant::now();
        let mut publication = fixture.publication(1, now);
        materializer
            .stage(
                fixture.generation,
                &fixture.physical,
                &physical,
                now,
                false,
                &mut publication,
            )
            .expect("Surface fill stages");
        let pixels = publication
            .surface_pixels_mut()
            .expect("Surface output stays writable");
        assert!(
            pixels[..20]
                .chunks_exact(4)
                .all(|pixel| pixel == expected_top)
        );
        assert!(pixels[20..40].chunks_exact(4).all(|pixel| pixel == red));
        assert!(pixels[40..60].chunks_exact(4).all(|pixel| pixel == blue));
        assert!(
            pixels[60..]
                .chunks_exact(4)
                .all(|pixel| pixel == expected_bottom)
        );
        materializer
            .discard_staged(fixture.generation)
            .expect("unpublished fill state discards");
    }
}

#[test]
fn surface_fill_is_exact_after_tuning_for_rgba_and_bgra() {
    let tuning = ColorTuning {
        saturation: 1.6,
        brightness: 0.7,
        gamma: 1.2,
    };
    let source_top = [224, 48, 16, 255];
    let source_bottom = [24, 96, 208, 173];
    let solid = [3, 17, 99, 41];
    let mut tuned = [[source_top[0], source_top[1], source_top[2]]];
    tuning.apply(&mut tuned);
    let tuned_top = [tuned[0][0], tuned[0][1], tuned[0][2], source_top[3]];
    tuned[0] = [source_bottom[0], source_bottom[1], source_bottom[2]];
    tuning.apply(&mut tuned);
    let tuned_bottom = [tuned[0][0], tuned[0][1], tuned[0][2], source_bottom[3]];

    for pixel_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
        for (fill, expected_top, expected_bottom) in [
            (ScreenLetterboxFill::Transparent, [0, 0, 0, 0], [0, 0, 0, 0]),
            (
                ScreenLetterboxFill::Solid(solid),
                encoded_pixel(solid, pixel_format),
                encoded_pixel(solid, pixel_format),
            ),
            (
                ScreenLetterboxFill::EdgeExtend,
                encoded_pixel(tuned_top, pixel_format),
                encoded_pixel(tuned_bottom, pixel_format),
            ),
        ] {
            let fixture = SurfaceFixture::new(
                4,
                2,
                5,
                5,
                ScreenAspectPolicy::Contain,
                ScreenProcessingProfileConfig {
                    letterbox_fill: fill,
                    smoothing: ScreenSmoothingPolicy::Exponential {
                        time_constant: Duration::from_secs(1),
                        scene_cut: ScreenSceneCutPolicy::Disabled,
                    },
                    tuning: ScreenColorTuning::try_new(
                        tuning.saturation,
                        tuning.brightness,
                        tuning.gamma,
                    )
                    .expect("test tuning is finite"),
                    target_pixel_format: pixel_format,
                    ..ScreenProcessingProfileConfig::default()
                },
            );
            let mut materializer = PreparedCpuSurfaceMaterializer::prepare_stateful(
                &fixture.descriptor,
                fixture.generation,
            )
            .expect("processed Surface materializer prepares");
            let mut physical = rgba_row(encoded_pixel(source_top, pixel_format), 5);
            physical.extend(rgba_row(encoded_pixel(source_bottom, pixel_format), 5));
            let now = Instant::now();
            let mut publication = fixture.publication(1, now);
            materializer
                .stage(
                    fixture.generation,
                    &fixture.physical,
                    &physical,
                    now,
                    false,
                    &mut publication,
                )
                .expect("processed Surface stages");
            let pixels = publication
                .surface_pixels_mut()
                .expect("processed Surface remains writable");
            assert!(
                pixels[..20]
                    .chunks_exact(4)
                    .all(|pixel| pixel == expected_top)
            );
            assert!(
                pixels[60..]
                    .chunks_exact(4)
                    .all(|pixel| pixel == expected_bottom)
            );
        }
    }
}

#[test]
fn stateful_surface_and_zones_smooth_before_non_neutral_tuning() {
    let smoothing = ScreenSmoothingPolicy::Exponential {
        time_constant: Duration::from_millis(250),
        scene_cut: ScreenSceneCutPolicy::Disabled,
    };
    let tuning = ColorTuning {
        saturation: 1.7,
        brightness: 0.65,
        gamma: 1.3,
    };
    let configured_tuning =
        ScreenColorTuning::try_new(tuning.saturation, tuning.brightness, tuning.gamma)
            .expect("test tuning is finite");
    let incoming = [160, 96, 32];
    let elapsed = Duration::from_millis(100);
    let mut expected = [smoothed_color(smoothing, incoming, elapsed)];
    tuning.apply(&mut expected);
    let mut tuned_first = [incoming];
    tuning.apply(&mut tuned_first);
    let old_order = smoothed_color(smoothing, tuned_first[0], elapsed);
    assert_ne!(expected[0], old_order);

    for pixel_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
        let profile = ScreenProcessingProfileConfig {
            smoothing,
            tuning: configured_tuning,
            target_pixel_format: pixel_format,
            ..ScreenProcessingProfileConfig::default()
        };
        let surface = SurfaceFixture::new(1, 1, 1, 1, ScreenAspectPolicy::Cover, profile.clone());
        let mut surface_materializer = PreparedCpuSurfaceMaterializer::prepare_stateful(
            &surface.descriptor,
            surface.generation,
        )
        .expect("stateful Surface prepares");
        let started = Instant::now();
        let mut surface_baseline = surface.publication(1, started);
        surface_materializer
            .stage(
                surface.generation,
                &surface.physical,
                &encoded_pixel([0, 0, 0, 255], pixel_format),
                started,
                false,
                &mut surface_baseline,
            )
            .expect("Surface baseline stages");
        surface_materializer
            .commit_staged(surface.generation)
            .expect("Surface baseline commits");
        drop(surface_baseline);
        let mut surface_next = surface.publication(2, started + elapsed);
        surface_materializer
            .stage(
                surface.generation,
                &surface.physical,
                &encoded_pixel([incoming[0], incoming[1], incoming[2], 255], pixel_format),
                started + elapsed,
                false,
                &mut surface_next,
            )
            .expect("Surface response stages");
        let surface_pixel: [u8; 4] = surface_next
            .surface_pixels_mut()
            .expect("Surface response remains writable")
            .try_into()
            .expect("one Surface pixel has four bytes");
        assert_eq!(decoded_pixel(surface_pixel, pixel_format), expected[0]);

        let zones = ZoneFixture::new(1, 1, 1, 1, profile, CaptureColorimetry::SRGB);
        let mut zone_materializer =
            PreparedCpuZoneMaterializer::prepare_stateful(&zones.descriptor, zones.generation)
                .expect("stateful Zones prepare");
        let mut zone_baseline = zones.publication(1, started);
        zone_materializer
            .stage(
                zones.generation,
                &zones.physical,
                &encoded_pixel([0, 0, 0, 255], pixel_format),
                started,
                false,
                &mut zone_baseline,
            )
            .expect("Zones baseline stages");
        zone_materializer
            .commit_staged(zones.generation)
            .expect("Zones baseline commits");
        drop(zone_baseline);
        let mut zone_next = zones.publication(2, started + elapsed);
        zone_materializer
            .stage(
                zones.generation,
                &zones.physical,
                &encoded_pixel([incoming[0], incoming[1], incoming[2], 255], pixel_format),
                started + elapsed,
                false,
                &mut zone_next,
            )
            .expect("Zones response stages");
        assert_eq!(
            zone_next
                .zone_colors_mut()
                .expect("Zones response remains writable")[0],
            expected[0]
        );
    }
}

#[test]
fn detected_bars_reflow_without_stretching_content_aspect() {
    let fixture = SurfaceFixture::new(
        7,
        5,
        7,
        5,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: scalar(0.02),
            },
            letterbox_fill: ScreenLetterboxFill::Transparent,
            ..ScreenProcessingProfileConfig::default()
        },
    );
    let mut materializer =
        PreparedCpuSurfaceMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("dynamic Surface materializer prepares");
    let mut physical = rgba_row([0, 0, 0, 255], 7);
    physical.extend(rgba_row([255, 0, 0, 255], 7));
    physical.extend(rgba_row([0, 255, 0, 255], 7));
    physical.extend(rgba_row([0, 0, 255, 255], 7));
    physical.extend(rgba_row([0, 0, 0, 255], 7));
    let now = Instant::now();
    let mut publication = fixture.publication(1, now);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &physical,
            now,
            false,
            &mut publication,
        )
        .expect("detected content stages");
    let pixels = publication
        .surface_pixels_mut()
        .expect("dynamic output stays writable");
    assert!(
        pixels[..28]
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 0])
    );
    assert!(
        pixels[28..56]
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255])
    );
    assert!(
        pixels[56..84]
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 255, 0, 255])
    );
    assert!(
        pixels[84..112]
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 255, 255])
    );
    assert!(
        pixels[112..]
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 0])
    );
    materializer
        .discard_staged(fixture.generation)
        .expect("dynamic state discards");
}

#[test]
fn surface_materializer_rejects_substituted_physical_storage_transactionally() {
    let fixture = SurfaceFixture::new(
        4,
        2,
        5,
        5,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
    );
    let mut materializer =
        PreparedCpuSurfaceMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("Surface materializer prepares");
    let now = Instant::now();
    let mut publication = fixture.publication(1, now);
    assert_eq!(
        materializer.stage(
            fixture.generation,
            &fixture.physical,
            &[0; 4],
            now,
            false,
            &mut publication,
        ),
        Err(CpuSurfaceMaterializationError::PhysicalByteLengthMismatch {
            expected: 40,
            actual: 4,
        })
    );
}

#[test]
fn rejected_moving_bars_preserve_committed_surface_history() {
    let fixture = SurfaceFixture::new(
        5,
        5,
        5,
        5,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: scalar(0.02),
            },
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(100),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..ScreenProcessingProfileConfig::default()
        },
    );
    let mut materializer =
        PreparedCpuSurfaceMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("smoothed Surface materializer prepares");
    let black = [0, 0, 0, 255];
    let mut horizontal = rgba_row(black, 5);
    horizontal.extend(rgba_row([255, 0, 0, 255], 15));
    horizontal.extend(rgba_row(black, 5));
    let start = Instant::now();
    let mut first = fixture.publication(1, start);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &horizontal,
            start,
            false,
            &mut first,
        )
        .expect("first bar state stages");
    materializer
        .commit_staged(fixture.generation)
        .expect("first bar state commits");
    drop(first);

    let mut vertical = Vec::new();
    for _ in 0..5 {
        vertical.extend_from_slice(&black);
        vertical.extend(rgba_row([0, 0, 255, 255], 3));
        vertical.extend_from_slice(&black);
    }
    let mut rejected = fixture.publication(2, start + Duration::from_millis(16));
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &vertical,
            start + Duration::from_millis(16),
            false,
            &mut rejected,
        )
        .expect("moving bars stage");
    materializer
        .discard_staged(fixture.generation)
        .expect("moving bars rollback");
    drop(rejected);

    let mut restored = rgba_row(black, 5);
    restored.extend(rgba_row([0, 255, 0, 255], 15));
    restored.extend(rgba_row(black, 5));
    let mut third = fixture.publication(3, start + Duration::from_millis(32));
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &restored,
            start + Duration::from_millis(32),
            false,
            &mut third,
        )
        .expect("restored bars stage from committed history");
    let pixels = third
        .surface_pixels_mut()
        .expect("smoothed Surface remains writable");
    let center = &pixels[(2 * 5 + 2) * 4..(2 * 5 + 2) * 4 + 4];
    assert!(center[0] > 0, "red committed history must survive rollback");
    assert!(center[1] > 0, "green incoming content must contribute");
    assert_ne!(center, [0, 255, 0, 255]);
    materializer
        .discard_staged(fixture.generation)
        .expect("final staged state discards");
}

#[test]
fn dynamic_crop_compacts_the_effective_grid_and_reuses_exact_scratch() {
    let profile = ScreenProcessingProfileConfig {
        content_bars: ScreenContentBarsPolicy::DetectAndCrop {
            luminance_threshold: scalar(0.02),
        },
        ..point_profile()
    };
    let fixture = ZoneFixture::new(5, 3, 5, 3, profile, CaptureColorimetry::SRGB);
    let mut materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("stateful zones prepare");
    let retained = materializer.precomputed_byte_len();
    let pixels = rgba_grid(&[
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [255, 0, 255],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
    ]);
    let now = Instant::now();
    let mut publication = fixture.publication(1, now);
    let staged = materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &pixels,
            now,
            false,
            &mut publication,
        )
        .expect("dynamic grid stages");

    assert_eq!((staged.columns(), staged.rows()), (5, 1));
    assert_eq!(staged.color_count(), 5);
    assert_eq!((staged.bars().top, staged.bars().bottom), (1, 1));
    assert_eq!(materializer.precomputed_byte_len(), retained);
    let colors = publication
        .zone_colors_mut()
        .expect("zone storage remains writable");
    assert_eq!(
        &colors[..staged.color_count()],
        &[
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [255, 0, 255]
        ]
    );
    assert!(
        colors[staged.color_count()..]
            .iter()
            .all(|color| *color == [0, 0, 0])
    );
}

#[test]
fn zone_fill_is_rejected_instead_of_being_silently_approximated() {
    let fixture = ZoneFixture::new(
        3,
        3,
        3,
        3,
        ScreenProcessingProfileConfig {
            letterbox_fill: ScreenLetterboxFill::Transparent,
            ..point_profile()
        },
        CaptureColorimetry::SRGB,
    );

    assert!(matches!(
        PreparedCpuZoneMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation),
        Err(CpuZoneMaterializationError::LetterboxFillRequiresMaterialization)
    ));
}

#[test]
fn rejected_publication_preserves_committed_smoothing_history() {
    let fixture = ZoneFixture::new(
        1,
        1,
        1,
        1,
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(250),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..point_profile()
        },
        CaptureColorimetry::SRGB,
    );
    let mut materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("stateful zones prepare");
    let started = Instant::now();
    let mut initial = fixture.publication(1, started);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &[0, 0, 0, 255],
            started,
            false,
            &mut initial,
        )
        .expect("initial frame stages");
    materializer
        .commit_staged(fixture.generation)
        .expect("initial frame commits");
    drop(initial);

    let next_at = started + Duration::from_millis(100);
    let mut rejected = fixture.publication(2, next_at);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &[255, 255, 255, 255],
            next_at,
            false,
            &mut rejected,
        )
        .expect("candidate frame stages");
    let rejected_color = rejected
        .zone_colors_mut()
        .expect("candidate storage remains writable")[0];
    materializer
        .discard_staged(fixture.generation)
        .expect("candidate state discards");
    drop(rejected);

    let mut retry = fixture.publication(3, next_at);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &[255, 255, 255, 255],
            next_at,
            false,
            &mut retry,
        )
        .expect("retry frame stages");
    let retry_color = retry
        .zone_colors_mut()
        .expect("retry storage remains writable")[0];
    assert_eq!(retry_color, rejected_color);
}

#[test]
fn content_region_change_resets_smoothing_even_when_shape_is_unchanged() {
    let fixture = ZoneFixture::new(
        3,
        4,
        3,
        4,
        ScreenProcessingProfileConfig {
            content_bars: ScreenContentBarsPolicy::DetectAndCrop {
                luminance_threshold: scalar(0.02),
            },
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_hours(1),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..point_profile()
        },
        CaptureColorimetry::SRGB,
    );
    let mut materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("stateful zones prepare");
    let started = Instant::now();
    let top_bar = rgba_grid(&[
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
        [255, 0, 0],
    ]);
    let mut first = fixture.publication(1, started);
    let first_stage = materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &top_bar,
            started,
            false,
            &mut first,
        )
        .expect("top-bar frame stages");
    assert_eq!((first_stage.columns(), first_stage.rows()), (3, 3));
    materializer
        .commit_staged(fixture.generation)
        .expect("top-bar frame commits");
    drop(first);

    let bottom_bar = rgba_grid(&[
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 255],
        [0, 0, 0],
        [0, 0, 0],
        [0, 0, 0],
    ]);
    let mut second = fixture.publication(2, started + Duration::from_millis(16));
    let second_stage = materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &bottom_bar,
            started + Duration::from_millis(16),
            false,
            &mut second,
        )
        .expect("bottom-bar frame stages");
    assert_eq!((second_stage.columns(), second_stage.rows()), (3, 3));
    assert!(
        second.zone_colors_mut().expect("zone storage is writable")[..second_stage.color_count()]
            .iter()
            .all(|color| *color == [0, 0, 255])
    );
}

#[test]
fn plan_generation_fences_state_and_reset_is_deterministic() {
    let fixture = ZoneFixture::new(
        1,
        1,
        1,
        1,
        ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_hours(1),
                scene_cut: ScreenSceneCutPolicy::Disabled,
            },
            ..point_profile()
        },
        CaptureColorimetry::SRGB,
    );
    let mut materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
            .expect("stateful zones prepare");
    let different_generation = ScreenPlanGeneration::default();
    let now = Instant::now();
    let mut publication = fixture.publication(1, now);
    assert!(matches!(
        materializer.stage(
            different_generation,
            &fixture.physical,
            &[255, 0, 0, 255],
            now,
            false,
            &mut publication,
        ),
        Err(CpuZoneMaterializationError::PlanGenerationMismatch { .. })
    ));

    let mut baseline = fixture.publication(2, now);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &[0, 0, 0, 255],
            now,
            false,
            &mut baseline,
        )
        .expect("baseline stages");
    materializer
        .commit_staged(fixture.generation)
        .expect("baseline commits");
    drop(baseline);
    let later = now + Duration::from_millis(16);
    let mut smoothed = fixture.publication(3, later);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &[255, 255, 255, 255],
            later,
            false,
            &mut smoothed,
        )
        .expect("pre-reset frame stages");
    assert_ne!(
        smoothed
            .zone_colors_mut()
            .expect("pre-reset storage remains writable")[0],
        [255, 255, 255]
    );
    materializer
        .discard_staged(fixture.generation)
        .expect("pre-reset frame discards");
    drop(smoothed);
    materializer
        .reset_for_plan_generation(fixture.generation)
        .expect("same-generation reset clears history");
    let mut reset = fixture.publication(4, later);
    materializer
        .stage(
            fixture.generation,
            &fixture.physical,
            &[255, 255, 255, 255],
            later,
            false,
            &mut reset,
        )
        .expect("post-reset frame stages");
    assert_eq!(
        reset
            .zone_colors_mut()
            .expect("post-reset storage remains writable")[0],
        [255, 255, 255]
    );
    materializer
        .discard_staged(fixture.generation)
        .expect("post-reset frame discards");
    materializer
        .reset_for_plan_generation(different_generation)
        .expect("state resets for the new generation");
    assert_eq!(materializer.plan_generation(), Some(different_generation));
}

#[test]
fn stateful_materialization_supports_rgba_bgra_srgb_and_linear() {
    let linear = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Linear,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("linear SDR colorimetry is valid");
    for (format, colorimetry, pixels) in [
        (
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            [10, 20, 30, 255],
        ),
        (
            CapturePixelFormat::Bgra8,
            CaptureColorimetry::SRGB,
            [30, 20, 10, 255],
        ),
        (
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::from_known(linear),
            [10, 20, 30, 255],
        ),
        (
            CapturePixelFormat::Bgra8,
            CaptureColorimetry::from_known(linear),
            [30, 20, 10, 255],
        ),
    ] {
        let fixture = ZoneFixture::new(
            1,
            1,
            1,
            1,
            ScreenProcessingProfileConfig {
                target_pixel_format: format,
                target_colorimetry: ScreenTargetColorimetry::PreserveSource,
                ..point_profile()
            },
            colorimetry,
        );
        let mut materializer =
            PreparedCpuZoneMaterializer::prepare_stateful(&fixture.descriptor, fixture.generation)
                .expect("stateful transfer prepares");
        let now = Instant::now();
        let mut publication = fixture.publication(1, now);
        materializer
            .stage(
                fixture.generation,
                &fixture.physical,
                &pixels,
                now,
                false,
                &mut publication,
            )
            .expect("stateful transfer stages");
        assert_eq!(
            publication
                .zone_colors_mut()
                .expect("zone storage remains writable")[0],
            [10, 20, 30]
        );
    }
}

#[test]
fn distinct_descriptors_keep_independent_temporal_history() {
    let smoothing = ScreenSmoothingPolicy::Exponential {
        time_constant: Duration::from_hours(1),
        scene_cut: ScreenSceneCutPolicy::Disabled,
    };
    let rgba = ZoneFixture::new(
        1,
        1,
        1,
        1,
        ScreenProcessingProfileConfig {
            smoothing,
            ..point_profile()
        },
        CaptureColorimetry::SRGB,
    );
    let bgra = ZoneFixture::new(
        1,
        1,
        1,
        1,
        ScreenProcessingProfileConfig {
            smoothing,
            target_pixel_format: CapturePixelFormat::Bgra8,
            ..point_profile()
        },
        CaptureColorimetry::SRGB,
    );
    assert_ne!(rgba.descriptor, bgra.descriptor);
    let mut rgba_materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&rgba.descriptor, rgba.generation)
            .expect("RGBA state prepares");
    let mut bgra_materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&bgra.descriptor, bgra.generation)
            .expect("BGRA state prepares");
    let started = Instant::now();
    let mut rgba_initial = rgba.publication(1, started);
    rgba_materializer
        .stage(
            rgba.generation,
            &rgba.physical,
            &[0, 0, 0, 255],
            started,
            false,
            &mut rgba_initial,
        )
        .expect("RGBA baseline stages");
    rgba_materializer
        .commit_staged(rgba.generation)
        .expect("RGBA baseline commits");
    drop(rgba_initial);
    let mut bgra_initial = bgra.publication(1, started);
    bgra_materializer
        .stage(
            bgra.generation,
            &bgra.physical,
            &[255, 255, 255, 255],
            started,
            false,
            &mut bgra_initial,
        )
        .expect("BGRA baseline stages");
    bgra_materializer
        .commit_staged(bgra.generation)
        .expect("BGRA baseline commits");
    drop(bgra_initial);

    let later = started + Duration::from_millis(16);
    let mut rgba_next = rgba.publication(2, later);
    rgba_materializer
        .stage(
            rgba.generation,
            &rgba.physical,
            &[128, 128, 128, 255],
            later,
            false,
            &mut rgba_next,
        )
        .expect("RGBA next frame stages");
    let rgba_output = rgba_next
        .zone_colors_mut()
        .expect("RGBA storage remains writable")[0][0];
    let mut bgra_next = bgra.publication(2, later);
    bgra_materializer
        .stage(
            bgra.generation,
            &bgra.physical,
            &[128, 128, 128, 255],
            later,
            false,
            &mut bgra_next,
        )
        .expect("BGRA next frame stages");
    let bgra_output = bgra_next
        .zone_colors_mut()
        .expect("BGRA storage remains writable")[0][0];
    assert!(rgba_output < bgra_output);
}

#[test]
fn prepared_smoothing_is_equivalent_at_30_60_and_120_hz() {
    fn response(fps: u32) -> u8 {
        let policy = ScreenSmoothingPolicy::Exponential {
            time_constant: Duration::from_millis(250),
            scene_cut: ScreenSceneCutPolicy::Disabled,
        };
        let mut smoother =
            PreparedTemporalSmoother::try_new(policy, 1, 1).expect("smoother prepares");
        let mut colors = [[0, 0, 0]];
        smoother
            .stage(
                &mut colors,
                1,
                1,
                CaptureTransferFunction::Srgb,
                Duration::ZERO,
                false,
                false,
            )
            .expect("initial state stages");
        assert!(smoother.commit_staged());
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
        for _ in 0..fps {
            colors[0] = [255, 255, 255];
            smoother
                .stage(
                    &mut colors,
                    1,
                    1,
                    CaptureTransferFunction::Srgb,
                    interval,
                    false,
                    false,
                )
                .expect("response stage succeeds");
            assert!(smoother.commit_staged());
        }
        colors[0][0]
    }

    let at_30_hz = response(30);
    let at_60_hz = response(60);
    let at_120_hz = response(120);
    assert!(at_30_hz.abs_diff(at_60_hz) <= 1);
    assert!(at_60_hz.abs_diff(at_120_hz) <= 1);
}

#[test]
fn normalized_scene_cut_resets_independent_of_grid_size() {
    for (width, height) in [(1, 1), (17, 9), (3, 31)] {
        let policy = ScreenSmoothingPolicy::Exponential {
            time_constant: Duration::from_mins(1),
            scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
                threshold: scalar(0.5),
            },
        };
        let count = usize::try_from(width * height).expect("test grid is addressable");
        let mut smoother = PreparedTemporalSmoother::try_new(policy, width, height)
            .expect("scene-cut smoother prepares");
        let mut colors = vec![[0, 0, 0]; count];
        smoother
            .stage(
                &mut colors,
                width,
                height,
                CaptureTransferFunction::Srgb,
                Duration::ZERO,
                false,
                false,
            )
            .expect("baseline stages");
        assert!(smoother.commit_staged());
        colors.fill([255, 255, 255]);
        smoother
            .stage(
                &mut colors,
                width,
                height,
                CaptureTransferFunction::Srgb,
                Duration::from_millis(16),
                false,
                false,
            )
            .expect("scene cut stages");
        assert!(colors.iter().all(|color| *color == [255, 255, 255]));
    }
}

#[test]
fn prepared_smoothing_can_suppress_scene_cut_bypass() {
    let policy = ScreenSmoothingPolicy::Exponential {
        time_constant: Duration::from_mins(1),
        scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
            threshold: scalar(0.01),
        },
    };
    let mut smoother = PreparedTemporalSmoother::try_new(policy, 1, 1).expect("smoother prepares");
    let mut colors = [[0, 0, 0]];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::ZERO,
            false,
            false,
        )
        .expect("baseline stages");
    assert!(smoother.commit_staged());

    colors[0] = [255, 255, 255];
    smoother
        .stage(
            &mut colors,
            1,
            1,
            CaptureTransferFunction::Srgb,
            Duration::from_millis(16),
            false,
            true,
        )
        .expect("suppressed scene cut stages");

    assert!(colors[0][0] < 255);
}

#[test]
fn materializers_forward_transition_suppression_to_both_smoothing_seams() {
    let profile = ScreenProcessingProfileConfig {
        smoothing: ScreenSmoothingPolicy::Exponential {
            time_constant: Duration::from_mins(1),
            scene_cut: ScreenSceneCutPolicy::MeanAbsoluteDelta {
                threshold: scalar(0.01),
            },
        },
        ..point_profile()
    };
    let started = Instant::now();
    let later = started + Duration::from_millis(16);

    let surface = SurfaceFixture::new(1, 1, 1, 1, ScreenAspectPolicy::Cover, profile.clone());
    let mut surface_materializer =
        PreparedCpuSurfaceMaterializer::prepare_stateful(&surface.descriptor, surface.generation)
            .expect("stateful Surface prepares");
    let mut surface_baseline = surface.publication(1, started);
    surface_materializer
        .stage(
            surface.generation,
            &surface.physical,
            &[0, 0, 0, 255],
            started,
            false,
            &mut surface_baseline,
        )
        .expect("Surface baseline stages");
    surface_materializer
        .commit_staged(surface.generation)
        .expect("Surface baseline commits");
    drop(surface_baseline);
    let mut surface_transition = surface.publication(2, later);
    surface_materializer
        .stage(
            surface.generation,
            &surface.physical,
            &[255, 255, 255, 255],
            later,
            true,
            &mut surface_transition,
        )
        .expect("Surface transition stages");
    assert!(
        surface_transition
            .surface_pixels_mut()
            .expect("Surface output remains writable")[0]
            < 255
    );

    let zones = ZoneFixture::new(1, 1, 1, 1, profile, CaptureColorimetry::SRGB);
    let mut zone_materializer =
        PreparedCpuZoneMaterializer::prepare_stateful(&zones.descriptor, zones.generation)
            .expect("stateful Zones prepare");
    let mut zone_baseline = zones.publication(1, started);
    zone_materializer
        .stage(
            zones.generation,
            &zones.physical,
            &[0, 0, 0, 255],
            started,
            false,
            &mut zone_baseline,
        )
        .expect("Zones baseline stages");
    zone_materializer
        .commit_staged(zones.generation)
        .expect("Zones baseline commits");
    drop(zone_baseline);
    let mut zone_transition = zones.publication(2, later);
    zone_materializer
        .stage(
            zones.generation,
            &zones.physical,
            &[255, 255, 255, 255],
            later,
            true,
            &mut zone_transition,
        )
        .expect("Zones transition stages");
    assert!(
        zone_transition
            .zone_colors_mut()
            .expect("Zones output remains writable")[0][0]
            < 255
    );
}

#[test]
fn prepared_state_admits_odd_portrait_ultrawide_and_one_pixel_shapes() {
    for (width, height) in [(1, 1), (7, 5), (127, 3), (3, 127)] {
        let mut detector =
            PreparedLetterboxDetector::try_new(width, height).expect("detector prepares");
        let capacities = detector.capacities();
        let count = usize::try_from(width * height).expect("test shape is addressable");
        let colors = vec![[32, 64, 96]; count];
        assert_eq!(
            detector
                .detect(&colors, CaptureTransferFunction::Srgb, 0.01)
                .expect("shape detects")
                .has_bars(),
            false
        );
        assert_eq!(detector.shape(), (width, height));
        assert_eq!(detector.capacities(), capacities);
    }
}

#[test]
fn oversize_history_admission_fails_before_allocation() {
    let result = PreparedTemporalSmoother::try_new(
        ScreenSmoothingPolicy::Exponential {
            time_constant: Duration::from_secs(1),
            scene_cut: ScreenSceneCutPolicy::Disabled,
        },
        u32::MAX,
        u32::MAX,
    );
    assert!(matches!(
        result,
        Err(SurfaceResourceError::ByteLengthOverflow {
            width: u32::MAX,
            height: u32::MAX
        })
    ));
    assert!(matches!(
        PreparedLetterboxDetector::try_new(0, 1),
        Err(LetterboxDetectionError::EmptyGrid {
            columns: 0,
            rows: 1
        })
    ));
}
