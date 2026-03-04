//! USB vendor-specific control transport placeholder.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::transport::{Transport, TransportError};

/// USB vendor-specific control transfer transport.
///
/// This transport is reserved for devices that communicate through
/// vendor-addressed control registers (for example, older Lian Li hubs).
pub struct UsbVendorTransport {
    closed: AtomicBool,
}

impl UsbVendorTransport {
    /// Create a vendor control transport wrapper.
    #[must_use]
    pub fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
        }
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }
}

impl Default for UsbVendorTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for UsbVendorTransport {
    fn name(&self) -> &'static str {
        "USB Vendor Control"
    }

    async fn send(&self, _data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;
        Err(TransportError::IoError {
            detail: "USB vendor transport is not implemented yet".to_owned(),
        })
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}
