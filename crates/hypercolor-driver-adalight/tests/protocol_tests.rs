//! Golden-byte tests for the Adalight and AWA wire framing (Spec 91 §3-5).

use std::time::Duration;

use hypercolor_driver_adalight::ColorOrder;
use hypercolor_driver_adalight::protocol::{
    ADA_MAGIC, AWA_CALIBRATED_MAGIC, AWA_MAGIC, AWA_RGBW_MAGIC, AdalightProtocol, AwaWhite,
    Bandwidth, FrameEncoder, FrameLayout, FrameLayoutError, Framing, WhiteCalibration, awa_trailer,
    extract_white, header,
};

fn layout(framing: Framing, led_count: u32) -> FrameLayout {
    FrameLayout::new(framing, led_count).expect("valid layout")
}

// ── Header ──────────────────────────────────────────────────────────────

#[test]
fn ada_header_carries_count_minus_one_and_xor_check() {
    assert_eq!(header(ADA_MAGIC, 0), [0x41, 0x64, 0x61, 0x00, 0x00, 0x55]);
    assert_eq!(
        layout(Framing::Ada, 1).header(),
        [0x41, 0x64, 0x61, 0x00, 0x00, 0x55]
    );
    // 300 LEDs: count field 299 = 0x012B, check 0x01 ^ 0x2B ^ 0x55 = 0x7F.
    assert_eq!(
        layout(Framing::Ada, 300).header(),
        [0x41, 0x64, 0x61, 0x01, 0x2B, 0x7F]
    );
    // The largest count fills both bytes.
    assert_eq!(
        layout(Framing::Ada, 65_536).header(),
        [0x41, 0x64, 0x61, 0xFF, 0xFF, 0x55]
    );
}

#[test]
fn awa_magic_selects_the_white_variant() {
    assert_eq!(ADA_MAGIC, *b"Ada");
    assert_eq!(AWA_MAGIC, *b"Awa");
    assert_eq!(AWA_CALIBRATED_MAGIC, *b"AwA");
    assert_eq!(AWA_RGBW_MAGIC, *b"AWa");
    assert_eq!(Framing::Awa(AwaWhite::Off).magic(), *b"Awa");
    assert_eq!(
        Framing::Awa(AwaWhite::Firmware(WhiteCalibration::COLD_WHITE)).magic(),
        *b"AwA"
    );
    assert_eq!(Framing::Awa(AwaWhite::Host).magic(), *b"AWa");
}

#[test]
fn layout_rejects_counts_outside_each_variant() {
    assert!(matches!(
        FrameLayout::new(Framing::Ada, 0),
        Err(FrameLayoutError::LedCount { led_count: 0, .. })
    ));
    assert!(FrameLayout::new(Framing::Ada, 65_536).is_ok());
    assert!(matches!(
        FrameLayout::new(Framing::Ada, 65_537),
        Err(FrameLayoutError::LedCount { max: 65_536, .. })
    ));
    // HyperSerial drops frames above 4096 LEDs.
    assert!(FrameLayout::new(Framing::Awa(AwaWhite::Off), 4_096).is_ok());
    let error = FrameLayout::new(Framing::Awa(AwaWhite::Off), 4_097).expect_err("too many LEDs");
    assert_eq!(
        error,
        FrameLayoutError::LedCount {
            led_count: 4_097,
            max: 4_096,
            protocol: AdalightProtocol::Awa,
        }
    );
}

// ── Trailer ─────────────────────────────────────────────────────────────

#[test]
fn awa_trailer_runs_fletcher_sums_and_the_position_mixed_sum() {
    // s1: 0x10, 0x30, 0x60. s2: 0x10, 0x40, 0xA0.
    // ext: 0x10^0 + 0x20^1 + 0x30^2 = 0x10 + 0x21 + 0x32 = 0x63.
    assert_eq!(awa_trailer(&[0x10, 0x20, 0x30]), [0x60, 0xA0, 0x63]);
}

#[test]
fn awa_trailer_reduces_modulo_255() {
    assert_eq!(awa_trailer(&[0xFF]), [0x00, 0x00, 0x00]);
}

#[test]
fn awa_trailer_never_sends_an_ext_of_0x41() {
    assert_eq!(awa_trailer(&[0x41]), [0x41, 0x41, 0xAA]);
}

#[test]
fn awa_trailer_position_wraps_at_256() {
    // Positions run 0..=255 then 0, 1: the sum of positions is
    // 32640 + 0 + 1, which is 1 modulo 255. Without the wrap it would be 3.
    assert_eq!(awa_trailer(&[0; 258]), [0x00, 0x00, 0x01]);
}

// ── Full frames ─────────────────────────────────────────────────────────

#[test]
fn ada_frame_is_header_then_rgb() {
    let mut encoder = FrameEncoder::new(layout(Framing::Ada, 2), ColorOrder::Rgb);
    assert_eq!(
        encoder.encode(&[[0xFF, 0x00, 0x00], [0x00, 0x80, 0xFF]]),
        [
            0x41, 0x64, 0x61, 0x00, 0x01, 0x54, // header, count field 1
            0xFF, 0x00, 0x00, 0x00, 0x80, 0xFF, // two LEDs, no trailer
        ]
    );
}

#[test]
fn awa_frame_appends_the_trailer() {
    let mut encoder = FrameEncoder::new(layout(Framing::Awa(AwaWhite::Off), 1), ColorOrder::Rgb);
    assert_eq!(
        encoder.encode(&[[0x10, 0x20, 0x30]]),
        [
            0x41, 0x77, 0x61, 0x00, 0x00, 0x55, // "Awa" header
            0x10, 0x20, 0x30, // RGB
            0x60, 0xA0, 0x63, // s1, s2, ext
        ]
    );
}

#[test]
fn awa_color_order_is_applied_before_the_checksum() {
    let mut encoder = FrameEncoder::new(layout(Framing::Awa(AwaWhite::Off), 2), ColorOrder::Grb);
    assert_eq!(
        encoder.encode(&[[0x0A, 0x0D, 0x11], [0x13, 0x41, 0xFF]]),
        [
            0x41, 0x77, 0x61, 0x00, 0x01, 0x54, // header
            0x0D, 0x0A, 0x11, 0x41, 0x13, 0xFF, // GRB
            0x7C, 0xAE, 0x7F, // trailer over the reordered bytes
        ]
    );
}

#[test]
fn awa_calibrated_frame_covers_the_calibration_block_in_the_checksum() {
    let mut encoder = FrameEncoder::new(
        layout(
            Framing::Awa(AwaWhite::Firmware(WhiteCalibration::COLD_WHITE)),
            1,
        ),
        ColorOrder::Rgb,
    );
    assert_eq!(
        encoder.encode(&[[0, 0, 0]]),
        [
            0x41, 0x77, 0x41, 0x00, 0x00, 0x55, // "AwA" header
            0x00, 0x00, 0x00, // RGB
            0xFF, 0xA0, 0xA0, 0xA0, // gain, red, green, blue
            0xE1, 0xC3, 0xF0, // trailer over RGB and calibration
        ]
    );
}

#[test]
fn awa_host_white_frame_sends_four_bytes_per_led() {
    let mut encoder = FrameEncoder::new(layout(Framing::Awa(AwaWhite::Host), 1), ColorOrder::Rgb);
    assert_eq!(
        encoder.encode(&[[200, 100, 50]]),
        [
            0x41, 0x57, 0x61, 0x00, 0x00, 0x55, // "AWa" header
            0x96, 0x32, 0x00, 0x32, // R-W, G-W, B-W, W
            0xFA, 0x23, 0xFC, // trailer over 4 bytes per LED
        ]
    );
}

#[test]
fn host_white_keeps_white_after_the_reordered_colors() {
    let mut encoder = FrameEncoder::new(layout(Framing::Awa(AwaWhite::Host), 1), ColorOrder::Bgr);
    let frame = encoder.encode(&[[60, 20, 10]]).to_vec();
    assert_eq!(&frame[6..10], &[0, 10, 50, 10]);
}

#[test]
fn extract_white_takes_the_shared_minimum() {
    assert_eq!(extract_white([200, 100, 50]), ([150, 50, 0], 50));
    assert_eq!(extract_white([255, 255, 255]), ([0, 0, 0], 255));
    assert_eq!(extract_white([255, 0, 128]), ([255, 0, 128], 0));
}

#[test]
fn encoder_pads_short_frames_with_black_and_truncates_long_ones() {
    let mut encoder = FrameEncoder::new(layout(Framing::Ada, 3), ColorOrder::Rgb);
    let short = encoder.encode(&[[1, 2, 3]]).to_vec();
    assert_eq!(short.len(), 6 + 9);
    assert_eq!(&short[6..], &[1, 2, 3, 0, 0, 0, 0, 0, 0]);

    let long = encoder
        .encode(&[[1, 1, 1], [2, 2, 2], [3, 3, 3], [4, 4, 4]])
        .to_vec();
    assert_eq!(long.len(), 6 + 9);
    assert_eq!(&long[6..], &[1, 1, 1, 2, 2, 2, 3, 3, 3]);
}

#[test]
fn encoder_starts_as_a_valid_black_frame() {
    let encoder = FrameEncoder::new(layout(Framing::Awa(AwaWhite::Off), 3), ColorOrder::Rgb);
    let mut expected = vec![0x41, 0x77, 0x61, 0x00, 0x02, 0x57];
    expected.extend_from_slice(&[0; 9]);
    // ext over nine zero bytes is the sum of positions 0..=8 = 36.
    expected.extend_from_slice(&[0x00, 0x00, 0x24]);
    assert_eq!(encoder.as_bytes(), expected.as_slice());

    let mut encoder = encoder;
    encoder.encode(&[[9, 9, 9], [9, 9, 9], [9, 9, 9]]);
    assert_eq!(encoder.encode_black(), expected.as_slice());
}

#[test]
fn frame_lengths_cover_header_payload_and_suffix() {
    assert_eq!(layout(Framing::Ada, 300).frame_len(), 906);
    assert_eq!(layout(Framing::Awa(AwaWhite::Off), 300).frame_len(), 909);
    assert_eq!(
        layout(
            Framing::Awa(AwaWhite::Firmware(WhiteCalibration::COLD_WHITE)),
            300
        )
        .frame_len(),
        913
    );
    assert_eq!(layout(Framing::Awa(AwaWhite::Host), 300).frame_len(), 1_209);
}

// ── Bandwidth ───────────────────────────────────────────────────────────

#[test]
fn bandwidth_uses_ten_line_bits_per_byte() {
    // 300 LEDs of classic Adalight at 115200 baud: 906 bytes, 9060 bits.
    let classic = layout(Framing::Ada, 300).bandwidth(115_200);
    assert_eq!(classic.frame_bits(), 9_060);
    assert_eq!(classic.max_fps(), 12);
    assert_eq!(classic.frame_time(), Duration::from_micros(78_646));

    // The same strip on HyperSerial's 2 Mbaud default: 909 bytes.
    let awa = layout(Framing::Awa(AwaWhite::Off), 300).bandwidth(2_000_000);
    assert_eq!(awa.max_fps(), 220);
    assert_eq!(awa.frame_time(), Duration::from_micros(4_545));
}

#[test]
fn bandwidth_rounds_frame_time_up() {
    // 1 byte at 3 baud is 10 bits: 3.33 s, rounded up to the microsecond.
    assert_eq!(
        Bandwidth::new(1, 3).frame_time(),
        Duration::from_micros(3_333_334)
    );
    assert_eq!(Bandwidth::new(1, 3).max_fps(), 0);
}

#[test]
fn protocol_names_parse_in_any_case() {
    assert_eq!(AdalightProtocol::parse("ADA"), Some(AdalightProtocol::Ada));
    assert_eq!(
        AdalightProtocol::parse("adalight"),
        Some(AdalightProtocol::Ada)
    );
    assert_eq!(
        AdalightProtocol::parse(" Awa "),
        Some(AdalightProtocol::Awa)
    );
    assert_eq!(
        AdalightProtocol::parse("hyperserial"),
        Some(AdalightProtocol::Awa)
    );
    assert_eq!(AdalightProtocol::parse("lbapa"), None);
}

#[test]
fn color_orders_permute_rgb() {
    let rgb = [1, 2, 3];
    let expected = [
        (ColorOrder::Rgb, [1, 2, 3]),
        (ColorOrder::Rbg, [1, 3, 2]),
        (ColorOrder::Grb, [2, 1, 3]),
        (ColorOrder::Gbr, [2, 3, 1]),
        (ColorOrder::Brg, [3, 1, 2]),
        (ColorOrder::Bgr, [3, 2, 1]),
    ];
    for (order, wire) in expected {
        assert_eq!(order.apply(rgb), wire, "{order}");
        assert_eq!(
            ColorOrder::parse(&order.as_str().to_uppercase()),
            Some(order)
        );
    }
}
