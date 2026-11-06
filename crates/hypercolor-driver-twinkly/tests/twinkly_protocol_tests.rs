//! Golden bytes for the discovery datagrams and every real-time datagram
//! version (Spec 92 §3 and §6).

use std::net::Ipv4Addr;

use hypercolor_driver_twinkly::protocol::{
    DISCOVERY_REQUEST, DiscoveryReply, FirmwareVersion, LedProfile, ProtocolError, RtFrameEncoder,
    RtProtocol, V3_FRAGMENT_PAYLOAD, decode_discovery_reply, encode_discovery_reply,
};

const TOKEN: [u8; 8] = [0xe6, 0x33, 0xde, 0xf8, 0xe3, 0x61, 0xc1, 0x46];

#[test]
fn discovery_request_is_version_byte_and_word() {
    assert_eq!(
        DISCOVERY_REQUEST,
        [0x01, 0x64, 0x69, 0x73, 0x63, 0x6f, 0x76, 0x65, 0x72]
    );
}

#[test]
fn discovery_reply_reverses_ip_octets() {
    let datagram = [
        0x2d, 0x01, 0xa8, 0xc0, b'O', b'K', b'T', b'w', b'i', b'n', b'k', b'l', b'y', b'_', b'3',
        b'3', b'A', b'E', b'5', b'C', 0x00,
    ];
    let reply = decode_discovery_reply(&datagram).expect("valid reply");
    assert_eq!(reply.ip, Ipv4Addr::new(192, 168, 1, 45));
    assert_eq!(reply.device_id, "Twinkly_33AE5C");
    assert_eq!(encode_discovery_reply(&reply), datagram);
}

#[test]
fn discovery_reply_without_terminator_still_decodes() {
    let reply = decode_discovery_reply(b"\x0a\x00\x00\x0aOKtree").expect("valid reply");
    assert_eq!(reply.ip, Ipv4Addr::new(10, 0, 0, 10));
    assert_eq!(reply.device_id, "tree");
}

#[test]
fn discovery_reply_rejects_short_and_unmarked_datagrams() {
    assert_eq!(
        decode_discovery_reply(b"\x01\x02OK"),
        Err(ProtocolError::DiscoveryTooShort(4))
    );
    assert_eq!(
        decode_discovery_reply(b"\x01\x02\x03\x04NO-name\x00"),
        Err(ProtocolError::DiscoveryMissingMarker)
    );
    let ours = encode_discovery_reply(&DiscoveryReply {
        ip: Ipv4Addr::new(1, 2, 3, 4),
        device_id: String::new(),
    });
    assert_eq!(ours, [4, 3, 2, 1, b'O', b'K', 0]);
}

#[test]
fn firmware_versions_compare_numerically() {
    let parse = |raw: &str| raw.parse::<FirmwareVersion>().expect("version parses");
    assert!(parse("2.4.14") > parse("2.4.6"));
    assert!(parse("2.10.0") > parse("2.9.99"));
    assert_eq!(parse("2.3"), FirmwareVersion::new(2, 3, 0));
    assert_eq!(parse(" 1.99.18 ").to_string(), "1.99.18");
    assert!("2.0.22-mqtt".parse::<FirmwareVersion>().is_err());
    assert!("".parse::<FirmwareVersion>().is_err());
    assert!("2.4.6.1".parse::<FirmwareVersion>().is_err());
}

#[test]
fn protocol_follows_family_and_firmware() {
    let version = |raw: &str| raw.parse::<FirmwareVersion>().expect("version parses");
    let cases = [
        (None, "1.99.30", RtProtocol::V1),
        (None, "2.3.8", RtProtocol::V1),
        (Some("D"), "2.3.8", RtProtocol::V1),
        (Some("d"), "2.5.0", RtProtocol::V1),
        (Some("F"), "2.2.1", RtProtocol::V2),
        (Some("F"), "2.4.6", RtProtocol::V2),
        (Some("F"), "2.4.10", RtProtocol::V2),
        (Some("F"), "2.4.14", RtProtocol::V3),
        (Some("G"), "2.4.21", RtProtocol::V3),
        (Some("G"), "2.8.3", RtProtocol::V3),
        (None, "2.8.3", RtProtocol::V3),
        (Some(" "), "2.4.16", RtProtocol::V3),
    ];
    for (family, firmware, expected) in cases {
        assert_eq!(
            RtProtocol::for_device(family, version(firmware)),
            expected,
            "family {family:?} firmware {firmware}"
        );
    }
}

#[test]
fn led_profiles_parse_and_order_channels() {
    assert_eq!(LedProfile::parse("RGB"), Ok(LedProfile::Rgb));
    assert_eq!(LedProfile::parse(" rgbw "), Ok(LedProfile::Rgbw));
    assert!(LedProfile::parse("GRB").is_err());

    let mut frame = Vec::new();
    LedProfile::Rgb.push_led(&mut frame, [1, 2, 3]);
    LedProfile::Rgbw.push_led(&mut frame, [4, 5, 6]);
    assert_eq!(frame, [1, 2, 3, 0, 4, 5, 6]);
}

#[test]
fn version_one_datagram_carries_led_count() {
    let mut encoder =
        RtFrameEncoder::new(RtProtocol::V1, LedProfile::Rgb, 3, TOKEN).expect("encoder");
    let datagrams = encoder.encode(&[[255, 0, 0], [0, 255, 0], [0, 0, 255]]);
    assert_eq!(datagrams.len(), 1);
    assert_eq!(
        datagrams[0],
        [
            0x01, 0xe6, 0x33, 0xde, 0xf8, 0xe3, 0x61, 0xc1, 0x46, 0x03, 255, 0, 0, 0, 255, 0, 0, 0,
            255,
        ]
    );
}

#[test]
fn version_one_rejects_more_than_255_leds() {
    assert_eq!(
        RtFrameEncoder::new(RtProtocol::V1, LedProfile::Rgb, 256, TOKEN).err(),
        Some(ProtocolError::V1TooManyLeds { leds: 256 })
    );
    assert!(RtFrameEncoder::new(RtProtocol::V1, LedProfile::Rgb, 255, TOKEN).is_ok());
}

#[test]
fn version_two_datagram_has_zero_byte_and_rgbw_body() {
    let mut encoder =
        RtFrameEncoder::new(RtProtocol::V2, LedProfile::Rgbw, 2, TOKEN).expect("encoder");
    let datagrams = encoder.encode(&[[10, 20, 30], [40, 50, 60]]);
    assert_eq!(datagrams.len(), 1);
    assert_eq!(
        datagrams[0],
        [
            0x02, 0xe6, 0x33, 0xde, 0xf8, 0xe3, 0x61, 0xc1, 0x46, 0x00, 0, 10, 20, 30, 0, 40, 50,
            60,
        ]
    );
}

#[test]
fn version_three_splits_frames_into_900_byte_fragments() {
    let mut encoder =
        RtFrameEncoder::new(RtProtocol::V3, LedProfile::Rgb, 750, TOKEN).expect("encoder");
    assert_eq!(encoder.datagrams_per_frame(), 3);
    let colors: Vec<[u8; 3]> = (0..750_u16)
        .map(|index| [(index % 256) as u8, 0x11, 0x22])
        .collect();
    let datagrams = encoder.encode(&colors).to_vec();
    let sizes: Vec<usize> = datagrams.iter().map(Vec::len).collect();
    assert_eq!(sizes, [12 + 900, 12 + 900, 12 + 450]);
    for (index, datagram) in datagrams.iter().enumerate() {
        assert_eq!(datagram[0], 0x03);
        assert_eq!(&datagram[1..9], &TOKEN);
        assert_eq!(&datagram[9..11], &[0, 0]);
        assert_eq!(usize::from(datagram[11]), index);
    }
    // LED 300 opens the second fragment: 300 * 3 = 900 bytes in.
    assert_eq!(&datagrams[1][12..15], &[44, 0x11, 0x22]);
    let rejoined: Vec<u8> = datagrams
        .iter()
        .flat_map(|datagram| datagram[12..].iter().copied())
        .collect();
    let expected: Vec<u8> = colors.iter().flatten().copied().collect();
    assert_eq!(rejoined, expected);
}

#[test]
fn version_three_small_frame_is_one_fragment() {
    let mut encoder =
        RtFrameEncoder::new(RtProtocol::V3, LedProfile::Rgbw, 1, TOKEN).expect("encoder");
    let datagrams = encoder.encode(&[[9, 8, 7]]);
    assert_eq!(
        datagrams,
        [vec![
            0x03, 0xe6, 0x33, 0xde, 0xf8, 0xe3, 0x61, 0xc1, 0x46, 0x00, 0x00, 0x00, 0, 9, 8, 7,
        ]]
    );
}

#[test]
fn encoder_pads_short_frames_and_truncates_long_ones() {
    let mut encoder =
        RtFrameEncoder::new(RtProtocol::V2, LedProfile::Rgb, 2, TOKEN).expect("encoder");
    assert_eq!(&encoder.encode(&[[1, 2, 3]])[0][10..], &[1, 2, 3, 0, 0, 0]);
    assert_eq!(
        &encoder.encode(&[[1, 1, 1], [2, 2, 2], [3, 3, 3]])[0][10..],
        &[1, 1, 1, 2, 2, 2]
    );
}

#[test]
fn encoder_takes_a_fresh_token() {
    let mut encoder =
        RtFrameEncoder::new(RtProtocol::V3, LedProfile::Rgb, 1, TOKEN).expect("encoder");
    encoder.set_token([1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        &encoder.encode(&[[0, 0, 0]])[0][1..9],
        &[1, 2, 3, 4, 5, 6, 7, 8]
    );
}

#[test]
fn fragment_count_matches_frame_bytes() {
    assert_eq!(RtProtocol::V3.datagrams_for(0), 1);
    assert_eq!(RtProtocol::V3.datagrams_for(V3_FRAGMENT_PAYLOAD), 1);
    assert_eq!(RtProtocol::V3.datagrams_for(V3_FRAGMENT_PAYLOAD + 1), 2);
    assert_eq!(RtProtocol::V2.datagrams_for(4_000), 1);
}
