//! Canonical raw-to-processed capture geometry tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureCursor, CaptureDamage, CaptureFrame, CaptureFrameMetadata,
    CaptureFrameProcessor, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStageKind, CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, PhysicalOrigin,
    PixelExtent, PixelRect, RawCaptureSurface, SourceScale,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn metadata(
    width: u32,
    height: u32,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
) -> CaptureFrameMetadata {
    let captured_at = Instant::now();
    CaptureFrameMetadata {
        source_id: CaptureSourceId::new(Arc::<str>::from("test:processor"))
            .expect("test source id is non-empty"),
        topology_generation: 1,
        session_generation: 1,
        sequence: 1,
        captured_at,
        fresh_until: captured_at + Duration::from_millis(50),
        geometry: CaptureGeometry::new(
            PhysicalOrigin::default(),
            extent(width, height),
            extent(width, height),
            rotation,
            crop,
            SourceScale::ONE,
        )
        .expect("test geometry is valid"),
        color_space: CaptureColorSpace::Srgb,
        transfer_function: CaptureTransferFunction::Srgb,
        cursor: CaptureCursor::default(),
    }
}

fn labeled_rgba(width: u32, height: u32) -> Arc<[u8]> {
    (1..=width * height)
        .flat_map(|label| [label as u8, 0, 0, 255])
        .collect::<Vec<_>>()
        .into()
}

fn raw_rgba(
    width: u32,
    height: u32,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
) -> CaptureFrame<RawCaptureSurface> {
    CaptureFrame::new(
        metadata(width, height, rotation, crop),
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(width, height),
            CapturePixelFormat::Rgba8,
            i64::from(width * 4),
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test raw frame is valid")
}

fn raw_rgba_with_geometry(
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    origin: PhysicalOrigin,
    rotation: CaptureRotation,
    crop: PixelRect,
    scale: SourceScale,
) -> CaptureFrame<RawCaptureSurface> {
    let mut metadata = metadata(
        native_extent.width(),
        native_extent.height(),
        rotation,
        Some(crop),
    );
    metadata.geometry = CaptureGeometry::new(
        origin,
        native_extent,
        storage_extent,
        rotation,
        Some(crop),
        scale,
    )
    .expect("test geometry is valid");
    CaptureFrame::new(
        metadata,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(storage_extent.width(), storage_extent.height()),
            CapturePixelFormat::Rgba8,
            i64::from(storage_extent.width() * 4),
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test raw frame is valid")
}

fn red_labels(
    frame: &CaptureFrame<hypercolor_core::input::screen::GeometryNormalizedCaptureSurface>,
) -> Vec<u8> {
    let CaptureStorage::Cpu(storage) = frame.storage() else {
        panic!("test processor should publish CPU storage");
    };
    storage
        .bytes()
        .chunks_exact(4)
        .map(|pixel| pixel[0])
        .collect()
}

#[test]
fn every_rotation_is_applied_exactly_once() {
    for (rotation, expected_extent, expected_labels) in [
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
    ] {
        let processed = CaptureFrameProcessor::default()
            .process(raw_rgba(3, 2, rotation, None))
            .expect("rotation should process");

        assert_eq!(processed.stage(), CaptureStageKind::GeometryNormalized);
        assert_eq!(
            processed.metadata().geometry.rotation(),
            CaptureRotation::Identity
        );
        assert_eq!(processed.metadata().geometry.crop(), None);
        assert_eq!(
            processed.metadata().geometry.native_extent(),
            expected_extent
        );
        assert_eq!(
            processed.metadata().geometry.storage_extent(),
            expected_extent
        );
        assert_eq!(red_labels(&processed), expected_labels);
    }
}

#[test]
fn native_crop_precedes_rotation() {
    let crop = PixelRect::new(1, 0, 2, 2).expect("test crop is valid");
    let processed = CaptureFrameProcessor::default()
        .process(raw_rgba(3, 2, CaptureRotation::Clockwise90, Some(crop)))
        .expect("crop and rotation should process");

    assert_eq!(processed.metadata().geometry.native_extent(), extent(2, 2));
    assert_eq!(processed.metadata().geometry.storage_extent(), extent(2, 2));
    assert_eq!(red_labels(&processed), vec![5, 2, 6, 3]);
}

#[test]
fn crop_origin_tracks_every_rotation_from_negative_desktop_coordinates() {
    let native = extent(8, 6);
    let crop = PixelRect::new(2, 1, 3, 2).expect("test crop is valid");
    let origin = PhysicalOrigin { x: -100, y: -200 };

    for (rotation, expected) in [
        (
            CaptureRotation::Identity,
            PhysicalOrigin { x: -98, y: -199 },
        ),
        (
            CaptureRotation::Clockwise90,
            PhysicalOrigin { x: -97, y: -198 },
        ),
        (
            CaptureRotation::Clockwise180,
            PhysicalOrigin { x: -97, y: -197 },
        ),
        (
            CaptureRotation::Clockwise270,
            PhysicalOrigin { x: -99, y: -197 },
        ),
    ] {
        let processed = CaptureFrameProcessor::default()
            .process(raw_rgba_with_geometry(
                native,
                native,
                origin,
                rotation,
                crop,
                SourceScale::ONE,
            ))
            .expect("crop origin should normalize");

        assert_eq!(processed.metadata().geometry.origin(), expected);
    }
}

#[test]
fn crop_origin_is_independent_of_backend_storage_extent() {
    let native = extent(8, 6);
    let crop = PixelRect::new(2, 2, 4, 2).expect("test crop is valid");
    let origin = PhysicalOrigin { x: -40, y: 20 };
    let rotation = CaptureRotation::Clockwise90;
    let full_resolution = CaptureFrameProcessor::default()
        .process(raw_rgba_with_geometry(
            native,
            native,
            origin,
            rotation,
            crop,
            SourceScale::ONE,
        ))
        .expect("full-resolution adapter frame should normalize");
    let reduced = CaptureFrameProcessor::default()
        .process(raw_rgba_with_geometry(
            native,
            extent(4, 3),
            origin,
            rotation,
            crop,
            SourceScale::ONE,
        ))
        .expect("reduced adapter frame should normalize");

    assert_eq!(
        full_resolution.metadata().geometry.origin(),
        reduced.metadata().geometry.origin()
    );
}

#[test]
fn crop_origin_applies_wayland_source_scale_with_checked_arithmetic() {
    let crop = PixelRect::new(4, 2, 2, 2).expect("test crop is valid");
    let processed = CaptureFrameProcessor::default()
        .process(raw_rgba_with_geometry(
            extent(8, 6),
            extent(4, 3),
            PhysicalOrigin { x: -10, y: -20 },
            CaptureRotation::Identity,
            crop,
            SourceScale::new(1, 2).expect("test source scale is valid"),
        ))
        .expect("scaled crop origin should normalize");

    assert_eq!(
        processed.metadata().geometry.origin(),
        PhysicalOrigin { x: -8, y: -19 }
    );
}

#[test]
fn signed_stride_bgra_becomes_canonical_rgba() {
    let bottom_up_bgra: Arc<[u8]> =
        vec![0, 0, 3, 255, 0, 0, 4, 255, 0, 0, 1, 255, 0, 0, 2, 255].into();
    let frame = CaptureFrame::new(
        metadata(2, 2, CaptureRotation::Identity, None),
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            bottom_up_bgra,
            CapturePixelFormat::Bgra8,
            -8,
            8,
        )),
        CaptureDamage::default(),
    )
    .expect("negative-stride BGRA frame is valid");
    let processed = CaptureFrameProcessor::default()
        .process(frame)
        .expect("BGRA frame should process");

    assert_eq!(red_labels(&processed), vec![1, 2, 3, 4]);
    let CaptureStorage::Cpu(storage) = processed.storage() else {
        panic!("processed frame should use CPU storage");
    };
    assert_eq!(storage.format(), CapturePixelFormat::Rgba8);
    assert_eq!(storage.row_stride(), 8);
    assert_eq!(storage.row0_offset(), 0);
}

#[test]
fn canonical_rgba_identity_transition_is_zero_copy() {
    let bytes = labeled_rgba(3, 2);
    let original = bytes.as_ptr();
    let frame = CaptureFrame::new(
        metadata(3, 2, CaptureRotation::Identity, None),
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            bytes,
            CapturePixelFormat::Rgba8,
            12,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("canonical raw frame is valid");
    let processor = CaptureFrameProcessor::default();
    let processed = processor.process(frame).expect("identity should process");
    let CaptureStorage::Cpu(storage) = processed.storage() else {
        panic!("processed frame should use CPU storage");
    };

    assert_eq!(storage.bytes().as_ptr(), original);
    assert_eq!(processor.allocation_count(), 0);
}

#[test]
fn transformed_planes_reuse_the_processor_pool() {
    let processor = CaptureFrameProcessor::default();
    let first = processor
        .process(raw_rgba(3, 2, CaptureRotation::Clockwise90, None))
        .expect("first frame should process");
    assert_eq!(processor.allocation_count(), 1);
    drop(first);

    let second = processor
        .process(raw_rgba(3, 2, CaptureRotation::Clockwise90, None))
        .expect("second frame should process");
    assert_eq!(processor.allocation_count(), 1);
    drop(second);
}

#[test]
fn cursor_geometry_tracks_crop_and_rotation() {
    let crop = PixelRect::new(1, 0, 3, 3).expect("test crop is valid");
    let mut metadata = metadata(4, 3, CaptureRotation::Clockwise90, Some(crop));
    metadata.cursor = CaptureCursor {
        visible: true,
        position: Some(PhysicalOrigin { x: 2, y: 1 }),
        hotspot: Some(PhysicalOrigin { x: 1, y: 0 }),
        shape_extent: Some(extent(2, 1)),
        shape_generation: Some(7),
        composed: true,
    };
    let frame = CaptureFrame::new(
        metadata,
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            labeled_rgba(4, 3),
            CapturePixelFormat::Rgba8,
            16,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("cursor test frame is valid");
    let processed = CaptureFrameProcessor::default()
        .process(frame)
        .expect("cursor frame should process");

    assert!(processed.metadata().cursor.visible);
    assert_eq!(
        processed.metadata().cursor.position,
        Some(PhysicalOrigin { x: 1, y: 1 })
    );
    assert_eq!(
        processed.metadata().cursor.hotspot,
        Some(PhysicalOrigin { x: 0, y: 1 })
    );
    assert_eq!(processed.metadata().cursor.shape_extent, Some(extent(1, 2)));
}
