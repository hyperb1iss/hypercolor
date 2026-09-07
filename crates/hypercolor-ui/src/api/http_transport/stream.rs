//! Headers-first HTTP exchanges with explicit cancellation ownership.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::{HttpBody, HttpCancellation, HttpCancelled, HttpHeader, HttpMethod};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpStreamError {
    Unsupported,
    Cancelled,
    InvalidChunk,
    LengthMismatch,
    BodyTooLarge,
    CapacityUnavailable,
    InvalidJson(String),
    Transport(String),
}

impl std::fmt::Display for HttpStreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("incremental HTTP is not supported by this transport")
            }
            Self::Cancelled => formatter.write_str("HTTP operation was cancelled"),
            Self::InvalidChunk => {
                formatter.write_str("body source violated the requested chunk bound")
            }
            Self::LengthMismatch => {
                formatter.write_str("body length does not match its declared length")
            }
            Self::CapacityUnavailable => {
                formatter.write_str("body consumer could not reserve memory")
            }
            Self::BodyTooLarge => formatter.write_str("body exceeds the consumer's byte limit"),
            Self::InvalidJson(message) | Self::Transport(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for HttpStreamError {}

/// An encoded body source. Multipart encoders provide a source without collecting file data.
pub struct HttpStreamRequest {
    pub method: HttpMethod,
    pub path: String,
    pub headers: Vec<HttpHeader>,
    pub body: HttpBody,
}

/// Status and headers are available before any response body read is required.
pub struct HttpStreamResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: HttpBody,
}

type HeadersFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpStreamResponse, HttpStreamError>> + 'a>>;

/// Owns the exchange until headers arrive. Dropping it cancels pending transport work.
/// A successful response transfers cancellation ownership to its body.
pub struct HttpStreamFuture<'a> {
    future: Option<HeadersFuture<'a>>,
    cancellation: HttpCancellation,
    cancelled: HttpCancelled,
    complete: bool,
}

impl<'a> HttpStreamFuture<'a> {
    /// The future and response body must share this signal. The transport must release
    /// its network operation when the future is dropped or the signal is cancelled.
    pub fn new(
        cancellation: HttpCancellation,
        future: impl Future<Output = Result<HttpStreamResponse, HttpStreamError>> + 'a,
    ) -> Self {
        let cancelled = cancellation.cancelled();
        Self {
            future: Some(Box::pin(future)),
            cancellation,
            cancelled,
            complete: false,
        }
    }
}

impl Future for HttpStreamFuture<'_> {
    type Output = Result<HttpStreamResponse, HttpStreamError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if Pin::new(&mut self.cancelled).poll(context).is_ready() {
            self.future.take();
            self.complete = true;
            return Poll::Ready(Err(HttpStreamError::Cancelled));
        }
        let result = self
            .future
            .as_mut()
            .expect("headers future polled after completion")
            .as_mut()
            .poll(context);
        if result.is_ready() {
            self.future.take();
            self.complete = true;
            if matches!(result, Poll::Ready(Err(_))) {
                self.cancellation.cancel();
            }
        }
        result
    }
}

impl Drop for HttpStreamFuture<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.cancellation.cancel();
        }
    }
}
