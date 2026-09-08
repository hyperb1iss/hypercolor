use hypercolor_driver_hue::{
    CieXyb, HueChannel, HuePosition, encode_packet_into, encode_rgb_packet_into,
};

const CONFIG_ID: &str = "12345678-1234-1234-1234-123456789abc";

fn channel(id: u8) -> HueChannel {
    HueChannel {
        id,
        name: format!("Channel {id}"),
        position: HuePosition {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        segment_count: 1,
        members: Vec::new(),
    }
}

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

#[test]
fn rgb_packets_preserve_full_saturated_colors_and_dim_channel_values() {
    let cases = [
        ([255, 0, 0], [255, 255, 0, 0, 0, 0]),
        ([0, 255, 0], [0, 0, 255, 255, 0, 0]),
        ([0, 0, 255], [0, 0, 0, 0, 255, 255]),
        ([255, 255, 255], [255; 6]),
        ([0, 0, 0], [0; 6]),
        ([1, 64, 128], [1, 1, 64, 64, 128, 128]),
    ];
    for (rgb, expected) in cases {
        let mut packet = Vec::new();
        encode_rgb_packet_into(&mut packet, CONFIG_ID, 255, &[channel(7)], &[rgb])
            .expect("RGB packet should encode");

        assert_eq!(packet.len(), 59);
        assert_eq!(&packet[..16], b"HueStream\x02\x00\xff\x00\x00\x00\x00");
        assert_eq!(&packet[16..52], CONFIG_ID.as_bytes());
        assert_eq!(packet[52], 7);
        assert_eq!(&packet[53..], &expected, "RGB {rgb:?}");
    }
}

#[test]
fn rgb_packets_follow_channel_order_and_replace_missing_colors_with_black() {
    let mut packet = vec![255; 100];
    encode_rgb_packet_into(
        &mut packet,
        CONFIG_ID,
        0,
        &[channel(9), channel(2)],
        &[[255, 6, 181]],
    )
    .expect("RGB packet should encode");

    assert_eq!(packet.len(), 66);
    assert_eq!(&packet[52..59], &[9, 255, 255, 6, 6, 181, 181]);
    assert_eq!(&packet[59..], &[2, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn rgb_packets_ignore_extra_colors_and_reset_reused_xy_headers() {
    let mut packet = Vec::new();
    encode_packet_into(&mut packet, CONFIG_ID, 4, &[channel(1), channel(2)], &[])
        .expect("CIE packet should encode");
    assert_eq!(packet[14], 1);

    encode_rgb_packet_into(
        &mut packet,
        CONFIG_ID,
        5,
        &[channel(3)],
        &[[0, 0, 0], [255, 255, 255]],
    )
    .expect("RGB packet should encode");
    assert_eq!(packet[14], 0);
    assert_eq!(packet[11], 5);
    assert_eq!(packet.len(), 59);
    assert_eq!(&packet[52..], &[3, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn rgb_packets_validate_config_ids_and_entertainment_channel_limit() {
    let mut packet = Vec::new();
    assert!(encode_rgb_packet_into(&mut packet, "short-id", 0, &[], &[]).is_err());
    assert!(encode_rgb_packet_into(&mut packet, &"é".repeat(18), 0, &[], &[]).is_err());

    let channels = (0..21).map(channel).collect::<Vec<_>>();
    assert!(encode_rgb_packet_into(&mut packet, CONFIG_ID, 0, &channels, &[]).is_err());
    encode_rgb_packet_into(&mut packet, CONFIG_ID, 0, &channels[..20], &[])
        .expect("twenty entertainment channels should encode");
    assert_eq!(packet.len(), 192);
}
