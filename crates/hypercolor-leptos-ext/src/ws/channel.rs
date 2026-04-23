use bytes::Bytes;
use std::collections::VecDeque;
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

pub trait BackpressurePolicy: 'static {
    const CAPACITY: usize;

    fn on_full(queue: &mut VecDeque<Bytes>, frame: Bytes) -> OverflowAction;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowAction {
    Accepted,
    Dropped { dropped_frames: u32 },
    Block,
}

pub struct DropOldest<const N: usize = 64>;
pub struct DropNewest<const N: usize = 64>;
pub struct Latest;
pub struct Queue<const N: usize>;
pub struct BlockOnFull<const N: usize = 64>;

pub struct BackpressureQueue<P>
where
    P: BackpressurePolicy,
{
    queue: VecDeque<Bytes>,
    dropped_frames: u64,
    _marker: PhantomData<fn() -> P>,
}

impl<P> BackpressureQueue<P>
where
    P: BackpressurePolicy,
{
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(P::CAPACITY),
            dropped_frames: 0,
            _marker: PhantomData,
        }
    }

    pub fn push(&mut self, frame: Bytes) -> OverflowAction {
        if self.queue.len() < P::CAPACITY {
            self.queue.push_back(frame);
            return OverflowAction::Accepted;
        }

        let action = P::on_full(&mut self.queue, frame);
        if let OverflowAction::Dropped { dropped_frames } = action {
            self.dropped_frames = self
                .dropped_frames
                .saturating_add(u64::from(dropped_frames));
        }
        action
    }

    pub fn pop_front(&mut self) -> Option<Bytes> {
        self.queue.pop_front()
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.queue.len()
    }

    #[must_use]
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.queue.len() >= P::CAPACITY
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl<P> Default for BackpressureQueue<P>
where
    P: BackpressurePolicy,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> BackpressurePolicy for DropOldest<N> {
    const CAPACITY: usize = N;

    fn on_full(queue: &mut VecDeque<Bytes>, frame: Bytes) -> OverflowAction {
        if Self::CAPACITY == 0 {
            return OverflowAction::Block;
        }

        let dropped_frames = u32::from(queue.pop_front().is_some());
        queue.push_back(frame);
        OverflowAction::Dropped { dropped_frames }
    }
}

impl<const N: usize> BackpressurePolicy for DropNewest<N> {
    const CAPACITY: usize = N;

    fn on_full(_queue: &mut VecDeque<Bytes>, _frame: Bytes) -> OverflowAction {
        if Self::CAPACITY == 0 {
            OverflowAction::Block
        } else {
            OverflowAction::Dropped { dropped_frames: 1 }
        }
    }
}

impl BackpressurePolicy for Latest {
    const CAPACITY: usize = 1;

    fn on_full(queue: &mut VecDeque<Bytes>, frame: Bytes) -> OverflowAction {
        let dropped_frames = queue.len().min(u32::MAX as usize) as u32;
        queue.clear();
        queue.push_back(frame);
        OverflowAction::Dropped { dropped_frames }
    }
}

impl<const N: usize> BackpressurePolicy for Queue<N> {
    const CAPACITY: usize = N;

    fn on_full(_queue: &mut VecDeque<Bytes>, _frame: Bytes) -> OverflowAction {
        OverflowAction::Block
    }
}

impl<const N: usize> BackpressurePolicy for BlockOnFull<N> {
    const CAPACITY: usize = N;

    fn on_full(_queue: &mut VecDeque<Bytes>, _frame: Bytes) -> OverflowAction {
        OverflowAction::Block
    }
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
