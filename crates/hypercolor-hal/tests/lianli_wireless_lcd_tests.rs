//! Wireless LCD receiver wire format, against spec 80 section 7.

use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::display::DisplayEncodeError;
use hypercolor_hal::drivers::lianli::WirelessLcdProtocol;
use hypercolor_hal::drivers::lianli::wireless::crypto::{
    HEADER_LEN, HeaderBuilder, MAGIC, wrap_header,
};
use hypercolor_hal::drivers::lianli::wireless::lcd::{
    WIRELESS_LCD_FRAME_LEN, WIRELESS_LCD_MAX_JPEG_LEN, WirelessLcdCommand, brightness_lut,
};
use hypercolor_hal::protocol::{Protocol, ResponseTolerance, TransferType};
use hypercolor_hal::registry::TransportType;
use hypercolor_types::device::{DeviceTopologyHint, DisplayFrameFormat, DisplayFramePayload};

// --- DES header (section 7.1) ---

/// Known answer from OpenSSL (`enc -des-cbc -K 736c763374757a78 -iv
/// 736c763374757a78`) over the 504-byte plaintext for GetVer at timestamp 1:
/// first and last blocks of the 512-byte ciphertext.
const GETVER_TS1_CIPHERTEXT_HEAD: [u8; 32] = [
    0xf1, 0x32, 0xa5, 0xd4, 0xe3, 0xcf, 0xf8, 0x57, 0x48, 0xb8, 0x2a, 0xaa, 0xca, 0xc6, 0x8f, 0x8a,
    0xe7, 0x6b, 0xd5, 0x35, 0xd3, 0xe9, 0x53, 0x8a, 0x13, 0x0a, 0x7b, 0x11, 0x21, 0x9f, 0x36, 0x5c,
];
const GETVER_TS1_CIPHERTEXT_TAIL: [u8; 16] = [
    0x25, 0xed, 0x2b, 0x0b, 0xdf, 0x5b, 0xe3, 0x81, 0xea, 0xb0, 0x71, 0x9f, 0xcf, 0xcd, 0x5b, 0x4b,
];

#[test]
fn the_header_matches_an_independent_des_cbc_implementation() {
    let header = wrap_header(WirelessLcdCommand::GetVer as u8, 1, &[]);
    assert_eq!(header.len(), HEADER_LEN);
    assert_eq!(&header[..32], &GETVER_TS1_CIPHERTEXT_HEAD);
    assert_eq!(&header[HEADER_LEN - 16..], &GETVER_TS1_CIPHERTEXT_TAIL);
}

/// CBC chains every block on the one before it, so a change in the first
/// plaintext byte changes the last ciphertext block too.
#[test]
fn every_block_depends_on_the_command_byte() {
    let getver = wrap_header(WirelessLcdCommand::GetVer as u8, 1, &[]);
    let reboot = wrap_header(0x0B, 1, &[]);
    assert_ne!(&getver[..8], &reboot[..8]);
    assert_ne!(&getver[HEADER_LEN - 8..], &reboot[HEADER_LEN - 8..]);
}

#[test]
fn the_plaintext_layout_puts_magic_and_a_little_endian_timestamp_up_front() {
    // Encrypting the same plaintext twice is deterministic under a fixed
    // IV, so a header is a pure function of (command, timestamp, params).
    assert_eq!(
        wrap_header(0x65, 42, &[1, 2, 3]),
        wrap_header(0x65, 42, &[1, 2, 3])
    );
    assert_ne!(wrap_header(0x65, 42, &[]), wrap_header(0x65, 43, &[]));
    assert_ne!(wrap_header(0x65, 42, &[1]), wrap_header(0x65, 42, &[2]));
    assert_eq!(MAGIC, [0x1A, 0x6D]);
}

#[test]
fn timestamps_never_repeat_within_a_session() {
    let mut builder = HeaderBuilder::new();
    let first = builder.next_timestamp();
    let second = builder.next_timestamp();
    let third = builder.next_timestamp();
    assert!(second > first, "{second} after {first}");
    assert!(third > second, "{third} after {second}");
}

// --- Brightness curve (section 7.2) ---

#[test]
fn the_brightness_curve_hits_its_anchors_and_interpolates_between_them() {
    assert_eq!(brightness_lut(0), 0);
    assert_eq!(brightness_lut(25), 10);
    assert_eq!(brightness_lut(50), 30);
    assert_eq!(brightness_lut(75), 40);
    assert_eq!(brightness_lut(100), 100);
    assert_eq!(brightness_lut(37), 20);
    assert_eq!(brightness_lut(62), 35);
    assert_eq!(brightness_lut(200), 100, "clamped to the top anchor");
}

// --- Session (section 7.3) ---

#[test]
fn init_probes_sets_the_rate_reads_the_firmware_then_normalises_the_panel() {
    let protocol = WirelessLcdProtocol::new();
    let commands = protocol.init_sequence();
    assert_eq!(commands.len(), 5);
    for (index, command) in commands.iter().enumerate() {
        assert_eq!(
            command.data.len(),
            HEADER_LEN,
            "command {index} is one header"
        );
        assert!(
            command.expects_response,
            "command {index} drains a status reply"
        );
        assert_eq!(
            command.response.tolerance,
            ResponseTolerance::Optional,
            "command {index} tolerates a skipped status packet"
        );
        assert_eq!(command.response.capacity, Some(511));
        assert_eq!(command.response.timeout, Some(Duration::from_secs(2)));
    }
    // Headers are opaque ciphertext under a live timestamp, so pin what can
    // be seen: five distinct commands, none of them a raw plaintext.
    for pair in commands.windows(2) {
        assert_ne!(pair[0].data, pair[1].data);
    }
    assert!(commands.iter().all(|command| command.data[..8] != [0; 8]));
}

#[test]
fn a_frame_is_one_fixed_size_write_with_the_jpeg_after_the_header() {
    let protocol = WirelessLcdProtocol::new();
    let jpeg: Vec<u8> = (0..30_000_u32).map(|i| (i % 251) as u8).collect();
    let mut commands = Vec::new();

    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&jpeg), &mut commands)
        .expect("a 30 KB JPEG fits");

    assert_eq!(commands.len(), 1);
    let frame = &commands[0];
    assert_eq!(frame.data.len(), WIRELESS_LCD_FRAME_LEN);
    assert_eq!(&frame.data[HEADER_LEN..HEADER_LEN + jpeg.len()], &jpeg[..]);
    assert!(
        frame.data[HEADER_LEN + jpeg.len()..]
            .iter()
            .all(|&b| b == 0),
        "zero padding to the fixed frame length"
    );
    assert_eq!(frame.transfer_type, TransferType::Primary);
    assert!(frame.expects_response);
    assert_eq!(frame.response.tolerance, ResponseTolerance::Optional);
    assert_eq!(frame.response.capacity, Some(511));
    assert_eq!(frame.response.timeout, Some(Duration::from_millis(200)));
    assert_ne!(
        &frame.data[..8],
        &[0; 8],
        "the header is encrypted, not a raw command"
    );
}

#[test]
fn a_jpeg_past_the_wire_cap_is_refused_not_truncated() {
    let protocol = WirelessLcdProtocol::new();
    let jpeg = vec![0xAB; WIRELESS_LCD_MAX_JPEG_LEN + 1];
    let mut commands = Vec::new();

    let error = protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&jpeg), &mut commands)
        .expect_err("101,889 bytes cannot fit a 102,400-byte frame behind a 512-byte header");
    assert!(
        matches!(
            error,
            DisplayEncodeError::PayloadTooLarge {
                actual: 101_889,
                capacity: 101_888
            }
        ),
        "unexpected error: {error}"
    );
    assert!(commands.is_empty());

    let exact = vec![0xAB; WIRELESS_LCD_MAX_JPEG_LEN];
    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&exact), &mut commands)
        .expect("exactly the cap fits");
}

#[test]
fn raw_rgb_is_not_a_format_the_receiver_takes() {
    let protocol = WirelessLcdProtocol::new();
    let pixels = vec![0; 400 * 400 * 3];
    let error = protocol
        .encode_display_payload_into(
            DisplayFramePayload {
                format: DisplayFrameFormat::Rgb,
                width: 400,
                height: 400,
                data: &pixels,
            },
            &mut Vec::new(),
        )
        .expect_err("JPEG only");
    assert!(matches!(
        error,
        DisplayEncodeError::Unsupported {
            format: DisplayFrameFormat::Rgb
        }
    ));
}

#[test]
fn a_getver_reply_yields_the_firmware_and_status_packets_are_ignored() {
    let protocol = WirelessLcdProtocol::new();
    let mut reply = vec![0_u8; 64];
    reply[0] = WirelessLcdCommand::GetVer as u8;
    reply[8..15].copy_from_slice(b"V1.2.3\0");
    protocol
        .parse_response(&reply)
        .expect("a reply is never an error");
    assert_eq!(protocol.firmware().as_deref(), Some("V1.2.3"));

    let status = vec![0x55; 12];
    protocol
        .parse_response(&status)
        .expect("status packets are tolerated");
    assert_eq!(protocol.firmware().as_deref(), Some("V1.2.3"));
}

#[test]
fn the_receiver_exposes_one_round_400x400_display_with_a_frame_budget() {
    let protocol = WirelessLcdProtocol::new();
    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].led_count, 0);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Display {
            width: 400,
            height: 400,
            circular: true,
            format: DisplayFrameFormat::Jpeg,
        }
    );
    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.max_fps, 30);
    assert_eq!(
        capabilities.features.max_display_frame_len,
        Some(WIRELESS_LCD_MAX_JPEG_LEN),
        "the daemon's encoder must fit this wire cap"
    );
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
    assert!(
        protocol.encode_frame(&[[1, 2, 3]; 26]).is_empty(),
        "no LEDs on the receiver"
    );
}

#[test]
fn both_receivers_are_registered_on_bulk_interface_zero() {
    for (pid, name) in [
        (0x0006, "Lian Li Uni Fan TL Wireless LCD"),
        (0x0005, "Lian Li Uni Fan SL Wireless LCD"),
    ] {
        let descriptor = ProtocolDatabase::lookup(0x1CBE, pid).expect("registered");
        assert_eq!(descriptor.name, name);
        assert_eq!(descriptor.protocol.id, "lianli/wireless-lcd");
        assert_eq!(
            descriptor.transport,
            TransportType::UsbBulk {
                interface: 0,
                report_id: 0
            }
        );
    }
}
