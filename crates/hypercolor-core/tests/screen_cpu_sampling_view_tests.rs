//! Exact logical-to-storage CPU sampling contracts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureCursor, CaptureCursorContent, CaptureDamage, CaptureEpoch,
    CaptureFrame, CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CaptureStorage, CpuCaptureStorage, CpuSamplingError, CpuSamplingPoint,
    CpuSamplingView, KnownCaptureColorimetry, PhysicalOrigin, PixelExtent, PixelRect,
    PlatformGpuApi, PlatformGpuSurface, RawCaptureSurface, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenBackendResourceIdentity, ScreenCaptureBackend,
    ScreenCursorCapabilities, ScreenRational, ScreenResourceApi, ScreenSourceReflection,
    ScreenSourceSelector, SourceScale,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn source(
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    format: CapturePixelFormat,
    cursor_capabilities: ScreenCursorCapabilities,
    resource_api: ScreenResourceApi,
) -> ResolvedScreenSource {
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new("synthetic:sampling")
                .expect("test source identity is non-empty"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            logical_extent,
            reflection,
            format,
            CaptureColorimetry::from_known(KnownCaptureColorimetry::SRGB),
            cursor_capabilities,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                resource_api,
                7,
                11,
            ),
        ),
    )
}

fn geometry(
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
    scale: SourceScale,
) -> CaptureGeometry {
    CaptureGeometry::new(
        PhysicalOrigin::default(),
        native_extent,
        storage_extent,
        rotation,
        crop,
        scale,
    )
    .expect("test geometry is valid")
}

fn frame(
    source: &ResolvedScreenSource,
    storage: CaptureStorage,
    geometry: CaptureGeometry,
    colorimetry: CaptureColorimetry,
    cursor: CaptureCursor,
) -> CaptureFrame<RawCaptureSurface> {
    let captured_at = Instant::now();
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation: source.epoch().session_generation,
            sequence: 17,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry,
            colorimetry,
            cursor,
        },
        storage,
        CaptureDamage::default(),
    )
    .expect("test frame is valid")
}

fn labeled_rgba(extent: PixelExtent) -> Arc<[u8]> {
    (1..=extent.width() * extent.height())
        .flat_map(|label| [label as u8, 0, 0, u8::MAX])
        .collect::<Vec<_>>()
        .into()
}

fn canonical_frame(source: &ResolvedScreenSource) -> CaptureFrame<RawCaptureSurface> {
    let geometry = source.config().geometry();
    let storage_extent = geometry.storage_extent();
    frame(
        source,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(storage_extent),
            source.config().pixel_format(),
            i64::from(storage_extent.width()) * 4,
            0,
        )),
        geometry,
        source.config().colorimetry(),
        CaptureCursor::default(),
    )
}

fn half_pixel(x: u32, y: u32) -> CpuSamplingPoint {
    CpuSamplingPoint::new(
        ScreenRational::new(u64::from(x) * 2 + 1, 2).expect("test coordinate is valid"),
        ScreenRational::new(u64::from(y) * 2 + 1, 2).expect("test coordinate is valid"),
    )
}

fn sampled_labels(view: &CpuSamplingView<'_>) -> Vec<u8> {
    let logical = view.logical_extent();
    (0..logical.height())
        .flat_map(|y| {
            (0..logical.width()).map(move |x| {
                view.read_logical_nearest(half_pixel(x, y))
                    .expect("logical center maps to storage")[0]
            })
        })
        .collect()
}

fn reflected(labels: &[u8], extent: PixelExtent, reflection: ScreenSourceReflection) -> Vec<u8> {
    let mut output = labels.to_vec();
    if matches!(
        reflection,
        ScreenSourceReflection::Horizontal | ScreenSourceReflection::Both
    ) {
        for row in output
            .chunks_exact_mut(usize::try_from(extent.width()).expect("test width is addressable"))
        {
            row.reverse();
        }
    }
    if matches!(
        reflection,
        ScreenSourceReflection::Vertical | ScreenSourceReflection::Both
    ) {
        let row_width = usize::try_from(extent.width()).expect("test width is addressable");
        let mut rows = output
            .chunks_exact(row_width)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        rows.reverse();
        output = rows.into_iter().flatten().collect();
    }
    output
}

#[test]
fn every_rotation_and_reflection_reads_the_expected_texels() {
    let native = extent(3, 2);
    for (rotation, rotated, base_labels) in [
        (
            CaptureRotation::Identity,
            extent(3, 2),
            vec![1, 2, 3, 4, 5, 6],
        ),
        (
            CaptureRotation::Clockwise90,
            extent(2, 3),
            vec![4, 1, 5, 2, 6, 3],
        ),
        (
            CaptureRotation::Clockwise180,
            extent(3, 2),
            vec![6, 5, 4, 3, 2, 1],
        ),
        (
            CaptureRotation::Clockwise270,
            extent(2, 3),
            vec![3, 6, 2, 5, 1, 4],
        ),
        (
            CaptureRotation::Flipped,
            extent(3, 2),
            vec![3, 2, 1, 6, 5, 4],
        ),
        (
            CaptureRotation::Flipped90,
            extent(2, 3),
            vec![6, 3, 5, 2, 4, 1],
        ),
        (
            CaptureRotation::Flipped180,
            extent(3, 2),
            vec![4, 5, 6, 1, 2, 3],
        ),
        (
            CaptureRotation::Flipped270,
            extent(2, 3),
            vec![1, 4, 2, 5, 3, 6],
        ),
    ] {
        for reflection in [
            ScreenSourceReflection::None,
            ScreenSourceReflection::Horizontal,
            ScreenSourceReflection::Vertical,
            ScreenSourceReflection::Both,
        ] {
            let geometry = geometry(native, native, rotation, None, SourceScale::ONE);
            let source = source(
                geometry,
                rotated,
                reflection,
                CapturePixelFormat::Rgba8,
                ScreenCursorCapabilities::clean_only(),
                ScreenResourceApi::Cpu,
            );
            let frame = canonical_frame(&source);
            let view = CpuSamplingView::try_new(&frame, &source)
                .expect("supported transform constructs a view");

            assert_eq!(
                sampled_labels(&view),
                reflected(&base_labels, rotated, reflection),
                "rotation={rotation:?} reflection={reflection:?}"
            );
        }
    }
}

#[test]
fn odd_crop_precedes_rotation_and_reflection_without_a_plane_copy() {
    let native = extent(5, 4);
    let crop = PixelRect::new(1, 1, 3, 2).expect("test crop is valid");
    let geometry = geometry(
        native,
        native,
        CaptureRotation::Clockwise90,
        Some(crop),
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        extent(2, 3),
        ScreenSourceReflection::Horizontal,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let frame = canonical_frame(&source);
    let CaptureStorage::Cpu(storage) = frame.storage() else {
        panic!("test frame has CPU storage");
    };
    let owner = storage.owner_identity();
    let view = CpuSamplingView::try_new(&frame, &source).expect("crop view is valid");

    assert_eq!(sampled_labels(&view), vec![7, 12, 8, 13, 9, 14]);
    assert_eq!(storage.owner_identity(), owner);
}

#[test]
fn source_scale_downsample_and_upsample_are_exact() {
    for (physical, logical, scale, expected) in [
        (
            extent(4, 2),
            extent(2, 1),
            SourceScale::new(1, 2).expect("test scale is valid"),
            vec![6, 8],
        ),
        (
            extent(2, 1),
            extent(4, 2),
            SourceScale::new(2, 1).expect("test scale is valid"),
            vec![1, 1, 2, 2, 1, 1, 2, 2],
        ),
        (
            extent(2, 2),
            extent(3, 3),
            SourceScale::new(3, 2).expect("test scale is valid"),
            vec![1, 2, 2, 3, 4, 4, 3, 4, 4],
        ),
    ] {
        let geometry = geometry(physical, physical, CaptureRotation::Identity, None, scale);
        let source = source(
            geometry,
            logical,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            ScreenCursorCapabilities::clean_only(),
            ScreenResourceApi::Cpu,
        );
        let frame = canonical_frame(&source);
        let view = CpuSamplingView::try_new(&frame, &source).expect("scaled view is valid");

        assert_eq!(sampled_labels(&view), expected);
    }
}

#[test]
fn native_to_storage_scaling_keeps_exact_rational_edges() {
    let native = extent(7, 5);
    let storage = extent(4, 3);
    let geometry = geometry(
        native,
        storage,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        native,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let frame = canonical_frame(&source);
    let view = CpuSamplingView::try_new(&frame, &source).expect("scaled storage view is valid");

    let origin = view
        .map_logical_edge(CpuSamplingPoint::from_u32(0, 0))
        .expect("origin maps");
    let end = view
        .map_logical_edge(CpuSamplingPoint::from_u32(7, 5))
        .expect("end maps");
    assert_eq!(
        (origin.x().numerator(), origin.x().denominator().get()),
        (0, 1)
    );
    assert_eq!(
        (origin.y().numerator(), origin.y().denominator().get()),
        (0, 1)
    );
    assert_eq!((end.x().numerator(), end.x().denominator().get()), (4, 1));
    assert_eq!((end.y().numerator(), end.y().denominator().get()), (3, 1));
    assert_eq!(
        view.read_logical_nearest(half_pixel(6, 4))
            .expect("last center maps")[0],
        12
    );
}

#[test]
fn combined_crop_rotation_reflection_and_two_scales_remains_exact() {
    let native = extent(7, 5);
    let storage = extent(4, 3);
    let crop = PixelRect::new(1, 1, 4, 2).expect("test crop is valid");
    let geometry = geometry(
        native,
        storage,
        CaptureRotation::Clockwise90,
        Some(crop),
        SourceScale::new(3, 2).expect("test scale is valid"),
    );
    let source = source(
        geometry,
        extent(3, 6),
        ScreenSourceReflection::Both,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let frame = canonical_frame(&source);
    let view = CpuSamplingView::try_new(&frame, &source).expect("combined view is valid");

    let mapped = view
        .map_logical_edge(half_pixel(0, 0))
        .expect("combined point maps");
    assert_eq!(
        (mapped.x().numerator(), mapped.x().denominator().get()),
        (8, 3)
    );
    assert_eq!(
        (mapped.y().numerator(), mapped.y().denominator().get()),
        (4, 5)
    );
    assert!(mapped.x_reversed());
    assert!(!mapped.y_reversed());
    assert_eq!(
        view.read_logical_nearest(half_pixel(0, 0))
            .expect("combined nearest sample maps")[0],
        3
    );
}

#[test]
fn reflected_integer_boundary_uses_the_descending_texel() {
    let physical = extent(2, 1);
    let geometry = geometry(
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        physical,
        ScreenSourceReflection::Horizontal,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let frame = canonical_frame(&source);
    let view = CpuSamplingView::try_new(&frame, &source).expect("reflected view is valid");
    let boundary = CpuSamplingPoint::new(
        ScreenRational::new(1, 1).expect("test coordinate is valid"),
        ScreenRational::new(1, 2).expect("test coordinate is valid"),
    );

    assert_eq!(
        view.read_logical_nearest(boundary)
            .expect("boundary sample maps")[0],
        1
    );
}

#[test]
fn signed_padded_bgra_storage_reads_canonical_rgba() {
    let physical = extent(2, 2);
    let geometry = geometry(
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        physical,
        ScreenSourceReflection::None,
        CapturePixelFormat::Bgra8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let bytes: Arc<[u8]> = Arc::from([
        90, 80, 70, 60, 130, 120, 110, 100, 0, 0, 0, 0, 30, 20, 10, 40, 70, 60, 50, 80,
    ]);
    let frame = frame(
        &source,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            bytes,
            CapturePixelFormat::Bgra8,
            -12,
            12,
        )),
        geometry,
        source.config().colorimetry(),
        CaptureCursor::default(),
    );
    let view = CpuSamplingView::try_new(&frame, &source).expect("signed-stride view is valid");

    assert_eq!(
        view.read_logical_nearest(half_pixel(0, 0))
            .expect("top-left sample maps"),
        [10, 20, 30, 40]
    );
    assert_eq!(
        view.read_logical_nearest(half_pixel(1, 1))
            .expect("bottom-right sample maps"),
        [110, 120, 130, 100]
    );
}

#[test]
fn exact_endpoint_is_valid_and_one_half_pixel_beyond_is_rejected() {
    let physical = extent(3, 2);
    let geometry = geometry(
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        physical,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let frame = canonical_frame(&source);
    let view = CpuSamplingView::try_new(&frame, &source).expect("identity view is valid");

    assert!(
        view.map_logical_edge(CpuSamplingPoint::from_u32(3, 2))
            .is_ok()
    );
    let outside = CpuSamplingPoint::new(
        ScreenRational::new(7, 2).expect("test coordinate is valid"),
        ScreenRational::from_u32(2),
    );
    assert!(matches!(
        view.map_logical_edge(outside),
        Err(CpuSamplingError::LogicalPointOutOfBounds { .. })
    ));
}

#[test]
fn logical_extent_must_match_source_scale_after_rotation() {
    let native = extent(3, 2);
    let geometry = geometry(
        native,
        native,
        CaptureRotation::Clockwise90,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        extent(3, 2),
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let frame = canonical_frame(&source);

    assert!(matches!(
        CpuSamplingView::try_new(&frame, &source),
        Err(CpuSamplingError::LogicalExtentScaleMismatch { .. })
    ));
}

#[test]
fn source_and_frame_contract_mismatches_are_typed() {
    let physical = extent(2, 2);
    let geometry = geometry(
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        physical,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );

    let different_geometry = CaptureGeometry::new(
        PhysicalOrigin { x: 1, y: 0 },
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    let geometry_frame = frame(
        &source,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(physical),
            CapturePixelFormat::Rgba8,
            8,
            0,
        )),
        different_geometry,
        source.config().colorimetry(),
        CaptureCursor::default(),
    );
    assert!(matches!(
        CpuSamplingView::try_new(&geometry_frame, &source),
        Err(CpuSamplingError::SourceGeometryMismatch { .. })
    ));

    let color_frame = frame(
        &source,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(physical),
            CapturePixelFormat::Rgba8,
            8,
            0,
        )),
        geometry,
        CaptureColorimetry::unknown(),
        CaptureCursor::default(),
    );
    assert!(matches!(
        CpuSamplingView::try_new(&color_frame, &source),
        Err(CpuSamplingError::SourceColorimetryMismatch { .. })
    ));

    let format_frame = frame(
        &source,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(physical),
            CapturePixelFormat::Bgra8,
            8,
            0,
        )),
        geometry,
        source.config().colorimetry(),
        CaptureCursor::default(),
    );
    assert!(matches!(
        CpuSamplingView::try_new(&format_frame, &source),
        Err(CpuSamplingError::SourcePixelFormatMismatch { .. })
    ));
}

#[test]
fn non_cpu_source_and_gpu_frame_are_rejected_separately() {
    let physical = extent(1, 1);
    let geometry = geometry(
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let gpu_source = source(
        geometry,
        physical,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::PlatformGpu(PlatformGpuApi::Direct3d11),
    );
    let cpu_frame = canonical_frame(&gpu_source);
    assert!(matches!(
        CpuSamplingView::try_new(&cpu_frame, &gpu_source),
        Err(CpuSamplingError::UnsupportedSourceResource)
    ));

    let cpu_source = source(
        geometry,
        physical,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let gpu_frame = frame(
        &cpu_source,
        CaptureStorage::Gpu(
            PlatformGpuSurface::new(
                PlatformGpuApi::Direct3d11,
                9,
                physical,
                CapturePixelFormat::Rgba8,
                Arc::new(()),
            )
            .expect("test GPU surface is valid"),
        ),
        geometry,
        cpu_source.config().colorimetry(),
        CaptureCursor::default(),
    );
    assert!(matches!(
        CpuSamplingView::try_new(&gpu_frame, &cpu_source),
        Err(CpuSamplingError::GpuFrameStorage)
    ));
}

#[test]
fn stale_epoch_and_cursor_capability_contradictions_are_rejected() {
    let physical = extent(1, 1);
    let geometry = geometry(
        physical,
        physical,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    );
    let source = source(
        geometry,
        physical,
        ScreenSourceReflection::None,
        CapturePixelFormat::Rgba8,
        ScreenCursorCapabilities::clean_only(),
        ScreenResourceApi::Cpu,
    );
    let mut stale_frame = canonical_frame(&source);
    let stale_source = ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation: source.epoch().session_generation + 1,
        },
        source.config().clone(),
    );
    assert!(matches!(
        CpuSamplingView::try_new(&stale_frame, &stale_source),
        Err(CpuSamplingError::CaptureFrame(_))
    ));

    let captured_at = Instant::now();
    stale_frame = CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: source.epoch().source_id.clone(),
            topology_generation: source.epoch().topology_generation,
            session_generation: source.epoch().session_generation,
            sequence: 18,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry,
            colorimetry: source.config().colorimetry(),
            cursor: CaptureCursor {
                visible: true,
                content: CaptureCursorContent::Composed,
                ..CaptureCursor::default()
            },
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(physical),
            CapturePixelFormat::Rgba8,
            4,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid");
    assert!(matches!(
        CpuSamplingView::try_new(&stale_frame, &source),
        Err(CpuSamplingError::CursorContentUnsupported)
    ));
}

#[test]
fn one_pixel_and_ultrawide_views_have_no_resolution_special_cases() {
    for physical in [extent(1, 1), extent(16_384, 1)] {
        let geometry = geometry(
            physical,
            physical,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        );
        let source = source(
            geometry,
            physical,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            ScreenCursorCapabilities::clean_only(),
            ScreenResourceApi::Cpu,
        );
        let frame = canonical_frame(&source);
        let view = CpuSamplingView::try_new(&frame, &source).expect("extent is supported");

        assert_eq!(view.logical_extent(), physical);
        assert_eq!(view.storage_extent(), physical);
        assert_eq!(view.source_sequence(), 17);
    }
}
