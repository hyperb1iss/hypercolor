//! Golden-byte tests for Art-Net 4 ArtDmx packets.
//!
//! Expected bytes follow the ArtDmx field table of the Art-Net 4
//! specification: ID, OpCode low byte first, ProtVerHi/Lo, Sequence,
//! Physical, SubUni, Net, LengthHi/Length (high byte first), Data.

use hypercolor_pixel_protocols::artnet::{
    ARTDMX_HEADER_SIZE, ARTNET_MAX_PORT_ADDRESS, ARTNET_PORT, ArtDmxPacket, ArtDmxSequenceTracker,
    PortAddress,
};

#[test]
fn artnet_port_is_0x1936() {
    assert_eq!(ARTNET_PORT, 0x1936);
}

#[test]
fn artdmx_matches_standard_layout() {
    let port = PortAddress::from_parts(1, 2, 3).expect("valid port address");
    let mut packet = ArtDmxPacket::new(port, 0);
    packet.set_data(&[0x10, 0x20, 0x30, 0x40], 1);

    let expected = [
        b'A', b'r', b't', b'-', b'N', b'e', b't', 0x00, // ID
        0x00, 0x50, // OpCode 0x5000, low byte first
        0x00, 0x0E, // ProtVerHi, ProtVerLo = 14
        0x01, // Sequence
        0x00, // Physical
        0x23, // SubUni: Sub-Net 2, Universe 3
        0x01, // Net
        0x00, 0x04, // LengthHi, Length
        0x10, 0x20, 0x30, 0x40, // Data
    ];
    assert_eq!(packet.as_bytes(), expected.as_slice());
    assert_eq!(ARTDMX_HEADER_SIZE, 18);
}

#[test]
fn artdmx_pads_odd_length_to_even() {
    let port = PortAddress::new(1).expect("valid port address");
    let mut packet = ArtDmxPacket::new(port, 0);
    packet.set_data(&[0xAA, 0xBB, 0xCC], 9);

    assert_eq!(packet.data_length(), 4);
    assert_eq!(&packet.as_bytes()[16..18], &[0x00, 0x04]);
    assert_eq!(&packet.as_bytes()[18..], &[0xAA, 0xBB, 0xCC, 0x00]);
}

#[test]
fn artdmx_minimum_length_is_two() {
    let port = PortAddress::new(1).expect("valid port address");
    let mut packet = ArtDmxPacket::new(port, 0);
    packet.set_data(&[], 1);
    assert_eq!(packet.data_length(), 2);
    assert_eq!(&packet.as_bytes()[18..], &[0x00, 0x00]);

    packet.set_data(&[0x7F], 2);
    assert_eq!(&packet.as_bytes()[18..], &[0x7F, 0x00]);
}

#[test]
fn artdmx_full_universe_is_530_bytes() {
    let port = PortAddress::new(ARTNET_MAX_PORT_ADDRESS).expect("valid port address");
    let mut packet = ArtDmxPacket::new(port, 0);
    let data = vec![0x55; 600];
    packet.set_data(&data, 200);

    let bytes = packet.as_bytes();
    assert_eq!(bytes.len(), 530);
    assert_eq!(&bytes[14..18], &[0xFF, 0x7F, 0x02, 0x00]);
    assert!(bytes[18..].iter().all(|&b| b == 0x55));
}

#[test]
fn artdmx_shorter_frame_clears_previous_tail() {
    let port = PortAddress::new(1).expect("valid port address");
    let mut packet = ArtDmxPacket::new(port, 0);
    packet.set_data(&[9, 9, 9, 9, 9, 9], 1);
    packet.set_data(&[1, 2, 3], 2);
    assert_eq!(&packet.as_bytes()[18..], &[1, 2, 3, 0]);
}

#[test]
fn port_address_splits_into_net_subnet_universe() {
    let port = PortAddress::new(0x1234).expect("valid port address");
    assert_eq!(port.net(), 0x12);
    assert_eq!(port.sub_net(), 0x3);
    assert_eq!(port.universe(), 0x4);
    assert_eq!(port.sub_uni(), 0x34);
    assert_eq!(PortAddress::from_parts(0x12, 0x3, 0x4), Some(port));

    let packet = ArtDmxPacket::new(port, 0);
    assert_eq!(packet.port_address(), port);
}

#[test]
fn port_address_rejects_out_of_range_parts() {
    assert!(PortAddress::new(0x8000).is_none());
    assert!(PortAddress::from_parts(0x80, 0, 0).is_none());
    assert!(PortAddress::from_parts(0, 0x10, 0).is_none());
    assert!(PortAddress::from_parts(0, 0, 0x10).is_none());
}

#[test]
fn sequence_runs_one_to_255_and_skips_zero() {
    let port = PortAddress::new(1).expect("valid port address");
    let other = PortAddress::new(2).expect("valid port address");
    let mut tracker = ArtDmxSequenceTracker::default();
    assert_eq!(tracker.advance(port), 1);
    assert_eq!(tracker.advance(other), 1);
    for _ in 0..254 {
        tracker.advance(port);
    }
    assert_eq!(tracker.current(port), 255);
    assert_eq!(tracker.advance(port), 1, "sequence 0 disables reordering");
}
