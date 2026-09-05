//! Wired Uni Fan TL LCD wire format, against the spec 80 section 5 tables.

use std::time::Duration;

use hypercolor_hal::display::DisplayEncodeError;
use hypercolor_hal::drivers::lianli::{
    TL_LCD_HEADER_LEN, TL_LCD_MAX_PAYLOAD, TL_LCD_PACKET_LEN, TL_LCD_REPORT_ID, TlLcdCommand,
    TlLcdMode, TlLcdProtocol,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand};
use hypercolor_types::device::{
    DeviceColorFormat, DeviceTopologyHint, DisplayFrameFormat, DisplayFramePayload,
};

const INIT_TIMEOUT: Duration = Duration::from_secs(3);
const STEADY_TIMEOUT: Duration = Duration::from_millis(200);

/// Header fields as the spec lays them out, read back from a packet.
struct Header {
    report_id: u8,
    command: u8,
    total_size: u32,
    packet_number: u32,
    payload_len: u16,
}

fn header_of(packet: &[u8]) -> Header {
    assert_eq!(packet.len(), TL_LCD_PACKET_LEN, "packets are fixed size");
    Header {
        report_id: packet[0],
        command: packet[1],
        total_size: u32::from_be_bytes([packet[2], packet[3], packet[4], packet[5]]),
        packet_number: u32::from_be_bytes([0, packet[6], packet[7], packet[8]]),
        payload_len: u16::from_be_bytes([packet[9], packet[10]]),
    }
}

fn jpeg(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from(index % 251).unwrap_or_default())
        .collect()
}

fn display_commands(protocol: &TlLcdProtocol, data: &[u8]) -> Vec<ProtocolCommand> {
    let mut commands = Vec::new();
    protocol
        .encode_display_payload_into(
            DisplayFramePayload {
                format: DisplayFrameFormat::Jpeg,
                width: 400,
                height: 400,
                data,
            },
            &mut commands,
        )
        .expect("a JPEG payload should encode");
    commands
}

// --- Packet header (section 5.2) ---

#[test]
fn every_chunk_repeats_the_transfer_size_and_counts_up_from_zero() {
    let protocol = TlLcdProtocol::new();
    let frame = jpeg(TL_LCD_MAX_PAYLOAD * 3 + 17);

    let commands = display_commands(&protocol, &frame);

    assert_eq!(commands.len(), 4);
    for (index, command) in commands.iter().enumerate() {
        let header = header_of(&command.data);
        assert_eq!(header.report_id, TL_LCD_REPORT_ID, "packet {index}");
        assert_eq!(
            header.command,
            TlLcdCommand::WriteSyncJpg as u8,
            "the live path streams WriteSyncJpg"
        );
        assert_eq!(
            header.total_size,
            u32::try_from(frame.len()).expect("frame length should fit in u32"),
            "the full transfer size repeats in packet {index}"
        );
        assert_eq!(
            header.packet_number,
            u32::try_from(index).expect("chunk index should fit"),
            "counter for packet {index}"
        );
    }

    let last = header_of(&commands[3].data);
    assert_eq!(
        last.payload_len, 17,
        "the final packet declares its own len"
    );
}

#[test]
fn the_packet_counter_resets_for_every_transfer() {
    let protocol = TlLcdProtocol::new();
    let frame = jpeg(TL_LCD_MAX_PAYLOAD * 2);

    let first = display_commands(&protocol, &frame);
    let second = display_commands(&protocol, &frame);

    assert_eq!(header_of(&first[0].data).packet_number, 0);
    assert_eq!(
        header_of(&second[0].data).packet_number,
        0,
        "a new transfer restarts the counter rather than continuing it"
    );
    assert_eq!(header_of(&second[1].data).packet_number, 1);
}

#[test]
fn payload_length_is_declared_per_packet_and_the_rest_is_zero_padded() {
    let protocol = TlLcdProtocol::new();
    let frame = jpeg(20);

    let commands = display_commands(&protocol, &frame);

    assert_eq!(commands.len(), 1);
    let header = header_of(&commands[0].data);
    assert_eq!(header.payload_len, 20);
    assert_eq!(header.total_size, 20);
    assert_eq!(
        &commands[0].data[TL_LCD_HEADER_LEN..TL_LCD_HEADER_LEN + 20],
        frame.as_slice()
    );
    assert!(
        commands[0].data[TL_LCD_HEADER_LEN + 20..]
            .iter()
            .all(|&byte| byte == 0),
        "the tail of the packet is zero-padded"
    );
}

// --- Chunk boundaries ---

#[test]
fn a_frame_of_exactly_one_payload_is_a_single_packet() {
    let protocol = TlLcdProtocol::new();

    let commands = display_commands(&protocol, &jpeg(TL_LCD_MAX_PAYLOAD));

    assert_eq!(TL_LCD_MAX_PAYLOAD, 501, "section 5.2 caps payloads at 501");
    assert_eq!(commands.len(), 1);
    assert_eq!(header_of(&commands[0].data).payload_len, 501);
}

#[test]
fn one_byte_past_the_payload_boundary_becomes_two_packets() {
    let protocol = TlLcdProtocol::new();

    let commands = display_commands(&protocol, &jpeg(TL_LCD_MAX_PAYLOAD + 1));

    assert_eq!(commands.len(), 2);
    assert_eq!(header_of(&commands[0].data).payload_len, 501);
    assert_eq!(header_of(&commands[1].data).payload_len, 1);
    assert_eq!(header_of(&commands[1].data).total_size, 502);
}

#[test]
fn a_streamed_frame_is_reassembled_byte_for_byte() {
    let protocol = TlLcdProtocol::new();
    let frame = jpeg(TL_LCD_MAX_PAYLOAD * 2 + 300);

    let commands = display_commands(&protocol, &frame);

    let mut reassembled = Vec::new();
    for command in &commands {
        let declared = usize::from(header_of(&command.data).payload_len);
        reassembled
            .extend_from_slice(&command.data[TL_LCD_HEADER_LEN..TL_LCD_HEADER_LEN + declared]);
    }
    assert_eq!(reassembled, frame);
}

// --- Command policy (section 5.3) ---

#[test]
fn streamed_frames_are_unacknowledged() {
    let protocol = TlLcdProtocol::new();

    let commands = display_commands(&protocol, &jpeg(TL_LCD_MAX_PAYLOAD * 2));

    assert!(
        commands.iter().all(|command| !command.expects_response),
        "WriteSyncJpg takes no acks; one per chunk would halve the frame rate"
    );
}

#[test]
fn a_raw_rgb_payload_is_refused() {
    let protocol = TlLcdProtocol::new();
    let mut commands = Vec::new();

    let encoded = protocol.encode_display_payload_into(
        DisplayFramePayload {
            format: DisplayFrameFormat::Rgb,
            width: 400,
            height: 400,
            data: &[0xFF; 64],
        },
        &mut commands,
    );

    assert!(
        matches!(
            encoded,
            Err(DisplayEncodeError::Unsupported {
                format: DisplayFrameFormat::Rgb
            })
        ),
        "the panel takes JPEG frames only: {encoded:?}"
    );
}

// --- Init sequence (section 5.6) ---

#[test]
fn the_init_sequence_runs_in_the_documented_order_at_the_init_timeout() {
    let protocol = TlLcdProtocol::new();

    let commands = protocol.init_sequence();

    let opcodes: Vec<u8> = commands
        .iter()
        .map(|command| header_of(&command.data).command)
        .collect();
    assert_eq!(
        opcodes,
        vec![
            TlLcdCommand::ReadSerial as u8,
            TlLcdCommand::GetHandshake as u8,
            TlLcdCommand::GetProductInfo as u8,
            TlLcdCommand::LcdControl as u8,
        ],
        "identity, handshake, firmware, then panel settings"
    );

    for (index, command) in commands.iter().enumerate() {
        assert!(
            command.expects_response,
            "init command {index} reads a reply"
        );
        assert_eq!(
            command.response.timeout,
            Some(INIT_TIMEOUT),
            "init command {index} uses the 3000ms init budget"
        );
    }

    assert_eq!(
        commands[2].response.count, 2,
        "GetProductInfo answers with a version report and a build-date report"
    );
    assert_eq!(
        protocol.response_timeout(),
        STEADY_TIMEOUT,
        "steady-state reads stay at 200ms"
    );
}

#[test]
fn the_init_control_command_sets_full_brightness_thirty_fps_and_no_rotation() {
    let protocol = TlLcdProtocol::new();

    let commands = protocol.init_sequence();
    let payload = &commands[3].data[TL_LCD_HEADER_LEN..];

    assert_eq!(payload[0], TlLcdMode::LcdSetting as u8, "mode at offset 0");
    assert_eq!(&payload[1..4], &[0, 0, 0], "reserved bytes 1-3");
    assert_eq!(payload[4], 100, "brightness at offset 4");
    assert_eq!(payload[5], 30, "fps at offset 5");
    assert_eq!(payload[6], 0, "rotation at offset 6");
    assert_eq!(&payload[7..11], &[0, 0, 0, 0], "reserved bytes 7-10");
    assert_eq!(header_of(&commands[3].data).payload_len, 11);
}

// --- Display settings (section 5.5) ---

// --- Replies (section 5.4) ---

fn reply(command: TlLcdCommand, payload: &[u8]) -> Vec<u8> {
    let mut data = vec![0_u8; TL_LCD_HEADER_LEN + payload.len()];
    data[0] = TL_LCD_REPORT_ID;
    data[1] = command as u8;
    data[9..11].copy_from_slice(
        &u16::try_from(payload.len())
            .expect("test payload should fit")
            .to_be_bytes(),
    );
    data[TL_LCD_HEADER_LEN..].copy_from_slice(payload);
    data
}

#[test]
fn a_serial_reply_yields_the_serial_and_the_chain_position() {
    let protocol = TlLcdProtocol::new();
    let mut payload = vec![0_u8; 34];
    payload[..10].copy_from_slice(b"TL_LCDV0.1");
    payload[32] = 2;
    payload[33] = 5;

    protocol
        .parse_response(&reply(TlLcdCommand::ReadSerial, &payload))
        .expect("a serial reply should parse");

    assert_eq!(protocol.serial().as_deref(), Some("TL_LCDV0.1"));
    assert_eq!(protocol.chain_position(), Some((2, 5)));
}

#[test]
fn a_handshake_reply_yields_the_mode_and_frame_counter() {
    let protocol = TlLcdProtocol::new();

    protocol
        .parse_response(&reply(TlLcdCommand::GetHandshake, &[5, 0x01, 0x2C]))
        .expect("a handshake reply should parse");

    assert_eq!(
        protocol.handshake(),
        Some((5, 300)),
        "mode at offset 0, frame index as a big-endian u16"
    );
}

#[test]
fn product_info_keeps_the_first_report_as_the_firmware_version() {
    let protocol = TlLcdProtocol::new();

    protocol
        .parse_response(&reply(TlLcdCommand::GetProductInfo, b"V0.1.7\0\0"))
        .expect("the version report should parse");
    protocol
        .parse_response(&reply(TlLcdCommand::GetProductInfo, b"Mar 14 2026\0"))
        .expect("the build-date report should parse");

    assert_eq!(
        protocol.firmware().as_deref(),
        Some("V0.1.7"),
        "the build date must not overwrite the version"
    );
}

#[test]
fn a_reply_shorter_than_a_header_is_rejected() {
    let protocol = TlLcdProtocol::new();

    let error = protocol.parse_response(&[0x02, 0x3C, 0x00]);

    assert!(error.is_err(), "a truncated reply is malformed, not empty");
}

#[test]
fn a_reply_declaring_more_than_it_carries_is_clipped_not_panicked() {
    let protocol = TlLcdProtocol::new();
    let mut data = reply(TlLcdCommand::GetHandshake, &[5, 0x00, 0x01]);
    data[9..11].copy_from_slice(&500_u16.to_be_bytes());

    let parsed = protocol
        .parse_response(&data)
        .expect("a short report is normal for this panel");

    assert_eq!(parsed.data.len(), 3, "only the bytes in hand are returned");
}

// --- Device surface (section 5.6) ---

#[test]
fn the_panel_exposes_one_round_400x400_display_zone() {
    let protocol = TlLcdProtocol::new();

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "Display");
    assert_eq!(zones[0].led_count, 0);
    assert_eq!(zones[0].color_format, DeviceColorFormat::Jpeg);
    assert!(matches!(
        zones[0].topology,
        DeviceTopologyHint::Display {
            width: 400,
            height: 400,
            circular: true,
        }
    ));

    let capabilities = protocol.capabilities();
    assert!(capabilities.has_display);
    assert_eq!(capabilities.display_resolution, Some((400, 400)));
    assert_eq!(capabilities.max_fps, 30);
    assert_eq!(protocol.total_leds(), 0);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
}

#[test]
fn a_new_session_forgets_the_firmware_the_last_one_learned() {
    let protocol = TlLcdProtocol::new();

    protocol
        .parse_response(&reply(TlLcdCommand::GetProductInfo, b"V0.1.7\0"))
        .expect("the version report should parse");
    assert_eq!(protocol.firmware().as_deref(), Some("V0.1.7"));

    let _ = protocol.init_sequence();
    protocol
        .parse_response(&reply(TlLcdCommand::GetProductInfo, b"V0.2.0\0"))
        .expect("the version report should parse");

    assert_eq!(
        protocol.firmware().as_deref(),
        Some("V0.2.0"),
        "a reflashed panel reports its new version, not the cached one"
    );
}
