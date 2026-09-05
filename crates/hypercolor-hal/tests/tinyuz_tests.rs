//! tinyuz conformance: the decoder is trusted because it reproduces every
//! stream the upstream compressor produced, and the encoder is trusted
//! because everything it emits decodes through that decoder.

use hypercolor_hal::drivers::lianli::wireless::tinyuz::{
    DecodeError, Params, compress, declared_dict_size, decompress,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/tinyuz/{name}.yuz",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture present")
}

// The inputs behind each fixture, as the upstream test suite generated them.
fn raw_static_16led() -> Vec<u8> {
    (0..16).flat_map(|_| [255u8, 128, 0]).collect()
}
fn raw_static_40led() -> Vec<u8> {
    (0..40).flat_map(|_| [0u8, 255, 0]).collect()
}
fn raw_breathing_40led() -> Vec<u8> {
    (0..40)
        .flat_map(|i| if i % 2 == 0 { [255, 0, 0] } else { [0, 0, 255] })
        .collect()
}
fn raw_palette_16() -> Vec<u8> {
    [
        [255u8, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [255, 0, 255],
        [0, 255, 255],
        [128, 0, 0],
        [0, 128, 0],
        [0, 0, 128],
        [128, 128, 0],
        [128, 0, 128],
        [0, 128, 128],
        [64, 64, 64],
        [192, 192, 192],
        [255, 255, 255],
        [0, 0, 0],
    ]
    .iter()
    .flat_map(|c| c.iter().copied())
    .collect()
}
fn raw_slv3_group() -> Vec<u8> {
    (0..6u8)
        .flat_map(|fan| {
            (0..40).flat_map(move |_| [fan.wrapping_mul(40), 128, 255 - fan.wrapping_mul(40)])
        })
        .collect()
}
fn raw_slinf_group() -> Vec<u8> {
    (0..5u8)
        .flat_map(|fan| (0..44).flat_map(move |_| [fan.wrapping_mul(50), 200, 55]))
        .collect()
}
fn raw_gradient_80() -> Vec<u8> {
    (0..80u8)
        .flat_map(|i| [i, i.wrapping_mul(2), 255 - i])
        .collect()
}
fn raw_black() -> Vec<u8> {
    vec![0u8; 240]
}
fn raw_white() -> Vec<u8> {
    vec![255u8; 240]
}
fn raw_tl_flex() -> Vec<u8> {
    (0..4u8)
        .flat_map(|fan| (0..26).flat_map(move |led| [fan * 60 + led, 100, fan * 30 + led]))
        .collect()
}

type Generator = fn() -> Vec<u8>;

const CASES: &[(&str, Generator)] = &[
    ("01_static_16led", raw_static_16led),
    ("02_static_40led", raw_static_40led),
    ("03_breathing_40led", raw_breathing_40led),
    ("04_palette_16", raw_palette_16),
    ("05_slv3_group", raw_slv3_group),
    ("06_slinf_group", raw_slinf_group),
    ("07_gradient_80", raw_gradient_80),
    ("08_black", raw_black),
    ("09_white", raw_white),
    ("10_tl_flex", raw_tl_flex),
];

#[test]
fn decoder_reproduces_every_upstream_fixture() {
    for (name, raw) in CASES {
        let expected = raw();
        let decoded = decompress(&fixture(name), expected.len())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(decoded, expected, "{name}");
    }
}

#[test]
fn encoder_round_trips_through_the_validated_decoder() {
    for (name, raw) in CASES {
        let input = raw();
        let code = compress(&input, Params::default());
        let decoded =
            decompress(&code, input.len()).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(decoded, input, "{name}");
        let reference_len = fixture(name).len();
        assert!(
            code.len() <= reference_len * 2 + 8,
            "{name}: ours is {} bytes against the reference's {reference_len}",
            code.len()
        );
    }
}

/// On inputs with one obvious parse the two encoders agree byte for byte,
/// which pins the bit layout rather than just decodability.
#[test]
fn encoder_matches_the_upstream_bytes_on_periodic_inputs() {
    for name in ["08_black", "09_white", "01_static_16led", "02_static_40led"] {
        let (_, raw) = CASES.iter().find(|(case, _)| *case == name).expect("case");
        assert_eq!(compress(&raw(), Params::default()), fixture(name), "{name}");
    }
}

#[test]
fn header_carries_the_largest_distance_used() {
    assert_eq!(
        declared_dict_size(&compress(&raw_black(), Params::default())),
        Some(1)
    );
    assert_eq!(
        declared_dict_size(&compress(&raw_static_16led(), Params::default())),
        Some(3)
    );
}

/// Noise past the big-position threshold followed by a repeat of the
/// opening, so the encoder must express a distance above 2687.
#[test]
fn long_distances_and_literal_lines_round_trip() {
    let mut state = 0x1234_5678_u32;
    let mut noise: Vec<u8> = (0..3000)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            u8::try_from(state >> 24).expect("top byte")
        })
        .collect();
    let head = noise[..64].to_vec();
    noise.extend_from_slice(&head);

    let code = compress(&noise, Params::default());
    assert_eq!(decompress(&code, noise.len()).expect("decodes"), noise);
    assert!(declared_dict_size(&code).expect("header") >= 2688);
}

/// A firmware-looped animation: sixty frames of a four-fan cluster.
#[test]
fn multi_frame_animation_round_trips_and_compresses() {
    let frames: Vec<u8> = (0..60u32)
        .flat_map(|frame| {
            (0..4 * 26).flat_map(move |led| {
                let phase = u8::try_from((frame * 7 + led) % 256).expect("byte");
                [phase, 255 - phase, phase / 2]
            })
        })
        .collect();

    let code = compress(&frames, Params::default());
    assert_eq!(decompress(&code, frames.len()).expect("decodes"), frames);
    assert!(code.len() < frames.len(), "periodic frames compress");
}

#[test]
fn window_never_exceeds_the_configured_dictionary() {
    let input: Vec<u8> = (0..6000u32)
        .map(|i| u8::try_from(i % 251).expect("byte"))
        .collect();
    let code = compress(&input, Params::default());
    assert!(declared_dict_size(&code).expect("header") <= 4096);
    assert_eq!(decompress(&code, input.len()).expect("decodes"), input);
}

#[test]
fn empty_input_is_a_bare_stream_end() {
    let code = compress(&[], Params::default());
    assert_eq!(decompress(&code, 0), Ok(Vec::new()));
}

#[test]
fn a_truncated_stream_is_an_error_not_a_panic() {
    let code = compress(&raw_gradient_80(), Params::default());
    for cut in 0..code.len() {
        let result = decompress(&code[..cut], 240);
        assert!(result.is_err(), "cut at {cut} should not decode");
    }
    assert_eq!(decompress(&[1, 0, 0, 0], 8), Err(DecodeError::Truncated));
}

#[test]
fn output_limit_is_enforced() {
    let code = compress(&raw_black(), Params::default());
    assert_eq!(
        decompress(&code, 100),
        Err(DecodeError::OutputTooLarge(100))
    );
}
