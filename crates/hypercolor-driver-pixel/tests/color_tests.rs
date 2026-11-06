//! Channel order encoding for pixel controllers.

use hypercolor_driver_pixel::{ColorOrder, encode_pixels};

#[test]
fn three_channel_orders_permute_rgb() {
    let rgb = [10, 20, 30];
    let expected = [
        (ColorOrder::Rgb, [10, 20, 30]),
        (ColorOrder::Rbg, [10, 30, 20]),
        (ColorOrder::Grb, [20, 10, 30]),
        (ColorOrder::Gbr, [20, 30, 10]),
        (ColorOrder::Brg, [30, 10, 20]),
        (ColorOrder::Bgr, [30, 20, 10]),
    ];
    for (order, bytes) in expected {
        let mut out = [0u8; 3];
        order.encode_into(rgb, &mut out);
        assert_eq!(out, bytes, "{order}");
        assert_eq!(order.color_channels(), 3);
    }
}

#[test]
fn white_orders_extract_shared_minimum() {
    let rgb = [200, 120, 80];
    let expected = [
        (ColorOrder::Rgbw, [120, 40, 0, 80]),
        (ColorOrder::Grbw, [40, 120, 0, 80]),
        (ColorOrder::Bgrw, [0, 40, 120, 80]),
    ];
    for (order, bytes) in expected {
        let mut out = [0u8; 4];
        order.encode_into(rgb, &mut out);
        assert_eq!(out, bytes, "{order}");
        assert!(order.has_white());
    }
}

#[test]
fn pure_white_moves_entirely_to_the_white_channel() {
    let mut out = [0u8; 4];
    ColorOrder::Rgbw.encode_into([255, 255, 255], &mut out);
    assert_eq!(out, [0, 0, 0, 255]);
}

#[test]
fn encode_pads_missing_pixels_and_extra_channels() {
    let mut out = Vec::new();
    encode_pixels(&[[1, 2, 3]], ColorOrder::Grb, 5, 2, &mut out);
    assert_eq!(out, vec![2, 1, 3, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn encode_truncates_extra_input() {
    let mut out = vec![0xEE; 64];
    encode_pixels(
        &[[1, 2, 3], [4, 5, 6], [7, 8, 9]],
        ColorOrder::Rgb,
        3,
        2,
        &mut out,
    );
    assert_eq!(out, vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn order_names_round_trip() {
    for order in ColorOrder::ALL {
        assert_eq!(ColorOrder::parse(order.as_str()), Some(order));
        assert_eq!(
            ColorOrder::parse(&order.as_str().to_uppercase()),
            Some(order)
        );
    }
    assert_eq!(ColorOrder::parse("rgbx"), None);
}
