use bytes::Bytes;
use std::marker::PhantomData;
use std::task::{Context, Poll};
use thiserror::Error;

use super::{BinaryFrame, DecodeError, transport::CinderTransport};

pub struct Channel<Tr> {
    transport: Tr,
}

impl<Tr> Channel<Tr> {
    #[must_use]
    pub const fn new(transport: Tr) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &Tr {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut Tr {
        &mut self.transport
    }

    pub fn into_inner(self) -> Tr {
        self.transport
    }
}

pub struct BinaryChannel<T, Tr> {
    transport: Tr,
    _marker: PhantomData<fn() -> T>,
}

impl<T, Tr> BinaryChannel<T, Tr> {
    #[must_use]
    pub const fn new(transport: Tr) -> Self {
        Self {
            transport,
            _marker: PhantomData,
        }
    }

    pub fn transport(&self) -> &Tr {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut Tr {
        &mut self.transport
    }

    pub fn into_inner(self) -> Tr {
        self.transport
    }
}

#[derive(Debug, Error)]
pub enum BinaryChannelRecvError<E>
where
    E: std::error::Error + 'static,
{
    #[error(transparent)]
    Transport(E),
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

impl<Tr> Channel<Tr>
where
    Tr: CinderTransport,
{
    pub async fn send_bytes(&mut self, frame: Bytes) -> Result<(), Tr::SendError> {
        self.transport.send(frame).await
    }

    pub async fn recv_bytes(&mut self) -> Result<Option<Bytes>, Tr::RecvError> {
        self.transport.recv().await
    }

    pub fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Tr::SendError>> {
        self.transport.poll_ready(cx)
    }

    pub async fn close(&mut self) -> Result<(), Tr::SendError> {
        self.transport.close().await
    }
}

impl<T, Tr> BinaryChannel<T, Tr>
where
    T: BinaryFrame,
    Tr: CinderTransport,
{
    pub async fn send(&mut self, frame: T) -> Result<(), Tr::SendError> {
        self.transport.send(frame.encode()).await
    }

    pub async fn recv(&mut self) -> Result<Option<T>, BinaryChannelRecvError<Tr::RecvError>> {
        match self.transport.recv().await {
            Ok(Some(bytes)) => T::decode(&bytes)
                .map(Some)
                .map_err(BinaryChannelRecvError::from),
            Ok(None) => Ok(None),
            Err(error) => Err(BinaryChannelRecvError::Transport(error)),
        }
    }

    pub fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Tr::SendError>> {
        self.transport.poll_ready(cx)
    }

    pub async fn close(&mut self) -> Result<(), Tr::SendError> {
        self.transport.close().await
    }
}
