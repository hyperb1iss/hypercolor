use async_trait::async_trait;
use bytes::Bytes;
use futures_channel::mpsc::{self, Receiver, Sender};
use futures_util::StreamExt;
use std::future::poll_fn;
use std::task::{Context, Poll};
use thiserror::Error;

use super::CinderTransport;

const DEFAULT_CAPACITY: usize = 16;

pub struct InMemoryTransport {
    tx: Sender<Bytes>,
    rx: Receiver<Bytes>,
}

impl InMemoryTransport {
    #[must_use]
    pub fn pair() -> (Self, Self) {
        Self::pair_with_capacity(DEFAULT_CAPACITY)
    }

    #[must_use]
    pub fn pair_with_capacity(capacity: usize) -> (Self, Self) {
        let capacity = capacity.max(1);
        let (tx_a, rx_b) = mpsc::channel(capacity);
        let (tx_b, rx_a) = mpsc::channel(capacity);

        (Self { tx: tx_a, rx: rx_a }, Self { tx: tx_b, rx: rx_b })
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InMemoryTransportError {
    #[error("in-memory transport is closed")]
    Closed,
}

#[async_trait(?Send)]
impl CinderTransport for InMemoryTransport {
    type SendError = InMemoryTransportError;
    type RecvError = InMemoryTransportError;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError> {
        poll_fn(|cx| self.tx.poll_ready(cx))
            .await
            .map_err(|_| InMemoryTransportError::Closed)
            .and_then(|()| {
                self.tx
                    .try_send(frame)
                    .map_err(|_| InMemoryTransportError::Closed)
            })
    }

    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError> {
        Ok(self.rx.next().await)
    }

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>> {
        match self.tx.poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(_)) => Poll::Ready(Err(InMemoryTransportError::Closed)),
            Poll::Pending => Poll::Pending,
        }
    }

    async fn close(&mut self) -> Result<(), Self::SendError> {
        self.tx.close_channel();
        Ok(())
    }
}
