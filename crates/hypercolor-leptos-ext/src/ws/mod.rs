mod channel;
mod frame;
mod preview;
mod reconnect;
mod rpc;
mod schema;
pub mod transport;

pub const HYPERCOLOR_WS_PROTOCOL: &str = "hypercolor-v1";

pub use channel::{
    BackpressurePolicy, BackpressureQueue, BinaryChannel, BinaryChannelRecvError, BlockOnFull,
    Channel, DropNewest, DropOldest, Latest, OverflowAction, Queue,
};
pub use frame::{
    BinaryFrame, BinaryFrameDecode, BinaryFrameEncode, DecodeError, validate_frame_prefix,
    write_frame_prefix,
};
pub use hypercolor_leptos_ext_macros::BinaryFrame;
#[cfg(all(feature = "ws-client-wasm", target_arch = "wasm32"))]
pub use preview::PreviewFrameView;
pub use preview::{
    PREVIEW_FRAME_HEADER_LEN, PreviewFrame, PreviewFrameChannel, PreviewFrameDecodeError,
    PreviewPixelFormat,
};
pub use reconnect::{
    Connector, ExponentialBackoff, Jitter, ReconnectError, ReconnectOutcome, ReconnectPolicy,
    ReconnectRecvError, ReconnectSendError, Reconnecting,
};
pub use rpc::{RPC_REQUEST_TAG, RPC_RESPONSE_TAG, RpcRequest, RpcResponse, RpcStatus};
pub use schema::{SchemaRange, negotiate_highest_common_schema};

pub trait BinaryFrameSchema {
    const TAG: u8;
    const SCHEMA: u8;
    const NAME: &'static str;
}

pub trait BinaryFrameMetadata: BinaryFrameSchema {}

impl<T: BinaryFrameSchema> BinaryFrameMetadata for T {}
