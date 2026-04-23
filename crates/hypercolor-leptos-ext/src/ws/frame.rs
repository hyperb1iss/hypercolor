use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::BinaryFrameSchema;

pub trait BinaryFrameEncode {
    fn encode_into(&self, out: &mut BytesMut);
    fn encoded_len_hint(&self) -> usize;

    fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(self.encoded_len_hint());
        self.encode_into(&mut out);
        out.freeze()
    }
}

pub trait BinaryFrameDecode: Sized {
    fn decode(input: &[u8]) -> Result<Self, DecodeError>;
}

pub trait BinaryFrame: BinaryFrameSchema + BinaryFrameEncode + BinaryFrameDecode {}

impl<T> BinaryFrame for T where T: BinaryFrameSchema + BinaryFrameEncode + BinaryFrameDecode {}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecodeError {
    #[error("frame is truncated")]
    Truncated,
    #[error("wrong frame tag: expected 0x{expected:02x}, got 0x{actual:02x}")]
    WrongTag { expected: u8, actual: u8 },
    #[error("wrong frame schema: expected {expected}, got {actual}")]
    WrongSchema { expected: u8, actual: u8 },
    #[error("invalid frame header: {0}")]
    InvalidHeader(&'static str),
    #[error("invalid frame body: {0}")]
    InvalidBody(&'static str),
}

pub fn validate_frame_prefix<T>(input: &[u8]) -> Result<&[u8], DecodeError>
where
    T: BinaryFrameSchema,
{
    if input.len() < 2 {
        return Err(DecodeError::Truncated);
    }

    let tag = input[0];
    if tag != T::TAG {
        return Err(DecodeError::WrongTag {
            expected: T::TAG,
            actual: tag,
        });
    }

    let schema = input[1];
    if schema != T::SCHEMA {
        return Err(DecodeError::WrongSchema {
            expected: T::SCHEMA,
            actual: schema,
        });
    }

    Ok(&input[2..])
}

pub fn write_frame_prefix<T>(out: &mut BytesMut)
where
    T: BinaryFrameSchema,
{
    out.put_u8(T::TAG);
    out.put_u8(T::SCHEMA);
}
