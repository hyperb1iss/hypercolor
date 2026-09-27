use std::io::Cursor;
use std::time::Duration;

use hypercolor_hal::drivers::push2::{Push2Protocol, build_push2_protocol};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus, TransferType};
use hypercolor_types::device::{
    DeviceColorFormat, DeviceTopologyHint, DisplayFrameFormat, DisplayFramePayload,
};
use image::{ColorType, ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};

/// Drive the one display seam with a JPEG payload, mapping failure to `None`
/// so the assertions below read as they did against the JPEG-only hook.
fn display_commands<P: Protocol + ?Sized>(
    protocol: &P,
    jpeg: &[u8],
) -> Option<Vec<ProtocolCommand>> {
    let mut commands = Vec::new();
    encode_into(protocol, jpeg, &mut commands).map(|()| commands)
}

fn encode_into<P: Protocol + ?Sized>(
    protocol: &P,
    jpeg: &[u8],
    commands: &mut Vec<ProtocolCommand>,
) -> Option<()> {
    protocol
        .encode_display_payload_into(DisplayFramePayload::jpeg(jpeg), commands)
        .ok()
}

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

fn count_palette_writes(commands: &[ProtocolCommand]) -> usize {
    commands
        .iter()
        .filter(|command| command.data.len() == 17 && command.data.get(6) == Some(&0x03))
        .count()
}

fn unique_pad_colors() -> Vec<[u8; 3]> {
    let mut colors = vec![[0_u8; 3]; 160];
    for (index, color) in colors.iter_mut().enumerate().take(64) {
        *color = [
            u8::try_from(index).expect("pad index fits in u8") + 1,
            40,
            200,
        ];
    }
    colors
}

#[test]
fn push2_palette_writes_keep_to_their_share_of_a_batch() {
    let protocol = Push2Protocol::new();

    let commands = protocol.encode_frame(&unique_pad_colors());

    // Palette writes plus their Reapply take at most two fifths of the
    // 512-byte USB-MIDI packet: 8 writes (24 wire bytes each) and the
    // Reapply (12), leaving the rest of the batch for LED moves.
    assert_eq!(count_palette_writes(&commands), 8);
    let notes = commands
        .iter()
        .filter(|command| command.data.len() == 3)
        .count();
    assert!(
        notes >= 48,
        "LED moves still get most of the batch, got {notes}"
    );
}

#[test]
fn push2_budget_fallback_maps_to_nearest_existing_entry() {
    let protocol = Push2Protocol::new();
    protocol
        .parse_response(&palette_reply(90, [255, 0, 0, 54]))
        .expect("palette reply should parse");

    let mut colors = vec![[0_u8; 3]; 160];
    for (index, color) in colors.iter_mut().enumerate().take(16) {
        *color = [
            40,
            u8::try_from(index).expect("pad index fits in u8") + 1,
            200,
        ];
    }
    colors[16] = [250, 0, 0];

    let commands = protocol.encode_frame(&colors);

    assert_eq!(count_palette_writes(&commands), 8);
    assert!(
        commands
            .iter()
            .any(|command| command.data == vec![0x90, 52, 90]),
        "near-red pad should borrow the seeded red entry in slot 90, not a write"
    );
}

#[test]
fn push2_white_button_palette_writes_go_ahead_of_rgb_writes() {
    let protocol = Push2Protocol::new();
    let mut colors = unique_pad_colors();
    colors[92] = [255, 255, 255];

    let commands = protocol.encode_frame(&colors);

    let palette_writes: Vec<_> = commands
        .iter()
        .filter(|command| command.data.len() == 17 && command.data.get(6) == Some(&0x03))
        .collect();
    // White levels are 31 fixed slots written once each, so they go first
    // and an RGB-heavy frame can never leave a white button unlit.
    assert_eq!(palette_writes.len(), 8);
    assert!(
        palette_writes[0].data[7] >= 97,
        "the white level is written first"
    );
    assert!(
        palette_writes[1..]
            .iter()
            .all(|command| command.data[7] < 97)
    );
    assert!(
        commands
            .iter()
            .any(|command| command.data == vec![0xB0, 28, 0x7F]),
        "the white button is lit in the same batch"
    );
}

#[test]
fn push2_init_sequence_reads_palette_and_clears_zones() {
    let protocol = build_push2_protocol();
    let commands = protocol.init_sequence();

    assert_eq!(commands.len(), 263);
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
    // The clear is acknowledged one endpoint packet at a time: 125 four-byte
    // USB-MIDI events fill 500 bytes, then a palette read waits for the
    // device before the rest goes out.
    let ack_read = vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 0x00, 0xF7];
    assert_eq!(commands[256].data, ack_read);
    assert!(commands[256].expects_response);
    assert_eq!(commands[260].data, vec![0xB0, 60, 0x00]);
    assert_eq!(
        commands[261].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF7
        ]
    );
    assert_eq!(commands[262].data, ack_read);
    assert!(commands[262].expects_response);
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
    // The touch strip goes first: one message covers 31 LEDs.
    assert_eq!(
        commands[0].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x19, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF7
        ]
    );
    // White levels ahead of RGB entries, then one Reapply.
    assert_eq!(
        commands[1].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x7F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x7F, 0x01, 0xF7
        ]
    );
    assert_eq!(
        commands[2].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x01, 0x7F, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x36, 0x00, 0xF7
        ]
    );
    assert_eq!(
        commands[3].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x02, 0x00, 0x00, 0x7F, 0x01, 0x00, 0x00,
            0x36, 0x01, 0xF7
        ]
    );
    assert_eq!(
        commands[4].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert_eq!(commands[5].data, vec![0x90, 36, 0x01]);
    assert_eq!(commands[6].data, vec![0x90, 37, 0x01]);
    assert_eq!(commands[7].data, vec![0xB0, 102, 0x02]);
    assert_eq!(commands[8].data, vec![0xB0, 28, 0x7F]);

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
fn push2_frame_encoding_moves_leds_onto_slots_that_already_hold_their_color() {
    let protocol = Push2Protocol::new();
    let mut first_frame = vec![[0_u8, 0_u8, 0_u8]; 160];
    first_frame[0] = [255, 0, 0];
    first_frame[1] = [0, 255, 0];
    let _ = protocol.encode_frame(&first_frame);

    let mut second_frame = vec![[0_u8, 0_u8, 0_u8]; 160];
    second_frame[0] = [0, 255, 0];
    second_frame[1] = [0, 0, 255];

    // Pad 0 takes green from pad 1's slot with a note instead of rewriting
    // its own slot, so only blue costs a palette write: one sysex instead of
    // two, and fewer wire bytes (24 + 12 + 2 x 4 against 2 x 24 + 12).
    let commands = protocol.encode_frame(&second_frame);
    assert_eq!(commands.len(), 4);
    assert_eq!(count_palette_writes(&commands), 1);
    assert_eq!(
        commands[0].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x03, 0x00, 0x00, 0x00, 0x00, 0x7F, 0x01,
            0x12, 0x00, 0xF7
        ]
    );
    assert_eq!(
        commands[1].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert_eq!(commands[2].data, vec![0x90, 36, 0x02]);
    assert_eq!(commands[3].data, vec![0x90, 37, 0x03]);
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
    let ack_read = vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 0x00, 0xF7];
    // The frame above left 40 wire bytes unconfirmed, so the first chunk of
    // the clear closes that much earlier than it would from a quiet lane.
    assert_eq!(shutdown.len(), 136);
    assert_eq!(shutdown[0].data, vec![0x90, 36, 0x00]);
    assert_eq!(shutdown[115].data, ack_read);
    assert!(shutdown[115].expects_response);
    assert_eq!(shutdown[130].data.len(), 24);
    assert_eq!(
        shutdown[131].data,
        vec![
            0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00, 0x7F, 0x01,
            0x12, 0x00, 0xF7
        ]
    );
    assert_eq!(
        shutdown[132].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
    );
    assert!(shutdown[133].expects_response);
    assert_eq!(
        shutdown[133].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x00, 0xF7]
    );
    assert_eq!(
        shutdown[134].data,
        vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x17, 0x68, 0xF7]
    );
    assert_eq!(shutdown[135].data, ack_read);
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
fn push2_has_no_timer_keepalive_outside_the_led_stream() {
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
        "steady-state frame should be diff-suppressed"
    );

    // User mode is re-asserted inside the acknowledged LED stream instead,
    // where a stalled endpoint is a recoverable hold, not an actor failure.
    assert!(protocol.keepalive().is_none());
    assert!(protocol.keepalive_commands().is_empty());
}

#[test]
fn push2_display_encoding_emits_header_and_bulk_packets() {
    let protocol = Push2Protocol::new();
    let commands =
        display_commands(&protocol, &solid_red_jpeg()).expect("display frames should be supported");

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
            format: DisplayFrameFormat::Rgb,
        }
    );

    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 160);
    assert!(capabilities.supports_direct);
    assert!(capabilities.supports_brightness);
    assert!(capabilities.has_display);
    assert_eq!(capabilities.display_resolution, Some((960, 160)));
    assert_eq!(capabilities.max_fps, 60);
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

#[test]
fn push2_palette_writes_go_to_the_worst_approximations_first() {
    let protocol = Push2Protocol::new();
    protocol
        .parse_response(&palette_reply(90, [250, 0, 0, 53]))
        .expect("palette reply should parse");

    // Pad 0 wants red, within a few steps of the seeded slot 90; nine blues
    // after it have nothing close. Only eight writes fit a batch, and the
    // scan starts at pad 0, so first-come order would spend one on red.
    let mut colors = vec![[0_u8; 3]; 160];
    colors[0] = [255, 0, 0];
    for (index, color) in colors.iter_mut().enumerate().skip(1).take(9) {
        *color = [0, u8::try_from(index * 20).expect("fits in u8"), 200];
    }

    let commands = protocol.encode_frame(&colors);

    assert_eq!(count_palette_writes(&commands), 8);
    assert!(
        commands
            .iter()
            .filter(|command| command.data.len() == 17)
            .all(|command| command.data[8..10] != [0x7F, 0x01]),
        "no write should be spent on red while far-off colors wait"
    );
    assert!(
        commands
            .iter()
            .any(|command| command.data == vec![0x90, 36, 90]),
        "red borrows the seeded near-red slot"
    );
}

#[test]
fn push2_reply_for_a_different_palette_index_acknowledges_nothing() {
    let protocol = Push2Protocol::new();
    let colors = unique_pad_colors();
    let batch = protocol.encode_frame(&colors);
    let ack = batch
        .last()
        .filter(|command| command.data.len() == 9 && command.data[6] == 0x04)
        .map(|command| command.data[7])
        .expect("a full batch closes with a palette read");

    // A stale reply for another index must not release the next batch.
    protocol
        .parse_response(&palette_reply((ack + 1) % 128, [1, 2, 3, 4]))
        .expect("stale palette reply still parses");
    let held = protocol.encode_frame(&colors);
    assert_eq!(held.len(), 1, "the lane holds and sends only a probe");
    assert_eq!(held[0].data[6], 0x04);

    // The reply to the probe releases it.
    protocol
        .parse_response(&palette_reply(held[0].data[7], [0, 0, 0, 0]))
        .expect("probe reply parses");
    let next = protocol.encode_frame(&colors);
    assert!(
        next.len() > 1,
        "LED traffic resumes once the probe is answered"
    );
}
