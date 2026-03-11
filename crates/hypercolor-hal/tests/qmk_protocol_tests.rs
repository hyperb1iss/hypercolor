//! QMK HID RGB protocol encoding tests.

use hypercolor_hal::drivers::qmk::{
    Command, PACKET_SIZE, ProtocolRevision, QmkKeyboardConfig, QmkProtocol,
};
use hypercolor_hal::protocol::Protocol;
use hypercolor_types::device::{DeviceColorFormat, DeviceTopologyHint};

// ── Helpers ──────────────────────────────────────────────────────────────

fn make_protocol(led_count: usize, revision: ProtocolRevision) -> QmkProtocol {
    QmkProtocol::new(QmkKeyboardConfig::new(led_count, revision))
}

fn make_protocol_with_matrix(
    led_count: usize,
    revision: ProtocolRevision,
    rows: u32,
    cols: u32,
) -> QmkProtocol {
    QmkProtocol::new(QmkKeyboardConfig::new(led_count, revision).with_matrix(rows, cols))
}

// ── Packet structure ─────────────────────────────────────────────────────

#[test]
fn all_packets_are_65_bytes() {
    let protocol = make_protocol(10, ProtocolRevision::RevD);

    for cmd in &protocol.init_sequence() {
        assert_eq!(cmd.data.len(), PACKET_SIZE, "init packet wrong size");
    }

    let colors = vec![[255_u8, 0, 128]; 10];
    let commands = protocol.encode_frame(&colors);
    for cmd in &commands {
        assert_eq!(cmd.data.len(), PACKET_SIZE, "frame packet wrong size");
    }

    for cmd in &protocol.shutdown_sequence() {
        assert_eq!(cmd.data.len(), PACKET_SIZE, "shutdown packet wrong size");
    }
}

// ── Init sequence ────────────────────────────────────────────────────────

#[test]
fn init_sequence_sends_version_query_then_direct_mode() {
    let protocol = make_protocol(87, ProtocolRevision::RevD);
    let init = protocol.init_sequence();

    assert_eq!(init.len(), 2, "expected version query + set mode");

    // First packet: GET_PROTOCOL_VERSION
    assert_eq!(init[0].data[0], 0x00, "report ID");
    assert_eq!(init[0].data[1], Command::GetProtocolVersion as u8);
    assert!(init[0].expects_response, "version query expects response");

    // Second packet: SET_MODE to Direct (mode 1)
    assert_eq!(init[1].data[0], 0x00, "report ID");
    assert_eq!(init[1].data[1], Command::SetMode as u8);
    assert_eq!(init[1].data[5], 0x01, "mode = Direct");
    assert_eq!(init[1].data[4], 0xFF, "brightness = max");
    assert_eq!(init[1].data[7], 0x00, "save = false (no EEPROM write)");
}

// ── Shutdown sequence ────────────────────────────────────────────────────

#[test]
fn shutdown_restores_solid_color_mode() {
    let protocol = make_protocol(68, ProtocolRevision::RevB);
    let shutdown = protocol.shutdown_sequence();

    assert_eq!(shutdown.len(), 1);
    assert_eq!(shutdown[0].data[1], Command::SetMode as u8);
    assert_eq!(shutdown[0].data[5], 0x02, "mode = SOLID_COLOR");
    assert_eq!(shutdown[0].data[7], 0x00, "save = false");
}

// ── RevB frame encoding ─────────────────────────────────────────────────

#[test]
fn revb_encodes_small_frame_in_single_packet() {
    let protocol = make_protocol(5, ProtocolRevision::RevB);
    let colors = vec![
        [10, 20, 30],
        [40, 50, 60],
        [70, 80, 90],
        [100, 110, 120],
        [130, 140, 150],
    ];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1, "5 LEDs fits in one RevB packet");

    let data = &commands[0].data;
    assert_eq!(data[1], Command::DirectModeSetLeds as u8);
    // payload[0] = start_idx = 0, payload[1] = count = 5
    assert_eq!(data[2], 0, "start index");
    assert_eq!(data[3], 5, "LED count");
    // First LED at offset 4
    assert_eq!(data[4], 10);
    assert_eq!(data[5], 20);
    assert_eq!(data[6], 30);
    // Second LED at offset 7
    assert_eq!(data[7], 40);
    assert_eq!(data[8], 50);
    assert_eq!(data[9], 60);
    // Fifth LED at offset 16
    assert_eq!(data[16], 130);
    assert_eq!(data[17], 140);
    assert_eq!(data[18], 150);
}

#[test]
fn revb_splits_frame_into_multiple_packets_at_20_leds() {
    let protocol = make_protocol(25, ProtocolRevision::RevB);
    let colors = vec![[0xFF, 0x00, 0x00]; 25];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 2, "25 LEDs needs 2 packets (20 + 5)");

    // First packet: 20 LEDs starting at index 0
    assert_eq!(commands[0].data[2], 0, "first packet start index");
    assert_eq!(commands[0].data[3], 20, "first packet LED count");

    // Second packet: 5 LEDs starting at index 20
    assert_eq!(commands[1].data[2], 20, "second packet start index");
    assert_eq!(commands[1].data[3], 5, "second packet LED count");
}

// ── RevD frame encoding ─────────────────────────────────────────────────

#[test]
fn revd_encodes_with_per_led_index() {
    let protocol = make_protocol(3, ProtocolRevision::RevD);
    let colors = vec![[0xAA, 0xBB, 0xCC], [0x11, 0x22, 0x33], [0x44, 0x55, 0x66]];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1);

    let data = &commands[0].data;
    assert_eq!(data[1], Command::DirectModeSetLeds as u8);
    // payload[0] = count
    assert_eq!(data[2], 3, "LED count");
    // RevD: [led_value, R, G, B] per LED
    // LED 0 at payload offset 1
    assert_eq!(data[3], 0, "LED 0 index");
    assert_eq!(data[4], 0xAA);
    assert_eq!(data[5], 0xBB);
    assert_eq!(data[6], 0xCC);
    // LED 1 at payload offset 5
    assert_eq!(data[7], 1, "LED 1 index");
    assert_eq!(data[8], 0x11);
    assert_eq!(data[9], 0x22);
    assert_eq!(data[10], 0x33);
    // LED 2 at payload offset 9
    assert_eq!(data[11], 2, "LED 2 index");
    assert_eq!(data[12], 0x44);
    assert_eq!(data[13], 0x55);
    assert_eq!(data[14], 0x66);
}

#[test]
fn revd_splits_at_15_leds() {
    let protocol = make_protocol(20, ProtocolRevision::RevD);
    let colors = vec![[0xFF, 0x00, 0x00]; 20];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 2, "20 LEDs needs 2 packets (15 + 5)");

    // First packet: 15 LEDs
    assert_eq!(commands[0].data[2], 15);
    // Second packet: 5 LEDs, indices start at 15
    assert_eq!(commands[1].data[2], 5);
    assert_eq!(commands[1].data[3], 15, "first LED index in second packet");
}

// ── Color normalization ──────────────────────────────────────────────────

#[test]
fn too_few_colors_are_zero_padded() {
    let protocol = make_protocol(10, ProtocolRevision::RevD);
    let colors = vec![[0xFF, 0x00, 0x00]; 5]; // only 5, expected 10

    let commands = protocol.encode_frame(&colors);
    // Should produce packets for all 10 LEDs
    let total_leds_encoded: u8 = commands.iter().map(|cmd| cmd.data[2]).sum();
    assert_eq!(total_leds_encoded, 10, "normalized to 10 LEDs");

    // Check that LED 9 (last, padded) has color [0, 0, 0]
    let last_pkt = commands.last().expect("should have packets");
    // In RevD, find the last LED entry
    let led_count = last_pkt.data[2] as usize;
    let last_led_offset = 3 + (led_count - 1) * 4;
    assert_eq!(last_pkt.data[last_led_offset + 1], 0, "padded R = 0");
    assert_eq!(last_pkt.data[last_led_offset + 2], 0, "padded G = 0");
    assert_eq!(last_pkt.data[last_led_offset + 3], 0, "padded B = 0");
}

#[test]
fn excess_colors_are_truncated() {
    let protocol = make_protocol(5, ProtocolRevision::RevB);
    let colors = vec![[0xFF, 0xFF, 0xFF]; 20]; // 20 supplied, only 5 expected

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data[3], 5, "only 5 LEDs encoded");
}

// ── Zones ────────────────────────────────────────────────────────────────

#[test]
fn single_keyboard_zone_with_strip_topology() {
    let protocol = make_protocol(87, ProtocolRevision::RevD);
    let zones = protocol.zones();

    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "Keyboard");
    assert_eq!(zones[0].led_count, 87);
    assert_eq!(zones[0].topology, DeviceTopologyHint::Strip);
    assert_eq!(zones[0].color_format, DeviceColorFormat::Rgb);
}

#[test]
fn matrix_topology_when_configured() {
    let protocol = make_protocol_with_matrix(87, ProtocolRevision::RevD, 6, 16);
    let zones = protocol.zones();

    assert_eq!(zones.len(), 1);
    assert_eq!(
        zones[0].topology,
        DeviceTopologyHint::Matrix { rows: 6, cols: 16 }
    );
}

#[test]
fn underglow_creates_second_zone() {
    let protocol = QmkProtocol::new(
        QmkKeyboardConfig::new(78, ProtocolRevision::RevD)
            .with_matrix(6, 14)
            .with_underglow(6),
    );
    let zones = protocol.zones();

    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].name, "Keyboard");
    assert_eq!(zones[0].led_count, 72, "78 total - 6 underglow = 72 keys");
    assert_eq!(zones[1].name, "Underglow");
    assert_eq!(zones[1].led_count, 6);
    assert_eq!(zones[1].topology, DeviceTopologyHint::Strip);
}

// ── Capabilities ─────────────────────────────────────────────────────────

#[test]
fn capabilities_report_correct_values() {
    let protocol = make_protocol(104, ProtocolRevision::RevD);
    let caps = protocol.capabilities();

    assert_eq!(caps.led_count, 104);
    assert!(caps.supports_direct);
    assert!(!caps.supports_brightness);
    assert!(!caps.has_display);
    assert_eq!(caps.max_fps, 30);
}

#[test]
fn total_leds_matches_config() {
    let protocol = make_protocol(68, ProtocolRevision::RevB);
    assert_eq!(protocol.total_leds(), 68);
}

// ── Response parsing ─────────────────────────────────────────────────────

#[test]
fn parse_response_accepts_valid_data() {
    let protocol = make_protocol(10, ProtocolRevision::RevD);
    let response = protocol
        .parse_response(&[0x00, 0x0D])
        .expect("should succeed");
    assert_eq!(response.data, vec![0x00, 0x0D]);
}

#[test]
fn parse_response_rejects_empty() {
    let protocol = make_protocol(10, ProtocolRevision::RevD);
    assert!(protocol.parse_response(&[]).is_err());
}

#[test]
fn parse_response_detects_failure_sentinel() {
    let protocol = make_protocol(10, ProtocolRevision::RevD);
    // Byte 3 (index 3) = STATUS_FAILURE (25)
    let mut data = [0_u8; 65];
    data[3] = 25;
    assert!(protocol.parse_response(&data).is_err());
}

// ── Protocol revision parameters ─────────────────────────────────────────

#[test]
fn revision_max_leds_per_update() {
    assert_eq!(ProtocolRevision::Rev9.max_leds_per_update(), 20);
    assert_eq!(ProtocolRevision::RevB.max_leds_per_update(), 20);
    assert_eq!(ProtocolRevision::RevD.max_leds_per_update(), 15);
}

#[test]
fn revision_bytes_per_led() {
    assert_eq!(ProtocolRevision::Rev9.bytes_per_led(), 3);
    assert_eq!(ProtocolRevision::RevB.bytes_per_led(), 3);
    assert_eq!(ProtocolRevision::RevD.bytes_per_led(), 4);
}

#[test]
fn revision_from_version_byte() {
    assert_eq!(
        ProtocolRevision::from_version_byte(0x09),
        Some(ProtocolRevision::Rev9)
    );
    assert_eq!(
        ProtocolRevision::from_version_byte(0x0B),
        Some(ProtocolRevision::RevB)
    );
    assert_eq!(
        ProtocolRevision::from_version_byte(0x0C),
        Some(ProtocolRevision::RevB)
    );
    assert_eq!(
        ProtocolRevision::from_version_byte(0x0D),
        Some(ProtocolRevision::RevD)
    );
    assert_eq!(
        ProtocolRevision::from_version_byte(0x0E),
        Some(ProtocolRevision::RevD)
    );
    assert_eq!(ProtocolRevision::from_version_byte(0xFF), None);
}

// ── Custom batch size override ───────────────────────────────────────────

#[test]
fn custom_leds_per_update_overrides_default() {
    let protocol = QmkProtocol::new(
        QmkKeyboardConfig::new(30, ProtocolRevision::RevD).with_leds_per_update(5),
    );
    let colors = vec![[0xFF, 0x00, 0x00]; 30];

    let commands = protocol.encode_frame(&colors);
    // 30 LEDs / 5 per batch = 6 packets
    assert_eq!(commands.len(), 6, "custom batch size of 5");
}

// ── Frame interval ───────────────────────────────────────────────────────

#[test]
fn frame_interval_targets_30fps() {
    let protocol = make_protocol(87, ProtocolRevision::RevD);
    let interval = protocol.frame_interval();
    assert_eq!(interval.as_millis(), 33);
}

// ── Device descriptors ───────────────────────────────────────────────────

#[test]
fn descriptor_table_is_non_empty() {
    let descriptors = hypercolor_hal::drivers::qmk::descriptors();
    assert!(
        !descriptors.is_empty(),
        "should have at least one QMK device"
    );
}

#[test]
fn all_descriptors_use_qmk_family() {
    for desc in hypercolor_hal::drivers::qmk::descriptors() {
        assert_eq!(
            desc.family,
            hypercolor_types::device::DeviceFamily::Qmk,
            "{} should be QMK family",
            desc.name
        );
    }
}

#[test]
fn all_descriptors_use_hidapi_with_qmk_usage_page() {
    use hypercolor_hal::database::TransportType;
    use hypercolor_hal::drivers::qmk::{USAGE_ID, USAGE_PAGE};

    for desc in hypercolor_hal::drivers::qmk::descriptors() {
        match desc.transport {
            TransportType::UsbHidApi {
                usage_page, usage, ..
            } => {
                assert_eq!(
                    usage_page,
                    Some(USAGE_PAGE),
                    "{} wrong usage page",
                    desc.name
                );
                assert_eq!(usage, Some(USAGE_ID), "{} wrong usage", desc.name);
            }
            _ => panic!("{} should use UsbHidApi transport", desc.name),
        }
    }
}

// ── Database registration ────────────────────────────────────────────────

#[test]
fn qmk_devices_appear_in_protocol_database() {
    use hypercolor_hal::database::ProtocolDatabase;
    use hypercolor_hal::drivers::qmk::VID_KEYCHRON;

    let result = ProtocolDatabase::lookup(VID_KEYCHRON, 0x0110);
    assert!(result.is_some(), "Keychron Q1 should be in the database");
    assert_eq!(result.expect("checked above").name, "Keychron Q1");
}

#[test]
fn qmk_builder_produces_valid_protocol() {
    use hypercolor_hal::database::ProtocolDatabase;
    use hypercolor_hal::drivers::qmk::VID_ZSA;

    let desc = ProtocolDatabase::lookup(VID_ZSA, 0x1969).expect("Moonlander should exist");
    let protocol = (desc.protocol.build)();

    assert_eq!(protocol.name(), "QMK HID RGB");
    assert!(protocol.total_leds() > 0);
    assert!(!protocol.zones().is_empty());
}
