use hypercolor_core::device::hue::{CieXyb, HueChannel, HuePosition, encode_packet_into};

#[test]
fn encode_packet_into_writes_huestream_header_and_channel_payload() {
    let mut packet = Vec::new();
    let channels = vec![HueChannel {
        id: 1,
        name: "Left".to_owned(),
        position: HuePosition {
            x: -0.5,
            y: 0.0,
            z: 0.0,
        },
        segment_count: 1,
        members: Vec::new(),
    }];
    let colors = vec![CieXyb {
        x: 0.5,
        y: 0.25,
        brightness: 1.0,
    }];

    encode_packet_into(
        &mut packet,
        "12345678-1234-1234-1234-123456789abc",
        7,
        channels.as_slice(),
        colors.as_slice(),
    )
    .expect("packet should encode");

    assert_eq!(&packet[..9], b"HueStream");
    assert_eq!(packet[9], 0x02);
    assert_eq!(packet[10], 0x00);
    assert_eq!(packet[11], 7);
    assert_eq!(packet[14], 0x01);
    assert_eq!(&packet[16..52], b"12345678-1234-1234-1234-123456789abc");
    assert_eq!(packet[52], 1);
    assert_eq!(&packet[53..55], &32_768_u16.to_be_bytes());
    assert_eq!(&packet[55..57], &16_384_u16.to_be_bytes());
    assert_eq!(&packet[57..59], &u16::MAX.to_be_bytes());
}

#[test]
fn encode_packet_into_rejects_invalid_config_ids() {
    let mut packet = Vec::new();
    let error = encode_packet_into(&mut packet, "short-id", 0, &[], &[]);
    assert!(error.is_err(), "invalid config IDs should be rejected");
}
