//! Descriptor-bound arbitrary-resolution CPU fanout contracts.

use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameError, CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CaptureStorage, CpuCaptureStorage, CpuReductionBatchJob, CpuReductionError,
    CpuReductionExecutor, CpuReductionLayout, CpuReductionRequest, InputPublicationDemandRevision,
    KnownCaptureColorimetry, PhysicalOrigin, PixelExtent, PlatformGpuApi, PlatformGpuSurface,
    RawCaptureSurface, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAdmissionCapacity, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenCursorCapabilities, ScreenCursorPolicy, ScreenExtentRequest, ScreenInputGraphGeneration,
    ScreenPlanBuilder, ScreenProcessingProfile, ScreenProcessingProfileConfig,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenReductionFilter, ScreenResourceApi,
    ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy, SourceScale,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn executor() -> CpuReductionExecutor {
    CpuReductionExecutor::new(
        NonZeroUsize::new(4).expect("test worker count is non-zero"),
        non_zero(3),
    )
    .expect("test worker pool builds")
}

fn source_with(
    source_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    cursor_capabilities: ScreenCursorCapabilities,
    resource_generation: u64,
) -> ResolvedScreenSource {
    source_named_with(
        "synthetic:cpu-batch",
        source_extent,
        reflection,
        cursor_capabilities,
        resource_generation,
    )
}

fn source_named_with(
    source_id: &str,
    source_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    cursor_capabilities: ScreenCursorCapabilities,
    resource_generation: u64,
) -> ResolvedScreenSource {
    source_named_with_api(
        source_id,
        source_extent,
        reflection,
        cursor_capabilities,
        resource_generation,
        ScreenResourceApi::Cpu,
    )
}

fn source_named_with_api(
    source_id: &str,
    source_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    cursor_capabilities: ScreenCursorCapabilities,
    resource_generation: u64,
    resource_api: ScreenResourceApi,
) -> ResolvedScreenSource {
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
            source_id: CaptureSourceId::new(source_id).expect("test source identity is non-empty"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            source_extent,
            reflection,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::from_known(KnownCaptureColorimetry::SRGB),
            cursor_capabilities,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                resource_api,
                7,
                resource_generation,
            ),
        ),
    )
}

fn source(source_extent: PixelExtent) -> ResolvedScreenSource {
    source_with(
        source_extent,
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_only(),
        11,
    )
}

fn demand(
    source: &ResolvedScreenSource,
    kind: ScreenPublicationKind,
    extent_request: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
    profile: ScreenProcessingProfileConfig,
    requested_hz: u32,
    capabilities: ScreenColorTransformCapabilities,
) -> ResolvedScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            extent_request,
            aspect,
            Arc::new(ScreenProcessingProfile::new(profile)),
        ),
        non_zero(requested_hz),
    )
    .resolve_with_color_capabilities(source, capabilities)
    .expect("test demand resolves")
}

fn prepare_batch(
    executor: &CpuReductionExecutor,
    source: &ResolvedScreenSource,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
) -> Result<hypercolor_core::input::screen::PreparedCpuReductionBatch, CpuReductionError> {
    let mut builder = ScreenPlanBuilder::new();
    let preparing = builder
        .prepare(
            demands,
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan is admitted");
    executor.prepare_batch(source, preparing.candidate_plan())
}

fn patterned_pixels(source_extent: PixelExtent) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(
        usize::try_from(u64::from(source_extent.width()) * u64::from(source_extent.height()) * 4)
            .expect("test pixels are addressable"),
    );
    for y in 0..source_extent.height() {
        for x in 0..source_extent.width() {
            pixels.extend_from_slice(&[
                ((u64::from(x) * 37 + u64::from(y) * 11 + 19) % 256) as u8,
                ((u64::from(x) * 13 + u64::from(y) * 53 + 71) % 256) as u8,
                ((u64::from(x) * 97 + u64::from(y) * 7 + 3) % 256) as u8,
                u8::MAX,
            ]);
        }
    }
    pixels
}

fn frame(
    source: &ResolvedScreenSource,
    session_generation: u64,
    cursor: CaptureCursor,
) -> CaptureFrame<RawCaptureSurface> {
    let captured_at = Instant::now();
    let source_extent = source.config().geometry().storage_extent();
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation,
            sequence: 17,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry: source.config().geometry(),
            colorimetry: source.config().colorimetry(),
            cursor,
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::from(patterned_pixels(source_extent)),
            source.config().pixel_format(),
            i64::from(source_extent.width()) * 4,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid")
}

fn scalar_output(
    executor: &CpuReductionExecutor,
    frame: &CaptureFrame<RawCaptureSurface>,
    descriptor: &hypercolor_core::input::screen::ScreenPhysicalReductionDescriptor,
) -> Vec<u8> {
    let CaptureStorage::Cpu(storage) = frame.storage() else {
        panic!("test frame uses CPU storage");
    };
    let layout = CpuReductionLayout::new(
        descriptor.source().logical_extent(),
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
        .expect("scalar reference reduction succeeds");
    output
}

#[test]
fn batch_executes_unique_physical_keys_once_from_one_frame() {
    let executor = executor();
    let source = source(extent(8, 6));
    let full = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    );
    let reduced = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(4)),
            Some(non_zero(3)),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Bilinear,
            ..ScreenProcessingProfileConfig::default()
        },
        30,
        executor.capabilities(),
    );
    let batch = prepare_batch(&executor, &source, [full, reduced]).expect("batch prepares");
    assert_eq!(batch.len(), 2);
    assert!(!batch.is_empty());
    assert_eq!(batch.plan_generation().get(), 1);
    assert_eq!(batch.source().epoch(), source.epoch());
    let frame = frame(
        &source,
        source.epoch().session_generation,
        CaptureCursor::default(),
    );
    let expected = (0..batch.len())
        .map(|index| {
            scalar_output(
                &executor,
                &frame,
                batch.descriptor(index).expect("batch descriptor exists"),
            )
        })
        .collect::<Vec<_>>();
    let mut outputs = (0..batch.len())
        .map(|index| vec![0; batch.output_byte_len(index).expect("output size exists")])
        .collect::<Vec<_>>();
    let mut jobs = outputs
        .iter_mut()
        .enumerate()
        .map(|(index, output)| {
            CpuReductionBatchJob::new(
                batch.descriptor(index).expect("batch descriptor exists"),
                output,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        jobs[0].output().len(),
        batch.output_byte_len(0).expect("output size exists")
    );
    assert_eq!(
        jobs[0].descriptor(),
        batch.descriptor(0).expect("batch descriptor exists")
    );

    let report = executor
        .execute_batch(&batch, &frame, &mut jobs)
        .expect("batch execution succeeds");

    assert_eq!(outputs, expected);
    assert_eq!(report.source_sequence(), 17);
    assert_eq!(report.completed_jobs(), 2);
    assert_eq!(
        report.output_bytes(),
        expected
            .iter()
            .map(|output| output.len() as u64)
            .sum::<u64>()
    );
}

#[test]
fn surface_and_zones_share_one_prepared_physical_key() {
    let executor = executor();
    let source = source(extent(16, 9));
    let profile = ScreenProcessingProfileConfig::default();
    let surface = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        profile.clone(),
        60,
        executor.capabilities(),
    );
    let zones = demand(
        &source,
        ScreenPublicationKind::Zones {
            columns: non_zero(8),
            rows: non_zero(6),
        },
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        profile,
        30,
        executor.capabilities(),
    );

    let batch = prepare_batch(&executor, &source, [surface, zones]).expect("batch prepares");

    assert_eq!(batch.len(), 1);
    assert_eq!(batch.output_byte_len(0), Some(16 * 9 * 4));
}

#[test]
fn reordered_equal_sized_outputs_are_rejected_before_writes() {
    let executor = executor();
    let source = source(extent(8, 6));
    let area = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    );
    let bilinear = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Bilinear,
            ..ScreenProcessingProfileConfig::default()
        },
        60,
        executor.capabilities(),
    );
    let batch = prepare_batch(&executor, &source, [area, bilinear]).expect("batch prepares");
    assert_eq!(batch.len(), 2);
    assert_eq!(batch.output_byte_len(0), batch.output_byte_len(1));
    let frame = frame(
        &source,
        source.epoch().session_generation,
        CaptureCursor::default(),
    );
    let mut outputs = [
        vec![0xA5; batch.output_byte_len(0).expect("output size exists")],
        vec![0x5A; batch.output_byte_len(1).expect("output size exists")],
    ];
    let originals = outputs.clone();
    let mut jobs = outputs
        .iter_mut()
        .enumerate()
        .map(|(index, output)| {
            CpuReductionBatchJob::new(
                batch.descriptor(index).expect("batch descriptor exists"),
                output,
            )
        })
        .collect::<Vec<_>>();
    jobs.swap(0, 1);

    assert_eq!(
        executor.execute_batch(&batch, &frame, &mut jobs),
        Err(CpuReductionError::BatchDescriptorMismatch { index: 0 })
    );
    drop(jobs);
    assert_eq!(outputs, originals);
}

#[test]
fn cover_region_and_pending_transforms_prepare_without_approximation() {
    let executor = executor();
    let source = source(extent(16, 9));
    let cover = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(8)),
            Some(non_zero(8)),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Cover,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    );
    let batch = prepare_batch(&executor, &source, [cover]).expect("Cover prepares exactly");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch.output_byte_len(0), Some(8 * 8 * 4));

    let reflected = source_with(
        extent(16, 9),
        ScreenSourceReflection::Horizontal,
        ScreenCursorCapabilities::clean_only(),
        11,
    );
    let reflected_batch = prepare_batch(&executor, &reflected, std::iter::empty())
        .expect("reflection is represented by the sampling transform");
    assert!(reflected_batch.is_empty());
}

#[test]
fn separate_cursor_and_algorithm_revision_fail_during_preparation() {
    let executor = executor();
    let separate_cursor_source = source_with(
        extent(16, 9),
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_with_separate_cursor(),
        11,
    );
    let cursor = demand(
        &separate_cursor_source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            cursor: ScreenCursorPolicy::Include,
            ..ScreenProcessingProfileConfig::default()
        },
        60,
        executor.capabilities(),
    );
    assert_eq!(
        prepare_batch(&executor, &separate_cursor_source, [cursor])
            .expect_err("separate cursor pixels require composition"),
        CpuReductionError::CursorCompositionRequired
    );

    let source = source(extent(16, 9));
    let revision = non_zero(2);
    let revision_mismatch = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            algorithm_revision: revision,
            ..ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Rgba8)
        },
        60,
        ScreenColorTransformCapabilities::NONE,
    );
    assert_eq!(
        prepare_batch(&executor, &source, [revision_mismatch])
            .expect_err("foreign algorithm revisions are rejected"),
        CpuReductionError::AlgorithmRevisionMismatch {
            expected: non_zero(1),
            actual: revision,
        }
    );
}

#[test]
fn stale_frame_and_bad_output_preflight_leave_every_plane_untouched() {
    let executor = executor();
    let source = source(extent(8, 6));
    let demands = [demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    )];
    let batch = prepare_batch(&executor, &source, demands).expect("batch prepares");
    let stale = frame(
        &source,
        source.epoch().session_generation + 1,
        CaptureCursor::default(),
    );
    let mut output = vec![0xA5; batch.output_byte_len(0).expect("output size exists")];
    let original = output.clone();
    let mut jobs = [CpuReductionBatchJob::new(
        batch.descriptor(0).expect("batch descriptor exists"),
        &mut output,
    )];
    assert_eq!(
        executor.execute_batch(&batch, &stale, &mut jobs),
        Err(CpuReductionError::CaptureFrame(
            CaptureFrameError::StaleSession {
                expected: source.epoch().session_generation,
                actual: source.epoch().session_generation + 1,
            }
        ))
    );
    assert_eq!(output, original);

    let current = frame(
        &source,
        source.epoch().session_generation,
        CaptureCursor::default(),
    );
    let mut short = vec![0x5A; original.len() - 1];
    let short_original = short.clone();
    let mut jobs = [CpuReductionBatchJob::new(
        batch.descriptor(0).expect("batch descriptor exists"),
        &mut short,
    )];
    assert_eq!(
        executor.execute_batch(&batch, &current, &mut jobs),
        Err(CpuReductionError::BatchOutputLengthMismatch {
            index: 0,
            expected: original.len(),
            actual: original.len() - 1,
        })
    );
    assert_eq!(short, short_original);
}

#[test]
fn preparation_preserves_8k_without_allocating_an_output_plane() {
    let executor = executor();
    let source = source(extent(7_680, 4_320));
    let native = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
        120,
        executor.capabilities(),
    );

    let batch = prepare_batch(&executor, &source, [native]).expect("8K batch prepares");

    assert_eq!(batch.len(), 1);
    assert_eq!(batch.output_byte_len(0), Some(132_710_400));
}

#[test]
fn source_config_and_output_count_mismatches_are_fenced() {
    let executor = executor();
    let source = source(extent(8, 6));
    let native = demand(
        &source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    );
    let mut builder = ScreenPlanBuilder::new();
    let preparing = builder
        .prepare(
            [native],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan is admitted");
    let replacement_resource = source_with(
        extent(8, 6),
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_only(),
        12,
    );
    assert_eq!(
        executor
            .prepare_batch(&replacement_resource, preparing.candidate_plan())
            .expect_err("resource generations are part of source identity"),
        CpuReductionError::SourceConfigMismatch
    );

    let batch = executor
        .prepare_batch(&source, preparing.candidate_plan())
        .expect("matching source prepares");
    let frame = frame(
        &source,
        source.epoch().session_generation,
        CaptureCursor::default(),
    );
    assert_eq!(
        executor.execute_batch(&batch, &frame, &mut []),
        Err(CpuReductionError::BatchOutputCountMismatch {
            expected: 1,
            actual: 0,
        })
    );

    let empty =
        prepare_batch(&executor, &source, std::iter::empty()).expect("empty batch prepares");
    assert!(empty.is_empty());
    assert_eq!(empty.descriptor(0), None);
    assert_eq!(empty.output_byte_len(0), None);
}

#[test]
fn opaque_gpu_sources_and_frames_request_typed_platform_readback() {
    let executor = executor();
    let source_extent = extent(8, 6);
    let gpu_source = source_named_with_api(
        "synthetic:gpu-fallback",
        source_extent,
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_only(),
        11,
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
    );
    let gpu_demand = demand(
        &gpu_source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(5)),
            Some(non_zero(3)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    );
    let mut gpu_builder = ScreenPlanBuilder::new();
    let gpu_preparing = gpu_builder
        .prepare(
            [gpu_demand],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("GPU exact plan prepares");
    let error = executor
        .prepare_batch(&gpu_source, gpu_preparing.candidate_plan())
        .expect_err("opaque GPU source requires platform readback");
    let CpuReductionError::FallbackRequired(need) = error else {
        panic!("opaque GPU source returned {error:?}");
    };
    assert_eq!(need.api(), &PlatformGpuApi::Direct3d11);

    let cpu_source = source(source_extent);
    let cpu_demand = demand(
        &cpu_source,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(5)),
            Some(non_zero(3)),
            ScreenUpscalePolicy::Allow,
        ),
        ScreenAspectPolicy::Cover,
        ScreenProcessingProfileConfig::default(),
        60,
        executor.capabilities(),
    );
    let batch =
        prepare_batch(&executor, &cpu_source, [cpu_demand]).expect("CPU exact batch prepares");
    let descriptor = batch
        .descriptor(0)
        .expect("exact physical key exists")
        .clone();
    let gpu_surface = PlatformGpuSurface::new(
        PlatformGpuApi::Direct3d11,
        41,
        source_extent,
        CapturePixelFormat::Rgba8,
        Arc::new(()),
    )
    .expect("test GPU surface is valid");
    let captured_at = Instant::now();
    let gpu_frame = |session_generation| {
        CaptureFrame::new(
            CaptureFrameMetadata {
                source_id: cpu_source.epoch().source_id.clone(),
                topology_generation: cpu_source.epoch().topology_generation,
                session_generation,
                sequence: 17,
                captured_at,
                fresh_until: captured_at + Duration::from_secs(1),
                geometry: cpu_source.config().geometry(),
                colorimetry: cpu_source.config().colorimetry(),
                cursor: CaptureCursor::default(),
            },
            CaptureStorage::Gpu(gpu_surface.clone()),
            hypercolor_core::input::screen::CaptureDamage::default(),
        )
        .expect("test GPU frame is valid")
    };
    let mut output = vec![0xA5; batch.output_byte_len(0).expect("output size exists")];
    let mut jobs = [CpuReductionBatchJob::new(&descriptor, &mut output)];

    assert_eq!(
        executor.execute_batch(
            &batch,
            &gpu_frame(cpu_source.epoch().session_generation + 1),
            &mut jobs,
        ),
        Err(CpuReductionError::CaptureFrame(
            CaptureFrameError::StaleSession {
                expected: cpu_source.epoch().session_generation,
                actual: cpu_source.epoch().session_generation + 1,
            }
        ))
    );
    let error = executor
        .execute_batch(
            &batch,
            &gpu_frame(cpu_source.epoch().session_generation),
            &mut jobs,
        )
        .expect_err("opaque GPU frame requires platform readback");
    let CpuReductionError::FallbackRequired(need) = error else {
        panic!("opaque GPU frame returned {error:?}");
    };
    assert_eq!(need.api(), &PlatformGpuApi::Direct3d11);
    assert!(output.iter().all(|byte| *byte == 0xA5));
    assert_eq!(batch.descriptor(0), Some(&descriptor));
}

#[test]
fn source_partition_selects_only_the_contiguous_canonical_range() {
    let executor = executor();
    let source_a = source_named_with(
        "synthetic:a",
        extent(8, 6),
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_only(),
        3,
    );
    let source_m = source_named_with(
        "synthetic:m",
        extent(8, 6),
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_only(),
        2,
    );
    let source_z = source_named_with(
        "synthetic:z",
        extent(8, 6),
        ScreenSourceReflection::None,
        ScreenCursorCapabilities::clean_only(),
        1,
    );
    let native = |source: &ResolvedScreenSource| {
        demand(
            source,
            ScreenPublicationKind::Surface,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            ScreenProcessingProfileConfig::default(),
            60,
            executor.capabilities(),
        )
    };
    let middle_reduced = demand(
        &source_m,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            Some(non_zero(4)),
            Some(non_zero(3)),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Contain,
        ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Bilinear,
            ..ScreenProcessingProfileConfig::default()
        },
        30,
        executor.capabilities(),
    );
    let mut builder = ScreenPlanBuilder::new();
    let preparing = builder
        .prepare(
            [
                native(&source_z),
                middle_reduced,
                native(&source_a),
                native(&source_m),
            ],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("multi-source plan is admitted");
    let canonical_ids = preparing
        .candidate_plan()
        .physical_reductions()
        .iter()
        .map(|reduction| reduction.descriptor().source_epoch().source_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        canonical_ids,
        ["synthetic:a", "synthetic:m", "synthetic:m", "synthetic:z"]
    );

    let batch = executor
        .prepare_batch(&source_m, preparing.candidate_plan())
        .expect("middle source range prepares");

    assert_eq!(batch.len(), 2);
    assert!((0..batch.len()).all(|index| {
        batch.descriptor(index).is_some_and(|descriptor| {
            descriptor.source_epoch().source_id == source_m.epoch().source_id
        })
    }));
    assert_eq!(
        (0..batch.len())
            .map(|index| {
                batch
                    .descriptor(index)
                    .expect("middle descriptor exists")
                    .reduction_extent()
            })
            .collect::<Vec<_>>(),
        [extent(4, 3), extent(8, 6)]
    );
}
