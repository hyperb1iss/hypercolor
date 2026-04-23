#![cfg(feature = "ws-core")]

use bytes::{Bytes, BytesMut};
use hypercolor_leptos_ext::ws::{
    BinaryFrameDecode, BinaryFrameEncode, DecodeError, validate_frame_prefix, write_frame_prefix,
};

#[derive(Debug, Clone, PartialEq, Eq, hypercolor_leptos_ext::ws::BinaryFrame)]
#[frame(tag = 0x03, schema = 2)]
struct TestFrame {
    payload: Bytes,
}

impl BinaryFrameEncode for TestFrame {
    fn encode_into(&self, out: &mut BytesMut) {
        write_frame_prefix::<Self>(out);
        out.extend_from_slice(&self.payload);
    }

    fn encoded_len_hint(&self) -> usize {
        2 + self.payload.len()
    }
}

impl BinaryFrameDecode for TestFrame {
    fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let payload = validate_frame_prefix::<Self>(input)?;
        Ok(Self {
            payload: Bytes::copy_from_slice(payload),
        })
    }
}

#[test]
fn binary_frame_roundtrips_through_encode_and_decode() {
    let frame = TestFrame {
        payload: Bytes::from_static(b"canvas"),
    };

    let encoded = frame.encode();
    let decoded = TestFrame::decode(&encoded).expect("encoded frame decodes");

    assert_eq!(decoded, frame);
}

#[test]
fn validate_frame_prefix_rejects_wrong_tag() {
    let error = TestFrame::decode(&[0x09, 0x02, 0xaa]).expect_err("decode fails");

    assert_eq!(
        error,
        DecodeError::WrongTag {
            expected: 0x03,
            actual: 0x09,
        }
    );
}

#[test]
fn validate_frame_prefix_rejects_wrong_schema() {
    let error = TestFrame::decode(&[0x03, 0x07, 0xaa]).expect_err("decode fails");

    assert_eq!(
        error,
        DecodeError::WrongSchema {
            expected: 2,
            actual: 7,
        }
    );
}

#[test]
fn validate_frame_prefix_requires_header_bytes() {
    let error = TestFrame::decode(&[0x03]).expect_err("decode fails");

    assert_eq!(error, DecodeError::Truncated);
}

#[test]
fn encoded_len_hint_matches_emitted_bytes() {
    let frame = TestFrame {
        payload: Bytes::from_static(b"rgb"),
    };

    assert_eq!(frame.encoded_len_hint(), frame.encode().len());
}
