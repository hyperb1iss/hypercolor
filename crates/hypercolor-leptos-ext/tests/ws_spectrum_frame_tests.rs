#![cfg(all(feature = "ws-core", not(target_arch = "wasm32")))]

use hypercolor_leptos_ext::ws::{
    SPECTRUM_FRAME_HEADER_LEN, SPECTRUM_FRAME_TAG, SpectrumFrame, SpectrumFrameDecodeError,
};

fn sample_frame() -> SpectrumFrame {
    SpectrumFrame {
        timestamp_ms: 123_456,
        level: 0.8,
        bass: 0.9,
        mid: 0.5,
        treble: 0.25,
        beat: true,
        beat_confidence: 0.75,
        bins: vec![0.1, 0.2, 0.3, 0.4],
    }
}

#[test]
fn spectrum_frame_roundtrips() {
    let frame = sample_frame();
    let encoded = frame.encode();

    assert_eq!(encoded[0], SPECTRUM_FRAME_TAG);
    assert_eq!(encoded.len(), SPECTRUM_FRAME_HEADER_LEN + 4 * 4);
    assert_eq!(SpectrumFrame::decode(&encoded), Ok(frame));
}

#[test]
fn spectrum_frame_roundtrips_without_bins() {
    let frame = SpectrumFrame {
        bins: Vec::new(),
        beat: false,
        ..sample_frame()
    };
    let encoded = frame.encode();

    assert_eq!(encoded.len(), SPECTRUM_FRAME_HEADER_LEN);
    assert_eq!(SpectrumFrame::decode(&encoded), Ok(frame));
}

#[test]
fn spectrum_frame_caps_bins_at_u8_max() {
    let frame = SpectrumFrame {
        bins: vec![0.5; 300],
        ..sample_frame()
    };
    let encoded = frame.encode();

    assert_eq!(encoded.len(), SPECTRUM_FRAME_HEADER_LEN + 255 * 4);
    let decoded = SpectrumFrame::decode(&encoded).expect("decode capped frame");
    assert_eq!(decoded.bins.len(), 255);
}

#[test]
fn spectrum_frame_rejects_short_input() {
    assert_eq!(
        SpectrumFrame::decode(&[SPECTRUM_FRAME_TAG; 10]),
        Err(SpectrumFrameDecodeError::TooShort {
            expected: SPECTRUM_FRAME_HEADER_LEN,
            actual: 10,
        })
    );
}

#[test]
fn spectrum_frame_rejects_wrong_tag() {
    let mut encoded = sample_frame().encode().to_vec();
    encoded[0] = 0x03;
    assert_eq!(
        SpectrumFrame::decode(&encoded),
        Err(SpectrumFrameDecodeError::UnknownTag { actual: 0x03 })
    );
}

#[test]
fn spectrum_frame_rejects_truncated_bins() {
    let encoded = sample_frame().encode();
    let truncated = &encoded[..encoded.len() - 4];
    assert_eq!(
        SpectrumFrame::decode(truncated),
        Err(SpectrumFrameDecodeError::BinsTooShort {
            expected: 4,
            actual: 3,
        })
    );
}
