//! USB HID interrupt transport used by PrismRGB-class devices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nusb::transfer::{Buffer, In, Interrupt, Out, TransferError};
use tracing::{debug, trace};

use crate::transport::{Transport, TransportError};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(1_000);
const HID_REPORT_SIZE: usize = 65;

/// USB HID interrupt transport.
///
/// This path claims a HID interface directly and streams reports through its
/// interrupt IN/OUT endpoints.
pub struct UsbHidTransport {
    _device: nusb::Device,
    _interface: nusb::Interface,
    interface_number: u8,
    out_endpoint_address: u8,
    in_endpoint_address: u8,
    out_max_packet_size: usize,
    in_max_packet_size: usize,
    out_endpoint: Arc<Mutex<nusb::Endpoint<Interrupt, Out>>>,
    in_endpoint: Arc<Mutex<nusb::Endpoint<Interrupt, In>>>,
    closed: AtomicBool,
    op_lock: Arc<Mutex<()>>,
}

impl UsbHidTransport {
    /// Open and claim a USB HID interrupt interface.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the interface cannot be claimed or no
    /// interrupt endpoints can be opened.
    pub async fn new(device: nusb::Device, interface_number: u8) -> Result<Self, TransportError> {
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
            if endpoint.transfer_type() != nusb::descriptors::TransferType::Interrupt {
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
                detail: format!("no interrupt OUT endpoint found on interface {interface_number}"),
            })?;
        let (in_address, in_max_packet_size) =
            in_endpoint.ok_or_else(|| TransportError::NotFound {
                detail: format!("no interrupt IN endpoint found on interface {interface_number}"),
            })?;

        let out_handle = interface
            .endpoint::<Interrupt, Out>(out_address)
            .map_err(|error| map_nusb_error(&error))?;
        let in_handle = interface
            .endpoint::<Interrupt, In>(in_address)
            .map_err(|error| map_nusb_error(&error))?;

        debug!(
            interface_number,
            out_endpoint = format_args!("0x{out_address:02X}"),
            in_endpoint = format_args!("0x{in_address:02X}"),
            out_max_packet_size,
            in_max_packet_size,
            "opened USB HID interrupt transport"
        );

        Ok(Self {
            _device: device,
            _interface: interface,
            interface_number,
            out_endpoint_address: out_address,
            in_endpoint_address: in_address,
            out_max_packet_size,
            in_max_packet_size,
            out_endpoint: Arc::new(Mutex::new(out_handle)),
            in_endpoint: Arc::new(Mutex::new(in_handle)),
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

    fn send_locked(&self, data: &[u8]) -> Result<(), TransportError> {
        let packet = normalize_outgoing_packet(data, self.out_max_packet_size)?;
        let mut endpoint = lock_mutex(&self.out_endpoint, "OUT endpoint")?;

        trace!(
            interface_number = self.interface_number,
            endpoint = format_args!("0x{:02X}", self.out_endpoint_address),
            packet_len = packet.len(),
            packet_hex = %format_hex_preview(&packet, 32),
            "usb hid interrupt send"
        );

        endpoint
            .transfer_blocking(packet.into(), DEFAULT_IO_TIMEOUT)
            .into_result()
            .map(|_| ())
            .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
    }

    fn receive_locked(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let mut endpoint = lock_mutex(&self.in_endpoint, "IN endpoint")?;
        let response = endpoint
            .transfer_blocking(Buffer::new(self.in_max_packet_size), timeout)
            .into_result()
            .map_err(|error| map_transfer_error(error, timeout))?
            .into_vec();

        let normalized = normalize_incoming_packet(&response, self.in_max_packet_size);

        trace!(
            interface_number = self.interface_number,
            endpoint = format_args!("0x{:02X}", self.in_endpoint_address),
            response_len = normalized.len(),
            response_hex = %format_hex_preview(&normalized, 32),
            "usb hid interrupt receive"
        );

        Ok(normalized)
    }
}

#[async_trait]
impl Transport for UsbHidTransport {
    fn name(&self) -> &'static str {
        "USB HID Interrupt"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = lock_mutex(&self.op_lock, "operation lock")?;
        self.send_locked(data)
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = lock_mutex(&self.op_lock, "operation lock")?;
        self.receive_locked(timeout)
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = lock_mutex(&self.op_lock, "operation lock")?;
        self.send_locked(data)?;
        self.receive_locked(timeout)
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn normalize_outgoing_packet(
    data: &[u8],
    endpoint_packet_size: usize,
) -> Result<Vec<u8>, TransportError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    if data.len() == endpoint_packet_size {
        return Ok(data.to_vec());
    }

    if data.len() == HID_REPORT_SIZE && endpoint_packet_size + 1 == HID_REPORT_SIZE && data[0] == 0
    {
        return Ok(data[1..].to_vec());
    }

    if data.len() <= endpoint_packet_size {
        let mut padded = vec![0_u8; endpoint_packet_size];
        padded[..data.len()].copy_from_slice(data);
        return Ok(padded);
    }

    Err(TransportError::IoError {
        detail: format!(
            "packet length {} exceeds interrupt endpoint packet size {}",
            data.len(),
            endpoint_packet_size
        ),
    })
}

fn normalize_incoming_packet(data: &[u8], endpoint_packet_size: usize) -> Vec<u8> {
    if data.len() == endpoint_packet_size && endpoint_packet_size + 1 == HID_REPORT_SIZE {
        let mut report = Vec::with_capacity(HID_REPORT_SIZE);
        report.push(0x00);
        report.extend_from_slice(data);
        return report;
    }

    data.to_vec()
}

fn lock_mutex<'a, T>(
    mutex: &'a Mutex<T>,
    label: &str,
) -> Result<std::sync::MutexGuard<'a, T>, TransportError> {
    mutex.lock().map_err(|_| TransportError::IoError {
        detail: format!("{label} poisoned"),
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
