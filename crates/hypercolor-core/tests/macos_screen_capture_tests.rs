//! ScreenCaptureKit core worker fixture contracts.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureConfig, MacosScreenCaptureFixture, PixelExtent, ScreenAdmissionCapacity,
    ScreenByteAdmissionCoordinator, ScreenCaptureDemand,
};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_macos_capture::{
    MacosAttachment, MacosCaptureColorimetry, MacosCaptureFrame, MacosCapturePixelFormat,
    MacosCaptureSurface, MacosColorPrimaries, MacosColorRange, MacosFrameDecoder, MacosFrameEvent,
    MacosPixelExtent, MacosPointRect, MacosProtectedSourceState, MacosRawCapturePlane,
    MacosRawCaptureSample, MacosRawCompleteFrame, MacosRawFrameAttachments, MacosTransferFunction,
};

const BGRA8: u32 = 0x4247_5241;

fn fixture_frame(epoch: u64, pixel: [u8; 4]) -> MacosCaptureFrame {
    let extent = MacosPixelExtent::new(4, 2).expect("fixture extent is valid");
    let stride = 16;
    let bytes = Arc::<[u8]>::from(pixel.repeat(8));
    let surface = MacosCaptureSurface::new_cpu_fixture(7, 32, epoch, vec![bytes])
        .expect("fixture surface is valid");
    let sample = MacosRawCaptureSample {
        frame: Some(MacosRawCompleteFrame {
            storage_extent: extent,
            planes: vec![MacosRawCapturePlane {
                index: 0,
                extent,
                bytes_per_row: stride,
                length_bytes: 32,
            }],
            pixel_format_fourcc: BGRA8,
            color: MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            cursor_composed: true,
            surface,
        }),
        attachments: MacosRawFrameAttachments {
            status: MacosAttachment::Value(0),
            display_time: MacosAttachment::Value(epoch * 1_000),
            display_scale_factor: MacosAttachment::Value(1.0),
            content_scale: MacosAttachment::Value(1.0),
            content_rect: MacosAttachment::Value(
                MacosPointRect::new(0.0, 0.0, 4.0, 2.0).expect("content rect is valid"),
            ),
            dirty_rects: MacosAttachment::Missing,
            screen_rect: MacosAttachment::Missing,
            bounding_rect: MacosAttachment::Missing,
        },
    };
    let mut decoder = MacosFrameDecoder::new(epoch);
    let MacosFrameEvent::Frame(frame) = decoder.decode(sample).expect("fixture frame decodes")
    else {
        panic!("complete sample must decode as a frame");
    };
    assert_eq!(frame.pixel_format, MacosCapturePixelFormat::Bgra8);
    *frame
}

fn fixture_source(
    config: CaptureConfig,
) -> (
    hypercolor_core::input::screen::MacosScreenCaptureInput,
    MacosScreenCaptureFixture,
) {
    let admission =
        ScreenByteAdmissionCoordinator::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX));
    MacosScreenCaptureFixture::source(config, admission)
}

fn wait_for_screen(source: &mut impl InputSource) -> hypercolor_core::input::ScreenData {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match source.sample().expect("fixture sample succeeds") {
            InputData::Screen(data) => return data,
            InputData::None if Instant::now() < deadline => thread::yield_now(),
            InputData::None => panic!("fixture worker did not publish before the deadline"),
            _ => panic!("macOS fixture published the wrong input kind"),
        }
    }
}

#[test]
fn fixture_capture_activates_only_for_live_demand() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let (mut source, fixture) = fixture_source(config);

    assert_eq!(source.name(), "macos_screen_capture");
    assert_eq!(
        source.protected_state(),
        MacosProtectedSourceState::ReadyIdle
    );
    assert!(!fixture.is_active());
    source.start().expect("fixture source starts idle");
    assert!(matches!(source.sample(), Ok(InputData::None)));

    source
        .set_screen_capture_demand(ScreenCaptureDemand::try_active(4, 2).expect("valid demand"))
        .expect("fixture demand activates");
    assert!(fixture.is_active());
    fixture.publish(fixture_frame(1, [0, 0, 255, 255]));
    let data = wait_for_screen(&mut source);
    assert_eq!(data.grid_width, 2);
    assert_eq!(data.grid_height, 1);
    assert_eq!(data.source_width, 4);
    assert_eq!(data.source_height, 2);
    assert_eq!(data.zone_colors.len(), 2);

    source
        .set_screen_capture_demand(ScreenCaptureDemand::Inactive)
        .expect("fixture demand deactivates");
    assert!(!fixture.is_active());
    assert!(matches!(source.sample(), Ok(InputData::None)));
}

#[test]
fn reconfiguration_fences_the_previous_worker_generation() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let (mut source, fixture) = fixture_source(config.clone());
    source.start().expect("fixture source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active(
            PixelExtent::new(4, 2).expect("fixture demand is valid"),
        ))
        .expect("fixture demand activates");
    fixture.publish(fixture_frame(1, [255, 0, 0, 255]));
    assert_eq!(wait_for_screen(&mut source).zone_colors.len(), 2);

    source
        .reconfigure_screen_capture(&CaptureConfig {
            grid_cols: 1,
            grid_rows: 1,
            ..config
        })
        .expect("fixture worker reconfigures");
    assert!(matches!(source.sample(), Ok(InputData::None)));

    fixture.publish(fixture_frame(2, [0, 255, 0, 255]));
    let data = wait_for_screen(&mut source);
    assert_eq!(data.grid_width, 1);
    assert_eq!(data.grid_height, 1);
    assert_eq!(data.zone_colors.len(), 1);
}
