use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::Sink;
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;

use super::CinderTransport;

#[derive(Debug)]
pub struct AxumWebSocketTransport {
    socket: WebSocket,
}

impl AxumWebSocketTransport {
    #[must_use]
    pub fn new(socket: WebSocket) -> Self {
        Self { socket }
    }

    #[must_use]
    pub fn into_inner(self) -> WebSocket {
        self.socket
    }
}

#[derive(Debug, Error)]
pub enum AxumWebSocketTransportError {
    #[error("websocket transport error")]
    Socket(#[from] axum::Error),
}

#[async_trait(?Send)]
impl CinderTransport for AxumWebSocketTransport {
    type SendError = AxumWebSocketTransportError;
    type RecvError = AxumWebSocketTransportError;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError> {
        self.socket
            .send(Message::Binary(frame))
            .await
            .map_err(Into::into)
    }

    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError> {
        while let Some(message) = self.socket.recv().await {
            match message? {
                Message::Binary(frame) => return Ok(Some(frame)),
                Message::Close(_) => return Ok(None),
                Message::Text(_) | Message::Ping(_) | Message::Pong(_) => {}
            }
        }

        Ok(None)
    }

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>> {
        Pin::new(&mut self.socket)
            .poll_ready(cx)
            .map_err(Into::into)
    }

    async fn close(&mut self) -> Result<(), Self::SendError> {
        self.socket
            .send(Message::Close(None))
            .await
            .map_err(Into::into)
    }
}
