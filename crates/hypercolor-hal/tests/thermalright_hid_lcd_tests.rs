//! Thermalright HID LCD wire format, against spec 96.

use std::collections::BTreeSet;
use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::display::{DisplayEncodeError, Packed16Format, RepackError};
use hypercolor_hal::drivers::thermalright::{
    HID_LCD_FRAME_HEADER_LEN, HID_LCD_FRAME_REPORTS, HID_LCD_KEEPALIVE_SILENCE, HID_LCD_MAGIC,
    HID_LCD_PACKET_LEN, HID_LCD_PIXEL_BYTES, HID_LCD_PUMP_INTERVAL, HID_LCD_REPORT_LEN,
    HandshakeReply, HidLcdCommand, HidLcdProtocol, THERMALRIGHT_HID_LCD_PID,
    THERMALRIGHT_STREAMING_FIRMWARE_RELEASE, THERMALRIGHT_VENDOR_ID, model_pixel_format,
    parse_handshake_reply,
};
use hypercolor_hal::protocol::{
    FrameBatch, Protocol, ProtocolCommand, ProtocolError, ResponseStatus, TransferType,
};
use hypercolor_hal::registry::{DeviceDescriptor, HidRawReportMode, TransportType};
use hypercolor_types::device::{
    DeviceTopologyHint, DisplayFrameFormat, DisplayFramePayload, DriverStability,
};

const RED: [u8; 3] = [0xFF, 0x00, 0x00];
const GREEN: [u8; 3] = [0x00, 0xFF, 0x00];
const BLUE: [u8; 3] = [0x00, 0x00, 0xFF];
const RED_LE: [u8; 2] = [0x00, 0xF8];
const GREEN_LE: [u8; 2] = [0xE0, 0x07];
const BLUE_LE: [u8; 2] = [0x1F, 0x00];
const RASTER_WIDTH: usize = 240;
const RASTER_HEIGHT: usize = 320;

/// A full-size standard reply: magic, SUB, PM, and the device-info echo.
fn standard_reply(pm: u8, sub: u8) -> Vec<u8> {
    let mut reply = vec![0_u8; HID_LCD_REPORT_LEN];
    reply[..4].copy_from_slice(&HID_LCD_MAGIC);
    reply[4] = sub;
    reply[5] = pm;
    reply[12] = 0x01;
    reply
}

/// The eight-byte reply a firmware 4.07 streaming unit gives.
fn short_reply() -> Vec<u8> {
    vec![0xDA, 0xDB, 0xDC, 0xDD, 0x00, 0x3A, 0x00, 0x00]
}

fn identified(reply: &[u8]) -> HidLcdProtocol {
    let protocol = HidLcdProtocol::new();
    protocol.init_sequence();
    protocol
        .parse_response(reply)
        .expect("a supported panel model is accepted");
    protocol
}

/// A black RGB888 frame with three marked pixels.
fn marked_frame(width: usize, height: usize, marks: &[((usize, usize), [u8; 3])]) -> Vec<u8> {
    let mut frame = vec![0_u8; width * height * 3];
    for &((x, y), color) in marks {
        let offset = (y * width + x) * 3;
        frame[offset..offset + 3].copy_from_slice(&color);
    }
    frame
}

fn rgb_payload(width: u32, height: u32, data: &[u8]) -> DisplayFramePayload<'_> {
    DisplayFramePayload {
        format: DisplayFrameFormat::Rgb,
        width,
        height,
        data,
    }
}

fn encode(protocol: &HidLcdProtocol, payload: DisplayFramePayload<'_>) -> Vec<ProtocolCommand> {
    let mut commands = Vec::new();
    protocol
        .encode_display_payload_into(payload, &mut commands)
        .expect("frame should encode");
    commands
}

/// The report stream with every report ID byte stripped.
fn stream(commands: &[ProtocolCommand]) -> Vec<u8> {
    commands
        .iter()
        .flat_map(|command| command.data[1..].iter().copied())
        .collect()
}

/// The two wire bytes of raster pixel (`x`, `y`).
fn raster_pixel(stream: &[u8], x: usize, y: usize) -> [u8; 2] {
    let offset = HID_LCD_FRAME_HEADER_LEN + (y * RASTER_WIDTH + x) * 2;
    [stream[offset], stream[offset + 1]]
}

fn lit_pixels(stream: &[u8]) -> usize {
    stream[HID_LCD_FRAME_HEADER_LEN..HID_LCD_FRAME_HEADER_LEN + HID_LCD_PIXEL_BYTES]
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|pixel| **pixel != [0, 0])
        .count()
}

// --- Handshake ---

#[test]
fn handshake_is_one_report_with_the_magic_and_device_info_at_byte_12() {
    let protocol = HidLcdProtocol::new();
    let init = protocol.init_sequence();
    assert_eq!(init.len(), 1, "one handshake report opens the session");

    let handshake = &init[0];
    assert_eq!(handshake.data.len(), HID_LCD_PACKET_LEN);
    assert_eq!(handshake.data[0], 0x00, "unnumbered report ID");
    let report = &handshake.data[1..];
    assert_eq!(&report[..4], &HID_LCD_MAGIC);
    assert_eq!(&report[4..12], &[0; 8]);
    assert_eq!(&report[12..16], &[0x01, 0x00, 0x00, 0x00]);
    assert!(
        report[16..].iter().all(|byte| *byte == 0),
        "the rest of the report is zero"
    );

    assert!(handshake.expects_response, "the reply identifies the panel");
    assert_eq!(handshake.response_delay, Duration::from_millis(200));
    assert_eq!(handshake.response.timeout, Some(Duration::from_secs(5)));
    assert_eq!(handshake.response.count, 1);
    assert_eq!(handshake.transfer_type, TransferType::Primary);
    assert!(protocol.shutdown_sequence().is_empty());
}

#[test]
fn standard_reply_parses_model_sub_and_serial() {
    let reply = parse_handshake_reply(&standard_reply(58, 3)).expect("standard reply");
    assert_eq!(
        reply,
        HandshakeReply {
            pm: 58,
            sub: 3,
            serial: None,
        }
    );

    let mut with_serial = standard_reply(51, 0);
    with_serial[16] = 0x10;
    let serial: [u8; 16] = std::array::from_fn(|index| 0xA0 + u8::try_from(index).expect("u8"));
    with_serial[20..36].copy_from_slice(&serial);
    let reply = parse_handshake_reply(&with_serial).expect("reply with serial");
    assert_eq!(reply.serial, Some(serial));
    assert_eq!(reply.pm, 51);
}

#[test]
fn replies_without_the_magic_or_too_short_are_malformed() {
    let mut bad_magic = standard_reply(58, 0);
    bad_magic[0] = 0xDB;
    assert!(matches!(
        parse_handshake_reply(&bad_magic),
        Err(ProtocolError::MalformedResponse { .. })
    ));

    assert!(matches!(
        parse_handshake_reply(&HID_LCD_MAGIC),
        Err(ProtocolError::MalformedResponse { .. })
    ));
    assert!(matches!(
        parse_handshake_reply(&[]),
        Err(ProtocolError::MalformedResponse { .. })
    ));

    // A full report with the magic but no device-info echo and other bytes
    // set is neither shape.
    let mut no_echo = standard_reply(58, 0);
    no_echo[12] = 0x02;
    no_echo[40] = 0x77;
    assert!(matches!(
        parse_handshake_reply(&no_echo),
        Err(ProtocolError::MalformedResponse { .. })
    ));
}

#[test]
fn short_replies_are_refused_as_the_streaming_variant() {
    let mut padded = short_reply();
    padded.resize(HID_LCD_REPORT_LEN, 0);

    // Bare as Linux hidraw hands it over, and zero-padded as Windows HID
    // returns every input report.
    for reply in [short_reply(), padded] {
        let Err(ProtocolError::MalformedResponse { detail }) = parse_handshake_reply(&reply) else {
            panic!("a {}-byte short reply must be refused", reply.len());
        };
        assert!(
            detail.contains("firmware 4.07 streaming variant") && detail.contains("model 58"),
            "the log line names the variant and the model: {detail}"
        );
    }

    let protocol = HidLcdProtocol::new();
    protocol.init_sequence();
    assert!(protocol.parse_response(&short_reply()).is_err());
    assert_eq!(protocol.handshake(), None);
}

#[test]
fn panel_models_map_to_byte_order() {
    for pm in [50, 51, 52, 53, 58] {
        assert_eq!(
            model_pixel_format(pm),
            Some(Packed16Format::Rgb565),
            "model {pm}"
        );
    }
    assert_eq!(
        model_pixel_format(49),
        Some(Packed16Format::Rgb565BigEndian)
    );
    for pm in [0, 36, 48, 54, 64, 100, 128] {
        assert_eq!(
            model_pixel_format(pm),
            None,
            "model {pm} is not a 320x240 RGB565 panel"
        );
    }
}

#[test]
fn unknown_model_fails_the_handshake_and_leaves_the_panel_unidentified() {
    let protocol = HidLcdProtocol::new();
    protocol.init_sequence();

    let error = protocol
        .parse_response(&standard_reply(128, 1))
        .expect_err("a 1280x480 panel is refused");
    assert!(matches!(
        error,
        ProtocolError::DeviceError {
            status: ResponseStatus::Unsupported
        }
    ));
    assert_eq!(protocol.handshake(), None);
}

#[test]
fn standard_handshake_declares_a_landscape_rgb_surface() {
    let before = HidLcdProtocol::new();
    assert_eq!(before.pixel_format(), Packed16Format::Rgb565);

    let protocol = identified(&standard_reply(52, 0));
    let response = protocol
        .parse_response(&standard_reply(52, 0))
        .expect("accepted again");
    assert_eq!(response.status, ResponseStatus::Ok);
    assert_eq!(response.data, vec![52, 0]);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "Display");
    assert_eq!(zones[0].led_count, 0);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Display {
            width: 320,
            height: 240,
            circular: false,
            format: DisplayFrameFormat::Rgb,
        }
    );

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 0);
    assert!(!capabilities.supports_direct);
    assert!(!capabilities.supports_brightness);
    assert_eq!(capabilities.max_fps, 30);
    assert_eq!(protocol.total_leds(), 0);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
}

#[test]
fn a_new_session_forgets_the_last_panel_and_frame() {
    let protocol = identified(&standard_reply(49, 0));
    assert_eq!(protocol.pixel_format(), Packed16Format::Rgb565BigEndian);
    let frame = vec![0x40; 320 * 240 * 3];
    encode(&protocol, rgb_payload(320, 240, &frame));

    protocol.init_sequence();
    assert_eq!(protocol.handshake(), None);
    assert_eq!(protocol.pixel_format(), Packed16Format::Rgb565);

    std::thread::sleep(HID_LCD_KEEPALIVE_SILENCE + Duration::from_millis(50));
    let mut pumped = Vec::new();
    protocol.pump_frame_into(&mut pumped);
    assert!(pumped.is_empty(), "the old session's frame is gone");
}

// --- Frames ---

#[test]
fn frame_is_301_reports_with_the_header_up_front() {
    let protocol = identified(&standard_reply(58, 0));
    let frame = vec![0x80; 320 * 240 * 3];
    let commands = encode(&protocol, rgb_payload(320, 240, &frame));

    assert_eq!(commands.len(), HID_LCD_FRAME_REPORTS);
    assert_eq!(commands.len(), 301);
    for (index, command) in commands.iter().enumerate() {
        assert_eq!(command.data.len(), HID_LCD_PACKET_LEN, "report {index}");
        assert_eq!(command.data[0], 0x00, "report {index} ID");
        assert!(
            !command.expects_response,
            "report {index} is unacknowledged"
        );
        assert_eq!(command.transfer_type, TransferType::Primary);
        let settle = if index + 1 == commands.len() {
            Duration::from_millis(1)
        } else {
            Duration::ZERO
        };
        assert_eq!(command.post_delay, settle, "report {index} pacing");
    }

    assert_eq!(
        &commands[0].data[1..=HID_LCD_FRAME_HEADER_LEN],
        &[
            0xDA, 0xDB, 0xDC, 0xDD, // magic
            0x02, 0x00, // picture
            0x01, 0x00, // RGB565
            0xF0, 0x00, // width 240
            0x40, 0x01, // height 320
            0x02, 0x00, 0x00, 0x00, // picture command
            0x00, 0x58, 0x02, 0x00, // 153,600 payload bytes
        ]
    );
}

#[test]
fn last_report_carries_the_final_20_pixel_bytes_then_zero_padding() {
    let protocol = identified(&standard_reply(58, 0));
    let frame = vec![0xFF; 320 * 240 * 3];
    let commands = encode(&protocol, rgb_payload(320, 240, &frame));
    let bytes = stream(&commands);

    assert_eq!(bytes.len(), HID_LCD_FRAME_REPORTS * HID_LCD_REPORT_LEN);
    assert_eq!(bytes.len(), 154_112);
    let tail_start = HID_LCD_FRAME_HEADER_LEN + HID_LCD_PIXEL_BYTES;
    assert_eq!(tail_start, 153_620);

    let last = &commands[HID_LCD_FRAME_REPORTS - 1].data[1..];
    assert!(
        last[..20].iter().all(|byte| *byte == 0xFF),
        "the frame's last 20 bytes are white pixels"
    );
    assert!(
        last[20..].iter().all(|byte| *byte == 0x00),
        "the padding after them is zero"
    );
    assert!(bytes[tail_start..].iter().all(|byte| *byte == 0));
}

#[test]
fn landscape_frames_turn_a_quarter_clockwise() {
    let protocol = identified(&standard_reply(58, 0));
    // Three corners of the landscape image, each a different color, so any
    // other turn or a mirror lands them somewhere else.
    let frame = marked_frame(
        320,
        240,
        &[((0, 0), RED), ((319, 0), GREEN), ((0, 239), BLUE)],
    );
    let bytes = stream(&encode(&protocol, rgb_payload(320, 240, &frame)));

    // Turned clockwise, the top-left corner goes to the raster's top-right,
    // the top-right to the bottom-right, and the bottom-left to the top-left.
    assert_eq!(raster_pixel(&bytes, RASTER_WIDTH - 1, 0), RED_LE);
    assert_eq!(
        raster_pixel(&bytes, RASTER_WIDTH - 1, RASTER_HEIGHT - 1),
        GREEN_LE
    );
    assert_eq!(raster_pixel(&bytes, 0, 0), BLUE_LE);
    assert_eq!(lit_pixels(&bytes), 3, "nothing else is lit");
}

#[test]
fn turned_pixels_follow_column_order() {
    let protocol = identified(&standard_reply(58, 0));
    // A pixel off every edge: source (10, 20) lands at raster
    // (239 - 20, 10).
    let frame = marked_frame(320, 240, &[((10, 20), GREEN)]);
    let bytes = stream(&encode(&protocol, rgb_payload(320, 240, &frame)));
    assert_eq!(raster_pixel(&bytes, 219, 10), GREEN_LE);
    assert_eq!(lit_pixels(&bytes), 1);
}

#[test]
fn model_49_sends_pixels_high_byte_first() {
    let protocol = identified(&standard_reply(49, 0));
    let frame = marked_frame(320, 240, &[((0, 0), RED)]);
    let bytes = stream(&encode(&protocol, rgb_payload(320, 240, &frame)));
    assert_eq!(raster_pixel(&bytes, RASTER_WIDTH - 1, 0), [0xF8, 0x00]);
}

#[test]
fn frames_reuse_the_command_buffer_without_stale_bytes() {
    let protocol = identified(&standard_reply(58, 0));
    let white = vec![0xFF; 320 * 240 * 3];
    let black = vec![0x00; 320 * 240 * 3];

    let mut commands = Vec::new();
    protocol
        .encode_display_payload_into(rgb_payload(320, 240, &white), &mut commands)
        .expect("white frame");
    protocol
        .encode_display_payload_into(rgb_payload(320, 240, &black), &mut commands)
        .expect("black frame");

    let fresh = encode(&protocol, rgb_payload(320, 240, &black));
    assert_eq!(commands.len(), fresh.len());
    for (reused, fresh) in commands.iter().zip(&fresh) {
        assert_eq!(reused.data, fresh.data);
        assert_eq!(reused.post_delay, fresh.post_delay);
    }
    assert_eq!(lit_pixels(&stream(&commands)), 0);
}

#[test]
fn frames_that_do_not_fit_the_panel_are_refused() {
    let protocol = identified(&standard_reply(58, 0));
    let mut commands = Vec::new();

    let portrait = vec![0; 240 * 320 * 3];
    let error = protocol
        .encode_display_payload_into(rgb_payload(240, 320, &portrait), &mut commands)
        .expect_err("a landscape panel takes 320x240");
    assert!(matches!(
        error,
        DisplayEncodeError::WrongGeometry {
            expected_width: 320,
            expected_height: 240,
            width: 240,
            height: 320,
        }
    ));

    let error = protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&[0xFF, 0xD8]), &mut commands)
        .expect_err("JPEG is not this panel's format");
    assert!(matches!(
        error,
        DisplayEncodeError::Unsupported {
            format: DisplayFrameFormat::Jpeg
        }
    ));

    let short = vec![0; 100];
    let error = protocol
        .encode_display_payload_into(rgb_payload(320, 240, &short), &mut commands)
        .expect_err("a short frame is not padded");
    assert!(matches!(
        error,
        DisplayEncodeError::Repack(RepackError::SourceTooSmall { .. })
    ));
}

#[test]
fn the_panel_has_no_leds() {
    let protocol = HidLcdProtocol::new();
    let mut commands = Vec::new();
    let batch = protocol.encode_frame_into(&[[1, 2, 3]], &mut commands);
    assert!(commands.is_empty());
    assert_eq!(batch, FrameBatch::Unchanged);
    assert!(protocol.encode_frame(&[[1, 2, 3]]).is_empty());
}

// --- Keepalive ---

#[test]
fn pump_resends_the_last_frame_after_a_second_of_silence() {
    let protocol = identified(&standard_reply(58, 0));
    assert_eq!(protocol.frame_pump_interval(), Some(HID_LCD_PUMP_INTERVAL));
    assert!(HID_LCD_PUMP_INTERVAL < HID_LCD_KEEPALIVE_SILENCE);

    let mut pumped = Vec::new();
    protocol.pump_frame_into(&mut pumped);
    assert!(
        pumped.is_empty(),
        "nothing to resend before the first frame"
    );

    let frame = marked_frame(320, 240, &[((5, 6), BLUE)]);
    let sent = encode(&protocol, rgb_payload(320, 240, &frame));

    protocol.pump_frame_into(&mut pumped);
    assert!(pumped.is_empty(), "a frame just went out");

    std::thread::sleep(HID_LCD_KEEPALIVE_SILENCE + Duration::from_millis(50));
    protocol.pump_frame_into(&mut pumped);
    assert_eq!(pumped.len(), sent.len(), "the whole frame goes again");
    for (resent, original) in pumped.iter().zip(&sent) {
        assert_eq!(resent.data, original.data);
        assert_eq!(resent.post_delay, original.post_delay);
        assert!(!resent.expects_response);
    }

    protocol.pump_frame_into(&mut pumped);
    assert!(pumped.is_empty(), "the resend restarts the silence clock");
}

#[test]
fn a_frame_that_fails_to_encode_is_never_resent() {
    let protocol = identified(&standard_reply(58, 0));
    let mut commands = Vec::new();
    protocol
        .encode_display_payload_into(rgb_payload(320, 240, &[0; 30]), &mut commands)
        .expect_err("short frame");

    std::thread::sleep(HID_LCD_KEEPALIVE_SILENCE + Duration::from_millis(50));
    let mut pumped = Vec::new();
    protocol.pump_frame_into(&mut pumped);
    assert!(pumped.is_empty());
}

// --- Command vocabulary ---

/// Every command a session can emit: handshake, frames in each byte order,
/// a keepalive resend, and shutdown.
fn every_session_command() -> Vec<ProtocolCommand> {
    let mut all = Vec::new();
    for reply in [
        standard_reply(58, 0),
        standard_reply(49, 2),
        standard_reply(51, 1),
    ] {
        let protocol = HidLcdProtocol::new();
        all.extend(protocol.init_sequence());
        protocol.parse_response(&reply).expect("supported model");
        let frame = vec![0xA5; 320 * 240 * 3];
        all.extend(encode(&protocol, rgb_payload(320, 240, &frame)));
        std::thread::sleep(HID_LCD_KEEPALIVE_SILENCE + Duration::from_millis(50));
        let mut pumped = Vec::new();
        protocol.pump_frame_into(&mut pumped);
        assert!(!pumped.is_empty());
        all.extend(pumped);
        all.extend(protocol.shutdown_sequence());
        assert!(protocol.encode_brightness(0x80).is_none());
        assert!(protocol.keepalive().is_none());
    }
    all
}

#[test]
fn no_command_ever_encodes_a_firmware_update() {
    const FIRMWARE_UPDATE: u32 = 4;
    let allowed = [
        u32::from(HidLcdCommand::DeviceInfo.code()),
        u32::from(HidLcdCommand::Picture.code()),
    ];

    let mut headers = 0;
    for command in every_session_command() {
        assert_eq!(command.data.len(), HID_LCD_PACKET_LEN);
        let report = &command.data[1..];
        if report[..4] != HID_LCD_MAGIC {
            continue;
        }
        headers += 1;
        let frame_type = u32::from(u16::from_le_bytes([report[4], report[5]]));
        let command_word = u32::from_le_bytes([report[12], report[13], report[14], report[15]]);
        assert_ne!(frame_type, FIRMWARE_UPDATE);
        assert_ne!(command_word, FIRMWARE_UPDATE);
        assert!(
            allowed.contains(&command_word),
            "command word {command_word} is outside the driver's vocabulary"
        );
        assert!(
            frame_type == 0 || allowed.contains(&frame_type),
            "frame type {frame_type} is outside the driver's vocabulary"
        );
    }
    // Three handshakes, three frames, three resends.
    assert_eq!(headers, 9);
}

// --- Registration ---

/// The descriptor a scan of a unit on `release` resolves to.
fn claimed_at(release: u16) -> Option<&'static DeviceDescriptor> {
    ProtocolDatabase::lookup_with_firmware_for_driver_ids(
        THERMALRIGHT_VENDOR_ID,
        THERMALRIGHT_HID_LCD_PID,
        Some("USBDISPLAY"),
        Some(release),
        None,
    )
}

#[test]
fn firmware_4_07_units_are_never_claimed() {
    assert_eq!(THERMALRIGHT_STREAMING_FIRMWARE_RELEASE, 0x0407);
    assert!(claimed_at(0x0407).is_none(), "4.07 stays unclaimed");
    for release in [0x0001, 0x0100, 0x0200, 0x0406, 0x0408, 0x0500] {
        let descriptor = claimed_at(release).expect("every other release is claimed");
        assert_eq!(
            descriptor.protocol.id, "thermalright/hid-lcd",
            "{release:#06X}"
        );
    }
    assert!(
        ProtocolDatabase::lookup(THERMALRIGHT_VENDOR_ID, THERMALRIGHT_HID_LCD_PID).is_none(),
        "a unit whose release is unknown is not claimed either"
    );

    let enabled = BTreeSet::from(["thermalright".to_owned()]);
    assert!(
        ProtocolDatabase::lookup_with_firmware_for_driver_ids(
            THERMALRIGHT_VENDOR_ID,
            THERMALRIGHT_HID_LCD_PID,
            None,
            Some(0x0407),
            Some(&enabled),
        )
        .is_none(),
        "enabling the driver does not claim 4.07"
    );
}

#[test]
fn one_experimental_descriptor_claims_the_hid_interface() {
    let descriptor = claimed_at(0x0100).expect("0416:5302 is registered");
    assert_eq!(descriptor.name, "Thermalright LCD");
    assert_eq!(descriptor.protocol.id, "thermalright/hid-lcd");
    assert_eq!(descriptor.driver_id(), "thermalright");
    assert_eq!(descriptor.stability, DriverStability::Experimental);
    assert!(descriptor.firmware_predicate.is_none());

    #[cfg(target_os = "linux")]
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidRaw {
            interface: 0,
            report_id: 0x00,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            usage_page: None,
            usage: None,
        }
    );
    #[cfg(not(target_os = "linux"))]
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: Some(0),
            report_id: 0x00,
            report_mode: HidRawReportMode::OutputReportWithReportId,
            max_report_len: HID_LCD_PACKET_LEN,
            usage_page: None,
            usage: None,
        }
    );

    let claims = ProtocolDatabase::all()
        .iter()
        .filter(|candidate| {
            candidate.vendor_id == THERMALRIGHT_VENDOR_ID
                && candidate.product_id == THERMALRIGHT_HID_LCD_PID
        })
        .count();
    assert_eq!(claims, 1, "one descriptor claims 0416:5302");

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Thermalright LCD");
    assert_eq!(protocol.init_sequence().len(), 1);
}
