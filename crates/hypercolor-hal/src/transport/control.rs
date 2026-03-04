//! USB control-transfer transport used by Razer devices.

use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};

use crate::transport::{Transport, TransportError};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(1_000);
const DEFAULT_MAX_PACKET_LEN: usize = 90;
const HID_REPORT_TYPE_FEATURE: u16 = 0x03;

/// USB control transfer transport for HID feature reports.
pub struct UsbControlTransport {
    _device: nusb::Device,
    interface: nusb::Interface,
    interface_number: u8,
    report_id: u8,
    max_packet_len: usize,
    closed: AtomicBool,
    op_lock: tokio::sync::Mutex<()>,
}

impl UsbControlTransport {
    /// Open and claim a USB interface for control transfers.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the interface cannot be claimed.
    pub async fn new(
        device: nusb::Device,
        interface_number: u8,
        report_id: u8,
    ) -> Result<Self, TransportError> {
        let interface = device
            .claim_interface(interface_number)
            .await
            .map_err(|error| map_nusb_error(&error))?;

        Ok(Self {
            _device: device,
            interface,
            interface_number,
            report_id,
            max_packet_len: DEFAULT_MAX_PACKET_LEN,
            closed: AtomicBool::new(false),
            op_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn w_value(&self) -> u16 {
        (HID_REPORT_TYPE_FEATURE << 8) | u16::from(self.report_id)
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }
}

#[async_trait]
impl Transport for UsbControlTransport {
    fn name(&self) -> &'static str {
        "USB Control (HID Feature Report)"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;

        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x09,
                    value: self.w_value(),
                    index: u16::from(self.interface_number),
                    data,
                },
                DEFAULT_IO_TIMEOUT,
            )
            .await
            .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let length = u16::try_from(self.max_packet_len).map_err(|_| TransportError::IoError {
            detail: "configured packet length exceeds u16".to_owned(),
        })?;

        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x01,
                    value: self.w_value(),
                    index: u16::from(self.interface_number),
                    length,
                },
                timeout,
            )
            .await
            .map_err(|error| map_transfer_error(error, timeout))
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn map_nusb_error(error: &nusb::Error) -> TransportError {
    match error.kind() {
        nusb::ErrorKind::NotFound => TransportError::NotFound {
            detail: error.to_string(),
        },
        nusb::ErrorKind::PermissionDenied => TransportError::PermissionDenied {
            detail: error.to_string(),
        },
        _ => TransportError::IoError {
            detail: error.to_string(),
        },
    }
}

fn map_transfer_error(error: TransferError, timeout: Duration) -> TransportError {
    match error {
        TransferError::Cancelled => TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        },
        TransferError::Disconnected => TransportError::NotFound {
            detail: error.to_string(),
        },
        _ => TransportError::IoError {
            detail: error.to_string(),
        },
    }
}
