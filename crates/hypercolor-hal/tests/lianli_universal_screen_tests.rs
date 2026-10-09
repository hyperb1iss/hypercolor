//! Lian Li Universal Screen 8.8" wire format, against spec 84.

use std::time::Duration;

use cbc::Decryptor;
use chrono::NaiveDate;
use des::Des;
use des::cipher::block_padding::Pkcs7;
use des::cipher::{BlockModeDecrypt, KeyIvInit};
use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::display::DisplayEncodeError;
use hypercolor_hal::drivers::lianli::UniversalScreenProtocol;
use hypercolor_hal::drivers::lianli::universal_screen::{
    PID_UNIVERSAL_SCREEN_88, UNIVERSAL_SCREEN_HEIGHT, UNIVERSAL_SCREEN_MAX_JPEG_LEN,
    UNIVERSAL_SCREEN_VENDOR_ID, UNIVERSAL_SCREEN_WIDTH, UniversalScreenCommand, clock_params,
};
use hypercolor_hal::drivers::lianli::winusb::{
    WINUSB_CIPHERTEXT_LEN, WINUSB_HEADER_LEN, WINUSB_MAX_PARAMS_LEN, WINUSB_PLAINTEXT_LEN,
    WINUSB_TRAILER, WinUsbHeaderBuilder, wrap_winusb_header,
};
use hypercolor_hal::drivers::lianli::wireless::crypto::DES_KEY;
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseTolerance, TransferType};
use hypercolor_hal::registry::TransportType;
use hypercolor_types::device::{DeviceTopologyHint, DisplayFrameFormat, DisplayFramePayload};

/// Decrypt a wire header back to its 500-byte plaintext.
fn plaintext(header: &[u8]) -> Vec<u8> {
    assert!(header.len() >= WINUSB_HEADER_LEN, "a whole header");
    let mut ciphertext = header[..WINUSB_CIPHERTEXT_LEN].to_vec();
    let decryptor = Decryptor::<Des>::new_from_slices(&DES_KEY, &DES_KEY).expect("8-byte key");
    decryptor
        .decrypt_padded::<Pkcs7>(&mut ciphertext)
        .expect("valid PKCS#7 padding")
        .to_vec()
}

fn timestamp(plain: &[u8]) -> u32 {
    u32::from_le_bytes(plain[4..8].try_into().expect("four bytes"))
}

fn size_param(plain: &[u8]) -> usize {
    usize::try_from(u32::from_be_bytes(
        plain[8..12].try_into().expect("four bytes"),
    ))
    .expect("fits usize")
}

fn assert_status_read(command: &ProtocolCommand, timeout: Duration, what: &str) {
    assert!(command.expects_response, "{what} drains a status reply");
    assert_eq!(
        command.response.tolerance,
        ResponseTolerance::Optional,
        "{what} tolerates a skipped status reply"
    );
    assert_eq!(
        command.response.capacity,
        Some(512),
        "{what} reply capacity"
    );
    assert_eq!(
        command.response.timeout,
        Some(timeout),
        "{what} reply timeout"
    );
    assert_eq!(command.transfer_type, TransferType::Primary, "{what} path");
}

// --- WinUSB header (section 3) ---

/// Known answer from OpenSSL (`enc -des-cbc -K 736c763374757a78 -iv
/// 736c763374757a78`) over the 500-byte plaintext for GetVer at
/// timestamp one. CBC chains left to right, so the first blocks equal the
/// wireless receiver's 504-byte variant; the padding block is where they
/// part.
const GETVER_TS1_HEAD: [u8; 32] = [
    0xf1, 0x32, 0xa5, 0xd4, 0xe3, 0xcf, 0xf8, 0x57, 0x48, 0xb8, 0x2a, 0xaa, 0xca, 0xc6, 0x8f, 0x8a,
    0xe7, 0x6b, 0xd5, 0x35, 0xd3, 0xe9, 0x53, 0x8a, 0x13, 0x0a, 0x7b, 0x11, 0x21, 0x9f, 0x36, 0x5c,
];
const GETVER_TS1_CIPHERTEXT_TAIL: [u8; 16] = [
    0x9d, 0x69, 0x87, 0xa8, 0x6a, 0x3d, 0x99, 0x54, 0xb1, 0x3e, 0xc4, 0x2b, 0xb4, 0x83, 0x67, 0x04,
];

/// OpenSSL known answer for PushJpg at timestamp 42 with a 123,456-byte
/// size parameter.
const PUSHJPG_TS42_HEAD: [u8; 16] = [
    0x6e, 0xdf, 0x63, 0xcc, 0x8e, 0x6c, 0x3f, 0xe4, 0x3f, 0x32, 0x42, 0x8b, 0x39, 0xcc, 0xe2, 0x2c,
];
const PUSHJPG_TS42_PADDING_BLOCK: [u8; 8] = [0x27, 0x38, 0x27, 0x32, 0xb8, 0x3e, 0xf6, 0x43];

#[test]
fn the_header_matches_an_independent_des_cbc_implementation() {
    let header = wrap_winusb_header(UniversalScreenCommand::GetVer as u8, 1, &[]);
    assert_eq!(header.len(), WINUSB_HEADER_LEN);
    assert_eq!(&header[..32], &GETVER_TS1_HEAD);
    assert_eq!(
        &header[WINUSB_CIPHERTEXT_LEN - 16..WINUSB_CIPHERTEXT_LEN],
        &GETVER_TS1_CIPHERTEXT_TAIL
    );

    let push = wrap_winusb_header(
        UniversalScreenCommand::PushJpg as u8,
        42,
        &123_456_u32.to_be_bytes(),
    );
    assert_eq!(&push[..16], &PUSHJPG_TS42_HEAD);
    assert_eq!(
        &push[WINUSB_CIPHERTEXT_LEN - 8..WINUSB_CIPHERTEXT_LEN],
        &PUSHJPG_TS42_PADDING_BLOCK
    );
}

#[test]
fn the_ciphertext_is_followed_by_six_zeros_and_the_trailer() {
    let header = wrap_winusb_header(0x7B, 7, &[]);
    assert_eq!(&header[WINUSB_CIPHERTEXT_LEN..510], &[0; 6]);
    assert_eq!(&header[510..], &WINUSB_TRAILER);
    assert_eq!(WINUSB_TRAILER, [0xA1, 0x1A]);
}

#[test]
fn the_plaintext_carries_command_magic_timestamp_and_big_endian_size() {
    let header = wrap_winusb_header(0x65, 0x0102_0304, &98_765_u32.to_be_bytes());
    let plain = plaintext(&header);
    assert_eq!(plain.len(), WINUSB_PLAINTEXT_LEN);
    assert_eq!(plain[0], 0x65);
    assert_eq!(plain[1], 0);
    assert_eq!(&plain[2..4], &[0x1A, 0x6D]);
    assert_eq!(&plain[4..8], &[0x04, 0x03, 0x02, 0x01], "little-endian ms");
    assert_eq!(&plain[8..12], &[0x00, 0x01, 0x81, 0xCD], "big-endian size");
    assert!(plain[12..].iter().all(|&byte| byte == 0));
}

#[test]
fn parameters_past_the_plaintext_are_dropped_not_overflowed() {
    let params = vec![0x5A; WINUSB_MAX_PARAMS_LEN + 9];
    let plain = plaintext(&wrap_winusb_header(0x66, 3, &params));
    assert_eq!(plain.len(), WINUSB_PLAINTEXT_LEN);
    assert!(plain[8..].iter().all(|&byte| byte == 0x5A));
}

#[test]
fn a_builder_never_repeats_a_timestamp() {
    let mut builder = WinUsbHeaderBuilder::new();
    let stamps: Vec<u32> = (0..4)
        .map(|_| timestamp(&plaintext(&builder.header(0x0A, &[]))))
        .collect();
    for pair in stamps.windows(2) {
        assert!(pair[1] > pair[0], "{} after {}", pair[1], pair[0]);
    }
}

// --- Clock (section 4) ---

#[test]
fn clock_parameters_are_a_big_endian_year_then_wall_clock_then_mode() {
    let at = NaiveDate::from_ymd_opt(2026, 10, 8)
        .and_then(|date| date.and_hms_opt(19, 5, 42))
        .expect("a valid instant");
    assert_eq!(clock_params(&at), [0x07, 0xEA, 10, 8, 19, 5, 42, 2]);
}

// --- Session (section 5) ---

#[test]
fn init_stops_playback_reads_firmware_settles_the_panel_and_clears_both_layers() {
    let protocol = UniversalScreenProtocol::new();
    let commands = protocol.init_sequence();
    let expected = [
        (UniversalScreenCommand::StopPlay, None),
        (UniversalScreenCommand::GetVer, None),
        (UniversalScreenCommand::FrameRate, Some(120)),
        (UniversalScreenCommand::Brightness, Some(100)),
        (UniversalScreenCommand::SetClock, None),
        (UniversalScreenCommand::StopClock, Some(0)),
        (UniversalScreenCommand::PushPng, None),
        (UniversalScreenCommand::PushJpg, None),
    ];
    assert_eq!(commands.len(), expected.len());

    let mut last_timestamp = 0;
    for (index, (command, (opcode, first_param))) in commands.iter().zip(expected).enumerate() {
        let what = format!("init command {index} ({opcode:?})");
        assert_status_read(command, Duration::from_secs(2), &what);
        let plain = plaintext(&command.data);
        assert_eq!(plain[0], opcode as u8, "{what} opcode");
        if let Some(param) = first_param {
            assert_eq!(plain[8], param, "{what} parameter");
        }
        let stamp = timestamp(&plain);
        assert!(stamp > last_timestamp || index == 0, "{what} timestamp");
        last_timestamp = stamp;
    }

    // Control commands are exactly one header.
    for command in &commands[..6] {
        assert_eq!(command.data.len(), WINUSB_HEADER_LEN);
    }
    assert_eq!(
        commands[0].post_delay,
        Duration::from_millis(150),
        "StopPlay lets playback wind down"
    );
    assert!(
        commands[1..]
            .iter()
            .all(|command| command.post_delay == Duration::ZERO),
        "nothing else waits"
    );
    let clock = plaintext(&commands[4].data);
    assert_eq!(clock[15], 2, "SetClock mode byte");
}

#[test]
fn the_init_layer_clears_are_portrait_images_sized_in_their_headers() {
    let protocol = UniversalScreenProtocol::new();
    let commands = protocol.init_sequence();

    for (command, format) in [
        (&commands[6], image::ImageFormat::Png),
        (&commands[7], image::ImageFormat::Jpeg),
    ] {
        let body = &command.data[WINUSB_HEADER_LEN..];
        assert_eq!(size_param(&plaintext(&command.data)), body.len());
        assert!(body.len() <= UNIVERSAL_SCREEN_MAX_JPEG_LEN);
        let decoded =
            image::load_from_memory_with_format(body, format).expect("a decodable clear image");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (UNIVERSAL_SCREEN_WIDTH, UNIVERSAL_SCREEN_HEIGHT),
            "{format:?} clear is 480x1920 portrait"
        );
    }

    let png = image::load_from_memory_with_format(
        &commands[6].data[WINUSB_HEADER_LEN..],
        image::ImageFormat::Png,
    )
    .expect("png")
    .to_rgba8();
    assert!(
        png.pixels().all(|pixel| pixel.0[3] == 0),
        "fully transparent"
    );
}

// --- Frames (section 5) ---

#[test]
fn a_frame_is_one_write_of_the_header_and_the_jpeg_at_its_natural_length() {
    let protocol = UniversalScreenProtocol::new();
    let jpeg: Vec<u8> = (0..30_011_u32).map(|i| (i % 251) as u8).collect();
    let mut commands = Vec::new();

    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&jpeg), &mut commands)
        .expect("a 30 KB JPEG fits");

    assert_eq!(commands.len(), 1);
    let frame = &commands[0];
    assert_eq!(
        frame.data.len(),
        WINUSB_HEADER_LEN + jpeg.len(),
        "no padding"
    );
    assert_eq!(
        &frame.data[WINUSB_HEADER_LEN..],
        &jpeg[..],
        "payload verbatim"
    );
    assert_eq!(&frame.data[510..WINUSB_HEADER_LEN], &WINUSB_TRAILER);
    let plain = plaintext(&frame.data);
    assert_eq!(plain[0], UniversalScreenCommand::PushJpg as u8);
    assert_eq!(size_param(&plain), jpeg.len());
    assert_status_read(frame, Duration::from_millis(200), "frame");
}

#[test]
fn packet_aligned_frames_get_no_extra_bytes() {
    let protocol = UniversalScreenProtocol::new();
    let jpeg = vec![0xC3; 512 * 99];
    let mut commands = Vec::new();
    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&jpeg), &mut commands)
        .expect("fits");
    assert_eq!(commands[0].data.len(), 512 * 100);
}

#[test]
fn a_jpeg_at_the_cap_fits_and_one_byte_more_is_refused() {
    let protocol = UniversalScreenProtocol::new();
    let mut commands = Vec::new();

    let exact = vec![0xAB; UNIVERSAL_SCREEN_MAX_JPEG_LEN];
    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&exact), &mut commands)
        .expect("exactly the cap fits");
    assert_eq!(
        commands[0].data.len(),
        WINUSB_HEADER_LEN + UNIVERSAL_SCREEN_MAX_JPEG_LEN
    );

    let over = vec![0xAB; UNIVERSAL_SCREEN_MAX_JPEG_LEN + 1];
    let mut refused = Vec::new();
    let error = protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(&over), &mut refused)
        .expect_err("512,001 bytes exceed the frame cap");
    assert!(
        matches!(
            error,
            DisplayEncodeError::PayloadTooLarge {
                actual: 512_001,
                capacity: 512_000
            }
        ),
        "unexpected error: {error}"
    );
    assert!(refused.is_empty());
}

#[test]
fn the_command_buffer_is_reused_across_frames() {
    let protocol = UniversalScreenProtocol::new();
    let mut commands = Vec::new();
    for len in [40_000, 12_000, 25_000] {
        let jpeg = vec![0x11; len];
        protocol
            .encode_display_payload_into(DisplayFramePayload::jpeg(&jpeg), &mut commands)
            .expect("fits");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].data.len(), WINUSB_HEADER_LEN + len);
    }
}

#[test]
fn raw_rgb_is_not_a_format_the_panel_takes() {
    let protocol = UniversalScreenProtocol::new();
    let pixels = vec![0; 480 * 1920 * 3];
    let error = protocol
        .encode_display_payload_into(
            DisplayFramePayload {
                format: DisplayFrameFormat::Rgb,
                width: 480,
                height: 1920,
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

// --- Replies (section 5) ---

#[test]
fn a_getver_reply_yields_the_firmware_and_garbage_does_not() {
    let protocol = UniversalScreenProtocol::new();
    let mut reply = vec![0_u8; 512];
    reply[0] = UniversalScreenCommand::GetVer as u8;
    reply[1] = 0xC8;
    reply[8..26].copy_from_slice(b"lianli88_0001_0018");
    protocol.parse_response(&reply).expect("never an error");
    assert_eq!(protocol.firmware().as_deref(), Some("lianli88_0001_0018"));

    let mut garbage = vec![0_u8; 64];
    garbage[0] = UniversalScreenCommand::GetVer as u8;
    garbage[8..12].copy_from_slice(&[0x01, 0xFF, 0x80, 0x7F]);
    protocol.parse_response(&garbage).expect("tolerated");
    assert_eq!(protocol.firmware().as_deref(), Some("lianli88_0001_0018"));

    protocol.init_sequence();
    assert_eq!(protocol.firmware(), None, "a new session forgets it");
}

#[test]
fn a_frame_acknowledgement_reports_the_buffered_frame_count() {
    let protocol = UniversalScreenProtocol::new();
    let mut ack = vec![0_u8; 16];
    ack[0] = UniversalScreenCommand::PushJpg as u8;
    ack[1] = 0xC8;
    ack[8] = 5;
    protocol.parse_response(&ack).expect("tolerated");
    assert_eq!(protocol.last_buffer_level(), 5);

    let rejected = [0x7B, 0x01];
    let response = protocol.parse_response(&rejected).expect("tolerated");
    assert_eq!(response.data, rejected);
    assert_eq!(protocol.last_buffer_level(), 5, "other replies leave it");
    protocol
        .parse_response(&[])
        .expect("an empty read is tolerated");
}

// --- Topology and registration (sections 2 and 6) ---

#[test]
fn the_panel_is_one_portrait_480x1920_jpeg_display_without_leds() {
    let protocol = UniversalScreenProtocol::new();
    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].led_count, 0);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Display {
            width: 480,
            height: 1920,
            circular: false,
            format: DisplayFrameFormat::Jpeg,
        }
    );
    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.max_fps, 30);
    assert!(!capabilities.supports_direct);
    assert_eq!(
        capabilities.features.max_display_frame_len,
        Some(UNIVERSAL_SCREEN_MAX_JPEG_LEN),
        "the daemon's encoder must fit this wire cap"
    );
    assert_eq!(protocol.total_leds(), 0);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
    assert!(protocol.shutdown_sequence().is_empty());

    let mut commands = vec![ProtocolCommand::default()];
    protocol.encode_frame_into(&[[1, 2, 3]; 60], &mut commands);
    assert!(commands.is_empty(), "the ring is not this device");
}

#[test]
fn the_panel_is_registered_once_on_bulk_interface_zero() {
    let descriptor = ProtocolDatabase::lookup(UNIVERSAL_SCREEN_VENDOR_ID, PID_UNIVERSAL_SCREEN_88)
        .expect("registered");
    assert_eq!(descriptor.name, "Lian Li Universal Screen 8.8\"");
    assert_eq!(descriptor.protocol.id, "lianli/universal-screen");
    assert_eq!(descriptor.driver_id(), "lianli");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbBulk {
            interface: 0,
            report_id: 0
        }
    );
    assert!(descriptor.firmware_predicate.is_none());

    let claims = ProtocolDatabase::all()
        .iter()
        .filter(|candidate| candidate.vendor_id == 0x1CBE && candidate.product_id == 0xA088)
        .count();
    assert_eq!(claims, 1, "one descriptor claims 1CBE:A088");
}
