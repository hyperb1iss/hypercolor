//! USB control-transfer transport used by Razer devices.

use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};
use tracing::{debug, trace};

use crate::transport::{Transport, TransportError};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);
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
        #[cfg(target_os = "linux")]
        let interface = device
            .detach_and_claim_interface(interface_number)
            .await
            .map_err(|error| map_nusb_error(&error))?;

        #[cfg(not(target_os = "linux"))]
        let interface = device
            .claim_interface(interface_number)
            .await
            .map_err(|error| map_nusb_error(&error))?;

        debug!(
            interface_number,
            report_id = format_args!("0x{report_id:02X}"),
            "opened USB control transport"
        );

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
        trace!(
            interface_number = self.interface_number,
            report_id = format_args!("0x{:02X}", self.report_id),
            packet_len = data.len(),
            packet_hex = %format_hex_preview(data, 32),
            "usb control feature report send"
        );

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
        self.receive_locked(timeout).await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        trace!(
            interface_number = self.interface_number,
            report_id = format_args!("0x{:02X}", self.report_id),
            packet_len = data.len(),
            packet_hex = %format_hex_preview(data, 32),
            "usb control feature report send_receive"
        );

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
            .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))?;

        self.receive_locked(timeout).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

impl UsbControlTransport {
    async fn receive_locked(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let length = u16::try_from(self.max_packet_len).map_err(|_| TransportError::IoError {
            detail: "configured packet length exceeds u16".to_owned(),
        })?;
        debug!(
            interface_number = self.interface_number,
            report_id = format_args!("0x{:02X}", self.report_id),
            timeout_ms = timeout.as_millis(),
            max_packet_len = self.max_packet_len,
            "usb control feature report receive requested"
        );

        let response = self
            .interface
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
            .map_err(|error| map_transfer_error(error, timeout))?;

        trace!(
            interface_number = self.interface_number,
            report_id = format_args!("0x{:02X}", self.report_id),
            response_len = response.len(),
            response_hex = %format_hex_preview(&response, 32),
            "usb control feature report received"
        );

        Ok(response)
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

fn format_hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    let preview_len = bytes.len().min(max_bytes);
    let mut rendered = bytes
        .iter()
        .take(preview_len)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");

    if bytes.len() > preview_len {
        use std::fmt::Write;
        let _ = write!(rendered, " ... (+{} bytes)", bytes.len() - preview_len);
    }

    if rendered.is_empty() {
        "<empty>".to_owned()
    } else {
        rendered
    }
}
