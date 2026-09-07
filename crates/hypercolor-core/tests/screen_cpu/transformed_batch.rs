//! Exact transformed CPU batch execution across arbitrary capture geometry.

use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::consumer::{
    CaptureEpoch, CaptureSourceId, PixelExtent, PixelRect,
};
use hypercolor_core::input::screen::implementer::{
    CaptureColorimetry, CaptureCursor, CaptureDamage, CaptureFrame, CaptureFrameMetadata,
    CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureStorage, CpuCaptureStorage,
    CpuReductionBatchJob, CpuReductionBatchReport, CpuReductionExecutor, CpuReductionLayout,
    CpuReductionRequest, CpuSamplingPoint, CpuSamplingView, KnownCaptureColorimetry,
    PhysicalOrigin, PreparedCpuReductionBatch, RawCaptureSurface, SourceScale,
};
use hypercolor_core::input::screen::planner::{
    InputPublicationDemandRevision, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenCursorCapabilities, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenPhysicalReductionDescriptor, ScreenPlanBuilder, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationExecutorRequest, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenRational, ScreenReductionFilter, ScreenResourceApi,
    ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn executor() -> CpuReductionExecutor {
    executor_with(4, 2)
}

fn executor_with(worker_count: usize, tile_rows: u32) -> CpuReductionExecutor {
    CpuReductionExecutor::new(
        NonZeroUsize::new(worker_count).expect("test worker count is non-zero"),
        non_zero(tile_rows),
    )
    .expect("test worker pool builds")
}

fn geometry(
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
    source_scale: SourceScale,
) -> CaptureGeometry {
    CaptureGeometry::new(
        PhysicalOrigin::default(),
        native_extent,
        storage_extent,
        rotation,
        crop,
        source_scale,
    )
    .expect("test geometry is valid")
}

fn source(
    id: &str,
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
) -> ResolvedScreenSource {
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new(id).expect("test source identity is non-empty"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            logical_extent,
            reflection,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::from_known(KnownCaptureColorimetry::SRGB),
            ScreenCursorCapabilities::clean_only(),
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn demand(
    source: &ResolvedScreenSource,
    extent_request: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
    filter: ScreenReductionFilter,
    capabilities: ScreenColorTransformCapabilities,
) -> ResolvedScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Surface,
            ScreenPublicationExecutorRequest::Cpu,
            extent_request,
            aspect,
            Arc::new(ScreenProcessingProfile::new(
                ScreenProcessingProfileConfig {
                    reduction_filter: filter,
                    ..ScreenProcessingProfileConfig::default()
                },
            )),
        ),
        non_zero(60),
    )
    .resolve_with_color_capabilities(source, capabilities)
    .expect("test demand resolves")
}

fn prepare(
    executor: &CpuReductionExecutor,
    source: &ResolvedScreenSource,
    demand: ResolvedScreenBranchDemand,
) -> PreparedCpuReductionBatch {
    let mut builder = ScreenPlanBuilder::new();
    let preparing = builder
        .prepare(
            [demand],
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan is admitted");
    executor
        .prepare_batch(source, preparing.candidate_plan())
        .expect("test batch prepares")
}

fn labeled_pixels(storage_extent: PixelExtent) -> Arc<[u8]> {
    (1..=storage_extent.width() * storage_extent.height())
        .flat_map(|label| {
            let label = u8::try_from(label).expect("test label fits in one byte");
            [label, label, label, u8::MAX]
        })
        .collect::<Vec<_>>()
        .into()
}

fn frame(source: &ResolvedScreenSource) -> CaptureFrame<RawCaptureSurface> {
    let captured_at = Instant::now();
    let geometry = source.config().geometry();
    let storage_extent = geometry.storage_extent();
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation: source.epoch().session_generation,
            sequence: 17,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry,
            colorimetry: source.config().colorimetry(),
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_pixels(storage_extent),
            source.config().pixel_format(),
            i64::from(storage_extent.width()) * 4,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid")
}

fn execute_one(
    executor: &CpuReductionExecutor,
    batch: &PreparedCpuReductionBatch,
    frame: &CaptureFrame<RawCaptureSurface>,
) -> Vec<u8> {
    execute_one_with_report(executor, batch, frame).0
}

fn execute_one_with_report(
    executor: &CpuReductionExecutor,
    batch: &PreparedCpuReductionBatch,
    frame: &CaptureFrame<RawCaptureSurface>,
) -> (Vec<u8>, CpuReductionBatchReport) {
    assert_eq!(batch.len(), 1);
    let mut output = vec![0; batch.output_byte_len(0).expect("output size exists")];
    let report = {
        let mut jobs = [CpuReductionBatchJob::new(
            batch.descriptor(0).expect("descriptor exists"),
            &mut output,
        )];
        executor
            .execute_batch(batch, frame, &mut jobs)
            .expect("batch execution succeeds")
    };
    (output, report)
}

fn logical_pixel_centers(view: &CpuSamplingView<'_>, extent: PixelExtent) -> Vec<u8> {
    (0..extent.height())
        .flat_map(|y| {
            (0..extent.width()).flat_map(move |x| {
                let point = CpuSamplingPoint::new(
                    ScreenRational::new(u64::from(x) * 2 + 1, 2).expect("test coordinate is valid"),
                    ScreenRational::new(u64::from(y) * 2 + 1, 2).expect("test coordinate is valid"),
                );
                view.read_logical_nearest(point)
                    .expect("logical center maps to storage")
            })
        })
        .collect()
}

fn exact_center(
    origin: ScreenRational,
    span: ScreenRational,
    index: u32,
    target_len: u32,
) -> ScreenRational {
    let doubled_target = u128::from(target_len) * 2;
    let numerator =
        u128::from(origin.numerator()) * u128::from(span.denominator().get()) * doubled_target
            + u128::from(span.numerator())
                * u128::from(origin.denominator().get())
                * (u128::from(index) * 2 + 1);
    let denominator = u128::from(origin.denominator().get())
        * u128::from(span.denominator().get())
        * doubled_target;
    ScreenRational::new(
        u64::try_from(numerator).expect("small test numerator fits"),
        u64::try_from(denominator).expect("small test denominator fits"),
    )
    .expect("test center is valid")
}

fn expected_nearest(
    view: &CpuSamplingView<'_>,
    descriptor: &ScreenPhysicalReductionDescriptor,
) -> Vec<u8> {
    let region = descriptor.source_region();
    let target = descriptor.reduction_extent();
    (0..target.height())
        .flat_map(|y| {
            (0..target.width()).flat_map(move |x| {
                view.read_logical_nearest(CpuSamplingPoint::new(
                    exact_center(region.x(), region.width(), x, target.width()),
                    exact_center(region.y(), region.height(), y, target.height()),
                ))
                .expect("exact target center maps to storage")
            })
        })
        .collect()
}

fn scalar_storage_reference(
    executor: &CpuReductionExecutor,
    frame: &CaptureFrame<RawCaptureSurface>,
    descriptor: &ScreenPhysicalReductionDescriptor,
) -> Vec<u8> {
    let CaptureStorage::Cpu(storage) = frame.storage() else {
        panic!("test frame uses CPU storage");
    };
    let layout = CpuReductionLayout::new(
        frame.metadata().geometry.storage_extent(),
        descriptor.reduction_extent(),
    )
    .expect("test reduction layout is valid");
    let mut output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(
            CpuReductionRequest::new(
                storage,
                layout,
                descriptor.target_pixel_format(),
                descriptor.reduction_filter(),
                descriptor.color_pipeline(),
            ),
            &mut output,
        )
        .expect("scalar storage reference succeeds");
    output
}

#[test]
fn every_rotation_reflection_and_filter_preserves_native_texels() {
    let executor = executor();
    let native = extent(3, 2);
    for rotation in [
        CaptureRotation::Identity,
        CaptureRotation::Clockwise90,
        CaptureRotation::Clockwise180,
        CaptureRotation::Clockwise270,
        CaptureRotation::Flipped,
        CaptureRotation::Flipped90,
        CaptureRotation::Flipped180,
        CaptureRotation::Flipped270,
    ] {
        let logical = rotation.apply_to_extent(native);
        for reflection in [
            ScreenSourceReflection::None,
            ScreenSourceReflection::Horizontal,
            ScreenSourceReflection::Vertical,
            ScreenSourceReflection::Both,
        ] {
            let source = source(
                "synthetic:transform-matrix",
                geometry(native, native, rotation, None, SourceScale::ONE),
                logical,
                reflection,
            );
            let frame = frame(&source);
            let view = CpuSamplingView::try_new(&frame, &source).expect("sampling view is valid");
            let expected = logical_pixel_centers(&view, logical);
            for filter in [
                ScreenReductionFilter::Nearest,
                ScreenReductionFilter::Bilinear,
                ScreenReductionFilter::Area,
            ] {
                let resolved = demand(
                    &source,
                    ScreenExtentRequest::Native,
                    ScreenAspectPolicy::Contain,
                    filter,
                    executor.capabilities(),
                );
                let batch = prepare(&executor, &source, resolved);
                assert_eq!(
                    execute_one(&executor, &batch, &frame),
                    expected,
                    "rotation={rotation:?} reflection={reflection:?} filter={filter:?}"
                );
            }
        }
    }
}

#[test]
fn compounded_crop_rotation_reflection_and_cover_use_exact_centers() {
    let executor = executor();
    let native = extent(7, 5);
    let source = source(
        "synthetic:compounded",
        geometry(
            native,
            extent(4, 3),
            CaptureRotation::Clockwise90,
            Some(PixelRect::new(1, 1, 4, 2).expect("test crop is valid")),
            SourceScale::new(3, 2).expect("test scale is valid"),
        ),
        extent(3, 6),
        ScreenSourceReflection::Both,
    );
    let frame = frame(&source);
    let resolved = demand(
        &source,
        ScreenExtentRequest::bounded(
            Some(non_zero(2)),
            Some(non_zero(3)),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Cover,
        ScreenReductionFilter::Nearest,
        executor.capabilities(),
    );
    let batch = prepare(&executor, &source, resolved);
    let descriptor = batch.descriptor(0).expect("descriptor exists");
    let view = CpuSamplingView::try_new(&frame, &source).expect("sampling view is valid");

    assert_eq!(
        execute_one(&executor, &batch, &frame),
        expected_nearest(&view, descriptor)
    );
}

#[test]
fn source_and_storage_scales_match_the_scalar_filter_reference() {
    let executor = executor();
    let cases = [
        source(
            "synthetic:source-scale",
            geometry(
                extent(4, 2),
                extent(4, 2),
                CaptureRotation::Identity,
                None,
                SourceScale::new(1, 2).expect("test scale is valid"),
            ),
            extent(2, 1),
            ScreenSourceReflection::None,
        ),
        source(
            "synthetic:storage-scale",
            geometry(
                extent(7, 5),
                extent(4, 3),
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            ),
            extent(7, 5),
            ScreenSourceReflection::None,
        ),
    ];
    for source in cases {
        let frame = frame(&source);
        for filter in [
            ScreenReductionFilter::Nearest,
            ScreenReductionFilter::Bilinear,
            ScreenReductionFilter::Area,
        ] {
            let extent_request = if source.logical_extent() == extent(2, 1) {
                ScreenExtentRequest::Native
            } else {
                ScreenExtentRequest::bounded(
                    Some(non_zero(3)),
                    Some(non_zero(3)),
                    ScreenUpscalePolicy::Never,
                )
            };
            let resolved = demand(
                &source,
                extent_request,
                ScreenAspectPolicy::Contain,
                filter,
                executor.capabilities(),
            );
            let batch = prepare(&executor, &source, resolved);
            let descriptor = batch.descriptor(0).expect("descriptor exists");
            assert_eq!(
                execute_one(&executor, &batch, &frame),
                scalar_storage_reference(&executor, &frame, descriptor),
                "source={} filter={filter:?}",
                source.epoch().source_id
            );
        }
    }
}

#[test]
fn maximum_u32_width_prepares_without_allocating_a_raster() {
    let executor = executor();
    let maximum = extent(u32::MAX, 1);
    let source = source(
        "synthetic:max-width",
        geometry(
            maximum,
            maximum,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        ),
        maximum,
        ScreenSourceReflection::None,
    );
    let resolved = demand(
        &source,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenReductionFilter::Area,
        executor.capabilities(),
    );

    let batch = prepare(&executor, &source, resolved);

    assert_eq!(batch.len(), 1);
    assert_eq!(batch.output_byte_len(0), Some(17_179_869_180));
}

#[test]
fn ultrawide_results_are_invariant_across_worker_and_tile_counts() {
    let serial = executor_with(1, 1);
    let parallel = executor_with(8, 64);
    for (native, rotation) in [
        (extent(251, 1), CaptureRotation::Identity),
        (extent(1, 251), CaptureRotation::Clockwise90),
    ] {
        let source = source(
            "synthetic:ultrawide-tiling",
            geometry(native, native, rotation, None, SourceScale::ONE),
            extent(251, 1),
            ScreenSourceReflection::Horizontal,
        );
        let frame = frame(&source);
        for filter in [
            ScreenReductionFilter::Nearest,
            ScreenReductionFilter::Bilinear,
            ScreenReductionFilter::Area,
        ] {
            let serial_demand = demand(
                &source,
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Contain,
                filter,
                serial.capabilities(),
            );
            let parallel_demand = demand(
                &source,
                ScreenExtentRequest::Native,
                ScreenAspectPolicy::Contain,
                filter,
                parallel.capabilities(),
            );
            let serial_batch = prepare(&serial, &source, serial_demand);
            let parallel_batch = prepare(&parallel, &source, parallel_demand);

            let (serial_output, serial_report) =
                execute_one_with_report(&serial, &serial_batch, &frame);
            let (parallel_output, parallel_report) =
                execute_one_with_report(&parallel, &parallel_batch, &frame);
            assert_eq!(
                serial_output, parallel_output,
                "rotation={rotation:?} filter={filter:?}"
            );
            assert_eq!(serial_report.scheduled_tiles(), 4);
            assert_eq!(parallel_report.scheduled_tiles(), 32);
        }
    }
}
