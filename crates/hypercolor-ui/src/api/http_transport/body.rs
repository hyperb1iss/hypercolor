//! Incremental HTTP body ownership and bounded consumers.

use std::any::Any;
use std::future::{Future, poll_fn};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};

use serde::de::DeserializeOwned;

use super::HttpCancellation;
use super::stream::HttpStreamError;

/// Single-consumer pull source. Implementations must not read ahead of demand.
pub trait HttpBodySource {
    /// Borrow this concrete source for an equivalent native representation.
    /// Implementations return `Some(self)`, never independently supplied hint data.
    /// Inspection must not consume bytes or start I/O. Transports inspect through
    /// `HttpBody::source_hint`, which enforces the pristine single-consumer boundary.
    fn source_hint(&self) -> Option<&dyn Any> {
        None
    }

    /// Exact encoded byte length when known, without materializing the body.
    fn exact_length(&self) -> Option<u64>;

    /// Produce at most `maximum` bytes. An empty body ends with `None`, never an empty chunk.
    /// A pending call may retain at most the requested chunk and must not start another read.
    fn poll_chunk(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>>;

    /// Release owned readers and buffers. Called once on failure, cancellation, or early drop.
    fn cancel(&mut self);
}

/// An owned body. Only one read can borrow it at a time.
pub struct HttpBody {
    source: Box<dyn HttpBodySource>,
    cancellation: HttpCancellation,
    exact_length: Option<u64>,
    consumed: u64,
    terminal: bool,
    started: bool,
}

impl HttpBody {
    #[must_use]
    pub fn new(source: Box<dyn HttpBodySource>, cancellation: HttpCancellation) -> Self {
        let exact_length = source.exact_length();
        Self {
            source,
            cancellation,
            exact_length,
            consumed: 0,
            terminal: false,
            started: false,
        }
    }

    #[must_use]
    pub fn exact_length(&self) -> Option<u64> {
        self.exact_length
    }

    #[must_use]
    pub fn cancellation(&self) -> HttpCancellation {
        self.cancellation.clone()
    }

    /// Inspect the owned source only before any read has been polled.
    /// A native transport must own exchange cancellation (for example through
    /// `HttpStreamFuture`) before cloning native handles and discarding this producer.
    /// A native representation is an alternative consumption path, never a retry copy.
    #[must_use]
    pub fn source_hint(&self) -> Option<&dyn Any> {
        if self.started || self.terminal || self.cancellation.is_cancelled() {
            None
        } else {
            self.source.source_hint()
        }
    }

    pub fn cancel(&mut self) {
        self.cancellation.cancel();
        self.abort_source();
    }

    /// Stop this body's producer without aborting the whole exchange.
    /// Transports use this after receiving early response headers (for example 413)
    /// so an unfinished upload is released while the response remains readable.
    /// Ordinary owner drop and `cancel` still abort the complete exchange.
    pub fn discard(mut self) {
        self.abort_source();
    }

    fn abort_source(&mut self) {
        if !self.terminal {
            self.terminal = true;
            self.source.cancel();
        }
    }

    /// Read only after the consumer has reserved capacity for `maximum` bytes.
    ///
    /// # Errors
    /// Returns cancellation, source failure, invalid chunk, or declared-length mismatch.
    pub async fn read_chunk(
        &mut self,
        maximum: NonZeroUsize,
    ) -> Result<Option<Vec<u8>>, HttpStreamError> {
        if self.cancellation.is_cancelled() {
            self.abort_source();
            return Err(HttpStreamError::Cancelled);
        }
        if self.terminal {
            return Ok(None);
        }
        let mut cancelled = self.cancellation.cancelled();
        let result = poll_fn(|context| {
            if Pin::new(&mut cancelled).poll(context).is_ready() {
                return Poll::Ready(Err(HttpStreamError::Cancelled));
            }
            self.started = true;
            self.source.poll_chunk(context, maximum)
        })
        .await;
        let result = result.and_then(|chunk| {
            if let Some(bytes) = &chunk {
                if bytes.is_empty() || bytes.len() > maximum.get() {
                    return Err(HttpStreamError::InvalidChunk);
                }
                self.consumed = self
                    .consumed
                    .checked_add(bytes.len() as u64)
                    .ok_or(HttpStreamError::LengthMismatch)?;
                if self
                    .exact_length
                    .is_some_and(|length| self.consumed > length)
                {
                    return Err(HttpStreamError::LengthMismatch);
                }
            } else {
                if self
                    .exact_length
                    .is_some_and(|length| self.consumed != length)
                {
                    return Err(HttpStreamError::LengthMismatch);
                }
                self.terminal = true;
            }
            Ok(chunk)
        });
        if result.is_err() {
            self.cancel();
        }
        result
    }

    /// Collect only a small, explicitly bounded JSON document. The bound covers encoded
    /// bytes; decoded application values have their own allocation cost. Unknown-length
    /// sources may produce one extra byte to distinguish exact-limit EOF from overflow.
    ///
    /// # Errors
    /// Returns stream failures, size overflow, or invalid JSON.
    pub async fn collect_json<T: DeserializeOwned>(
        mut self,
        maximum_bytes: usize,
        chunk_size: NonZeroUsize,
    ) -> Result<T, HttpStreamError> {
        if self
            .exact_length
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(HttpStreamError::BodyTooLarge);
        }
        let mut bytes = Vec::new();
        loop {
            let remaining = maximum_bytes - bytes.len();
            let maximum = NonZeroUsize::new(chunk_size.get().min(remaining.saturating_add(1)))
                .expect("probe size is nonzero");
            let Some(chunk) = self.read_chunk(maximum).await? else {
                break;
            };
            if chunk.len() > remaining {
                return Err(HttpStreamError::BodyTooLarge);
            }
            bytes
                .try_reserve_exact(chunk.len())
                .map_err(|_| HttpStreamError::CapacityUnavailable)?;
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| HttpStreamError::InvalidJson(error.to_string()))
    }

    /// Deliver the next chunk only after the sink consumed the previous one.
    ///
    /// # Errors
    /// Returns source, sink, cancellation, or length-validation failures.
    pub async fn copy_to(
        mut self,
        sink: &mut dyn HttpBodySink,
        chunk_size: NonZeroUsize,
    ) -> Result<u64, HttpStreamError> {
        let mut sink = SinkOwner {
            sink,
            cancellation: self.cancellation.clone(),
            finished: false,
        };
        let mut cancelled = self.cancellation.cancelled();
        while let Some(chunk) = self.read_chunk(chunk_size).await? {
            poll_fn(|context| {
                if Pin::new(&mut cancelled).poll(context).is_ready() {
                    return Poll::Ready(Err(HttpStreamError::Cancelled));
                }
                sink.sink.poll_write(context, &chunk)
            })
            .await?;
            // Ready in-memory sources must yield between chunks so other streams and
            // interactive work can run even when neither endpoint blocks.
            let mut yielded = false;
            poll_fn(|context| {
                if yielded {
                    return Poll::Ready(());
                }
                yielded = true;
                context.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
        }
        poll_fn(|context| {
            if Pin::new(&mut cancelled).poll(context).is_ready() {
                return Poll::Ready(Err(HttpStreamError::Cancelled));
            }
            sink.sink.poll_finish(context)
        })
        .await?;
        sink.finished = true;
        Ok(self.consumed)
    }
}

impl Drop for HttpBody {
    fn drop(&mut self) {
        if !self.terminal {
            self.cancel();
        }
    }
}

/// A sink acknowledges a chunk only after its destination consumed the bytes.
/// Copying into an unbounded intermediate queue is not consumption.
pub trait HttpBodySink {
    fn poll_write(
        &mut self,
        context: &mut Context<'_>,
        chunk: &[u8],
    ) -> Poll<Result<(), HttpStreamError>>;
    fn poll_finish(&mut self, context: &mut Context<'_>) -> Poll<Result<(), HttpStreamError>>;
    fn cancel(&mut self);
}

struct SinkOwner<'a> {
    sink: &'a mut dyn HttpBodySink,
    cancellation: HttpCancellation,
    finished: bool,
}

impl Drop for SinkOwner<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancellation.cancel();
            self.sink.cancel();
        }
    }
}
