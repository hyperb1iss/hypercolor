//! Async transport abstraction for USB I/O.

use std::time::Duration;

use async_trait::async_trait;

pub mod control;
pub mod hid;
pub mod vendor;

/// Async byte-level I/O transport.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Human-readable transport name.
    fn name(&self) -> &'static str;

    /// Send raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when I/O fails.
    async fn send(&self, data: &[u8]) -> Result<(), TransportError>;

    /// Receive raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when I/O fails.
    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError>;

    /// Send then receive in one helper operation.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when send/receive fails.
    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        self.receive(timeout).await
    }

    /// Close transport and release resources.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when close fails.
    async fn close(&self) -> Result<(), TransportError>;
}

/// Transport-level errors.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Device could not be found.
    #[error("device not found: {detail}")]
    NotFound {
        /// Human-readable detail.
        detail: String,
    },

    /// Generic I/O failure.
    #[error("USB I/O error: {detail}")]
    IoError {
        /// Human-readable detail.
        detail: String,
    },

    /// I/O timeout.
    #[error("transport timeout after {timeout_ms}ms")]
    Timeout {
        /// Timeout budget used for the operation.
        timeout_ms: u64,
    },

    /// Transport already closed.
    #[error("transport closed")]
    Closed,

    /// Access denied by OS policy or udev rules.
    #[error("permission denied: {detail}")]
    PermissionDenied {
        /// Human-readable detail.
        detail: String,
    },
}
