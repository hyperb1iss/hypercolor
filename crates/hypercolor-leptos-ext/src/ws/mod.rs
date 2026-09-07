mod backoff;
mod codec_manifest;
mod input_event;
mod preview;
pub mod registry;
mod spectrum;
pub mod topic;
pub mod transport;

pub const HYPERCOLOR_WS_PROTOCOL: &str = "hypercolor-v1";
pub const HYPERCOLOR_WS_VERSION: &str = "1.0";

pub use backoff::{ExponentialBackoff, Jitter};
pub use codec_manifest::{codec_binary_messages, codec_frame_layouts};
pub use input_event::{
    INPUT_EVENT_PAYLOAD_SCHEMA, InputEventPayloadDecodeError, TimedInputEventPayload,
};
#[cfg(feature = "ws-client-wasm")]
pub use preview::DisplayPreviewFrameView;
#[cfg(feature = "ws-client-wasm")]
pub use preview::InteractivePreviewFrameView;
#[cfg(feature = "ws-client-wasm")]
pub use preview::PreviewFrameView;
#[cfg(feature = "ws-client-wasm")]
pub use preview::ZonePreviewFrameView;
pub use preview::{
    DEFAULT_PREVIEW_MAX_CONNECTION_BYTES, DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES,
    DEFAULT_PREVIEW_MAX_ENCODED_PUBLICATION_BYTES, DEFAULT_PREVIEW_MAX_IDLE_MS,
    DEFAULT_PREVIEW_MAX_MESSAGE_BYTES, DISPLAY_PREVIEW_FRAME_PREFIX_LEN, DISPLAY_PREVIEW_FRAME_TAG,
    DISPLAY_PREVIEW_ID_MAX_BYTES, DisplayPreviewFrame, EXTENDED_SCREEN_ZONES_FRAME_HEADER_LEN,
    EXTENDED_SCREEN_ZONES_FRAME_TAG, INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN,
    INTERACTIVE_PREVIEW_FRAME_TAG, INTERACTIVE_PREVIEW_ID_MAX_BYTES, InteractivePreviewFrame,
    PREVIEW_CANCEL_FIXED_HEADER_LEN, PREVIEW_CANCEL_FRAME_TAG, PREVIEW_CANCEL_SCHEMA,
    PREVIEW_CHUNK_FIXED_HEADER_LEN, PREVIEW_CHUNK_FRAME_TAG, PREVIEW_CHUNK_SCHEMA,
    PREVIEW_FRAME_HEADER_LEN, PREVIEW_MIN_MESSAGE_BYTES, PreviewCancelFrame, PreviewChunkError,
    PreviewChunkFrame, PreviewChunkReassembler, PreviewFrame, PreviewFrameChannel,
    PreviewFrameDecodeError, PreviewPixelFormat, PreviewPublicationMetadata,
    PreviewReassemblyLimits, PreviewStreamId, PreviewTransportLimits,
    ReassembledPreviewPublication, SCREEN_ZONES_FRAME_HEADER_LEN, SCREEN_ZONES_FRAME_TAG,
    ScreenZonesFrame, WIDE_DISPLAY_PREVIEW_FRAME_PREFIX_LEN, WIDE_DISPLAY_PREVIEW_FRAME_TAG,
    WIDE_INTERACTIVE_PREVIEW_FRAME_PREFIX_LEN, WIDE_INTERACTIVE_PREVIEW_FRAME_TAG,
    WIDE_PREVIEW_FRAME_HEADER_LEN, WIDE_PREVIEW_FRAME_TAG, WIDE_SCREEN_ZONES_FRAME_HEADER_LEN,
    WIDE_SCREEN_ZONES_FRAME_TAG, WIDE_ZONE_PREVIEW_FRAME_HEADER_LEN, WIDE_ZONE_PREVIEW_FRAME_TAG,
    ZONE_PREVIEW_FRAME_HEADER_LEN, ZONE_PREVIEW_FRAME_TAG, ZonePreviewFrame,
    split_preview_publication, split_preview_publication_with_limits,
};
pub use spectrum::{
    SPECTRUM_FRAME_HEADER_LEN, SPECTRUM_FRAME_TAG, SpectrumFrame, SpectrumFrameDecodeError,
};
