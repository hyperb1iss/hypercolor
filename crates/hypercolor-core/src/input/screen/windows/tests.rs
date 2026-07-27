use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hypercolor_windows_capture::DisplayRotation;

use super::{
    CaptureTopologySignature, SharedSettings, capture_epoch, capture_geometry,
    capture_topology_generation,
};
use crate::input::screen::{
    CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage, CaptureFrame,
    CaptureFrameError, CaptureFrameMetadata, CapturePixelFormat, CaptureRotation, CaptureStorage,
    CaptureTransferFunction, CpuCaptureStorage, PhysicalOrigin, PixelExtent, RawCaptureSurface,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is valid")
}

#[test]
fn physical_geometry_changes_advance_the_adapter_topology_epoch() {
    let settings = SharedSettings {
        config: Mutex::new(CaptureConfig::default()),
        generation: AtomicU64::new(0),
        topology_generation: AtomicU64::new(1),
        session_generation: AtomicU64::new(7),
    };
    let first = CaptureTopologySignature {
        native_width: 3840,
        native_height: 2160,
        origin_x: 0,
        origin_y: 0,
        rotation: DisplayRotation::Identity,
    };
    let moved = CaptureTopologySignature {
        origin_x: -3840,
        ..first
    };
    let mut previous = None;

    assert_eq!(
        capture_topology_generation(&settings, &mut previous, first),
        1
    );
    assert_eq!(
        capture_topology_generation(&settings, &mut previous, first),
        1
    );
    assert_eq!(
        capture_topology_generation(&settings, &mut previous, moved),
        2
    );
    assert_eq!(settings.topology_generation.load(Ordering::Acquire), 2);
}

#[test]
fn adapter_preserves_native_and_stored_geometry_for_every_dxgi_rotation() {
    for (native_rotation, expected_rotation) in [
        (DisplayRotation::Identity, CaptureRotation::Identity),
        (DisplayRotation::Clockwise90, CaptureRotation::Clockwise90),
        (DisplayRotation::Clockwise180, CaptureRotation::Clockwise180),
        (DisplayRotation::Clockwise270, CaptureRotation::Clockwise270),
    ] {
        let geometry = capture_geometry(
            extent(3840, 2160),
            extent(1280, 720),
            PhysicalOrigin { x: -3840, y: 120 },
            native_rotation,
        )
        .expect("DXGI geometry is valid");

        assert_eq!(geometry.native_extent(), extent(3840, 2160));
        assert_eq!(geometry.storage_extent(), extent(1280, 720));
        assert_eq!(geometry.origin(), PhysicalOrigin { x: -3840, y: 120 });
        assert_eq!(geometry.rotation(), expected_rotation);
    }
}

#[test]
fn adapter_epoch_rejects_a_frame_from_another_monitor_or_generation() {
    let captured_at = Instant::now();
    let geometry = capture_geometry(
        extent(4, 2),
        extent(2, 1),
        PhysicalOrigin::default(),
        DisplayRotation::Identity,
    )
    .expect("test geometry is valid");
    let frame = CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id: capture_epoch(1, 3, 7)
                .expect("test source id is valid")
                .source_id,
            topology_generation: 3,
            session_generation: 7,
            sequence: 1,
            captured_at,
            fresh_until: captured_at + Duration::from_millis(50),
            geometry,
            color_space: CaptureColorSpace::Unknown,
            transfer_function: CaptureTransferFunction::Unknown,
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::<[u8]>::from([0_u8; 8]),
            CapturePixelFormat::Rgba8,
            8,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("test frame is valid");

    assert!(matches!(
        frame.validate_epoch(&capture_epoch(0, 3, 7).expect("expected epoch is valid")),
        Err(CaptureFrameError::SourceMismatch { .. })
    ));
    assert!(matches!(
        frame.validate_epoch(&capture_epoch(1, 4, 7).expect("expected epoch is valid")),
        Err(CaptureFrameError::StaleTopology { .. })
    ));
    assert!(matches!(
        frame.validate_epoch(&capture_epoch(1, 3, 8).expect("expected epoch is valid")),
        Err(CaptureFrameError::StaleSession { .. })
    ));
}
