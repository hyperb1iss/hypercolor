//! Windows capture adapter-boundary fixture contracts.

#![cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, PhysicalOrigin, PixelExtent,
    RawCaptureSurface, ScreenCaptureDemand, SourceScale, WindowsScreenCaptureInput,
};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_windows_capture::CaptureError;

fn fixture_epoch() -> CaptureEpoch {
    CaptureEpoch {
        source_id: CaptureSourceId::new("windows:fixture-display")
            .expect("fixture source id is valid"),
        topology_generation: 1,
        session_generation: 1,
    }
}

fn fixture_frame(epoch: &CaptureEpoch) -> CaptureFrame<RawCaptureSurface> {
    let extent = PixelExtent::new(4, 2).expect("fixture extent is nonempty");
    let captured_at = Instant::now();
    let pixels: Arc<[u8]> = Arc::from([
        255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255, 255, 0, 0,
        255, 0, 0, 255, 255, 0, 0, 255, 255,
    ]);
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: epoch.source_id.clone(),
            topology_generation: epoch.topology_generation,
            session_generation: epoch.session_generation,
            sequence: 1,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry: CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent,
                extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("fixture geometry is valid"),
            color_space: CaptureColorSpace::Unknown,
            transfer_function: CaptureTransferFunction::Unknown,
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            pixels,
            CapturePixelFormat::Rgba8,
            16,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("fixture frame is valid")
}

#[test]
fn deterministic_fixture_reuses_windows_analysis_and_publication() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let epoch = fixture_epoch();
    let (mut source, fixture) =
        WindowsScreenCaptureInput::new_deterministic_fixture(config, epoch.clone())
            .expect("deterministic Windows source is valid");

    source.start().expect("deterministic source starts idle");
    assert!(!fixture.is_active());
    assert!(fixture.publish(fixture_frame(&epoch)).is_err());
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(
            PixelExtent::new(4, 2).expect("fixture extent is nonempty"),
        ))
        .expect("deterministic capture activates without hardware");
    assert!(fixture.is_active());
    assert!(
        fixture
            .publish(fixture_frame(&epoch))
            .expect("adapter frame is accepted")
    );

    let InputData::Screen(screen) = source
        .sample()
        .expect("published screen sample is readable")
    else {
        panic!("expected a published screen sample");
    };
    assert_eq!((screen.source_width, screen.source_height), (4, 2));
    assert_eq!((screen.grid_width, screen.grid_height), (2, 1));
    assert_eq!(screen.zone_colors[0].colors, [[255, 0, 0]]);
    assert_eq!(screen.zone_colors[1].colors, [[0, 0, 255]]);

    source.stop();
    assert!(!fixture.is_active());
    assert!(fixture.publish(fixture_frame(&epoch)).is_err());
}

#[test]
fn active_extent_adoption_rolls_back_without_losing_last_good_frame() {
    let epoch = fixture_epoch();
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        epoch.clone(),
    )
    .expect("deterministic Windows source is valid");
    let admitted_extent = PixelExtent::new(4, 2).expect("fixture extent is nonempty");

    source.start().expect("deterministic source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(admitted_extent))
        .expect("initial deterministic demand is admitted");
    assert!(
        fixture
            .publish(fixture_frame(&epoch))
            .expect("adapter frame is accepted")
    );

    let oversized_extent =
        PixelExtent::new(u32::MAX, u32::MAX).expect("oversized extent remains nonempty");
    assert!(
        source
            .set_screen_capture_demand(ScreenCaptureDemand::active(oversized_extent))
            .is_err()
    );
    assert_eq!(
        source.screen_capture_demand(),
        ScreenCaptureDemand::active(admitted_extent)
    );
    assert_eq!(fixture.requested_extent(), admitted_extent);
    assert!(matches!(
        source.sample().expect("last good sample remains readable"),
        InputData::Screen(_)
    ));
}

#[test]
fn resource_frame_failures_retain_last_good_publication() {
    let epoch = fixture_epoch();
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        epoch.clone(),
    )
    .expect("deterministic Windows source is valid");
    let extent = PixelExtent::new(4, 2).expect("fixture extent is nonempty");

    source.start().expect("deterministic source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(extent))
        .expect("deterministic demand is admitted");
    assert!(
        fixture
            .publish(fixture_frame(&epoch))
            .expect("adapter frame is accepted")
    );

    fixture.inject_frame_failure(&CaptureError::ResourceExhausted {
        operation: "inject frame pressure",
        requested_bytes: usize::MAX,
    });

    assert!(matches!(
        source.sample().expect("last good sample remains readable"),
        InputData::Screen(_)
    ));
}

#[test]
fn rebuild_resource_failures_fence_the_invalidated_session() {
    let epoch = fixture_epoch();
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        epoch.clone(),
    )
    .expect("deterministic Windows source is valid");
    let extent = PixelExtent::new(4, 2).expect("fixture extent is nonempty");

    source.start().expect("deterministic source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(extent))
        .expect("deterministic demand is admitted");
    assert!(
        fixture
            .publish(fixture_frame(&epoch))
            .expect("adapter frame is accepted")
    );

    fixture.inject_frame_failure(&CaptureError::SessionResourceExhausted {
        operation: "inject rebuild pressure",
        requested_bytes: usize::MAX,
    });

    assert!(matches!(
        source.sample().expect("invalidated session samples idle"),
        InputData::None
    ));
}
