//! USB HID interrupt transport placeholder.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::transport::{Transport, TransportError};

/// USB HID interrupt transport.
///
/// This transport is reserved for controllers that communicate over HID
/// interrupt endpoints (Lian Li modern hubs and `PrismRGB`).
pub struct UsbHidTransport {
    interface_number: u8,
    closed: AtomicBool,
}

impl UsbHidTransport {
    /// Create a HID interrupt transport wrapper.
    #[must_use]
    pub fn new(interface_number: u8) -> Self {
        Self {
            interface_number,
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

#[async_trait]
impl Transport for UsbHidTransport {
    fn name(&self) -> &'static str {
        "USB HID Interrupt"
    }

    async fn send(&self, _data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;
        Err(TransportError::IoError {
            detail: format!(
                "USB HID interrupt transport is not implemented yet for interface {}",
                self.interface_number
            ),
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
