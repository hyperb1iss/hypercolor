#![cfg(feature = "ws-core")]

use bytes::{Bytes, BytesMut};
use hypercolor_leptos_ext::ws::transport::InMemoryTransport;
use hypercolor_leptos_ext::ws::{
    BinaryChannel, BinaryChannelRecvError, BinaryFrameDecode, BinaryFrameEncode, DecodeError,
    write_frame_prefix,
};

#[derive(Debug, Clone, PartialEq, Eq, hypercolor_leptos_ext::ws::BinaryFrame)]
#[frame(tag = 0x06, schema = 1)]
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
        let payload = hypercolor_leptos_ext::ws::validate_frame_prefix::<Self>(input)?;
        Ok(Self {
            payload: Bytes::copy_from_slice(payload),
        })
    }
}

#[tokio::test]
async fn binary_channel_roundtrips_typed_frames() {
    let (left, right) = InMemoryTransport::pair();
    let mut sender = BinaryChannel::<TestFrame, _>::new(left);
    let mut receiver = BinaryChannel::<TestFrame, _>::new(right);

    sender
        .send(TestFrame {
            payload: Bytes::from_static(b"preview"),
        })
        .await
        .expect("send succeeds");

    let received = receiver.recv().await.expect("recv succeeds");
    assert_eq!(
        received,
        Some(TestFrame {
            payload: Bytes::from_static(b"preview"),
        })
    );
}

#[tokio::test]
async fn binary_channel_surfaces_decode_errors() {
    let (left, right) = InMemoryTransport::pair();
    let mut sender = hypercolor_leptos_ext::ws::Channel::new(left);
    let mut receiver = BinaryChannel::<TestFrame, _>::new(right);

    sender
        .send_bytes(Bytes::from_static(&[0x09, 0x01, 0xff]))
        .await
        .expect("raw send succeeds");

    let error = receiver.recv().await.expect_err("recv fails");

    assert!(matches!(
        error,
        BinaryChannelRecvError::Decode(DecodeError::WrongTag {
            expected: 0x06,
            actual: 0x09,
        })
    ));
}
