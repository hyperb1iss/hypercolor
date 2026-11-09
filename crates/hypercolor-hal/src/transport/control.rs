//! USB control-transfer transport used by Razer devices.

use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};
use tracing::{debug, trace};

use crate::transport::{Transport, TransportError};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);
const HID_REPORT_TYPE_FEATURE: u16 = 0x03;
const HID_REQUEST_GET_REPORT: u8 = 0x01;
const HID_REQUEST_SET_REPORT: u8 = 0x09;

/// Data-stage length this transport requests on every feature report read.
#[doc(hidden)]
pub const FEATURE_REPORT_READ_LEN: usize = 90;

/// Setup fields of one HID class feature-report request, as this transport
/// hands them to the host controller.
///
/// `bmRequestType` is always class, interface recipient, so only the
/// fields that vary are carried.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureReportSetup {
    /// `bRequest`: `0x09` for `SET_REPORT`, `0x01` for `GET_REPORT`.
    pub request: u8,
    /// `wValue`: the feature report type in the high byte, the report ID in
    /// the low byte.
    pub value: u16,
    /// `wIndex`: the interface number.
    pub index: u16,
}

impl FeatureReportSetup {
    /// The `SET_REPORT` request that carries an outgoing feature report.
    #[must_use]
    pub fn set_report(report_id: u8, interface_number: u8) -> Self {
        Self::new(HID_REQUEST_SET_REPORT, report_id, interface_number)
    }

    /// The `GET_REPORT` request that reads a feature report back.
    #[must_use]
    pub fn get_report(report_id: u8, interface_number: u8) -> Self {
        Self::new(HID_REQUEST_GET_REPORT, report_id, interface_number)
    }

    fn new(request: u8, report_id: u8, interface_number: u8) -> Self {
        Self {
            request,
            value: (HID_REPORT_TYPE_FEATURE << 8) | u16::from(report_id),
            index: u16::from(interface_number),
        }
    }
}

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
            max_packet_len: FEATURE_REPORT_READ_LEN,
            closed: AtomicBool::new(false),
            op_lock: tokio::sync::Mutex::new(()),
        })
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

        self.send_locked(data).await
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

        self.send_locked(data).await?;

        self.receive_locked(timeout).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

impl UsbControlTransport {
    async fn send_locked(&self, data: &[u8]) -> Result<(), TransportError> {
        let setup = FeatureReportSetup::set_report(self.report_id, self.interface_number);
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: setup.request,
                    value: setup.value,
                    index: setup.index,
                    data,
                },
                DEFAULT_IO_TIMEOUT,
            )
            .await
            .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
    }

    async fn receive_locked(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let setup = FeatureReportSetup::get_report(self.report_id, self.interface_number);
        let length = u16::try_from(self.max_packet_len).map_err(|_| TransportError::IoError {
            detail: "configured packet length exceeds u16".to_owned(),
        })?;
        trace!(
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
                    request: setup.request,
                    value: setup.value,
                    index: setup.index,
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
        TransferError::Disconnected => TransportError::Disconnected {
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
