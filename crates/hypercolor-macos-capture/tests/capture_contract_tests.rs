use std::sync::Arc;

use hypercolor_macos_capture::{
    MACOS_STREAM_QUEUE_DEPTH, MacosAttachment, MacosCaptureCadence,
    MacosCaptureCallbackDiagnostics, MacosCaptureColorimetry, MacosCaptureError,
    MacosCapturePixelFormat, MacosCaptureSurface, MacosChromaLocation, MacosColorPrimaries,
    MacosColorRange, MacosDisplayClock, MacosDisplayClockError, MacosFrameDecoder,
    MacosFrameDropReason, MacosFrameEvent, MacosFrameMailbox, MacosFrameStatus, MacosGeometryError,
    MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosRawCapturePlane, MacosRawCaptureSample,
    MacosRawCompleteFrame, MacosRawFrameAttachments, MacosScale, MacosStreamRequest,
    MacosTransferFunction, MacosYuvMatrix,
};
use std::time::{Duration, Instant};

const BGRA8: u32 = 0x4247_5241;
const ARGB2101010: u32 = 0x5231_306b;
const RGBA16_FLOAT: u32 = 0x5247_6841;
const YUV420_VIDEO_RANGE: u32 = 0x3432_3076;
const YUV420_FULL_RANGE: u32 = 0x3432_3066;
const YUV44410_VIDEO_RANGE: u32 = 0x7834_3434;
const YUV44410_FULL_RANGE: u32 = 0x7866_3434;

#[test]
fn display_clock_maps_mach_ticks_around_its_monotonic_anchor() {
    let anchor = Instant::now();
    let clock = MacosDisplayClock::new(100, anchor, 125, 3).expect("valid timebase");

    assert_eq!(clock.timestamp(100), Ok(anchor));
    assert_eq!(clock.timestamp(124), Ok(anchor + Duration::from_micros(1)));
    assert_eq!(clock.timestamp(76), Ok(anchor - Duration::from_micros(1)));
}

#[test]
fn display_clock_rejects_invalid_or_unrepresentable_timebases() {
    let anchor = Instant::now();
    assert_eq!(
        MacosDisplayClock::new(0, anchor, 0, 1).expect_err("zero numerator must fail"),
        MacosDisplayClockError::InvalidTimebase {
            numerator: 0,
            denominator: 1,
        }
    );
    assert_eq!(
        MacosDisplayClock::new(0, anchor, 1, 0).expect_err("zero denominator must fail"),
        MacosDisplayClockError::InvalidTimebase {
            numerator: 1,
            denominator: 0,
        }
    );
    let clock = MacosDisplayClock::new(0, anchor, u32::MAX, 1).expect("valid timebase");
    assert_eq!(
        clock.timestamp(u64::MAX),
        Err(MacosDisplayClockError::DurationOutOfRange)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn system_display_clock_reads_the_native_timebase() {
    MacosDisplayClock::system().expect("macOS exposes its monotonic timebase");
}

#[test]
fn queue_depth_is_the_full_framework_limit() {
    assert_eq!(MACOS_STREAM_QUEUE_DEPTH, 8);
}

#[test]
fn stream_requests_preserve_native_refresh_and_reject_invalid_rates() {
    assert_eq!(
        MacosStreamRequest::new(MacosCaptureCadence::NativeRefresh, false)
            .expect("native refresh should be supported")
            .cadence,
        MacosCaptureCadence::NativeRefresh
    );
    assert_eq!(
        MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(0), true),
        Err(MacosCaptureError::InvalidCadence(0))
    );
    assert_eq!(
        MacosStreamRequest::default().cadence,
        MacosCaptureCadence::FramesPerSecond(60)
    );
}

#[test]
fn all_native_frame_statuses_decode_exactly() {
    let expected = [
        MacosFrameStatus::Complete,
        MacosFrameStatus::Idle,
        MacosFrameStatus::Blank,
        MacosFrameStatus::Suspended,
        MacosFrameStatus::Started,
        MacosFrameStatus::Stopped,
    ];
    for (raw, expected) in (0_i64..=5).zip(expected) {
        assert_eq!(MacosFrameStatus::try_from(raw), Ok(expected));
    }
    assert_eq!(
        MacosFrameStatus::try_from(6),
        Err(MacosCaptureError::UnknownFrameStatus(6))
    );
}

#[test]
fn lifecycle_frames_do_not_consume_complete_sequence_numbers() {
    let mut decoder = MacosFrameDecoder::new(41);
    for status in 1..=5 {
        let mut sample = sample_with_status(status);
        sample.frame = None;
        let event = decoder
            .decode(sample)
            .expect("lifecycle status should decode");
        assert!(matches!(event, MacosFrameEvent::Lifecycle(_)));
        assert_eq!(decoder.next_sequence(), 0);
    }
    let frame = decode_frame(&mut decoder, sample_with_status(0));
    assert_eq!(frame.epoch, 41);
    assert_eq!(frame.sequence, 0);
    assert_eq!(decoder.next_sequence(), 1);
}

#[test]
fn complete_frames_require_well_formed_mandatory_attachments() {
    let mut missing_frame = complete_sample();
    missing_frame.frame = None;
    assert_eq!(
        decode_error(missing_frame),
        MacosCaptureError::MissingFramePayload
    );

    let mut missing = complete_sample();
    missing.attachments.display_time = MacosAttachment::Missing;
    assert_eq!(
        decode_error(missing),
        MacosCaptureError::MissingAttachment("display_time")
    );

    let mut malformed = complete_sample();
    malformed.attachments.content_rect = MacosAttachment::Malformed;
    assert_eq!(
        decode_error(malformed),
        MacosCaptureError::MalformedAttachment("content_rect")
    );
}

#[test]
fn optional_attachments_have_explicit_absence_semantics() {
    let frame = decode_frame(&mut MacosFrameDecoder::new(1), complete_sample());
    assert_eq!(frame.geometry.screen_rect_points, None);
    assert_eq!(frame.geometry.bounding_rect_points, None);
    assert_eq!(frame.damage.as_ref(), &[pixel_rect(0, 0, 8, 6)]);

    let mut malformed = complete_sample();
    malformed.attachments.screen_rect = MacosAttachment::Malformed;
    assert_eq!(
        decode_error(malformed),
        MacosCaptureError::MalformedAttachment("screen_rect")
    );
}

#[test]
fn fractional_retina_rects_round_outward_without_content_double_scale() {
    let mut sample = complete_sample();
    sample.attachments.display_scale_factor = MacosAttachment::Value(2.0);
    sample.attachments.content_scale = MacosAttachment::Value(0.75);
    sample.attachments.content_rect = MacosAttachment::Value(point_rect(0.25, 0.25, 3.25, 2.25));
    sample.attachments.bounding_rect = MacosAttachment::Value(point_rect(0.75, 0.25, 2.5, 2.25));
    sample.attachments.screen_rect =
        MacosAttachment::Value(point_rect(-1200.25, -40.5, 3.25, 2.25));

    let frame = decode_frame(&mut MacosFrameDecoder::new(1), sample);
    assert_eq!(frame.geometry.content_rect_pixels, pixel_rect(0, 0, 7, 5));
    assert_eq!(
        frame.geometry.bounding_rect_pixels,
        Some(pixel_rect(1, 0, 6, 5))
    );
    assert_eq!(frame.geometry.content_scale.get(), 0.75);
    assert_eq!(
        frame.geometry.screen_rect_points,
        Some(point_rect(-1200.25, -40.5, 3.25, 2.25))
    );
}

#[test]
fn point_geometry_outside_storage_is_rejected_after_conversion() {
    let mut sample = complete_sample();
    sample.attachments.content_rect = MacosAttachment::Value(point_rect(0.0, 0.0, 8.1, 6.0));
    assert_eq!(
        decode_error(sample),
        MacosCaptureError::GeometryOutsideStorage("content_rect")
    );
}

#[test]
fn dirty_rects_remain_pixel_native_and_clip_to_storage() {
    let mut sample = complete_sample();
    sample.attachments.display_scale_factor = MacosAttachment::Value(2.0);
    sample.attachments.content_rect = MacosAttachment::Value(point_rect(0.0, 0.0, 4.0, 3.0));
    sample.attachments.dirty_rects = MacosAttachment::Value(vec![pixel_rect(-1, 1, 4, 3)]);
    let frame = decode_frame(&mut MacosFrameDecoder::new(1), sample);
    assert_eq!(frame.damage.as_ref(), &[pixel_rect(0, 1, 3, 3)]);
}

#[test]
fn invalid_extent_scale_and_rect_arithmetic_fail_closed() {
    assert_eq!(
        MacosPixelExtent::new(0, 1),
        Err(MacosGeometryError::EmptyExtent)
    );
    assert!(matches!(
        MacosScale::new(f64::NAN),
        Err(MacosGeometryError::InvalidScale(value)) if value.is_nan()
    ));
    assert_eq!(
        MacosScale::display(4.1),
        Err(MacosGeometryError::InvalidDisplayScale(4.1))
    );
    assert_eq!(
        MacosPixelRect::new(i64::MAX, 0, 1, 1),
        Err(MacosGeometryError::RectOverflow)
    );
}

#[test]
fn every_required_pixel_format_validates_its_exact_plane_layout() {
    let cases = [
        (BGRA8, rgb_color(), packed_planes(4)),
        (ARGB2101010, rgb_color(), packed_planes(4)),
        (RGBA16_FLOAT, rgb_color(), packed_planes(8)),
        (
            YUV420_VIDEO_RANGE,
            yuv_color(MacosColorRange::Video),
            yuv420_planes(),
        ),
        (
            YUV420_FULL_RANGE,
            yuv_color(MacosColorRange::Full),
            yuv420_planes(),
        ),
        (
            YUV44410_VIDEO_RANGE,
            yuv_color(MacosColorRange::Video),
            yuv44410_planes(),
        ),
        (
            YUV44410_FULL_RANGE,
            yuv_color(MacosColorRange::Full),
            yuv44410_planes(),
        ),
    ];
    for (fourcc, color, planes) in cases {
        let mut sample = complete_sample();
        let frame = complete_frame_mut(&mut sample);
        frame.pixel_format_fourcc = fourcc;
        frame.color = color;
        frame.planes = planes;
        frame.surface = surface_for(&frame.planes);
        let frame = decode_frame(&mut MacosFrameDecoder::new(1), sample);
        assert_eq!(
            frame.pixel_format,
            MacosCapturePixelFormat::from_fourcc(fourcc).expect("format should decode")
        );
    }
}

#[test]
fn unknown_pixel_formats_never_fall_back_to_bgra() {
    let mut sample = complete_sample();
    complete_frame_mut(&mut sample).pixel_format_fourcc = u32::from_be_bytes(*b"NOPE");
    assert_eq!(
        decode_error(sample),
        MacosCaptureError::UnsupportedPixelFormat(u32::from_be_bytes(*b"NOPE"))
    );
}

#[test]
fn yuv_color_metadata_is_mandatory_and_range_checked() {
    let mut missing = complete_sample();
    let frame = complete_frame_mut(&mut missing);
    frame.pixel_format_fourcc = YUV420_VIDEO_RANGE;
    frame.planes = yuv420_planes();
    frame.surface = surface_for(&frame.planes);
    assert_eq!(
        decode_error(missing),
        MacosCaptureError::MissingYuvColorMetadata
    );

    let mut wrong_range = complete_sample();
    let frame = complete_frame_mut(&mut wrong_range);
    frame.pixel_format_fourcc = YUV420_VIDEO_RANGE;
    frame.color = yuv_color(MacosColorRange::Full);
    frame.planes = yuv420_planes();
    frame.surface = surface_for(&frame.planes);
    assert_eq!(
        decode_error(wrong_range),
        MacosCaptureError::ColorMetadataMismatch
    );

    let mut wrong_444_range = complete_sample();
    let frame = complete_frame_mut(&mut wrong_444_range);
    frame.pixel_format_fourcc = YUV44410_VIDEO_RANGE;
    frame.color = yuv_color(MacosColorRange::Full);
    frame.planes = yuv44410_planes();
    frame.surface = surface_for(&frame.planes);
    assert_eq!(
        decode_error(wrong_444_range),
        MacosCaptureError::ColorMetadataMismatch
    );
}

#[test]
fn plane_index_extent_stride_length_and_count_are_checked() {
    let mut index = complete_sample();
    complete_frame_mut(&mut index).planes[0].index = 1;
    assert!(matches!(
        MacosFrameDecoder::new(1).decode(index),
        Err(MacosCaptureError::InvalidPlaneIndex { .. })
    ));

    let mut extent = complete_sample();
    complete_frame_mut(&mut extent).planes[0].extent = pixel_extent(7, 6);
    assert!(matches!(
        MacosFrameDecoder::new(1).decode(extent),
        Err(MacosCaptureError::InvalidPlaneExtent { .. })
    ));

    let mut stride = complete_sample();
    complete_frame_mut(&mut stride).planes[0].bytes_per_row = 31;
    assert!(matches!(
        MacosFrameDecoder::new(1).decode(stride),
        Err(MacosCaptureError::StrideTooSmall { .. })
    ));

    let mut length = complete_sample();
    complete_frame_mut(&mut length).planes[0].length_bytes = 191;
    assert!(matches!(
        MacosFrameDecoder::new(1).decode(length),
        Err(MacosCaptureError::PlaneLengthTooSmall { .. })
    ));

    let mut count = complete_sample();
    complete_frame_mut(&mut count).planes.clear();
    assert_eq!(
        decode_error(count),
        MacosCaptureError::PlaneCount {
            expected: 1,
            actual: 0
        }
    );
}

#[test]
fn summed_plane_lengths_must_fit_the_iosurface_allocation() {
    let mut sample = complete_sample();
    complete_frame_mut(&mut sample).surface =
        MacosCaptureSurface::new_fixture(7, 191, 99).expect("fixture surface should be valid");
    assert_eq!(
        decode_error(sample),
        MacosCaptureError::AllocationTooSmall {
            required: 192,
            actual: 191
        }
    );
}

#[test]
fn decoded_frames_keep_the_pixel_buffer_owner_alive() {
    let frame = decode_frame(&mut MacosFrameDecoder::new(1), complete_sample());
    let surface = frame.surface.clone();
    assert_eq!(frame.surface.retained_owner_count(), 2);
    assert_eq!(surface.fixture_id(), Some(99));
    drop(frame);
    assert_eq!(surface.retained_owner_count(), 1);
}

#[test]
fn bgra_cpu_copy_preserves_rows_and_ignores_padding() {
    let mut sample = complete_sample();
    let source = (0_u8..192).collect::<Vec<_>>();
    complete_frame_mut(&mut sample).surface =
        MacosCaptureSurface::new_cpu_fixture(7, 192, 99, vec![Arc::<[u8]>::from(source.clone())])
            .expect("CPU fixture surface should be valid");
    let frame = decode_frame(&mut MacosFrameDecoder::new(1), sample);
    let mut destination = vec![0xcc; 36 * 6];

    frame
        .copy_bgra8_to(&mut destination, 36)
        .expect("BGRA rows should copy");

    for row in 0..6 {
        assert_eq!(
            &destination[row * 36..row * 36 + 32],
            &source[row * 32..row * 32 + 32]
        );
        assert_eq!(&destination[row * 36 + 32..(row + 1) * 36], &[0xcc; 4]);
    }
    assert_eq!(
        frame.copy_bgra8_to(&mut destination, 31),
        Err(MacosCaptureError::InvalidCpuDestinationStride {
            minimum: 32,
            actual: 31,
        })
    );
}

#[test]
fn cpu_copy_rejects_non_bgra_input_without_mapping_it() {
    let mut sample = complete_sample();
    complete_frame_mut(&mut sample).pixel_format_fourcc = ARGB2101010;
    let frame = decode_frame(&mut MacosFrameDecoder::new(1), sample);
    assert_eq!(
        frame.copy_bgra8_to(&mut [0; 192], 32),
        Err(MacosCaptureError::UnsupportedCpuPixelFormat(
            MacosCapturePixelFormat::Argb2101010
        ))
    );
}

#[test]
fn bgra_sdr_conversion_swizzles_channels_and_preserves_alpha() {
    let frame = bgra_cpu_frame([30, 20, 10, 127], rgb_color());
    let mut destination = vec![0; 192];
    frame
        .convert_bgra8_sdr_to_rgba8(&mut destination, 32)
        .expect("sRGB BGRA should convert");
    for pixel in destination.chunks_exact(4) {
        assert_eq!(pixel, &[10, 20, 30, 127]);
    }
}

#[test]
fn bgra_sdr_conversion_compresses_wide_gamut_primaries() {
    let mut color = rgb_color();
    color.primaries = MacosColorPrimaries::DisplayP3;
    let frame = bgra_cpu_frame([0, 0, 255, 255], color);
    let mut destination = vec![0; 192];
    frame
        .convert_bgra8_sdr_to_rgba8(&mut destination, 32)
        .expect("Display P3 red should convert");
    assert_eq!(destination[0], 255);
    assert_eq!(destination[1], 0);
    assert!(destination[2] < 64);
    assert_eq!(destination[3], 255);
}

#[test]
fn bgra_sdr_conversion_rejects_hdr_transfer_functions() {
    let mut color = rgb_color();
    color.transfer = MacosTransferFunction::Pq;
    let frame = bgra_cpu_frame([0, 0, 255, 255], color);
    assert_eq!(
        frame.convert_bgra8_sdr_to_rgba8(&mut [0; 192], 32),
        Err(MacosCaptureError::UnsupportedCpuTransferFunction(
            MacosTransferFunction::Pq
        ))
    );
}

#[test]
fn mailbox_replaces_stale_deliveries_without_growing() {
    let mailbox = MacosFrameMailbox::new();
    assert!(!mailbox.has_pending());
    assert_eq!(mailbox.superseded_count(), 0);

    mailbox.publish(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started)));
    mailbox.publish(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle)));

    assert!(mailbox.has_pending());
    assert_eq!(mailbox.superseded_count(), 1);
    assert!(matches!(
        mailbox.take_latest(),
        Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle)))
    ));
    assert!(!mailbox.has_pending());
}

#[test]
fn mailbox_wait_returns_a_ready_delivery_without_polling() {
    let mailbox = MacosFrameMailbox::new();
    mailbox.publish(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started)));
    assert!(matches!(
        mailbox.wait_latest(std::time::Duration::from_secs(1)),
        Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started)))
    ));
    assert!(
        mailbox
            .wait_latest(std::time::Duration::from_millis(0))
            .is_none()
    );
}

#[test]
fn callback_diagnostics_start_with_every_drop_reason_at_zero() {
    let diagnostics = MacosCaptureCallbackDiagnostics::default();
    assert_eq!(diagnostics.frames_received, 0);
    assert_eq!(diagnostics.frames_published, 0);
    assert_eq!(diagnostics.lifecycle_events, 0);
    assert_eq!(diagnostics.superseded_deliveries, 0);
    assert_eq!(diagnostics.total_dropped(), 0);
    for reason in MacosFrameDropReason::ALL {
        assert_eq!(diagnostics.dropped(reason), 0);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn screen_capture_session_handle_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<hypercolor_macos_capture::MacosScreenCaptureSession>();
}

fn sample_with_status(status: i64) -> MacosRawCaptureSample {
    let mut sample = complete_sample();
    sample.attachments.status = MacosAttachment::Value(status);
    sample
}

fn complete_sample() -> MacosRawCaptureSample {
    let planes = packed_planes(4);
    MacosRawCaptureSample {
        frame: Some(MacosRawCompleteFrame {
            storage_extent: pixel_extent(8, 6),
            surface: surface_for(&planes),
            planes,
            pixel_format_fourcc: BGRA8,
            color: rgb_color(),
            cursor_composed: true,
        }),
        attachments: MacosRawFrameAttachments {
            status: MacosAttachment::Value(0),
            display_time: MacosAttachment::Value(12_345),
            display_scale_factor: MacosAttachment::Value(1.0),
            content_scale: MacosAttachment::Value(1.0),
            content_rect: MacosAttachment::Value(point_rect(0.0, 0.0, 8.0, 6.0)),
            dirty_rects: MacosAttachment::Missing,
            screen_rect: MacosAttachment::Missing,
            bounding_rect: MacosAttachment::Missing,
        },
    }
}

fn bgra_cpu_frame(
    pixel: [u8; 4],
    color: MacosCaptureColorimetry,
) -> hypercolor_macos_capture::MacosCaptureFrame {
    let mut sample = complete_sample();
    let source = pixel.repeat(48);
    let frame = complete_frame_mut(&mut sample);
    frame.color = color;
    frame.surface =
        MacosCaptureSurface::new_cpu_fixture(7, 192, 99, vec![Arc::<[u8]>::from(source)])
            .expect("CPU fixture surface should be valid");
    decode_frame(&mut MacosFrameDecoder::new(1), sample)
}

fn complete_frame_mut(sample: &mut MacosRawCaptureSample) -> &mut MacosRawCompleteFrame {
    sample.frame.as_mut().expect("fixture frame should exist")
}

fn decode_frame(
    decoder: &mut MacosFrameDecoder,
    sample: MacosRawCaptureSample,
) -> hypercolor_macos_capture::MacosCaptureFrame {
    match decoder.decode(sample).expect("sample should decode") {
        MacosFrameEvent::Frame(frame) => *frame,
        MacosFrameEvent::Lifecycle(status) => panic!("expected frame, got {status:?}"),
    }
}

fn decode_error(sample: MacosRawCaptureSample) -> MacosCaptureError {
    MacosFrameDecoder::new(1)
        .decode(sample)
        .expect_err("fixture should fail validation")
}

fn packed_planes(bytes_per_pixel: usize) -> Vec<MacosRawCapturePlane> {
    let stride = 8 * bytes_per_pixel;
    vec![MacosRawCapturePlane {
        index: 0,
        extent: pixel_extent(8, 6),
        bytes_per_row: stride,
        length_bytes: (stride * 6) as u64,
    }]
}

fn yuv420_planes() -> Vec<MacosRawCapturePlane> {
    vec![
        MacosRawCapturePlane {
            index: 0,
            extent: pixel_extent(8, 6),
            bytes_per_row: 8,
            length_bytes: 48,
        },
        MacosRawCapturePlane {
            index: 1,
            extent: pixel_extent(4, 3),
            bytes_per_row: 8,
            length_bytes: 24,
        },
    ]
}

fn yuv44410_planes() -> Vec<MacosRawCapturePlane> {
    vec![
        MacosRawCapturePlane {
            index: 0,
            extent: pixel_extent(8, 6),
            bytes_per_row: 16,
            length_bytes: 96,
        },
        MacosRawCapturePlane {
            index: 1,
            extent: pixel_extent(8, 6),
            bytes_per_row: 32,
            length_bytes: 192,
        },
    ]
}

fn surface_for(planes: &[MacosRawCapturePlane]) -> MacosCaptureSurface {
    let allocation = planes.iter().map(|plane| plane.length_bytes).sum();
    MacosCaptureSurface::new_fixture(7, allocation, 99).expect("fixture surface should be valid")
}

fn rgb_color() -> MacosCaptureColorimetry {
    MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Srgb,
        transfer: MacosTransferFunction::Srgb,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    }
}

fn yuv_color(range: MacosColorRange) -> MacosCaptureColorimetry {
    MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Pq,
        matrix: Some(MacosYuvMatrix::Bt2020),
        range,
        chroma_location: Some(MacosChromaLocation::Left),
    }
}

fn pixel_extent(width: u32, height: u32) -> MacosPixelExtent {
    MacosPixelExtent::new(width, height).expect("fixture extent should be valid")
}

fn pixel_rect(x: i64, y: i64, width: u32, height: u32) -> MacosPixelRect {
    MacosPixelRect::new(x, y, width, height).expect("fixture pixel rect should be valid")
}

fn point_rect(x: f64, y: f64, width: f64, height: f64) -> MacosPointRect {
    MacosPointRect::new(x, y, width, height).expect("fixture point rect should be valid")
}
