mod channel;
mod frame;
mod input_event;
mod preview;
mod reconnect;
mod replay;
mod rpc;
mod schema;
mod spectrum;
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
pub use input_event::{
    INPUT_EVENT_PAYLOAD_SCHEMA, InputEventPayloadDecodeError, TimedInputEventPayload,
};
#[cfg(feature = "ws-client-wasm")]
pub use preview::InteractivePreviewFrameView;
#[cfg(feature = "ws-client-wasm")]
pub use preview::PreviewFrameView;
#[cfg(feature = "ws-client-wasm")]
pub use preview::ZonePreviewFrameView;
pub use preview::{
    DEFAULT_PREVIEW_MAX_CHUNK_COUNT, DEFAULT_PREVIEW_MAX_CONNECTION_BYTES,
    DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES, DEFAULT_PREVIEW_MAX_ENCODED_PUBLICATION_BYTES,
    DEFAULT_PREVIEW_MAX_IDLE_MS, DEFAULT_PREVIEW_MAX_MESSAGE_BYTES,
    DEFAULT_PREVIEW_MAX_REASSEMBLY_STREAMS, DEFAULT_PREVIEW_MAX_TOMBSTONES,
    EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN, EXTENDED_SCREEN_ZONES_FRAME_TAG,
    INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, INTERACTIVE_PREVIEW_FRAME_TAG,
    INTERACTIVE_PREVIEW_ID_MAX_BYTES, InteractivePreviewFrame, PREVIEW_CANCEL_FIXED_HEADER_LEN,
    PREVIEW_CANCEL_FRAME_TAG, PREVIEW_CANCEL_SCHEMA, PREVIEW_CHUNK_FIXED_HEADER_LEN,
    PREVIEW_CHUNK_FRAME_TAG, PREVIEW_CHUNK_SCHEMA, PREVIEW_FRAME_HEADER_LEN,
    PREVIEW_MIN_MESSAGE_BYTES, PREVIEW_TRANSPORT_CAPABILITY_PREFIX, PreviewCancelFrame,
    PreviewCapabilityError, PreviewChunkError, PreviewChunkFrame, PreviewChunkReassembler,
    PreviewFrame, PreviewFrameChannel, PreviewFrameDecodeError, PreviewPixelFormat,
    PreviewPublicationMetadata, PreviewReassemblyLimits, PreviewStreamId,
    PreviewTransportCapability, PreviewTransportVersion, ReassembledPreviewPublication,
    SCREEN_ZONES_FRAME_HEADER_LEN, SCREEN_ZONES_FRAME_TAG, ScreenZonesFrame,
    WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, WIDE_INTERACTIVE_PREVIEW_FRAME_TAG,
    WIDE_PREVIEW_FRAME_HEADER_LEN, WIDE_PREVIEW_FRAME_TAG, WIDE_SCREEN_ZONES_FRAME_HEADER_LEN,
    WIDE_SCREEN_ZONES_FRAME_TAG, WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN, WIDE_ZONE_PREVIEW_FRAME_TAG,
    ZONE_PREVIEW_FRAME_HEADER_LEN, ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame,
    split_preview_publication, split_preview_publication_with_capability,
};
pub use reconnect::{
    Connector, ExponentialBackoff, Jitter, ReconnectError, ReconnectOutcome, ReconnectPolicy,
    ReconnectRecvError, ReconnectSendError, Reconnecting,
};
pub use replay::{
    ChannelDescriptor, Direction, ReplayEntry, SessionPlayer, SessionRecord, SessionRecorder,
    SessionTape,
};
pub use rpc::{
    RPC_REQUEST_TAG, RPC_RESPONSE_TAG, RpcClient, RpcClientError, RpcRequest, RpcResponse,
    RpcServer, RpcServerError, RpcStatus,
};
pub use schema::{SchemaRange, negotiate_highest_common_schema};
pub use spectrum::{
    SPECTRUM_FRAME_HEADER_LEN, SPECTRUM_FRAME_TAG, SpectrumFrame, SpectrumFrameDecodeError,
};

pub trait BinaryFrameSchema {
    const TAG: u8;
    const SCHEMA: u8;
    const NAME: &'static str;
}

pub trait BinaryFrameMetadata: BinaryFrameSchema {}

impl<T: BinaryFrameSchema> BinaryFrameMetadata for T {}
