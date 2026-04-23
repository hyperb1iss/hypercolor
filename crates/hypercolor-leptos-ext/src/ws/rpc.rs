use bytes::{BufMut, Bytes, BytesMut};

use super::{
    BinaryFrameDecode, BinaryFrameEncode, BinaryFrameSchema, DecodeError, validate_frame_prefix,
    write_frame_prefix,
};

pub const RPC_REQUEST_TAG: u8 = 0x80;
pub const RPC_RESPONSE_TAG: u8 = 0x81;
const RPC_SCHEMA: u8 = 1;
const REQUEST_FIXED_LEN: usize = 10;
const RESPONSE_FIXED_LEN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    pub payload: Bytes,
}

impl RpcRequest {
    pub fn new(id: u64, method: impl Into<String>, payload: impl Into<Bytes>) -> Self {
        Self {
            id,
            method: method.into(),
            payload: payload.into(),
        }
    }
}

impl BinaryFrameSchema for RpcRequest {
    const TAG: u8 = RPC_REQUEST_TAG;
    const SCHEMA: u8 = RPC_SCHEMA;
    const NAME: &'static str = "rpc_request";
}

impl BinaryFrameEncode for RpcRequest {
    fn encode_into(&self, out: &mut BytesMut) {
        write_frame_prefix::<Self>(out);
        out.put_u64_le(self.id);
        let method_bytes = self.method.as_bytes();
        let method_len = u16::try_from(method_bytes.len()).unwrap_or(u16::MAX);
        out.put_u16_le(method_len);
        out.extend_from_slice(&method_bytes[..usize::from(method_len)]);
        out.extend_from_slice(&self.payload);
    }

    fn encoded_len_hint(&self) -> usize {
        let method_len = self.method.len().min(usize::from(u16::MAX));
        2 + REQUEST_FIXED_LEN + method_len + self.payload.len()
    }
}

impl BinaryFrameDecode for RpcRequest {
    fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let body = validate_frame_prefix::<Self>(input)?;
        if body.len() < REQUEST_FIXED_LEN {
            return Err(DecodeError::Truncated);
        }

        let id = u64::from_le_bytes(body[0..8].try_into().expect("slice has 8 bytes"));
        let method_len = usize::from(u16::from_le_bytes(
            body[8..10].try_into().expect("slice has 2 bytes"),
        ));
        let payload_offset = REQUEST_FIXED_LEN
            .checked_add(method_len)
            .ok_or(DecodeError::InvalidHeader("method length overflows"))?;
        if body.len() < payload_offset {
            return Err(DecodeError::Truncated);
        }

        let method = std::str::from_utf8(&body[REQUEST_FIXED_LEN..payload_offset])
            .map_err(|_| DecodeError::InvalidBody("method is not valid UTF-8"))?
            .to_owned();

        Ok(Self {
            id,
            method,
            payload: Bytes::copy_from_slice(&body[payload_offset..]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcResponse {
    pub id: u64,
    pub status: RpcStatus,
    pub payload: Bytes,
}

impl RpcResponse {
    pub fn ok(id: u64, payload: impl Into<Bytes>) -> Self {
        Self {
            id,
            status: RpcStatus::OK,
            payload: payload.into(),
        }
    }

    pub fn error(id: u64, status: RpcStatus, payload: impl Into<Bytes>) -> Self {
        Self {
            id,
            status,
            payload: payload.into(),
        }
    }
}

impl BinaryFrameSchema for RpcResponse {
    const TAG: u8 = RPC_RESPONSE_TAG;
    const SCHEMA: u8 = RPC_SCHEMA;
    const NAME: &'static str = "rpc_response";
}

impl BinaryFrameEncode for RpcResponse {
    fn encode_into(&self, out: &mut BytesMut) {
        write_frame_prefix::<Self>(out);
        out.put_u64_le(self.id);
        out.put_u16_le(self.status.code());
        out.extend_from_slice(&self.payload);
    }

    fn encoded_len_hint(&self) -> usize {
        2 + RESPONSE_FIXED_LEN + self.payload.len()
    }
}

impl BinaryFrameDecode for RpcResponse {
    fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let body = validate_frame_prefix::<Self>(input)?;
        if body.len() < RESPONSE_FIXED_LEN {
            return Err(DecodeError::Truncated);
        }

        Ok(Self {
            id: u64::from_le_bytes(body[0..8].try_into().expect("slice has 8 bytes")),
            status: RpcStatus::from_code(u16::from_le_bytes(
                body[8..10].try_into().expect("slice has 2 bytes"),
            )),
            payload: Bytes::copy_from_slice(&body[RESPONSE_FIXED_LEN..]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcStatus(u16);

impl RpcStatus {
    pub const OK: Self = Self(200);
    pub const BAD_REQUEST: Self = Self(400);
    pub const NOT_FOUND: Self = Self(404);
    pub const INTERNAL_ERROR: Self = Self(500);

    #[must_use]
    pub const fn from_code(code: u16) -> Self {
        Self(code)
    }

    #[must_use]
    pub const fn code(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 >= 200 && self.0 < 300
    }
}
