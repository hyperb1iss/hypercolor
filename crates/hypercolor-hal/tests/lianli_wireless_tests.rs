//! L-Wireless controller wire format, against spec 80 section 6 and the
//! bytes captured from a V1 controller (`SLV3TX_V1.6`) on 2026-09-04.

use std::time::Duration;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::lianli::WirelessControllerProtocol;
use hypercolor_hal::drivers::lianli::wireless::discovery::{
    DiscoveryError, GET_DEV_REPLY_CAPACITY, RECORD_LEN, RECORD_VALIDATION, WirelessFanModel,
    parse_device_table, parse_master_reply,
};
use hypercolor_hal::drivers::lianli::wireless::frame::{
    CLOCK_PAYLOAD_LEN, RF_BROADCAST_SLOT, RF_ENVELOPE_LEN, RF_SELECT, RGB_CHUNK_LEN,
    RGB_DATA_OFFSET, RfSubCommand, TX_RESET, TX_VIDEO_START, USB_CMD_GET_MAC, USB_CMD_SEND_RF,
    USB_PACKET_LEN, WallClock, clock_payload, clock_sync_envelope, pwm_envelope, reverse_fan_order,
    rgb_transfer,
};
use hypercolor_hal::drivers::lianli::wireless::tinyuz;
use hypercolor_hal::protocol::{Protocol, ResponseTolerance, TransferType};
use hypercolor_hal::registry::TransportType;
use hypercolor_types::device::DeviceTopologyHint;

const MASTER_MAC: [u8; 6] = [0xA0, 0x71, 0xAE, 0x72, 0xAB, 0x3C];

/// The TX's reply to `11 08`, exactly as captured.
fn captured_master_reply() -> Vec<u8> {
    let mut reply = vec![0_u8; 64];
    reply[..16].copy_from_slice(&[
        0x11, 0xA0, 0x71, 0xAE, 0x72, 0xAB, 0x3C, 0x00, 0x0A, 0xE7, 0x4B, 0x00, 0x10, 0x00, 0x00,
        0x00,
    ]);
    reply
}

/// The TX's status packet echoing a `10 01` poll, exactly as captured.
fn captured_tx_status_echo() -> Vec<u8> {
    let mut reply = captured_master_reply();
    reply[0] = USB_CMD_SEND_RF;
    reply
}

/// The RX's answer to a one-page poll with no fans, exactly as captured:
/// 448 bytes, count zero, motherboard PWM indicator `32 00`.
fn captured_empty_rx_table() -> Vec<u8> {
    let mut reply = vec![0_u8; 448];
    reply[..4].copy_from_slice(&[0x10, 0x00, 0x32, 0x00]);
    reply
}

fn record(mac: [u8; 6], device_type: u8, raw_fan_count: u8, fan_type: u8) -> [u8; RECORD_LEN] {
    let mut record = [0_u8; RECORD_LEN];
    record[0..6].copy_from_slice(&mac);
    record[6..12].copy_from_slice(&MASTER_MAC);
    record[12] = 8;
    record[13] = 3;
    record[18] = device_type;
    record[19] = raw_fan_count;
    record[20..24].copy_from_slice(&[0, 0, 0, 7]);
    record[24] = fan_type;
    // RPM 1234 (0x04D2) with the PWM-line status bit set in the high nibble.
    let [rpm_high, rpm_low] = 1234_u16.to_be_bytes();
    record[28] = 0x20 | rpm_high;
    record[29] = rpm_low;
    record[36..40].copy_from_slice(&[128, 64, 32, 0]);
    record[40] = 9;
    record[41] = RECORD_VALIDATION;
    record
}

fn table_with(records: &[[u8; RECORD_LEN]]) -> Vec<u8> {
    let mut reply = vec![0_u8; 448];
    reply[0] = USB_CMD_SEND_RF;
    reply[1] = u8::try_from(records.len()).expect("record count");
    reply[2] = 0x80;
    for (index, record) in records.iter().enumerate() {
        let start = 4 + index * RECORD_LEN;
        reply[start..start + RECORD_LEN].copy_from_slice(record);
    }
    reply
}

// --- Discovery replies (section 6.5, corrected) ---

#[test]
fn the_controllers_mac_reply_carries_its_mac_and_firmware() {
    let master = parse_master_reply(&captured_master_reply()).expect("a MAC reply");
    assert_eq!(master.mac, MASTER_MAC);
    assert_eq!(master.firmware, Some(0x0010), "V1.6 reports 0x0010");
}

#[test]
fn an_empty_rx_table_has_no_clusters_and_a_motherboard_duty() {
    let table = parse_device_table(&captured_empty_rx_table(), Some(MASTER_MAC))
        .expect("a page-sized reply is a table");
    assert!(table.clusters.is_empty());
    assert_eq!(
        table.motherboard_pwm,
        Some(255),
        "0x32 on and 0x00 off is full duty"
    );
}

#[test]
fn a_motherboard_pwm_with_the_unavailable_bit_is_none() {
    let mut reply = captured_empty_rx_table();
    reply[2] = 0x80;
    reply[3] = 0x10;
    let table = parse_device_table(&reply, Some(MASTER_MAC)).expect("table");
    assert_eq!(table.motherboard_pwm, None);
}

/// The TX answers a poll with its own status packet. With the master MAC
/// known it is recognised; without it the count byte reads as 160, which is
/// refused rather than clamped.
#[test]
fn the_tx_status_echo_is_never_mistaken_for_a_table() {
    assert_eq!(
        parse_device_table(&captured_tx_status_echo(), Some(MASTER_MAC)),
        Err(DiscoveryError::StatusEcho)
    );
    assert_eq!(
        parse_device_table(&captured_tx_status_echo(), None),
        Err(DiscoveryError::TooManyDevices(0xA0))
    );
}

#[test]
fn a_reply_with_the_wrong_echo_or_too_short_is_refused() {
    assert_eq!(
        parse_device_table(&captured_master_reply(), None),
        Err(DiscoveryError::WrongEcho(USB_CMD_GET_MAC))
    );
    assert_eq!(
        parse_device_table(&[0x10, 0x00], None),
        Err(DiscoveryError::Short(2))
    );
}

#[test]
fn records_parse_field_by_field_and_masters_are_skipped() {
    let fan_mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    let reply = table_with(&[record(MASTER_MAC, 0xFF, 0, 0), record(fan_mac, 0x00, 3, 28)]);

    let table = parse_device_table(&reply, Some(MASTER_MAC)).expect("table");
    assert_eq!(
        table.clusters.len(),
        1,
        "the master record is not a cluster"
    );
    let cluster = &table.clusters[0];
    assert_eq!(cluster.mac, fan_mac);
    assert_eq!(cluster.master_mac, MASTER_MAC);
    assert_eq!(cluster.channel, 8);
    assert_eq!(cluster.rx_type, 3);
    assert_eq!(cluster.fan_count, 3);
    assert!(!cluster.right_attach);
    assert_eq!(cluster.model, WirelessFanModel::TlV2 { lcd: false });
    assert_eq!(
        cluster.rpm[0], 1234,
        "status bits are masked out of the RPM"
    );
    assert_eq!(cluster.pwm, [128, 64, 32, 0]);
    assert_eq!(cluster.cmd_seq, 9);
    assert_eq!(cluster.effect_index, [0, 0, 0, 7]);
    assert_eq!(cluster.led_count(), 78);
}

#[test]
fn a_fan_count_of_ten_or_more_flags_a_right_attached_chain() {
    let reply = table_with(&[record([1; 6], 0x00, 12, 36)]);
    let cluster = &parse_device_table(&reply, None).expect("table").clusters[0];
    assert_eq!(cluster.fan_count, 2);
    assert!(cluster.right_attach);
    assert_eq!(cluster.model, WirelessFanModel::SlInf);
}

#[test]
fn a_record_without_the_validation_byte_is_dropped() {
    let mut bad = record([1; 6], 0x00, 2, 28);
    bad[41] = 0x00;
    let table = parse_device_table(&table_with(&[bad]), None).expect("table");
    assert!(table.clusters.is_empty());
}

#[test]
fn fan_type_bytes_classify_the_documented_models() {
    assert_eq!(
        WirelessFanModel::from_type_byte(27),
        WirelessFanModel::TlV2 { lcd: true }
    );
    assert_eq!(
        WirelessFanModel::from_type_byte(33),
        WirelessFanModel::TlV2 { lcd: true }
    );
    assert_eq!(
        WirelessFanModel::from_type_byte(30),
        WirelessFanModel::TlV2 { lcd: false }
    );
    assert_eq!(
        WirelessFanModel::from_type_byte(55),
        WirelessFanModel::TlV3 { lcd: true }
    );
    assert_eq!(
        WirelessFanModel::from_type_byte(53),
        WirelessFanModel::TlV3 { lcd: false }
    );
    assert_eq!(
        WirelessFanModel::from_type_byte(24),
        WirelessFanModel::SlV3 { lcd: true }
    );
    assert_eq!(
        WirelessFanModel::from_type_byte(99),
        WirelessFanModel::Unknown(99)
    );
    assert_eq!(WirelessFanModel::TlV2 { lcd: true }.leds_per_fan(), 26);
    assert_eq!(WirelessFanModel::SlV3 { lcd: false }.leds_per_fan(), 40);
}

// --- Envelopes (sections 6.1 to 6.3, 6.7, 6.8) ---

#[test]
fn an_envelope_ships_as_four_usb_packets_with_the_rf_header() {
    let envelope = pwm_envelope([1; 6], MASTER_MAC, 3, 8, 1, [10, 20, 30, 0]);
    assert_eq!(envelope.0.len(), RF_ENVELOPE_LEN);
    assert_eq!(envelope.0[0], RF_SELECT);
    assert_eq!(envelope.0[1], RfSubCommand::Pwm as u8);
    assert_eq!(&envelope.0[2..8], &[1; 6]);
    assert_eq!(&envelope.0[8..14], &MASTER_MAC);
    assert_eq!(&envelope.0[14..21], &[3, 8, 1, 10, 20, 30, 0]);

    let packets = envelope.usb_packets(8, 3);
    for (index, packet) in packets.iter().enumerate() {
        assert_eq!(packet.len(), USB_PACKET_LEN);
        assert_eq!(&packet[..4], &[USB_CMD_SEND_RF, index as u8, 8, 3]);
        assert_eq!(&packet[4..], &envelope.0[index * 60..(index + 1) * 60]);
    }
}

#[test]
fn the_first_clock_sync_of_a_session_carries_the_unset_sentinel() {
    let clock = WallClock {
        year: 2026,
        month: 9,
        day: 4,
        hour: 20,
        minute: 15,
        second: 30,
    };
    let payload = clock_payload(clock);
    assert_eq!(payload.len(), CLOCK_PAYLOAD_LEN);
    assert_eq!(&payload[32..39], &[0x07, 0xEA, 9, 4, 20, 15, 30]);
    assert!(
        payload[..32].iter().all(|&b| b == 0),
        "sensor fields stay zero"
    );

    let first = clock_sync_envelope(MASTER_MAC, &payload, true);
    assert_eq!(first.0[1], RfSubCommand::ClockSync as u8);
    assert_eq!(&first.0[2..8], &[0; 6], "clock sync is broadcast");
    assert!(first.0[14..64].iter().all(|&b| b == 0x14));
    assert_eq!(&first.0[64..234], &payload[50..]);

    let steady = clock_sync_envelope(MASTER_MAC, &payload, false);
    assert_eq!(&steady.0[14..234], &payload[..]);
}

#[test]
fn an_rgb_transfer_has_a_header_then_220_byte_chunks_of_compressed_data() {
    // 4 fans x 26 LEDs of pseudo-random color: incompressible enough to
    // need more than one data envelope.
    let mut state = 7_u32;
    let raw: Vec<u8> = (0..4 * 26 * 3)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (state >> 16) as u8
        })
        .collect();
    let compressed = tinyuz::compress(&raw, tinyuz::Params::default());
    let data_packets = compressed.len().div_ceil(RGB_CHUNK_LEN);
    assert!(data_packets >= 2, "the test needs a multi-chunk payload");

    let transfer = rgb_transfer(
        [1; 6],
        MASTER_MAC,
        [0xDE, 0xAD, 0xBE, 0xEF],
        26,
        1,
        5000,
        &raw,
    );

    let header = &transfer.header.0;
    assert_eq!(header[1], RfSubCommand::SetRgb as u8);
    assert_eq!(&header[14..18], &[0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(header[18], 0, "the header is packet zero");
    assert_eq!(
        usize::from(header[19]),
        data_packets + 1,
        "total counts the header"
    );
    assert_eq!(
        u32::from_be_bytes([header[20], header[21], header[22], header[23]]) as usize,
        compressed.len()
    );
    assert_eq!(&header[25..27], &[0, 1], "one frame: a live still");
    assert_eq!(header[27], 26);
    assert_eq!(&header[32..34], &5000_u16.to_be_bytes());

    assert_eq!(transfer.data.len(), data_packets);
    let mut reassembled = Vec::new();
    for (index, envelope) in transfer.data.iter().enumerate() {
        assert_eq!(usize::from(envelope.0[18]), index + 1);
        assert_eq!(usize::from(envelope.0[19]), data_packets + 1);
        let remaining = compressed.len() - index * RGB_CHUNK_LEN;
        let chunk = remaining.min(RGB_CHUNK_LEN);
        reassembled.extend_from_slice(&envelope.0[RGB_DATA_OFFSET..RGB_DATA_OFFSET + chunk]);
    }
    assert_eq!(
        reassembled, compressed,
        "the chunks carry the payload in order"
    );
    assert_eq!(
        tinyuz::decompress(&reassembled, raw.len()).expect("decodes"),
        raw
    );
}

#[test]
fn right_attached_chains_reverse_the_per_fan_runs() {
    let raw: Vec<u8> = (0..3 * 2 * 3).map(|i| i as u8).collect();
    let reversed = reverse_fan_order(&raw, 2, 3);
    assert_eq!(&reversed[..6], &raw[12..18]);
    assert_eq!(&reversed[6..12], &raw[6..12]);
    assert_eq!(&reversed[12..18], &raw[..6]);
    assert_eq!(reverse_fan_order(&raw, 2, 1), raw);
}

// --- Protocol ---

fn discovered_protocol() -> WirelessControllerProtocol {
    let protocol = WirelessControllerProtocol::new();
    protocol
        .parse_response(&captured_master_reply())
        .expect("MAC reply parses");
    let reply = table_with(&[
        record([0x11; 6], 0x00, 3, 28),
        record([0x22; 6], 0x00, 2, 27),
    ]);
    protocol.parse_response(&reply).expect("table parses");
    protocol
}

#[test]
fn init_resets_the_radio_learns_the_mac_then_polls_the_rx() {
    let protocol = WirelessControllerProtocol::new();
    let commands = protocol.init_sequence();
    assert_eq!(commands.len(), 3);

    assert_eq!(&commands[0].data[..4], &TX_RESET);
    assert_eq!(commands[0].transfer_type, TransferType::Primary);
    assert_eq!(
        commands[0].response.tolerance,
        ResponseTolerance::Optional,
        "the radio's reply to a reset is a courtesy"
    );
    assert!(commands[0].post_delay >= Duration::from_millis(500));

    assert_eq!(&commands[1].data[..2], &[USB_CMD_GET_MAC, 8]);
    assert!(commands[1].expects_response);
    assert_eq!(commands[1].response.tolerance, ResponseTolerance::Required);

    assert_eq!(&commands[2].data[..2], &[USB_CMD_SEND_RF, 2]);
    assert_eq!(
        commands[2].transfer_type,
        TransferType::Companion,
        "the device table comes from the RX"
    );
    assert_eq!(
        commands[2].response.capacity,
        Some(GET_DEV_REPLY_CAPACITY),
        "two pages of 448 bytes need more than one packet"
    );
    assert!(
        commands
            .iter()
            .all(|command| command.data.len() == USB_PACKET_LEN)
    );
}

#[test]
fn a_discovered_controller_exposes_one_ring_per_fan_in_table_order() {
    let protocol = discovered_protocol();
    assert_eq!(protocol.master().map(|master| master.mac), Some(MASTER_MAC));

    let zones = protocol.zones();
    let names: Vec<&str> = zones.iter().map(|zone| zone.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "UNI FAN TL Wireless 1 Fan 1",
            "UNI FAN TL Wireless 1 Fan 2",
            "UNI FAN TL Wireless 1 Fan 3",
            "UNI FAN TL Wireless LCD 2 Fan 1",
            "UNI FAN TL Wireless LCD 2 Fan 2",
        ]
    );
    assert!(zones.iter().all(|zone| {
        zone.led_count == 26 && zone.topology == DeviceTopologyHint::Ring { count: 26 }
    }));
    assert_eq!(protocol.total_leds(), 5 * 26);
    assert_eq!(protocol.capabilities().led_count, 5 * 26);
}

#[test]
fn the_first_frame_switches_the_radio_to_streaming_and_later_frames_do_not() {
    let protocol = discovered_protocol();
    let colors = vec![[255, 0, 0]; 5 * 26];

    let first = protocol.encode_frame(&colors);
    assert_eq!(&first[0].data[..4], &TX_VIDEO_START);
    assert_eq!(
        &first[1].data[..4],
        &[USB_CMD_SEND_RF, 0, 8, RF_BROADCAST_SLOT]
    );
    assert_eq!(
        &first[2].data[..4],
        &[USB_CMD_SEND_RF, 1, 8, RF_BROADCAST_SLOT]
    );
    // Two clusters, each a header envelope plus one data envelope of a
    // solid color, four USB packets per envelope.
    assert_eq!(first.len(), 3 + 2 * 2 * 4);
    assert!(first.iter().all(|command| !command.expects_response));
    assert!(
        first
            .iter()
            .all(|command| command.post_delay >= Duration::from_millis(1))
    );

    let header = &first[3].data;
    assert_eq!(&header[..4], &[USB_CMD_SEND_RF, 0, 8, 3]);
    assert_eq!(header[4], RF_SELECT);
    assert_eq!(header[5], RfSubCommand::SetRgb as u8);
    assert_eq!(&header[6..12], &[0x11; 6], "first cluster first");

    let second = protocol.encode_frame(&colors);
    assert_eq!(second.len(), 2 * 2 * 4, "no preamble once streaming");
    assert_ne!(
        &first[3].data[18..22],
        &second[0].data[18..22],
        "every transfer carries a fresh effect tag"
    );
}

#[test]
fn a_short_color_slice_pads_the_missing_fans_with_black() {
    let protocol = discovered_protocol();
    let commands = protocol.encode_frame(&[[9, 9, 9]; 10]);
    assert_eq!(commands.len(), 3 + 2 * 2 * 4);
}

#[test]
fn upkeep_polls_the_table_holds_pwm_steady_and_broadcasts_the_clock() {
    let protocol = discovered_protocol();
    let keepalive = protocol.keepalive().expect("the radio needs upkeep");
    assert_eq!(keepalive.interval, Duration::from_secs(1));

    let commands = protocol.keepalive_commands();
    assert_eq!(commands[0].transfer_type, TransferType::Companion);
    assert_eq!(commands[0].response.capacity, Some(GET_DEV_REPLY_CAPACITY));
    // Two PWM envelopes and one clock envelope, four packets each.
    assert_eq!(commands.len(), 1 + 3 * 4);

    let pwm = &commands[1].data;
    assert_eq!(pwm[5], RfSubCommand::Pwm as u8);
    assert_eq!(&pwm[6..12], &[0x11; 6]);
    assert_eq!(
        &pwm[18..25],
        &[3, 8, 1, 128, 64, 32, 0],
        "observed duty, never invented"
    );
    let second_pwm = &commands[5].data;
    assert_eq!(second_pwm[20], 2, "slot index counts clusters from one");

    let clock = &commands[9].data;
    assert_eq!(&clock[..4], &[USB_CMD_SEND_RF, 0, 8, RF_BROADCAST_SLOT]);
    assert_eq!(clock[5], RfSubCommand::ClockSync as u8);
    assert!(
        clock[18..64].iter().all(|&b| b == 0x14),
        "first tick sends the sentinel"
    );

    let later = protocol.keepalive_commands();
    assert!(
        later[9].data[18..64].iter().any(|&b| b != 0x14),
        "later ticks send the blob"
    );
}

#[test]
fn a_status_echo_mid_session_keeps_the_table() {
    let protocol = discovered_protocol();
    protocol
        .parse_response(&captured_tx_status_echo())
        .expect("a status packet is not an error");
    assert_eq!(protocol.clusters().len(), 2);
}

#[test]
fn a_new_session_forgets_the_last_table() {
    let protocol = discovered_protocol();
    let _ = protocol.init_sequence();
    assert!(protocol.clusters().is_empty());
    assert!(protocol.zones().is_empty());
}

#[test]
fn the_controller_is_registered_with_a_driver_owned_transport() {
    let descriptor = ProtocolDatabase::lookup(0x0416, 0x8040).expect("the TX is registered");
    assert_eq!(descriptor.name, "Lian Li L-Wireless Controller");
    assert_eq!(descriptor.protocol.id, "lianli/wireless");
    assert!(matches!(
        descriptor.transport,
        TransportType::DriverUsb { ref binding } if binding.id == "lianli/wireless"
    ));
    assert!(
        ProtocolDatabase::lookup(0x0416, 0x8041).is_none(),
        "the RX is the TX's companion, never a device of its own"
    );
}
