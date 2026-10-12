//! Frozen L-Wireless routing must reconnect when a complete table adds a fan.

use hypercolor_hal::drivers::lianli::WirelessControllerProtocol;
use hypercolor_hal::drivers::lianli::wireless::discovery::{RECORD_LEN, RECORD_VALIDATION};
use hypercolor_hal::protocol::Protocol;

const MASTER: [u8; 6] = [0xa0, 0x71, 0xae, 0x72, 0xab, 0x3c];
const FIRST: [u8; 6] = [0x31; 6];
const SECOND: [u8; 6] = [0x32; 6];

fn fan(mac: [u8; 6]) -> [u8; RECORD_LEN] {
    let mut record = [0; RECORD_LEN];
    record[..6].copy_from_slice(&mac);
    record[6..12].copy_from_slice(&MASTER);
    record[12] = 8;
    record[13] = 1;
    record[19] = 1;
    record[24] = 27;
    record[36..40].copy_from_slice(&[128; 4]);
    record[41] = RECORD_VALIDATION;
    record
}

fn table(records: &[[u8; RECORD_LEN]]) -> Vec<u8> {
    let mut reply = vec![0; 448];
    reply[0] = 0x10;
    reply[1] = u8::try_from(records.len()).expect("small fixture table");
    reply[2] = 0x80;
    for (index, record) in records.iter().enumerate() {
        let offset = 4 + index * RECORD_LEN;
        reply[offset..offset + RECORD_LEN].copy_from_slice(record);
    }
    reply
}

fn discovered(records: &[[u8; RECORD_LEN]]) -> WirelessControllerProtocol {
    let protocol = WirelessControllerProtocol::new();
    let mut master = [0; 64];
    master[0] = 0x11;
    master[1..7].copy_from_slice(&MASTER);
    master[12] = 0x10;
    protocol.parse_response(&master).expect("master reply");
    protocol
        .parse_response(&table(records))
        .expect("initial table");
    protocol
}

#[test]
fn a_fan_returning_after_empty_startup_requests_reconnect() {
    for upkeep_first in [false, true] {
        let protocol = discovered(&[]);
        if upkeep_first {
            protocol.keepalive_commands();
        } else {
            protocol.encode_frame(&[]);
        }
        assert_eq!(protocol.total_leds(), 0);
        assert!(protocol.session_restart().is_none());

        protocol
            .parse_response(&table(&[fan(FIRST)]))
            .expect("late fan");

        assert!(
            protocol.session_restart().is_some(),
            "late fans need published routing"
        );
        assert_eq!(
            protocol.total_leds(),
            0,
            "routing stays frozen until reconnect"
        );
        assert!(protocol.zones().is_empty());
        protocol.init_sequence();
        assert!(
            protocol.session_restart().is_none(),
            "new session clears the request"
        );
    }
}

#[test]
fn a_new_group_requests_reconnect_without_expanding_existing_routing() {
    let protocol = discovered(&[fan(FIRST)]);
    protocol.encode_frame(&[[12, 34, 56]; 26]);

    protocol
        .parse_response(&table(&[fan(SECOND), fan(FIRST)]))
        .expect("added group");

    assert!(protocol.session_restart().is_some());
    assert_eq!(protocol.total_leds(), 26);
    assert_eq!(protocol.clusters().len(), 1);
    assert_eq!(
        protocol.clusters()[0].mac,
        FIRST,
        "existing LED offsets remain stable"
    );
}

#[test]
fn a_missing_known_group_can_return_without_topology_reconnect() {
    let protocol = discovered(&[fan(FIRST), fan(SECOND)]);
    protocol.encode_frame(&[[12, 34, 56]; 52]);

    protocol
        .parse_response(&table(&[fan(FIRST)]))
        .expect("temporary omission");
    assert!(protocol.session_restart().is_none());
    protocol
        .parse_response(&table(&[fan(SECOND), fan(FIRST)]))
        .expect("known group returns");

    assert!(protocol.session_restart().is_none());
    assert_eq!(protocol.total_leds(), 52);
    assert_eq!(protocol.clusters()[0].mac, FIRST);
    assert_eq!(protocol.clusters()[1].mac, SECOND);
}

#[test]
fn an_invalid_table_cannot_request_topology_reconnect() {
    let protocol = discovered(&[]);
    protocol.keepalive_commands();
    let mut invalid = fan(SECOND);
    invalid[41] = 0;

    protocol
        .parse_response(&table(&[fan(FIRST), invalid]))
        .expect("invalid record is ignored");

    assert!(
        protocol.session_restart().is_none(),
        "membership is not trustworthy"
    );
    assert_eq!(protocol.total_leds(), 0);
    protocol
        .parse_response(&table(&[fan(FIRST)]))
        .expect("complete valid membership");
    assert!(protocol.session_restart().is_some());
}

#[test]
fn a_truncated_table_cannot_request_topology_reconnect() {
    let protocol = discovered(&[]);
    protocol.keepalive_commands();
    let mut partial = table(&[fan(FIRST), fan(SECOND)]);
    partial.truncate(4 + RECORD_LEN + 10);

    protocol
        .parse_response(&partial)
        .expect("partial telemetry is tolerated");

    assert!(
        protocol.session_restart().is_none(),
        "one valid record does not complete the table"
    );
    assert_eq!(protocol.total_leds(), 0);
    protocol
        .parse_response(&table(&[fan(FIRST), fan(SECOND)]))
        .expect("complete membership");
    assert!(protocol.session_restart().is_some());
}
