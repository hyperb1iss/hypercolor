mod in_memory;
#[cfg(feature = "axum")]
mod websocket_axum;
#[cfg(all(feature = "ws-client-wasm", target_arch = "wasm32"))]
mod websocket_wasm;

use async_trait::async_trait;
use bytes::Bytes;
use std::task::{Context, Poll};

use crate::MaybeSend;

pub use in_memory::{InMemoryTransport, InMemoryTransportError};
#[cfg(feature = "axum")]
pub use websocket_axum::{AxumWebSocketTransport, AxumWebSocketTransportError};
#[cfg(all(feature = "ws-client-wasm", target_arch = "wasm32"))]
pub use websocket_wasm::{WebSocketTransport, WebSocketTransportError, WebSocketTransportState};

#[async_trait(?Send)]
pub trait CinderTransport: MaybeSend + 'static {
    type SendError: std::error::Error + MaybeSend + Sync + 'static;
    type RecvError: std::error::Error + MaybeSend + Sync + 'static;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError>;
    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>>;
    async fn close(&mut self) -> Result<(), Self::SendError>;
}
