//! HTTP for driver clients, bounded by construction.
//!
//! Finite exchanges use a [`ReadBudget`]: a byte cap and an absolute
//! deadline covering headers and body. [`BoundedResponse`] counts bytes as
//! chunks arrive whatever `Content-Length` claims. A slow drip cannot extend
//! the deadline, and an oversized reply is refused before it is buffered.
//!
//! Long-lived SSE subscriptions instead bound header receipt and each event
//! or standalone comment through [`BoundedRequest::subscribe`]. Completed
//! comments leave a quiet connection open without an idle deadline.
//!
//! Drivers use this module instead of depending on `reqwest`, which keeps
//! the unbounded readers out of reach. Authentication, status handling,
//! parsing, and each endpoint's limit stay in the vendor client.

use std::fmt;
use std::time::Duration;

use reqwest::IntoUrl;
pub use reqwest::{Method, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio::time::Instant;

use crate::deadline::until_deadline;

/// Why an HTTP exchange failed.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("HTTP client could not be built: {0}")]
    ClientBuild(#[source] reqwest::Error),
    /// The transport failed. Carries no URL: some device APIs put a
    /// credential in the path, and this error ends up in logs.
    #[error("HTTP transport failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("HTTP status {0}")]
    Status(StatusCode),
    #[error("HTTP exchange missed its deadline")]
    DeadlineExceeded,
    #[error("HTTP body exceeds {limit} bytes")]
    BodyTooLarge { limit: usize },
    #[error("HTTP body is not the expected JSON: {0}")]
    Json(#[source] serde_json::Error),
}

/// What one finite exchange may spend: a cap on the body and an absolute
/// deadline covering connect, headers, and every chunk of the body.
/// SSE subscriptions use per-event limits through [`BoundedRequest::subscribe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadBudget {
    max_bytes: usize,
    deadline: Instant,
}

impl ReadBudget {
    /// A budget of `max_bytes` that expires `timeout` from now.
    #[must_use]
    pub fn new(max_bytes: usize, timeout: Duration) -> Self {
        Self {
            max_bytes,
            deadline: Instant::now() + timeout,
        }
    }
}

/// An HTTP client with bounded finite responses and SSE subscriptions.
#[derive(Debug, Clone)]
pub struct BoundedHttpClient {
    inner: reqwest::Client,
}

impl BoundedHttpClient {
    /// A client with default transport policy.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::ClientBuild`] if the TLS backend cannot start.
    pub fn new() -> Result<Self, HttpError> {
        Self::builder().build()
    }

    /// Start a client with non-default transport policy.
    #[must_use]
    pub fn builder() -> BoundedHttpClientBuilder {
        BoundedHttpClientBuilder {
            inner: reqwest::Client::builder(),
        }
    }

    /// Start a `GET`.
    pub fn get(&self, url: impl IntoUrl) -> BoundedRequest {
        self.request(Method::GET, url)
    }

    /// Start a `POST`.
    pub fn post(&self, url: impl IntoUrl) -> BoundedRequest {
        self.request(Method::POST, url)
    }

    /// Start a `PUT`.
    pub fn put(&self, url: impl IntoUrl) -> BoundedRequest {
        self.request(Method::PUT, url)
    }

    /// Start a request with any method.
    pub fn request(&self, method: Method, url: impl IntoUrl) -> BoundedRequest {
        BoundedRequest {
            inner: self.inner.request(method, url),
        }
    }
}

/// Transport policy for a [`BoundedHttpClient`]. Time and size limits are
/// not here: finite exchanges use [`ReadBudget`], while subscriptions set
/// their event limits through [`BoundedRequest::subscribe`].
#[derive(Debug)]
pub struct BoundedHttpClientBuilder {
    inner: reqwest::ClientBuilder,
}

impl BoundedHttpClientBuilder {
    /// Answer redirects with the redirect itself. For clients that send a
    /// custom credential header, which would otherwise follow the redirect
    /// to another origin.
    #[must_use]
    pub fn no_redirects(mut self) -> Self {
        self.inner = self.inner.redirect(reqwest::redirect::Policy::none());
        self
    }

    /// Ignore system proxy settings, for devices that are only reachable
    /// on the local network.
    #[must_use]
    pub fn no_proxy(mut self) -> Self {
        self.inner = self.inner.no_proxy();
        self
    }

    /// Accept any TLS certificate, for devices that present self-signed
    /// ones with no way to pin them.
    #[must_use]
    pub fn accept_invalid_certs(mut self) -> Self {
        self.inner = self.inner.danger_accept_invalid_certs(true);
        self
    }

    /// Finish the client.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::ClientBuild`] if the TLS backend cannot start.
    pub fn build(self) -> Result<BoundedHttpClient, HttpError> {
        self.inner
            .build()
            .map(|inner| BoundedHttpClient { inner })
            .map_err(HttpError::ClientBuild)
    }
}

/// A request that requires a finite response budget or SSE event limits.
#[derive(Debug)]
pub struct BoundedRequest {
    inner: reqwest::RequestBuilder,
}

impl BoundedRequest {
    /// Add a header. An invalid name or value fails at [`Self::send`].
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.inner = self.inner.header(name, value);
        self
    }

    /// Append URL query parameters.
    #[must_use]
    pub fn query<T: Serialize + ?Sized>(mut self, query: &T) -> Self {
        self.inner = self.inner.query(query);
        self
    }

    /// Send `body` as JSON.
    #[must_use]
    pub fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Self {
        self.inner = self.inner.json(body);
        self
    }

    /// Subscribe to server-sent events without expiring a quiet connection.
    ///
    /// `timeout` bounds receipt of the headers and completion of each event
    /// once its first byte arrives. `max_event_bytes` bounds one event,
    /// including comments and framing. Standalone comment lines have the same
    /// limits but return to idle when their line ends. Comments inside a
    /// pending event do not reset that event's limits. The body is drained
    /// without collecting payloads; use this when the subscription itself
    /// owns another transport.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::DeadlineExceeded`] for late headers, or
    /// [`HttpError::Transport`] without the request URL.
    pub async fn subscribe(
        self,
        max_event_bytes: usize,
        timeout: Duration,
    ) -> Result<EventStream, HttpError> {
        let inner = until_deadline(Instant::now() + timeout, self.inner.send())
            .await
            .ok_or(HttpError::DeadlineExceeded)?
            .map_err(transport_error)?;
        Ok(EventStream {
            inner,
            max_event_bytes,
            timeout,
            deadline: None,
            event_bytes: 0,
            line: EventLine::Empty,
            event_has_fields: false,
            previous_cr: false,
        })
    }

    /// Send the request. The budget's clock is already running and keeps
    /// running through the body.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::DeadlineExceeded`] when no response head
    /// arrived in time, or [`HttpError::Transport`].
    pub async fn send(self, budget: ReadBudget) -> Result<BoundedResponse, HttpError> {
        let inner = until_deadline(budget.deadline, self.inner.send())
            .await
            .ok_or(HttpError::DeadlineExceeded)?
            .map_err(transport_error)?;
        Ok(BoundedResponse { inner, budget })
    }
}

/// A response whose body can only be read through its [`ReadBudget`].
/// Dropping it without reading abandons the body unread.
#[derive(Debug)]
pub struct BoundedResponse {
    inner: reqwest::Response,
    budget: ReadBudget,
}

impl BoundedResponse {
    /// The response status.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// Turn a 4xx or 5xx status into an error.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::Status`] for a client or server error status.
    pub fn error_for_status(self) -> Result<Self, HttpError> {
        let status = self.status();
        if status.is_client_error() || status.is_server_error() {
            return Err(HttpError::Status(status));
        }
        Ok(self)
    }

    /// Read the body.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::BodyTooLarge`] as soon as the declared or
    /// received length passes the cap, [`HttpError::DeadlineExceeded`] at
    /// the deadline, or [`HttpError::Transport`].
    pub async fn bytes(mut self) -> Result<Vec<u8>, HttpError> {
        let ReadBudget {
            max_bytes,
            deadline,
        } = self.budget;
        let too_large = || HttpError::BodyTooLarge { limit: max_bytes };
        if self
            .inner
            .content_length()
            .is_some_and(|declared| declared > max_bytes as u64)
        {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = until_deadline(deadline, self.inner.chunk())
            .await
            .ok_or(HttpError::DeadlineExceeded)?
            .map_err(transport_error)?
        {
            if body.len() + chunk.len() > max_bytes {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// Read the body and decode it as JSON.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::bytes`], or [`HttpError::Json`].
    pub async fn json<T: DeserializeOwned>(self) -> Result<T, HttpError> {
        let body = self.bytes().await?;
        serde_json::from_slice(&body).map_err(HttpError::Json)
    }
}

/// An SSE response with bounded event framing and no idle deadline.
///
/// The response stays open until its owner drops it or the peer closes it.
/// A healthy peer can remain silent indefinitely; TCP keepalive still detects
/// dead connections through the client's transport policy.
pub struct EventStream {
    inner: reqwest::Response,
    max_event_bytes: usize,
    timeout: Duration,
    deadline: Option<Instant>,
    event_bytes: usize,
    line: EventLine,
    event_has_fields: bool,
    previous_cr: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventLine {
    Empty,
    Comment,
    Field,
}

impl fmt::Debug for EventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventStream")
            .field("status", &self.status())
            .field("max_event_bytes", &self.max_event_bytes)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl EventStream {
    /// The subscription response status, before reading its body.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// Drain the next chunk while validating event size and completion time.
    /// Returns `false` when the stream closes. Cancellation preserves framing
    /// state, so callers can select this against another transport.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::BodyTooLarge`] for an oversized event,
    /// [`HttpError::DeadlineExceeded`] for a partial event that stops making
    /// progress within its original deadline, or [`HttpError::Transport`].
    pub async fn receive(&mut self) -> Result<bool, HttpError> {
        let chunk = if let Some(deadline) = self.deadline {
            until_deadline(deadline, self.inner.chunk())
                .await
                .ok_or(HttpError::DeadlineExceeded)?
        } else {
            self.inner.chunk().await
        }
        .map_err(transport_error)?;
        let Some(chunk) = chunk else {
            return Ok(false);
        };
        for byte in chunk {
            // CR completes a line immediately; a following LF belongs to the
            // same event's byte budget, even after its terminating blank line.
            let continues_cr = byte == b'\n' && self.previous_cr;
            if self.deadline.is_none() && !continues_cr {
                self.event_bytes = 0;
                self.deadline = Some(Instant::now() + self.timeout);
            }
            self.previous_cr = byte == b'\r';
            self.event_bytes = self.event_bytes.saturating_add(1);
            if self.event_bytes > self.max_event_bytes {
                return Err(HttpError::BodyTooLarge {
                    limit: self.max_event_bytes,
                });
            }
            if continues_cr {
                continue;
            }
            if matches!(byte, b'\r' | b'\n') {
                if self.line == EventLine::Empty
                    || (self.line == EventLine::Comment && !self.event_has_fields)
                {
                    self.deadline = None;
                    self.event_has_fields = false;
                }
                self.line = EventLine::Empty;
            } else if self.line == EventLine::Empty {
                self.line = if byte == b':' {
                    EventLine::Comment
                } else {
                    self.event_has_fields = true;
                    EventLine::Field
                };
            }
        }
        Ok(true)
    }
}

fn transport_error(error: reqwest::Error) -> HttpError {
    HttpError::Transport(error.without_url())
}
