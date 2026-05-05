mod in_memory;
#[cfg(feature = "axum")]
mod websocket_axum;
#[cfg(feature = "ws-client-wasm")]
mod websocket_wasm;

use async_trait::async_trait;
use bytes::Bytes;
use std::task::{Context, Poll};

use crate::MaybeSend;

pub use in_memory::{InMemoryTransport, InMemoryTransportError};
#[cfg(feature = "axum")]
pub use websocket_axum::{AxumWebSocketTransport, AxumWebSocketTransportError};
#[cfg(feature = "ws-client-wasm")]
pub use websocket_wasm::{
    WebSocketEventHandlers, WebSocketTransport, WebSocketTransportError, WebSocketTransportState,
    arraybuffer_websocket, message_array_buffer, send_websocket_json, send_websocket_text,
};

#[async_trait(?Send)]
pub trait CinderTransport: MaybeSend + 'static {
    type SendError: std::error::Error + MaybeSend + Sync + 'static;
    type RecvError: std::error::Error + MaybeSend + Sync + 'static;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError>;
    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>>;
    async fn close(&mut self) -> Result<(), Self::SendError>;
}
