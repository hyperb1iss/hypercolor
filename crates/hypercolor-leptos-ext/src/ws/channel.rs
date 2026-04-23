use bytes::Bytes;
use std::task::{Context, Poll};

use super::transport::CinderTransport;

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
