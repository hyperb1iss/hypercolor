mod in_memory;

use async_trait::async_trait;
use bytes::Bytes;
use std::task::{Context, Poll};

use crate::MaybeSend;

pub use in_memory::{InMemoryTransport, InMemoryTransportError};

#[async_trait(?Send)]
pub trait CinderTransport: MaybeSend + 'static {
    type SendError: std::error::Error + MaybeSend + Sync + 'static;
    type RecvError: std::error::Error + MaybeSend + Sync + 'static;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError>;
    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>>;
    async fn close(&mut self) -> Result<(), Self::SendError>;
}
