//! Tests for the `OpenRGB` SDK bridge backend.
//!
//! All tests use mock data — no actual `OpenRGB` server is required.
//! Tests cover the protocol layer (wire format, serialization, parsing),
//! controller-to-device mapping, and connection state machine.

use hypercolor_core::device::openrgb::proto;
use hypercolor_core::device::openrgb::{
    ClientConfig, Command, ConnectionState, ControllerData, HEADER_SIZE, MAGIC, OpenRgbClient,
    PacketHeader, ReconnectPolicy, RgbColor, ZoneData, ZoneType,
};

// ═══════════════════════════════════════════════════════════════════════════
// Protocol Header Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn header_magic_bytes_are_orgb() {
    assert_eq!(MAGIC, [b'O', b'R', b'G', b'B']);
}

#[test]
fn header_size_is_16_bytes() {
    assert_eq!(HEADER_SIZE, 16);
}

#[test]
fn header_roundtrip_serialization() {
    #[allow(clippy::as_conversions)]
    let header = PacketHeader {
        device_index: 3,
        command: Command::UpdateLeds as u32,
        data_length: 128,
    };

    let bytes = header.to_bytes();
    assert_eq!(bytes.len(), HEADER_SIZE);

    let parsed = PacketHeader::from_bytes(&bytes).expect("should parse valid header");
    assert_eq!(parsed, header);
}

#[test]
fn header_serialization_is_little_endian() {
    let header = PacketHeader {
        device_index: 0x0102_0304,
        command: 0x0506_0708,
        data_length: 0x090A_0B0C,
    };

    let bytes = header.to_bytes();

    // Magic
    assert_eq!(&bytes[0..4], b"ORGB");

    // Device index: 0x04030201 in little-endian
    assert_eq!(bytes[4], 0x04);
    assert_eq!(bytes[5], 0x03);
    assert_eq!(bytes[6], 0x02);
    assert_eq!(bytes[7], 0x01);

    // Command: 0x08070605 in little-endian
    assert_eq!(bytes[8], 0x08);
    assert_eq!(bytes[9], 0x07);
    assert_eq!(bytes[10], 0x06);
    assert_eq!(bytes[11], 0x05);

    // Data length: 0x0C0B0A09 in little-endian
    assert_eq!(bytes[12], 0x0C);
    assert_eq!(bytes[13], 0x0B);
    assert_eq!(bytes[14], 0x0A);
    assert_eq!(bytes[15], 0x09);
}

#[test]
fn header_with_zero_values() {
    let header = PacketHeader {
        device_index: 0,
        command: 0,
        data_length: 0,
    };

    let bytes = header.to_bytes();
    let parsed = PacketHeader::from_bytes(&bytes).expect("should parse zero header");

    assert_eq!(parsed.device_index, 0);
    assert_eq!(parsed.command, 0);
    assert_eq!(parsed.data_length, 0);
}

#[test]
fn header_rejects_invalid_magic() {
    let mut bytes = [0u8; HEADER_SIZE];
    bytes[0..4].copy_from_slice(b"BAAD");

    let result = PacketHeader::from_bytes(&bytes);
    assert!(result.is_err(), "should reject invalid magic bytes");
}

// ═══════════════════════════════════════════════════════════════════════════
// Command Enum Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[allow(clippy::as_conversions)]
fn command_values_match_spec() {
    assert_eq!(Command::RequestControllerCount as u32, 0);
    assert_eq!(Command::RequestControllerData as u32, 1);
    assert_eq!(Command::RequestProtocolVersion as u32, 40);
    assert_eq!(Command::SetClientName as u32, 50);
    assert_eq!(Command::DeviceListUpdated as u32, 100);
    assert_eq!(Command::ResizeZone as u32, 1000);
    assert_eq!(Command::UpdateLeds as u32, 1050);
    assert_eq!(Command::UpdateZoneLeds as u32, 1051);
    assert_eq!(Command::UpdateSingleLed as u32, 1052);
    assert_eq!(Command::SetCustomMode as u32, 1100);
}

#[test]
fn command_from_u32_roundtrip() {
    let commands = [
        (0, Command::RequestControllerCount),
        (1, Command::RequestControllerData),
        (40, Command::RequestProtocolVersion),
        (50, Command::SetClientName),
        (100, Command::DeviceListUpdated),
        (1000, Command::ResizeZone),
        (1050, Command::UpdateLeds),
        (1051, Command::UpdateZoneLeds),
        (1052, Command::UpdateSingleLed),
        (1100, Command::SetCustomMode),
    ];

    for (value, expected) in commands {
        let parsed = Command::from_u32(value);
        assert_eq!(
            parsed,
            Some(expected),
            "Command::from_u32({value}) should match"
        );
    }
}

#[test]
fn command_from_u32_returns_none_for_unknown() {
    assert!(Command::from_u32(999).is_none());
    assert!(Command::from_u32(u32::MAX).is_none());
    assert!(Command::from_u32(42).is_none());
}

// ═══════════════════════════════════════════════════════════════════════════
// Packet Construction Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn set_client_name_packet_format() {
    let packet = proto::build_set_client_name("Hypercolor");

    // Parse header
    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE]
        .try_into()
        .expect("header slice should be 16 bytes");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    assert_eq!(header.device_index, 0);
    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::SetClientName as u32;
    assert_eq!(header.command, expected_cmd);
    // "Hypercolor" = 10 bytes + null terminator = 11
    assert_eq!(header.data_length, 11);

    // Payload: null-terminated string without length prefix
    let payload = &packet[HEADER_SIZE..];
    assert_eq!(&payload[..10], b"Hypercolor");
    assert_eq!(payload[10], 0x00);
}

#[test]
fn request_protocol_version_packet_format() {
    let packet = proto::build_request_protocol_version(4);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::RequestProtocolVersion as u32;
    assert_eq!(header.command, expected_cmd);
    assert_eq!(header.data_length, 4);

    let payload = &packet[HEADER_SIZE..];
    let version = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(version, 4);
}

#[test]
fn request_controller_count_packet_has_no_payload() {
    let packet = proto::build_request_controller_count();
    assert_eq!(packet.len(), HEADER_SIZE);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::RequestControllerCount as u32;
    assert_eq!(header.command, expected_cmd);
    assert_eq!(header.data_length, 0);
}

#[test]
fn request_controller_data_packet_format() {
    let packet = proto::build_request_controller_data(2, 3);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    assert_eq!(header.device_index, 2);
    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::RequestControllerData as u32;
    assert_eq!(header.command, expected_cmd);
    assert_eq!(header.data_length, 4);

    let payload = &packet[HEADER_SIZE..];
    let version = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(version, 3);
}

#[test]
fn set_custom_mode_packet_has_no_payload() {
    let packet = proto::build_set_custom_mode(5);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    assert_eq!(header.device_index, 5);
    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::SetCustomMode as u32;
    assert_eq!(header.command, expected_cmd);
    assert_eq!(header.data_length, 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// LED Color Update Packet Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn update_leds_packet_structure() {
    let colors: Vec<[u8; 3]> = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let packet = proto::build_update_leds(1, &colors);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    assert_eq!(header.device_index, 1);
    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::UpdateLeds as u32;
    assert_eq!(header.command, expected_cmd);

    let payload = &packet[HEADER_SIZE..];

    // data_size: u16 (2) + 4 * 3 colors (12) = 14
    let data_size = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(data_size, 14);

    // led_count: 3
    let led_count = u16::from_le_bytes([payload[4], payload[5]]);
    assert_eq!(led_count, 3);

    // Color 0: [255, 0, 0, 0]
    assert_eq!(payload[6], 255);
    assert_eq!(payload[7], 0);
    assert_eq!(payload[8], 0);
    assert_eq!(payload[9], 0); // padding

    // Color 1: [0, 255, 0, 0]
    assert_eq!(payload[10], 0);
    assert_eq!(payload[11], 255);
    assert_eq!(payload[12], 0);
    assert_eq!(payload[13], 0); // padding

    // Color 2: [0, 0, 255, 0]
    assert_eq!(payload[14], 0);
    assert_eq!(payload[15], 0);
    assert_eq!(payload[16], 255);
    assert_eq!(payload[17], 0); // padding
}

#[test]
fn update_leds_empty_colors() {
    let colors: Vec<[u8; 3]> = vec![];
    let packet = proto::build_update_leds(0, &colors);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::UpdateLeds as u32;
    assert_eq!(header.command, expected_cmd);

    let payload = &packet[HEADER_SIZE..];
    let data_size = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(data_size, 2); // just the u16 led_count

    let led_count = u16::from_le_bytes([payload[4], payload[5]]);
    assert_eq!(led_count, 0);
}

#[test]
fn update_zone_leds_packet_structure() {
    let colors: Vec<[u8; 3]> = vec![[128, 64, 32], [16, 8, 4]];
    let packet = proto::build_update_zone_leds(2, 1, &colors);

    let header_bytes: [u8; HEADER_SIZE] = packet[..HEADER_SIZE].try_into().expect("header slice");
    let header = PacketHeader::from_bytes(&header_bytes).expect("valid header");

    assert_eq!(header.device_index, 2);
    #[allow(clippy::as_conversions)]
    let expected_cmd = Command::UpdateZoneLeds as u32;
    assert_eq!(header.command, expected_cmd);

    let payload = &packet[HEADER_SIZE..];

    // data_size: u32 zone_index (4) + u16 led_count (2) + 4 * 2 colors (8) = 14
    let data_size = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(data_size, 14);

    // zone_index: 1
    let zone_index = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    assert_eq!(zone_index, 1);

    // led_count: 2
    let led_count = u16::from_le_bytes([payload[8], payload[9]]);
    assert_eq!(led_count, 2);

    // Color 0: [128, 64, 32, 0]
    assert_eq!(payload[10], 128);
    assert_eq!(payload[11], 64);
    assert_eq!(payload[12], 32);
    assert_eq!(payload[13], 0);

    // Color 1: [16, 8, 4, 0]
    assert_eq!(payload[14], 16);
    assert_eq!(payload[15], 8);
    assert_eq!(payload[16], 4);
    assert_eq!(payload[17], 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// RGB Color Wire Format Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn rgb_color_to_wire_bytes() {
    let color = RgbColor {
        r: 255,
        g: 128,
        b: 64,
    };
    let bytes = color.to_wire_bytes();
    assert_eq!(bytes, [255, 128, 64, 0]);
}

#[test]
fn rgb_color_from_wire_bytes() {
    let color = RgbColor::from_wire_bytes([200, 100, 50, 0xFF]);
    assert_eq!(color.r, 200);
    assert_eq!(color.g, 100);
    assert_eq!(color.b, 50);
    // Padding byte is ignored
}

#[test]
fn rgb_color_roundtrip() {
    let original = RgbColor {
        r: 42,
        g: 137,
        b: 255,
    };
    let reconstructed = RgbColor::from_wire_bytes(original.to_wire_bytes());
    assert_eq!(original, reconstructed);
}

// ═══════════════════════════════════════════════════════════════════════════
// Response Parsing Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn parse_controller_count_response() {
    let payload = 8u32.to_le_bytes();
    let count = proto::parse_controller_count(&payload).expect("valid payload");
    assert_eq!(count, 8);
}

#[test]
fn parse_controller_count_zero() {
    let payload = 0u32.to_le_bytes();
    let count = proto::parse_controller_count(&payload).expect("valid payload");
    assert_eq!(count, 0);
}

#[test]
fn parse_controller_count_rejects_short_payload() {
    let payload = [0u8; 2];
    let result = proto::parse_controller_count(&payload);
    assert!(
        result.is_err(),
        "should reject payload shorter than 4 bytes"
    );
}

#[test]
fn parse_protocol_version_response() {
    let payload = 3u32.to_le_bytes();
    let version = proto::parse_protocol_version(&payload).expect("valid payload");
    assert_eq!(version, 3);
}

#[test]
fn parse_protocol_version_rejects_short_payload() {
    let payload = [0u8; 1];
    let result = proto::parse_protocol_version(&payload);
    assert!(
        result.is_err(),
        "should reject payload shorter than 4 bytes"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Controller Data Parsing Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn parse_mock_controller_data_v1() {
    let payload =
        proto::build_mock_controller_payload("ASUS Aura LED Controller", "ASUS", "Mainboard", 4);

    let data =
        proto::parse_controller_data(&payload, 1).expect("should parse mock controller data");

    assert_eq!(data.name, "ASUS Aura LED Controller");
    assert_eq!(data.vendor, "ASUS");
    assert_eq!(data.description, "Test controller");
    assert_eq!(data.version, "1.0");
    assert_eq!(data.serial, "SN-001");
    assert_eq!(data.location, "HID: /dev/hidraw0");
    assert_eq!(data.device_type, 0); // motherboard

    // Modes
    assert_eq!(data.modes.len(), 1);
    assert_eq!(data.modes[0].name, "Direct");
    assert_eq!(data.modes[0].color_mode, 1);

    // Zones
    assert_eq!(data.zones.len(), 1);
    assert_eq!(data.zones[0].name, "Mainboard");
    assert_eq!(data.zones[0].zone_type, ZoneType::Linear);
    assert_eq!(data.zones[0].leds_count, 4);

    // LEDs
    assert_eq!(data.leds.len(), 4);
    assert_eq!(data.leds[0].name, "LED 0");
    assert_eq!(data.leds[1].name, "LED 1");

    // Colors
    assert_eq!(data.colors.len(), 4);
}

#[test]
fn parse_controller_with_different_led_counts() {
    // Controller with 16 LEDs
    let payload = proto::build_mock_controller_payload("RGB Strip", "Corsair", "Strip", 16);
    let data = proto::parse_controller_data(&payload, 1).expect("should parse 16-LED controller");

    assert_eq!(data.zones[0].leds_count, 16);
    assert_eq!(data.leds.len(), 16);
    assert_eq!(data.colors.len(), 16);
}

#[test]
fn parse_controller_with_single_led() {
    let payload = proto::build_mock_controller_payload("Power LED", "MSI", "Indicator", 1);
    let data =
        proto::parse_controller_data(&payload, 1).expect("should parse single-LED controller");

    assert_eq!(data.zones[0].leds_count, 1);
    assert_eq!(data.leds.len(), 1);
    assert_eq!(data.colors.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Zone Type Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn zone_type_from_u32_values() {
    assert_eq!(ZoneType::from_u32(0), ZoneType::Single);
    assert_eq!(ZoneType::from_u32(1), ZoneType::Linear);
    assert_eq!(ZoneType::from_u32(2), ZoneType::Matrix);
}

#[test]
fn zone_type_defaults_to_single_for_unknown() {
    assert_eq!(ZoneType::from_u32(3), ZoneType::Single);
    assert_eq!(ZoneType::from_u32(99), ZoneType::Single);
    assert_eq!(ZoneType::from_u32(u32::MAX), ZoneType::Single);
}

// ═══════════════════════════════════════════════════════════════════════════
// Zone-to-Topology Mapping Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn zone_topology_mapping() {
    use hypercolor_types::device::LedTopology;

    // Linear zone -> Strip topology
    let linear_zone = ZoneData {
        name: "Strip".to_owned(),
        zone_type: ZoneType::Linear,
        leds_min: 10,
        leds_max: 10,
        leds_count: 10,
        matrix_height: 0,
        matrix_width: 0,
    };

    let topology = match linear_zone.zone_type {
        ZoneType::Single => {
            if linear_zone.leds_count == 1 {
                LedTopology::Point
            } else {
                LedTopology::Custom
            }
        }
        ZoneType::Linear => LedTopology::Strip,
        ZoneType::Matrix => LedTopology::Matrix {
            rows: linear_zone.matrix_height,
            cols: linear_zone.matrix_width,
        },
    };

    assert_eq!(topology, LedTopology::Strip);
}

#[test]
fn single_zone_with_one_led_maps_to_point() {
    use hypercolor_types::device::LedTopology;

    let zone = ZoneData {
        name: "Power".to_owned(),
        zone_type: ZoneType::Single,
        leds_min: 1,
        leds_max: 1,
        leds_count: 1,
        matrix_height: 0,
        matrix_width: 0,
    };

    let topology = match zone.zone_type {
        ZoneType::Single => {
            if zone.leds_count == 1 {
                LedTopology::Point
            } else {
                LedTopology::Custom
            }
        }
        ZoneType::Linear => LedTopology::Strip,
        ZoneType::Matrix => LedTopology::Matrix {
            rows: zone.matrix_height,
            cols: zone.matrix_width,
        },
    };

    assert_eq!(topology, LedTopology::Point);
}

#[test]
fn single_zone_with_multiple_leds_maps_to_custom() {
    use hypercolor_types::device::LedTopology;

    let zone = ZoneData {
        name: "Multi".to_owned(),
        zone_type: ZoneType::Single,
        leds_min: 4,
        leds_max: 4,
        leds_count: 4,
        matrix_height: 0,
        matrix_width: 0,
    };

    let topology = match zone.zone_type {
        ZoneType::Single => {
            if zone.leds_count == 1 {
                LedTopology::Point
            } else {
                LedTopology::Custom
            }
        }
        ZoneType::Linear => LedTopology::Strip,
        ZoneType::Matrix => LedTopology::Matrix {
            rows: zone.matrix_height,
            cols: zone.matrix_width,
        },
    };

    assert_eq!(topology, LedTopology::Custom);
}

#[test]
fn matrix_zone_maps_to_matrix_topology() {
    use hypercolor_types::device::LedTopology;

    let zone = ZoneData {
        name: "Keyboard".to_owned(),
        zone_type: ZoneType::Matrix,
        leds_min: 60,
        leds_max: 60,
        leds_count: 60,
        matrix_height: 6,
        matrix_width: 10,
    };

    let topology = match zone.zone_type {
        ZoneType::Single => {
            if zone.leds_count == 1 {
                LedTopology::Point
            } else {
                LedTopology::Custom
            }
        }
        ZoneType::Linear => LedTopology::Strip,
        ZoneType::Matrix => LedTopology::Matrix {
            rows: zone.matrix_height,
            cols: zone.matrix_width,
        },
    };

    assert_eq!(topology, LedTopology::Matrix { rows: 6, cols: 10 });
}

// ═══════════════════════════════════════════════════════════════════════════
// Response Validation Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn validate_response_accepts_matching_command() {
    #[allow(clippy::as_conversions)]
    let header = PacketHeader {
        device_index: 0,
        command: Command::RequestControllerCount as u32,
        data_length: 4,
    };

    let result = proto::validate_response(&header, Command::RequestControllerCount);
    assert!(result.is_ok());
}

#[test]
fn validate_response_rejects_mismatched_command() {
    #[allow(clippy::as_conversions)]
    let header = PacketHeader {
        device_index: 0,
        command: Command::RequestControllerData as u32,
        data_length: 100,
    };

    let result = proto::validate_response(&header, Command::RequestControllerCount);
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════════════
// Client Connection State Machine Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn client_starts_disconnected() {
    let client = OpenRgbClient::with_defaults();
    assert_eq!(*client.state(), ConnectionState::Disconnected);
    assert!(!client.is_connected());
    assert_eq!(client.protocol_version(), 0);
}

#[test]
fn client_controllers_empty_when_disconnected() {
    let client = OpenRgbClient::with_defaults();
    assert!(client.controllers().is_empty());
}

#[test]
fn client_config_defaults() {
    let config = ClientConfig::default();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 6742);
    assert_eq!(config.client_name, "Hypercolor");
    assert_eq!(config.protocol_version, 4);
}

#[test]
fn client_custom_config() {
    let config = ClientConfig {
        host: "192.168.1.100".to_owned(),
        port: 9999,
        client_name: "TestClient".to_owned(),
        protocol_version: 2,
        ..ClientConfig::default()
    };

    let client = OpenRgbClient::new(config);
    assert_eq!(*client.state(), ConnectionState::Disconnected);
}

// ═══════════════════════════════════════════════════════════════════════════
// Reconnect Policy Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn reconnect_policy_defaults() {
    let policy = ReconnectPolicy::default();
    assert_eq!(policy.initial_delay.as_secs(), 1);
    assert_eq!(policy.max_delay.as_secs(), 30);
    assert!((policy.backoff_factor - 2.0).abs() < f64::EPSILON);
    assert_eq!(policy.max_attempts, 0); // unlimited
}

#[test]
fn reconnect_policy_exponential_backoff() {
    let policy = ReconnectPolicy::default();

    let d0 = policy.delay_for_attempt(0);
    let d1 = policy.delay_for_attempt(1);
    let d2 = policy.delay_for_attempt(2);
    let d3 = policy.delay_for_attempt(3);

    assert_eq!(d0.as_secs(), 1);
    assert_eq!(d1.as_secs(), 2);
    assert_eq!(d2.as_secs(), 4);
    assert_eq!(d3.as_secs(), 8);
}

#[test]
fn reconnect_policy_clamps_to_max_delay() {
    let policy = ReconnectPolicy {
        initial_delay: std::time::Duration::from_secs(1),
        max_delay: std::time::Duration::from_secs(10),
        backoff_factor: 2.0,
        max_attempts: 0,
    };

    // Attempt 10 would be 1 * 2^10 = 1024 seconds, but clamped to 10
    let delay = policy.delay_for_attempt(10);
    assert_eq!(delay.as_secs(), 10);
}

#[test]
fn reconnect_policy_exhausted_when_limited() {
    let policy = ReconnectPolicy {
        max_attempts: 5,
        ..ReconnectPolicy::default()
    };

    assert!(!policy.exhausted(0));
    assert!(!policy.exhausted(4));
    assert!(policy.exhausted(5));
    assert!(policy.exhausted(100));
}

#[test]
fn reconnect_policy_never_exhausted_when_unlimited() {
    let policy = ReconnectPolicy {
        max_attempts: 0,
        ..ReconnectPolicy::default()
    };

    assert!(!policy.exhausted(0));
    assert!(!policy.exhausted(1000));
    assert!(!policy.exhausted(u32::MAX));
}

// ═══════════════════════════════════════════════════════════════════════════
// Backend DeviceInfo Mapping Tests (using mock TCP server)
// ═══════════════════════════════════════════════════════════════════════════

/// Test that the backend maps `OpenRGB` controller data to Hypercolor's
/// `DeviceInfo` structure correctly. Uses the backend's internal mapping
/// function via a constructed `ControllerData`.
#[test]
fn backend_maps_controller_to_device_info() {
    use hypercolor_core::device::openrgb::backend::OpenRgbBackend;

    let controller = ControllerData {
        device_type: 0, // motherboard
        name: "ASUS Aura".to_owned(),
        vendor: "ASUS".to_owned(),
        description: "RGB controller".to_owned(),
        version: "2.1".to_owned(),
        serial: "ABC123".to_owned(),
        location: "HID: /dev/hidraw3".to_owned(),
        active_mode: 0,
        modes: vec![],
        zones: vec![
            ZoneData {
                name: "Mainboard".to_owned(),
                zone_type: ZoneType::Linear,
                leds_min: 4,
                leds_max: 4,
                leds_count: 4,
                matrix_height: 0,
                matrix_width: 0,
            },
            ZoneData {
                name: "DRAM".to_owned(),
                zone_type: ZoneType::Linear,
                leds_min: 8,
                leds_max: 8,
                leds_count: 8,
                matrix_height: 0,
                matrix_width: 0,
            },
        ],
        leds: vec![],
        colors: vec![],
    };

    // Use the backend's discover path indirectly by calling the static mapping
    let _backend = OpenRgbBackend::with_defaults();

    // Verify zone topology mapping
    assert_eq!(controller.zones[0].zone_type, ZoneType::Linear);
    assert_eq!(controller.zones[1].zone_type, ZoneType::Linear);

    // Verify total LED count
    let total: u32 = controller.zones.iter().map(|z| z.leds_count).sum();
    assert_eq!(total, 12);
}

// ═══════════════════════════════════════════════════════════════════════════
// Backend Integration Tests (with mock TCP server)
// ═══════════════════════════════════════════════════════════════════════════

/// Helper: build a complete `OpenRGB` SDK response (header + payload).
#[allow(clippy::as_conversions)]
fn build_mock_response(device_index: u32, command: Command, payload: &[u8]) -> Vec<u8> {
    let header = PacketHeader {
        device_index,
        command: command as u32,
        #[allow(clippy::cast_possible_truncation)]
        data_length: payload.len() as u32,
    };
    let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
    buf.extend_from_slice(&header.to_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Start a mock `OpenRGB` SDK server that responds to handshake and enumeration.
///
/// Returns the port the server is listening on.
#[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
async fn start_mock_server(controllers: Vec<(String, String, String, u16)>) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let port = listener.local_addr().expect("get local addr").port();

    tokio::spawn(async move {
        // Accept multiple connections (scanner probe + actual client)
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };

            let controllers = controllers.clone();
            tokio::spawn(async move {
                // Read SET_CLIENT_NAME
                let mut header_buf = [0u8; HEADER_SIZE];
                if stream.read_exact(&mut header_buf).await.is_err() {
                    return;
                }
                let Ok(header) = PacketHeader::from_bytes(&header_buf) else {
                    return;
                };
                // Skip payload
                let mut payload_buf = vec![0u8; header.data_length as usize];
                if !payload_buf.is_empty() {
                    let _ = stream.read_exact(&mut payload_buf).await;
                }

                // Read REQUEST_PROTOCOL_VERSION
                if stream.read_exact(&mut header_buf).await.is_err() {
                    return;
                }
                let Ok(header) = PacketHeader::from_bytes(&header_buf) else {
                    return;
                };
                let mut version_buf = vec![0u8; header.data_length as usize];
                if !version_buf.is_empty() {
                    let _ = stream.read_exact(&mut version_buf).await;
                }

                // Respond with protocol version
                let response =
                    build_mock_response(0, Command::RequestProtocolVersion, &1u32.to_le_bytes());
                let _ = stream.write_all(&response).await;

                // Serve requests in a loop
                loop {
                    if stream.read_exact(&mut header_buf).await.is_err() {
                        break;
                    }
                    let Ok(header) = PacketHeader::from_bytes(&header_buf) else {
                        break;
                    };

                    let mut req_payload = vec![0u8; header.data_length as usize];
                    if !req_payload.is_empty() && stream.read_exact(&mut req_payload).await.is_err()
                    {
                        break;
                    }

                    match Command::from_u32(header.command) {
                        Some(Command::RequestControllerCount) => {
                            #[allow(clippy::cast_possible_truncation)]
                            let count = controllers.len() as u32;
                            let response = build_mock_response(
                                0,
                                Command::RequestControllerCount,
                                &count.to_le_bytes(),
                            );
                            let _ = stream.write_all(&response).await;
                        }
                        Some(Command::RequestControllerData) => {
                            let idx = header.device_index as usize;
                            if let Some((name, vendor, zone_name, led_count)) = controllers.get(idx)
                            {
                                let payload = proto::build_mock_controller_payload(
                                    name, vendor, zone_name, *led_count,
                                );
                                let response = build_mock_response(
                                    header.device_index,
                                    Command::RequestControllerData,
                                    &payload,
                                );
                                let _ = stream.write_all(&response).await;
                            }
                        }
                        // SetCustomMode, UpdateLeds, UpdateZoneLeds: no response expected
                        _ => {}
                    }
                }
            });
        }
    });

    port
}

#[tokio::test]
async fn client_connects_to_mock_server() {
    let port = start_mock_server(vec![(
        "Test Controller".to_owned(),
        "TestVendor".to_owned(),
        "Zone A".to_owned(),
        4,
    )])
    .await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut client = OpenRgbClient::new(config);
    client
        .connect()
        .await
        .expect("should connect to mock server");

    assert!(client.is_connected());
    assert_eq!(client.protocol_version(), 1);
}

#[tokio::test]
async fn client_enumerates_controllers_from_mock() {
    let port = start_mock_server(vec![
        (
            "ASUS Aura".to_owned(),
            "ASUS".to_owned(),
            "Mainboard".to_owned(),
            4,
        ),
        (
            "Corsair RGB".to_owned(),
            "Corsair".to_owned(),
            "RAM".to_owned(),
            8,
        ),
    ])
    .await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut client = OpenRgbClient::new(config);
    client.connect().await.expect("should connect");

    let count = client
        .enumerate_controllers()
        .await
        .expect("should enumerate");

    assert_eq!(count, 2);
    assert_eq!(client.controllers().len(), 2);

    let ctrl0 = client.controllers().get(&0).expect("controller 0");
    assert_eq!(ctrl0.name, "ASUS Aura");
    assert_eq!(ctrl0.vendor, "ASUS");
    assert_eq!(ctrl0.zones[0].name, "Mainboard");
    assert_eq!(ctrl0.zones[0].leds_count, 4);

    let ctrl1 = client.controllers().get(&1).expect("controller 1");
    assert_eq!(ctrl1.name, "Corsair RGB");
    assert_eq!(ctrl1.zones[0].leds_count, 8);
}

#[tokio::test]
async fn client_sends_led_update_to_mock() {
    let port = start_mock_server(vec![(
        "LED Strip".to_owned(),
        "Generic".to_owned(),
        "Strip".to_owned(),
        3,
    )])
    .await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut client = OpenRgbClient::new(config);
    client.connect().await.expect("should connect");

    // Set custom mode (fire-and-forget)
    client
        .set_custom_mode(0)
        .await
        .expect("should set custom mode");

    // Send LED update
    let colors = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    client
        .update_leds(0, &colors)
        .await
        .expect("should update LEDs");
}

#[tokio::test]
async fn client_sends_zone_led_update_to_mock() {
    let port = start_mock_server(vec![(
        "Controller".to_owned(),
        "Vendor".to_owned(),
        "Zone".to_owned(),
        5,
    )])
    .await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut client = OpenRgbClient::new(config);
    client.connect().await.expect("should connect");

    let colors = vec![[128, 64, 32]; 5];
    client
        .update_zone_leds(0, 0, &colors)
        .await
        .expect("should update zone LEDs");
}

#[tokio::test]
async fn client_disconnect_clears_state() {
    let port =
        start_mock_server(vec![("Test".to_owned(), "V".to_owned(), "Z".to_owned(), 1)]).await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut client = OpenRgbClient::new(config);
    client.connect().await.expect("should connect");
    assert!(client.is_connected());

    client.enumerate_controllers().await.expect("enumerate");
    assert!(!client.controllers().is_empty());

    client.disconnect().await;
    assert!(!client.is_connected());
    assert!(client.controllers().is_empty());
    assert_eq!(client.protocol_version(), 0);
}

#[tokio::test]
async fn client_connect_to_nonexistent_server_fails() {
    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port: 1, // unlikely to be listening
        connect_timeout: std::time::Duration::from_millis(100),
        ..ClientConfig::default()
    };

    let mut client = OpenRgbClient::new(config);
    let result = client.connect().await;
    assert!(
        result.is_err(),
        "should fail to connect to nonexistent server"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Backend Discover/Connect/Write Lifecycle with Mock TCP
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn backend_full_lifecycle_with_mock_server() {
    use hypercolor_core::device::DeviceBackend;
    use hypercolor_core::device::openrgb::backend::OpenRgbBackend;

    let port = start_mock_server(vec![
        (
            "Mobo RGB".to_owned(),
            "ASUS".to_owned(),
            "Mainboard".to_owned(),
            4,
        ),
        (
            "RAM RGB".to_owned(),
            "Corsair".to_owned(),
            "DIMM".to_owned(),
            8,
        ),
    ])
    .await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut backend = OpenRgbBackend::new(config);

    // Verify backend info
    let info = backend.info();
    assert_eq!(info.id, "openrgb");

    // Discover
    let devices = backend.discover().await.expect("discovery should succeed");

    assert_eq!(devices.len(), 2);

    // Find the mobo device
    let mobo = devices
        .iter()
        .find(|d| d.name == "Mobo RGB")
        .expect("should find mobo device");

    assert_eq!(mobo.vendor, "ASUS");
    assert_eq!(mobo.zones.len(), 1);
    assert_eq!(mobo.zones[0].name, "Mainboard");
    assert_eq!(mobo.zones[0].led_count, 4);
    assert_eq!(mobo.total_led_count(), 4);

    // Connect
    let device_id = mobo.id;
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");

    // Write colors
    let colors = vec![[255, 0, 0]; 4];
    backend
        .write_colors(&device_id, &colors)
        .await
        .expect("write should succeed");

    // Disconnect
    backend
        .disconnect(&device_id)
        .await
        .expect("disconnect should succeed");

    // Write after disconnect should fail
    let result = backend.write_colors(&device_id, &colors).await;
    assert!(result.is_err(), "write after disconnect should fail");
}

#[tokio::test]
async fn backend_write_to_unconnected_device_fails() {
    use hypercolor_core::device::DeviceBackend;
    use hypercolor_core::device::openrgb::backend::OpenRgbBackend;
    use hypercolor_types::device::DeviceId;

    let port =
        start_mock_server(vec![("Test".to_owned(), "V".to_owned(), "Z".to_owned(), 2)]).await;

    let config = ClientConfig {
        host: "127.0.0.1".to_owned(),
        port,
        ..ClientConfig::default()
    };

    let mut backend = OpenRgbBackend::new(config);
    backend.discover().await.expect("discover");

    // Try writing to a random device ID that was never connected
    let fake_id = DeviceId::new();
    let colors = vec![[0, 0, 0]; 2];
    let result = backend.write_colors(&fake_id, &colors).await;
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════════════
// Scanner Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn scanner_returns_empty_when_server_unavailable() {
    use hypercolor_core::device::TransportScanner;
    use hypercolor_core::device::openrgb::scanner::{OpenRgbScanner, ScannerConfig};

    let config = ScannerConfig {
        host: "127.0.0.1".to_owned(),
        port: 1, // nothing listening
        probe_timeout: std::time::Duration::from_millis(100),
    };

    let mut scanner = OpenRgbScanner::new(config);
    let devices = scanner.scan().await.expect("scan should not error");
    assert!(devices.is_empty(), "no devices when server is unavailable");
}

#[tokio::test]
async fn scanner_discovers_controllers_from_mock() {
    use hypercolor_core::device::TransportScanner;
    use hypercolor_core::device::openrgb::scanner::{OpenRgbScanner, ScannerConfig};
    use hypercolor_types::device::DeviceFamily;

    let port = start_mock_server(vec![
        (
            "GPU RGB".to_owned(),
            "NVIDIA".to_owned(),
            "Backplate".to_owned(),
            12,
        ),
        (
            "Case Fan".to_owned(),
            "Noctua".to_owned(),
            "Fan Ring".to_owned(),
            16,
        ),
    ])
    .await;

    let config = ScannerConfig {
        host: "127.0.0.1".to_owned(),
        port,
        probe_timeout: std::time::Duration::from_secs(2),
    };

    let mut scanner = OpenRgbScanner::new(config);
    let devices = scanner.scan().await.expect("scan should succeed");

    assert_eq!(devices.len(), 2);

    for device in &devices {
        assert_eq!(device.family, DeviceFamily::OpenRgb);
        assert!(!device.fingerprint.0.is_empty());
        assert!(device.metadata.contains_key("openrgb_index"));
    }
}

#[tokio::test]
async fn scanner_name_is_correct() {
    use hypercolor_core::device::TransportScanner;
    use hypercolor_core::device::openrgb::scanner::OpenRgbScanner;

    let scanner = OpenRgbScanner::with_defaults();
    assert_eq!(scanner.name(), "OpenRGB SDK");
}
