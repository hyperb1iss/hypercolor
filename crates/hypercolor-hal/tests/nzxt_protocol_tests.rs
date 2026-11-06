//! NZXT Phase 1 lighting encoding tests (Spec 39 §4, §9.5).
//!
//! Every expected byte array is written out from the spec's tables rather
//! than derived from the code under test.

use std::time::Duration;

use hypercolor_hal::drivers::nzxt::{
    ACCESSORY_CATALOG, AnalogColorOrder, Gen2Profile, KrakenX3Logo, MotherboardHeaders,
    NzxtGen2Protocol, NzxtKrakenEliteV2Protocol, analog_header_packet, kraken_x3_logo_packet,
    lookup_accessory, parse_topology_response,
};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseStatus};
use hypercolor_types::device::{DeviceColorFormat, DeviceTopologyHint};
use zerocopy::IntoBytes;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build a 64-byte `0x21 0x03` topology reply: channel count at byte 14,
/// channel `c` slot `s` at byte `15 + 6c + s`.
fn topology_report(channels: &[&[u8]]) -> Vec<u8> {
    let mut report = vec![0_u8; 64];
    report[0] = 0x21;
    report[1] = 0x03;
    report[14] = u8::try_from(channels.len()).expect("test channel count fits u8");
    for (channel, slots) in channels.iter().enumerate() {
        for (slot, id) in slots.iter().enumerate() {
            report[15 + channel * 6 + slot] = *id;
        }
    }
    report
}

/// Distinct, position-identifying colors so ordering and chunking are visible.
fn ramp(count: usize) -> Vec<[u8; 3]> {
    (0..count)
        .map(|index| {
            let low = u8::try_from(index % 256).expect("index modulo 256 fits u8");
            [low, low.wrapping_add(0x40), low.wrapping_add(0x80)]
        })
        .collect()
}

fn grb([r, g, b]: [u8; 3]) -> [u8; 3] {
    [g, r, b]
}

fn controller() -> NzxtGen2Protocol {
    NzxtGen2Protocol::new("NZXT Test Controller", Gen2Profile::controller())
}

fn with_topology(protocol: NzxtGen2Protocol, channels: &[&[u8]]) -> NzxtGen2Protocol {
    let response = protocol
        .parse_response(&topology_report(channels))
        .expect("topology reply parses");
    assert_eq!(response.status, ResponseStatus::Ok);
    protocol
}

fn encode(protocol: &dyn Protocol, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
    let mut commands = Vec::new();
    protocol.encode_frame_into(colors, &mut commands);
    commands
}

fn apply_packet(mask: u8) -> Vec<u8> {
    let mut expected = vec![
        0x22, 0xA0, mask, 0x00, 0x01, 0x00, 0x00, 0x28, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00,
        0x01,
    ];
    expected.resize(64, 0x00);
    expected
}

fn fixed_color_packet(target: [u8; 3], wire: [u8; 3]) -> Vec<u8> {
    let mut expected = vec![0_u8; 64];
    expected[0] = 0x2A;
    expected[1..4].copy_from_slice(&target);
    expected[4..7].copy_from_slice(&[0x00, 0x32, 0x00]);
    expected[7..10].copy_from_slice(&wire);
    expected[56..60].copy_from_slice(&[0x01, 0x00, 0x01, 0x03]);
    expected
}

fn stream_masks(commands: &[ProtocolCommand]) -> Vec<(u8, u8)> {
    commands
        .iter()
        .map(|command| (command.data[1], command.data[2]))
        .collect()
}

// ── Accessory catalog (§3.6) ───────────────────────────────────────────────

#[test]
fn accessory_catalog_matches_spec_table() {
    let expected: &[(u8, u8)] = &[
        (0x01, 10),
        (0x02, 8),
        (0x04, 10),
        (0x05, 8),
        (0x06, 6),
        (0x08, 14),
        (0x09, 15),
        (0x0A, 10),
        (0x0B, 8),
        (0x0C, 8),
        (0x10, 8),
        (0x11, 1),
        (0x13, 18),
        (0x14, 18),
        (0x15, 20),
        (0x16, 20),
        (0x17, 8),
        (0x18, 8),
        (0x19, 8),
        (0x1D, 24),
        (0x1E, 24),
        (0x1F, 24),
    ];

    assert_eq!(ACCESSORY_CATALOG.len(), expected.len());
    for (id, leds) in expected {
        let accessory = lookup_accessory(*id).expect("spec accessory is in the catalog");
        assert_eq!(accessory.leds, *leds, "accessory 0x{id:02X} LED count");
    }
    assert!(lookup_accessory(0x00).is_none());
    assert!(lookup_accessory(0x03).is_none());
    assert!(lookup_accessory(0x12).is_none());
}

#[test]
fn accessory_shapes_drive_topology_hints() {
    let fan = lookup_accessory(0x13).expect("F120 RGB");
    assert_eq!(fan.topology_hint(), DeviceTopologyHint::Ring { count: 18 });
    let strip = lookup_accessory(0x04).expect("Hue 2 strip");
    assert_eq!(strip.topology_hint(), DeviceTopologyHint::Strip);
    let logo = lookup_accessory(0x11).expect("Kraken X3 logo");
    assert_eq!(logo.topology_hint(), DeviceTopologyHint::Point);
    let ring = lookup_accessory(0x1E).expect("Kraken Elite ring");
    assert_eq!(ring.topology_hint(), DeviceTopologyHint::Ring { count: 24 });
}

// ── Topology parsing (§4.3, §9.5 items 3 and 4) ─────────────────────────────

#[test]
fn topology_parses_channels_and_slots() {
    let topology = parse_topology_response(&topology_report(&[&[0x13, 0x13], &[0x04], &[]]))
        .expect("topology parses");

    assert_eq!(topology.channels.len(), 3);
    assert_eq!(topology.channels[0].index, 0);
    assert_eq!(topology.channels[0].led_count(), 36);
    assert_eq!(topology.channels[1].led_count(), 10);
    assert_eq!(topology.channels[2].led_count(), 0);
}

#[test]
fn unknown_accessory_ends_the_channel_chain() {
    let topology =
        parse_topology_response(&topology_report(&[&[0x13, 0x7E, 0x13]])).expect("parses");
    assert_eq!(topology.channels[0].accessories.len(), 1);
    assert_eq!(topology.channels[0].led_count(), 18);
}

#[test]
fn empty_slot_ends_the_channel_chain() {
    let topology =
        parse_topology_response(&topology_report(&[&[0x04, 0x00, 0x13]])).expect("parses");
    assert_eq!(topology.channels[0].led_count(), 10);
}

#[test]
fn channel_count_is_clamped_to_eight() {
    let mut report = topology_report(&[]);
    report[14] = 12;
    let topology = parse_topology_response(&report).expect("parses");
    assert_eq!(topology.channels.len(), 8);
}

#[test]
fn truncated_report_drops_channels_past_its_end() {
    let mut report = topology_report(&[&[0x04], &[0x05], &[0x06]]);
    // Channel 2's record spans bytes 27..33; end the report at byte 30.
    report.truncate(30);
    let topology = parse_topology_response(&report).expect("parses");
    assert_eq!(topology.channels.len(), 2);
}

#[test]
fn report_without_a_channel_count_is_malformed() {
    let report = [0x21, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(parse_topology_response(&report).is_err());
    assert!(parse_topology_response(&[0x67, 0x02]).is_err());
}

// ── Gen2 init and response handling (§6.6) ─────────────────────────────────

#[test]
fn gen2_init_sends_one_zero_padded_topology_query() {
    let commands = controller().init_sequence();
    assert_eq!(commands.len(), 1);

    let mut expected = vec![0x20, 0x03];
    expected.resize(64, 0x00);
    assert_eq!(commands[0].data, expected);
    assert!(commands[0].expects_response);
}

#[test]
fn gen2_layout_is_empty_until_the_topology_reply() {
    let protocol = controller();
    assert_eq!(protocol.total_leds(), 0);
    assert!(protocol.zones().is_empty());
    assert!(encode(&protocol, &ramp(10)).is_empty());
}

#[test]
fn unrelated_reports_ask_for_a_resend() {
    let protocol = controller();

    let fan_status = protocol
        .parse_response(&[0x67, 0x02, 0x00, 0x00])
        .expect("fan status is not an error");
    assert_eq!(fan_status.status, ResponseStatus::Busy);

    let kraken_status = protocol
        .parse_response(&[0x75, 0x02])
        .expect("Kraken status is not an error");
    assert_eq!(kraken_status.status, ResponseStatus::Busy);

    let empty = protocol
        .parse_response(&[])
        .expect("an empty hidapi read is not an error");
    assert_eq!(empty.status, ResponseStatus::Busy);

    assert_eq!(protocol.total_leds(), 0);
}

#[test]
fn topology_reply_resolves_zones_and_capabilities() {
    let protocol = with_topology(controller(), &[&[0x13, 0x13], &[0x04], &[]]);

    let zones = protocol.zones();
    let names: Vec<&str> = zones.iter().map(|zone| zone.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Channel 1 F120 RGB 1",
            "Channel 1 F120 RGB 2",
            "Channel 2 Hue 2 Strip 10 LED",
        ]
    );
    assert_eq!(zones[0].topology, DeviceTopologyHint::Ring { count: 18 });
    assert_eq!(zones[2].topology, DeviceTopologyHint::Strip);
    assert!(
        zones
            .iter()
            .all(|zone| zone.color_format == DeviceColorFormat::Grb)
    );

    assert_eq!(protocol.total_leds(), 46);
    let capabilities = protocol.capabilities();
    assert_eq!(capabilities.led_count, 46);
    assert!(capabilities.supports_direct);
    assert_eq!(capabilities.max_fps, 30);
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
}

// ── Gen2 stream and apply (§4.3, §9.5 tables) ───────────────────────────────

#[test]
fn stream_packet_count_follows_twenty_led_chunks() {
    // (slots on channel 0, LED total, stream packets)
    let cases: &[(&[u8], usize, usize)] = &[
        (&[0x11], 1, 1),
        (&[0x15], 20, 1),
        (&[0x01, 0x11, 0x0A], 21, 2),
        (&[0x15, 0x15], 40, 2),
        (&[0x15, 0x15, 0x11], 41, 3),
        (&[0x1F, 0x1F, 0x1F, 0x1F, 0x1F, 0x1F], 144, 8),
    ];

    for (slots, leds, packets) in cases {
        let protocol = with_topology(controller(), &[slots]);
        assert_eq!(protocol.total_leds() as usize, *leds);

        let commands = encode(&protocol, &ramp(*leds));
        assert_eq!(commands.len(), packets + 1, "{leds} LEDs");
        for (index, command) in commands[..*packets].iter().enumerate() {
            assert_eq!(command.data.len(), 64);
            assert_eq!(command.data[0], 0x22);
            assert_eq!(
                command.data[1],
                0x10 | u8::try_from(index).expect("packet index fits u8")
            );
            assert_eq!(command.data[2], 0x01);
            assert_eq!(command.data[3], 0x00);
            assert!(!command.expects_response);
        }
        assert_eq!(commands[*packets].data, apply_packet(0x01));
    }
}

#[test]
fn stream_colors_are_grb_and_cross_the_chunk_boundary_in_order() {
    let protocol = with_topology(controller(), &[&[0x15, 0x15, 0x11]]);
    let colors = ramp(41);
    let commands = encode(&protocol, &colors);

    // Packet 0 carries LEDs 0..20.
    assert_eq!(commands[0].data[4..7], grb(colors[0]));
    assert_eq!(commands[0].data[61..64], grb(colors[19]));
    // Packet 1 starts at LED 20.
    assert_eq!(commands[1].data[4..7], grb(colors[20]));
    assert_eq!(commands[1].data[61..64], grb(colors[39]));
    // Packet 2 carries LED 40 and is zero after it.
    assert_eq!(commands[2].data[4..7], grb(colors[40]));
    assert!(commands[2].data[7..].iter().all(|byte| *byte == 0));
}

#[test]
fn grb_wire_order_for_a_single_led() {
    let protocol = with_topology(controller(), &[&[0x11]]);
    let commands = encode(&protocol, &[[0xAA, 0xBB, 0xCC]]);

    let mut expected = vec![0x22, 0x10, 0x01, 0x00, 0xBB, 0xAA, 0xCC];
    expected.resize(64, 0x00);
    assert_eq!(commands[0].data, expected);
    assert_eq!(commands[1].data, apply_packet(0x01));
}

#[test]
fn each_channel_gets_its_own_mask_and_apply() {
    let protocol = with_topology(controller(), &[&[0x04], &[], &[0x05]]);
    let colors = ramp(18);
    let commands = encode(&protocol, &colors);

    assert_eq!(
        stream_masks(&commands),
        [(0x10, 0x01), (0xA0, 0x01), (0x10, 0x04), (0xA0, 0x04)]
    );
    assert_eq!(commands[1].data, apply_packet(0x01));
    assert_eq!(commands[3].data, apply_packet(0x04));
    // Channel 2's first LED is frame LED 10.
    assert_eq!(commands[2].data[4..7], grb(colors[10]));
}

#[test]
fn short_frames_pad_with_black_and_long_frames_truncate() {
    let protocol = with_topology(controller(), &[&[0x04]]);

    let short = encode(&protocol, &ramp(3));
    assert_eq!(short[0].data[4..7], grb(ramp(3)[0]));
    assert!(short[0].data[13..].iter().all(|byte| *byte == 0));

    let long = encode(&protocol, &ramp(25));
    assert_eq!(long.len(), 2);
    assert_eq!(long[0].data[31..34], grb(ramp(25)[9]));
    assert!(long[0].data[34..].iter().all(|byte| *byte == 0));
}

#[test]
fn encode_frame_into_reuses_command_buffers() {
    let protocol = with_topology(controller(), &[&[0x15, 0x15, 0x11]]);
    let mut commands = Vec::new();

    protocol.encode_frame_into(&ramp(41), &mut commands);
    let first_ptr = commands[0].data.as_ptr();
    let first_len = commands.len();

    protocol.encode_frame_into(&ramp(41), &mut commands);
    assert_eq!(commands.len(), first_len);
    assert_eq!(commands[0].data.as_ptr(), first_ptr);
    assert_eq!(protocol.encode_frame(&ramp(41)).len(), first_len);
}

// ── Motherboards (§4.4) ─────────────────────────────────────────────────────

#[test]
fn analog_header_packet_matches_template() {
    let packet = analog_header_packet([0x11, 0x22, 0x33], AnalogColorOrder::Rgb);
    assert_eq!(
        packet.as_bytes(),
        fixed_color_packet([0x04, 0x08, 0x08], [0x11, 0x22, 0x33]).as_slice()
    );
}

#[test]
fn analog_header_supports_all_six_byte_orders() {
    let cases = [
        (AnalogColorOrder::Rgb, [0x11, 0x22, 0x33]),
        (AnalogColorOrder::Rbg, [0x11, 0x33, 0x22]),
        (AnalogColorOrder::Bgr, [0x33, 0x22, 0x11]),
        (AnalogColorOrder::Brg, [0x33, 0x11, 0x22]),
        (AnalogColorOrder::Gbr, [0x22, 0x33, 0x11]),
        (AnalogColorOrder::Grb, [0x22, 0x11, 0x33]),
    ];
    for (order, wire) in cases {
        let packet = analog_header_packet([0x11, 0x22, 0x33], order);
        assert_eq!(packet.as_bytes()[7..10], wire, "{order:?}");
    }
}

#[test]
fn motherboard_with_a_12v_header_exposes_it_before_topology() {
    let protocol = NzxtGen2Protocol::new(
        "NZXT Test Board",
        Gen2Profile::motherboard(MotherboardHeaders::new(2, 1, 1)),
    );

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "12V RGB Header");
    assert_eq!(zones[0].topology, DeviceTopologyHint::Point);
    assert_eq!(protocol.total_leds(), 1);

    let commands = encode(&protocol, &[[0x10, 0x20, 0x30]]);
    assert_eq!(commands.len(), 1);
    assert_eq!(
        commands[0].data,
        fixed_color_packet([0x04, 0x08, 0x08], [0x10, 0x20, 0x30])
    );
}

#[test]
fn motherboard_streams_only_its_digital_header_channels() {
    // Two NZXT headers plus one ARGB header: channels 0..3 are digital.
    let protocol = with_topology(
        NzxtGen2Protocol::new(
            "NZXT Test Board",
            Gen2Profile::motherboard(MotherboardHeaders::new(2, 1, 1)),
        ),
        &[&[0x04], &[0x05], &[0x06], &[0x04], &[0x04]],
    );

    let zones = protocol.zones();
    assert_eq!(zones.len(), 4);
    assert_eq!(zones[3].name, "12V RGB Header");
    assert_eq!(protocol.total_leds(), 10 + 8 + 6 + 1);

    let colors = ramp(25);
    let commands = encode(&protocol, &colors);
    assert_eq!(
        stream_masks(&commands[..6]),
        [
            (0x10, 0x01),
            (0xA0, 0x01),
            (0x10, 0x02),
            (0xA0, 0x02),
            (0x10, 0x04),
            (0xA0, 0x04),
        ]
    );
    // The analog write carries the last frame color, after every channel.
    assert_eq!(commands.len(), 7);
    assert_eq!(
        commands[6].data,
        fixed_color_packet([0x04, 0x08, 0x08], colors[24])
    );
}

#[test]
fn motherboard_analog_order_override_applies_to_frames() {
    let protocol = NzxtGen2Protocol::new(
        "NZXT Test Board",
        Gen2Profile::motherboard(MotherboardHeaders::new(2, 1, 1))
            .with_analog_color_order(AnalogColorOrder::Grb),
    );
    assert_eq!(protocol.zones()[0].color_format, DeviceColorFormat::Grb);
    let commands = encode(&protocol, &[[0x10, 0x20, 0x30]]);
    assert_eq!(commands[0].data[7..10], [0x20, 0x10, 0x30]);
}

#[test]
fn motherboard_without_a_12v_header_has_no_fixed_zone() {
    let protocol = NzxtGen2Protocol::new(
        "NZXT Test Board",
        Gen2Profile::motherboard(MotherboardHeaders::new(4, 2, 0)),
    );
    assert!(protocol.zones().is_empty());
    assert_eq!(protocol.profile().max_channels(), 6);
}

// ── Kraken X3 (§4.6.1) ──────────────────────────────────────────────────────

#[test]
fn kraken_x3_logo_packet_matches_template() {
    let packet = kraken_x3_logo_packet([0xAA, 0xBB, 0xCC]);
    assert_eq!(
        packet.as_bytes(),
        fixed_color_packet([0x04, 0x04, 0x04], [0xBB, 0xAA, 0xCC]).as_slice()
    );
}

fn kraken_x3(logo: KrakenX3Logo) -> NzxtGen2Protocol {
    with_topology(
        NzxtGen2Protocol::new("NZXT Test Kraken X3", Gen2Profile::kraken_x3(logo)),
        // External: three F120 RGB (54 LEDs, capped to two). Ring. Logo.
        &[&[0x13, 0x13, 0x13], &[0x10], &[0x11]],
    )
}

#[test]
fn kraken_x3_rgb_drives_ring_external_and_logo() {
    let protocol = kraken_x3(KrakenX3Logo::FixedColorWrite);

    let zones = protocol.zones();
    let names: Vec<&str> = zones.iter().map(|zone| zone.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Channel 1 F120 RGB 1",
            "Channel 1 F120 RGB 2",
            "Pump Ring",
            "Logo"
        ]
    );
    assert_eq!(zones[2].topology, DeviceTopologyHint::Ring { count: 8 });
    assert_eq!(zones[3].topology, DeviceTopologyHint::Point);
    assert_eq!(protocol.total_leds(), 36 + 8 + 1);

    let colors = ramp(45);
    let commands = encode(&protocol, &colors);
    // External: 2 stream + apply. Ring: 1 stream + apply. Logo write.
    assert_eq!(commands.len(), 6);
    assert_eq!(
        stream_masks(&commands[..5]),
        [
            (0x10, 0x01),
            (0x11, 0x01),
            (0xA0, 0x01),
            (0x10, 0x02),
            (0xA0, 0x02),
        ]
    );
    assert_eq!(commands[3].data[4..7], grb(colors[36]));
    assert_eq!(
        commands[5].data,
        fixed_color_packet([0x04, 0x04, 0x04], grb(colors[44]))
    );
    assert!(
        commands
            .iter()
            .all(|command| !(command.data[0] == 0x22 && command.data[2] == 0x04)),
        "the logo channel is never streamed"
    );
}

#[test]
fn kraken_x3_without_a_resolved_logo_skips_it_entirely() {
    let protocol = kraken_x3(KrakenX3Logo::Unresolved);
    assert!(protocol.zones().iter().all(|zone| zone.name != "Logo"));
    assert_eq!(protocol.total_leds(), 36 + 8);

    let commands = encode(&protocol, &ramp(44));
    assert_eq!(commands.len(), 5);
    assert!(commands.iter().all(|command| command.data[0] == 0x22));
    assert!(commands.iter().all(|command| command.data[2] != 0x04));
}

#[test]
fn kraken_x3_external_cap_keeps_exactly_forty() {
    let protocol = with_topology(
        NzxtGen2Protocol::new(
            "NZXT Test Kraken X3",
            Gen2Profile::kraken_x3(KrakenX3Logo::FixedColorWrite),
        ),
        &[&[0x15, 0x15]],
    );
    assert_eq!(protocol.total_leds(), 40 + 1);
    assert_eq!(protocol.profile().channel_led_cap(), Some(40));
}

// ── Kraken Z3 (§4.6.2) ──────────────────────────────────────────────────────

#[test]
fn kraken_z3_streams_a_fixed_forty_led_channel_zero() {
    let protocol = NzxtGen2Protocol::new("NZXT Test Kraken Z3", Gen2Profile::kraken_z3());
    assert!(protocol.init_sequence().is_empty());

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "External Channel");
    assert_eq!(zones[0].led_count, 40);
    assert_eq!(protocol.total_leds(), 40);

    let colors = ramp(40);
    let commands = encode(&protocol, &colors);
    assert_eq!(
        stream_masks(&commands),
        [(0x10, 0x01), (0x11, 0x01), (0xA0, 0x01)]
    );
    assert_eq!(commands[1].data[4..7], grb(colors[20]));
    assert_eq!(commands[1].data[61..64], grb(colors[39]));
    assert_eq!(commands[2].data, apply_packet(0x01));
}

// ── Kraken Elite v2 (§4.6.4) ────────────────────────────────────────────────

#[test]
fn kraken_elite_v2_writes_the_ring_then_applies() {
    let protocol = NzxtKrakenEliteV2Protocol::new("NZXT Test Kraken Elite V2");
    assert!(protocol.init_sequence().is_empty());
    assert_eq!(protocol.total_leds(), 24);

    let zones = protocol.zones();
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].name, "Pump Ring");
    assert_eq!(zones[0].topology, DeviceTopologyHint::Ring { count: 24 });
    assert_eq!(zones[0].color_format, DeviceColorFormat::Grb);

    let colors = ramp(24);
    let commands = encode(&protocol, &colors);
    assert_eq!(commands.len(), 2);

    let ring = &commands[0].data;
    assert_eq!(ring.len(), 512);
    assert_eq!(ring[..4], [0x26, 0x14, 0x01, 0x01]);
    assert_eq!(ring[4..7], grb(colors[0]));
    assert_eq!(ring[73..76], grb(colors[23]));
    assert!(ring[76..].iter().all(|byte| *byte == 0));

    let mut apply = vec![
        0x26, 0x06, 0x01, 0x00, 0x01, 0x00, 0x00, 0x18, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00,
        0x01,
    ];
    apply.resize(512, 0x00);
    assert_eq!(commands[1].data, apply);
    assert!(commands.iter().all(|command| !command.expects_response));
}

#[test]
fn kraken_elite_v2_pads_short_frames() {
    let protocol = NzxtKrakenEliteV2Protocol::new("NZXT Test Kraken Elite V2");
    let commands = encode(&protocol, &ramp(2));
    assert_eq!(
        commands[0].data[4..10],
        [0x40, 0x00, 0x80, 0x41, 0x01, 0x81]
    );
    assert!(commands[0].data[10..].iter().all(|byte| *byte == 0));
    assert_eq!(protocol.frame_interval(), Duration::from_millis(33));
}
