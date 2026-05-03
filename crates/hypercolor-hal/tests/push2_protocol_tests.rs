use std::io::Cursor;
use std::time::Duration;

use hypercolor_hal::drivers::push2::{Push2Protocol, build_push2_protocol};
use hypercolor_hal::protocol::{Protocol, ResponseStatus, TransferType};
use hypercolor_types::device::{DeviceColorFormat, DeviceTopologyHint};
use image::{ColorType, ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};

fn palette_reply(index: u8, rgba: [u8; 4]) -> Vec<u8> {
    let mut response = vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, index];
    for value in rgba {
        response.push(value & 0x7F);
        response.push((value >> 7) & 0x01);
    }
    response.push(0xF7);
    response
}

fn solid_red_jpeg() -> Vec<u8> {
    let image = RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0]));
    let mut bytes = Vec::new();
    JpegEncoder::new(&mut Cursor::new(&mut bytes))
        .write_image(image.as_raw(), 4, 4, ColorType::Rgb8.into())
        .expect("JPEG encoding should succeed");
    bytes
}

#[test]
fn push2_init_sequence_reads_palette_and_clears_zones() {
    let protocol = build_push2_protocol();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 261);
    assert!(commands[0].expects_response);
    assert_eq!(commands[0].data, vec![0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7]);
    assert_eq!(commands[1].transfer_type, TransferType::Primary);
    assert!(commands[1].expects_response);
    assert_eq!(
        commands[1].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]
    );
    assert_eq!(
        commands[2].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x17, 0x6B, 0xF7]
    );
    assert!(commands[3].expects_response);
    assert_eq!(
        commands[3].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 0x00, 0xF7]
    );
    assert_eq!(
        commands[130].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 0x7F, 0xF7]
    );
    assert_eq!(commands[131].data, vec![0x90, 36, 0x00]);
    assert_eq!(commands[194].data, vec![0x90, 99, 0x00]);
    assert_eq!(commands[195].data, vec![0xB0, 102, 0x00]);
    assert_eq!(commands[222].data, vec![0xB0, 9, 0x00]);
    assert_eq!(commands[223].data, vec![0xB0, 28, 0x00]);
    assert_eq!(commands[259].data, vec![0xB0, 60, 0x00]);
    assert_eq!(
        commands[260].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF7
        ]
    );
}

#[test]
fn push2_frame_encoding_deduplicates_palette_and_tracks_diff() {
    let protocol = Push2Protocol::new();
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 160];
    colors[0] = [255, 0, 0];
    colors[1] = [255, 0, 0];
    colors[64] = [0, 255, 0];
    colors[92] = [255, 255, 255];
    colors[129] = [255, 255, 255];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 9);
    assert_eq!(
        commands[0].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x01, 0x7F, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x36, 0x00, 0xF7
        ]
    );
    assert_eq!(
        commands[1].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x02, 0x00, 0x00, 0x7F, 0x01, 0x00, 0x00,
            0x36, 0x01, 0xF7
        ]
    );
    assert_eq!(
        commands[2].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x7F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x7F, 0x01, 0xF7
        ]
    );
    assert_eq!(
        commands[3].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert_eq!(commands[4].data, vec![0x90, 36, 0x01]);
    assert_eq!(commands[5].data, vec![0x90, 37, 0x01]);
    assert_eq!(commands[6].data, vec![0xB0, 102, 0x02]);
    assert_eq!(commands[7].data, vec![0xB0, 28, 0x7F]);
    assert_eq!(
        commands[8].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x19, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF7
        ]
    );

    let steady_state = protocol.encode_frame(&colors);
    assert!(steady_state.is_empty());
}

#[test]
fn push2_frame_encoding_uses_spare_slot_when_splitting_a_shared_color() {
    let protocol = Push2Protocol::new();
    let mut first_frame = vec![[0_u8, 0_u8, 0_u8]; 160];
    first_frame[0] = [255, 0, 0];
    first_frame[1] = [255, 0, 0];
    let _ = protocol.encode_frame(&first_frame);

    let mut second_frame = vec![[0_u8, 0_u8, 0_u8]; 160];
    second_frame[0] = [0, 255, 0];
    second_frame[1] = [255, 0, 0];

    let commands = protocol.encode_frame(&second_frame);
    assert_eq!(commands.len(), 3);
    assert_eq!(
        commands[0].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x02, 0x00, 0x00, 0x7F, 0x01, 0x00, 0x00,
            0x36, 0x01, 0xF7
        ]
    );
    assert_eq!(
        commands[1].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert_eq!(commands[2].data, vec![0x90, 36, 0x02]);
}

#[test]
fn push2_frame_encoding_keeps_unique_leds_on_their_existing_slots() {
    let protocol = Push2Protocol::new();
    let mut first_frame = vec![[0_u8, 0_u8, 0_u8]; 160];
    first_frame[0] = [255, 0, 0];
    first_frame[1] = [0, 255, 0];
    let _ = protocol.encode_frame(&first_frame);

    let mut second_frame = vec![[0_u8, 0_u8, 0_u8]; 160];
    second_frame[0] = [0, 255, 0];
    second_frame[1] = [0, 0, 255];

    let commands = protocol.encode_frame(&second_frame);
    assert_eq!(commands.len(), 3);
    assert_eq!(
        commands[0].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x01, 0x00, 0x00, 0x7F, 0x01, 0x00, 0x00,
            0x36, 0x01, 0xF7
        ]
    );
    assert_eq!(
        commands[1].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x7F, 0x01,
            0x12, 0x00, 0xF7
        ]
    );
    assert_eq!(
        commands[2].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert!(
        commands
            .iter()
            .all(|command| command.data.first() == Some(&0xF0)),
        "slot-stable color changes should not require per-key remap messages"
    );
}

#[test]
fn push2_shutdown_restores_cached_factory_palette() {
    let protocol = Push2Protocol::new();
    protocol
        .parse_response(&palette_reply(1, [0, 0, 255, 18]))
        .expect("palette reply should parse");

    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 160];
    colors[0] = [255, 0, 0];
    let _ = protocol.encode_frame(&colors);

    let shutdown = protocol.shutdown_sequence();
    assert_eq!(shutdown.len(), 134);
    assert_eq!(shutdown[0].data, vec![0x90, 36, 0x00]);
    assert_eq!(shutdown[129].data.len(), 24);
    assert_eq!(
        shutdown[130].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00, 0x7F, 0x01,
            0x12, 0x00, 0xF7
        ]
    );
    assert_eq!(
        shutdown[131].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert!(shutdown[132].expects_response);
    assert_eq!(
        shutdown[132].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x00, 0xF7]
    );
    assert_eq!(
        shutdown[133].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x17, 0x68, 0xF7]
    );
}

#[test]
fn push2_white_buttons_quantize_nonzero_brightness_to_lit_slots() {
    let protocol = Push2Protocol::new();
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 160];
    colors[92] = [1, 1, 1];

    let commands = protocol.encode_frame(&colors);
    let white_button_write = commands
        .iter()
        .find(|command| command.data.first() == Some(&0xB0) && command.data.get(1) == Some(&28))
        .expect("white button CC write should be emitted");

    assert!(
        white_button_write.data[2] > 0,
        "non-black white button colors should not quantize to off"
    );
}

#[test]
fn push2_brightness_and_diagnostics_use_primary_sysex() {
    let protocol = Push2Protocol::new();

    let brightness = protocol
        .encode_brightness(128)
        .expect("brightness should be supported");
    assert_eq!(brightness.len(), 2);
    assert_eq!(brightness[0].transfer_type, TransferType::Primary);
    assert_eq!(
        brightness[0].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x06, 0x40, 0xF7]
    );
    assert_eq!(
        brightness[1].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x08, 0x00, 0x01, 0xF7]
    );

    let diagnostics = protocol.connection_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].expects_response);
    assert_eq!(
        diagnostics[0].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x1A, 0xF7]
    );
}

#[test]
fn push2_keepalive_reasserts_user_mode_and_resends_last_led_frame() {
    let protocol = Push2Protocol::new();
    let mut colors = vec![[0_u8, 0_u8, 0_u8]; 160];
    colors[0] = [255, 0, 0];

    let first_frame = protocol.encode_frame(&colors);
    assert!(
        first_frame
            .iter()
            .any(|command| command.data == vec![0x90, 36, 0x01]),
        "first frame should light pad 0 from palette slot 1"
    );

    assert!(
        protocol.encode_frame(&colors).is_empty(),
        "steady-state frame should normally be diff-suppressed"
    );

    let keepalive = protocol
        .keepalive()
        .expect("Push 2 should run a MIDI resync keepalive");
    assert_eq!(keepalive.interval, Duration::from_secs(5));

    let resync = protocol.keepalive_commands();
    assert_eq!(
        resync[0].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]
    );
    assert_eq!(
        resync[1].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x17, 0x6B, 0xF7]
    );
    assert!(
        resync.iter().any(
            |command| command.data.first() == Some(&0xF0) && command.data.get(6) == Some(&0x03)
        ),
        "resync should force palette writes even when the software cache is already warm"
    );
    assert!(
        resync
            .iter()
            .any(|command| command.data == vec![0x90, 36, 0x01]),
        "resync should resend key mappings from the last frame"
    );
}

#[test]
fn push2_display_encoding_emits_header_and_bulk_packets() {
    let protocol = Push2Protocol::new();
    let commands = protocol
        .encode_display_frame(&solid_red_jpeg())
        .expect("display frames should be supported");

    assert_eq!(commands.len(), 21);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_eq!(commands[0].data.len(), 16);
    assert_eq!(
        commands[0].data,
        vec![
            0xFF, 0xCC, 0xAA, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00
        ]
    );
    assert_eq!(commands[1].transfer_type, TransferType::Bulk);
    assert_eq!(commands[1].data.len(), 16 * 1024);
    assert_eq!(&commands[1].data[..4], &[0xF8, 0xF3, 0xF8, 0xFF]);
}

#[test]
fn push2_parse_response_accepts_identity_reply_and_reports_capabilities() {
    let protocol = Push2Protocol::new();
    let parsed = protocol
        .parse_response(&[
            0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x21, 0x1D, 0x67, 0x32, 0x02, 0x00, 0x01, 0x00,
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF7,
        ])
        .expect("identity reply should parse");

    assert_eq!(parsed.status, ResponseStatus::Ok);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 8);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Matrix { rows: 8, cols: 8 }
    );
    assert_eq!(zones[5].led_count, 37);
    assert_eq!(zones[6].led_count, 31);
    assert_eq!(zones[7].color_format, DeviceColorFormat::Rgb);
    assert_eq!(
        zones[7].topology,
        DeviceTopologyHint::Display {
            width: 960,
            height: 160,
            circular: false,
        }
    );

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 160);
    assert!(capabilities.supports_direct);
    assert!(capabilities.supports_brightness);
    assert!(capabilities.has_display);
    assert_eq!(capabilities.display_resolution, Some((960, 160)));
    assert_eq!(protocol.total_leds(), 160);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(16));
}

#[test]
fn push2_parse_response_rejects_out_of_range_palette_index() {
    let protocol = Push2Protocol::new();
    let response = vec![
        0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xF7,
    ];

    let error = protocol
        .parse_response(&response)
        .expect_err("invalid palette index should be rejected");

    assert!(
        error
            .to_string()
            .contains("palette reply index out of range"),
        "unexpected error: {error}"
    );
}
