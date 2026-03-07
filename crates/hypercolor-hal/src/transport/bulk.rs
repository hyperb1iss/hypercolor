//! USB bulk transport with optional HID feature-report sideband support.

use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nusb::MaybeFuture;
use nusb::transfer::{
    Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Out, Recipient, TransferError,
};
use tracing::{debug, trace};

use crate::protocol::TransferType;
use crate::transport::{Transport, TransportError};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(1_000);
const DEFAULT_REPORT_LEN: usize = 32;
const HID_REPORT_TYPE_FEATURE: u16 = 0x03;

/// USB bulk transport for high-bandwidth devices with HID sideband control.
pub struct UsbBulkTransport {
    _device: nusb::Device,
    interface: nusb::Interface,
    interface_number: u8,
    report_id: u8,
    out_endpoint_address: u8,
    in_endpoint_address: u8,
    out_max_packet_size: usize,
    in_max_packet_size: usize,
    out_endpoint: Arc<Mutex<nusb::Endpoint<Bulk, Out>>>,
    in_endpoint: Arc<Mutex<nusb::Endpoint<Bulk, In>>>,
    report_len: usize,
    closed: AtomicBool,
    op_lock: Arc<Mutex<()>>,
}

impl UsbBulkTransport {
    /// Open and claim a USB bulk interface.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the interface cannot be claimed or bulk
    /// endpoints cannot be opened.
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

        let descriptor = interface
            .descriptor()
            .ok_or_else(|| TransportError::NotFound {
                detail: format!("interface {interface_number} has no active descriptor"),
            })?;

        let mut out_endpoint = None;
        let mut in_endpoint = None;

        for endpoint in descriptor.endpoints() {
            if endpoint.transfer_type() != nusb::descriptors::TransferType::Bulk {
                continue;
            }

            if endpoint.address() & 0x80 == 0 {
                out_endpoint = Some((endpoint.address(), endpoint.max_packet_size()));
            } else {
                in_endpoint = Some((endpoint.address(), endpoint.max_packet_size()));
            }
        }

        let (out_address, out_max_packet_size) =
            out_endpoint.ok_or_else(|| TransportError::NotFound {
                detail: format!("no bulk OUT endpoint found on interface {interface_number}"),
            })?;
        let (in_address, in_max_packet_size) =
            in_endpoint.ok_or_else(|| TransportError::NotFound {
                detail: format!("no bulk IN endpoint found on interface {interface_number}"),
            })?;

        let out_handle = interface
            .endpoint::<Bulk, Out>(out_address)
            .map_err(|error| map_nusb_error(&error))?;
        let in_handle = interface
            .endpoint::<Bulk, In>(in_address)
            .map_err(|error| map_nusb_error(&error))?;

        debug!(
            interface_number,
            report_id = format_args!("0x{report_id:02X}"),
            out_endpoint = format_args!("0x{out_address:02X}"),
            in_endpoint = format_args!("0x{in_address:02X}"),
            out_max_packet_size,
            in_max_packet_size,
            "opened USB bulk transport"
        );

        Ok(Self {
            _device: device,
            interface,
            interface_number,
            report_id,
            out_endpoint_address: out_address,
            in_endpoint_address: in_address,
            out_max_packet_size,
            in_max_packet_size,
            out_endpoint: Arc::new(Mutex::new(out_handle)),
            in_endpoint: Arc::new(Mutex::new(in_handle)),
            report_len: DEFAULT_REPORT_LEN,
            closed: AtomicBool::new(false),
            op_lock: Arc::new(Mutex::new(())),
        })
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }

        Ok(())
    }

    fn send_bulk_locked(&self, data: &[u8]) -> Result<(), TransportError> {
        let packet = normalize_packet(data, self.out_max_packet_size)?;
        let mut endpoint = lock_mutex(&self.out_endpoint, "bulk OUT endpoint")?;

        trace!(
            interface_number = self.interface_number,
            endpoint = format_args!("0x{:02X}", self.out_endpoint_address),
            packet_len = packet.len(),
            packet_hex = %format_hex_preview(&packet, 32),
            "usb bulk send"
        );

        endpoint
            .transfer_blocking(packet.into(), DEFAULT_IO_TIMEOUT)
            .into_result()
            .map(|_| ())
            .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
    }

    fn receive_bulk_locked(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let mut endpoint = lock_mutex(&self.in_endpoint, "bulk IN endpoint")?;
        let response = endpoint
            .transfer_blocking(Buffer::new(self.in_max_packet_size), timeout)
            .into_result()
            .map_err(|error| map_transfer_error(error, timeout))?
            .into_vec();

        trace!(
            interface_number = self.interface_number,
            endpoint = format_args!("0x{:02X}", self.in_endpoint_address),
            response_len = response.len(),
            response_hex = %format_hex_preview(&response, 32),
            "usb bulk receive"
        );

        Ok(response)
    }

    fn send_report_locked(&self, data: &[u8]) -> Result<(), TransportError> {
        trace!(
            interface_number = self.interface_number,
            report_id = format_args!("0x{:02X}", self.report_id),
            packet_len = data.len(),
            packet_hex = %format_hex_preview(data, 32),
            "usb hid report send over bulk transport"
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
            .wait()
            .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
    }

    fn receive_report_locked(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let length = u16::try_from(self.report_len).map_err(|_| TransportError::IoError {
            detail: "configured report length exceeds u16".to_owned(),
        })?;

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
            .wait()
            .map_err(|error| map_transfer_error(error, timeout))?;

        trace!(
            interface_number = self.interface_number,
            report_id = format_args!("0x{:02X}", self.report_id),
            response_len = response.len(),
            response_hex = %format_hex_preview(&response, 32),
            "usb hid report receive over bulk transport"
        );

        Ok(response)
    }

    fn w_value(&self) -> u16 {
        (HID_REPORT_TYPE_FEATURE << 8) | u16::from(self.report_id)
    }
}

#[async_trait]
impl Transport for UsbBulkTransport {
    fn name(&self) -> &'static str {
        "USB Bulk"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = lock_mutex(&self.op_lock, "operation lock")?;
        match transfer_type {
            TransferType::Primary | TransferType::Bulk => self.send_bulk_locked(data),
            TransferType::HidReport => self.send_report_locked(data),
        }
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, TransferType::Primary).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = lock_mutex(&self.op_lock, "operation lock")?;
        match transfer_type {
            TransferType::Primary | TransferType::Bulk => self.receive_bulk_locked(timeout),
            TransferType::HidReport => self.receive_report_locked(timeout),
        }
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_receive_with_type(data, timeout, TransferType::Primary)
            .await
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = lock_mutex(&self.op_lock, "operation lock")?;
        match transfer_type {
            TransferType::Primary | TransferType::Bulk => {
                self.send_bulk_locked(data)?;
                self.receive_bulk_locked(timeout)
            }
            TransferType::HidReport => {
                self.send_report_locked(data)?;
                self.receive_report_locked(timeout)
            }
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn normalize_packet(data: &[u8], endpoint_packet_size: usize) -> Result<Vec<u8>, TransportError> {
    if data.len() > endpoint_packet_size {
        return Err(TransportError::IoError {
            detail: format!(
                "packet too large for endpoint ({} > {})",
                data.len(),
                endpoint_packet_size
            ),
        });
    }

    if data.len() == endpoint_packet_size {
        return Ok(data.to_vec());
    }

    let mut padded = vec![0_u8; endpoint_packet_size];
    padded[..data.len()].copy_from_slice(data);
    Ok(padded)
}

fn lock_mutex<'a, T>(
    mutex: &'a Mutex<T>,
    name: &str,
) -> Result<std::sync::MutexGuard<'a, T>, TransportError> {
    mutex.lock().map_err(|_| TransportError::IoError {
        detail: format!("{name} mutex poisoned"),
    })
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
