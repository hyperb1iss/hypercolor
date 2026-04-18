//! USB HID interrupt transport used by PrismRGB-class devices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nusb::MaybeFuture;
use nusb::transfer::{
    Buffer, ControlIn, ControlOut, ControlType, In, Interrupt, Out, Recipient, TransferError,
};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, trace};

use crate::protocol::TransferType;
use crate::transport::{Transport, TransportError, spawn_blocking_transport_io};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);
const HID_REPORT_SIZE: usize = 65;
const HID_REPORT_TYPE_FEATURE: u16 = 0x03;

/// USB HID interrupt transport.
///
/// This path claims a HID interface directly and streams reports through its
/// interrupt IN/OUT endpoints.
pub struct UsbHidTransport {
    _device: nusb::Device,
    interface: Arc<nusb::Interface>,
    interface_number: u8,
    out_endpoint_address: u8,
    in_endpoint_address: u8,
    out_max_packet_size: usize,
    in_max_packet_size: usize,
    out_endpoint: Arc<Mutex<nusb::Endpoint<Interrupt, Out>>>,
    in_endpoint: Arc<Mutex<nusb::Endpoint<Interrupt, In>>>,
    closed: AtomicBool,
    op_lock: Arc<AsyncMutex<()>>,
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
            interface: Arc::new(interface),
            interface_number,
            out_endpoint_address: out_address,
            in_endpoint_address: in_address,
            out_max_packet_size,
            in_max_packet_size,
            out_endpoint: Arc::new(Mutex::new(out_handle)),
            in_endpoint: Arc::new(Mutex::new(in_handle)),
            closed: AtomicBool::new(false),
            op_lock: Arc::new(AsyncMutex::new(())),
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
impl Transport for UsbHidTransport {
    fn name(&self) -> &'static str {
        "USB HID Interrupt"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, TransferType::Primary).await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_receive_with_type(data, timeout, TransferType::Primary)
            .await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        match transfer_type {
            TransferType::Primary | TransferType::Bulk => {
                let endpoint = Arc::clone(&self.out_endpoint);
                let interface_number = self.interface_number;
                let endpoint_address = self.out_endpoint_address;
                let max_packet_size = self.out_max_packet_size;
                let packet = data.to_vec();
                spawn_blocking_transport_io("usb hid interrupt send", move || {
                    send_locked(
                        endpoint.as_ref(),
                        interface_number,
                        endpoint_address,
                        max_packet_size,
                        &packet,
                    )
                })
                .await
            }
            TransferType::HidReport => {
                let interface = Arc::clone(&self.interface);
                let interface_number = self.interface_number;
                let packet = data.to_vec();
                spawn_blocking_transport_io("usb hid feature report send", move || {
                    send_feature_report_locked(interface.as_ref(), interface_number, &packet)
                })
                .await
            }
        }
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        match transfer_type {
            TransferType::Primary | TransferType::Bulk => {
                let endpoint = Arc::clone(&self.in_endpoint);
                let interface_number = self.interface_number;
                let endpoint_address = self.in_endpoint_address;
                let max_packet_size = self.in_max_packet_size;
                spawn_blocking_transport_io("usb hid interrupt receive", move || {
                    receive_locked(
                        endpoint.as_ref(),
                        interface_number,
                        endpoint_address,
                        max_packet_size,
                        timeout,
                    )
                })
                .await
            }
            TransferType::HidReport => Err(TransportError::UnsupportedTransfer {
                transport: self.name().to_owned(),
                transfer_type,
            }),
        }
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        match transfer_type {
            TransferType::Primary | TransferType::Bulk => {
                let out_endpoint = Arc::clone(&self.out_endpoint);
                let in_endpoint = Arc::clone(&self.in_endpoint);
                let interface_number = self.interface_number;
                let out_endpoint_address = self.out_endpoint_address;
                let in_endpoint_address = self.in_endpoint_address;
                let out_max_packet_size = self.out_max_packet_size;
                let in_max_packet_size = self.in_max_packet_size;
                let packet = data.to_vec();
                spawn_blocking_transport_io("usb hid interrupt send_receive", move || {
                    send_locked(
                        out_endpoint.as_ref(),
                        interface_number,
                        out_endpoint_address,
                        out_max_packet_size,
                        &packet,
                    )?;
                    receive_locked(
                        in_endpoint.as_ref(),
                        interface_number,
                        in_endpoint_address,
                        in_max_packet_size,
                        timeout,
                    )
                })
                .await
            }
            TransferType::HidReport => {
                let Some(&report_id) = data.first() else {
                    return Err(TransportError::IoError {
                        detail: "feature report payload must include a report ID byte".to_owned(),
                    });
                };
                let interface = Arc::clone(&self.interface);
                let interface_number = self.interface_number;
                let report_len = data.len();
                let packet = data.to_vec();
                spawn_blocking_transport_io("usb hid feature report send_receive", move || {
                    send_feature_report_locked(interface.as_ref(), interface_number, &packet)?;
                    receive_feature_report_locked(
                        interface.as_ref(),
                        interface_number,
                        timeout,
                        report_id,
                        report_len,
                    )
                })
                .await
            }
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn send_locked(
    endpoint: &Mutex<nusb::Endpoint<Interrupt, Out>>,
    interface_number: u8,
    endpoint_address: u8,
    max_packet_size: usize,
    data: &[u8],
) -> Result<(), TransportError> {
    let packet = normalize_outgoing_packet(data, max_packet_size);
    let mut endpoint = lock_mutex(endpoint, "OUT endpoint")?;

    trace!(
        interface_number,
        endpoint = format_args!("0x{endpoint_address:02X}"),
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

fn receive_locked(
    endpoint: &Mutex<nusb::Endpoint<Interrupt, In>>,
    interface_number: u8,
    endpoint_address: u8,
    max_packet_size: usize,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    let mut endpoint = lock_mutex(endpoint, "IN endpoint")?;
    let response = endpoint
        .transfer_blocking(Buffer::new(max_packet_size), timeout)
        .into_result()
        .map_err(|error| map_transfer_error(error, timeout))?
        .into_vec();

    let normalized = normalize_incoming_packet(&response, max_packet_size);

    trace!(
        interface_number,
        endpoint = format_args!("0x{endpoint_address:02X}"),
        response_len = normalized.len(),
        response_hex = %format_hex_preview(&normalized, 32),
        "usb hid interrupt receive"
    );

    Ok(normalized)
}

fn send_feature_report_locked(
    interface: &nusb::Interface,
    interface_number: u8,
    data: &[u8],
) -> Result<(), TransportError> {
    let Some(&report_id) = data.first() else {
        return Err(TransportError::IoError {
            detail: "feature report payload must include a report ID byte".to_owned(),
        });
    };

    trace!(
        interface_number,
        report_id = format_args!("0x{report_id:02X}"),
        packet_len = data.len(),
        packet_hex = %format_hex_preview(data, 32),
        "usb hid feature report send"
    );

    interface
        .control_out(
            ControlOut {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0x09,
                value: feature_w_value(report_id),
                index: u16::from(interface_number),
                data,
            },
            DEFAULT_IO_TIMEOUT,
        )
        .wait()
        .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
}

fn receive_feature_report_locked(
    interface: &nusb::Interface,
    interface_number: u8,
    timeout: Duration,
    report_id: u8,
    report_len: usize,
) -> Result<Vec<u8>, TransportError> {
    let length = u16::try_from(report_len).map_err(|_| TransportError::IoError {
        detail: "feature report length exceeds u16".to_owned(),
    })?;
    let response = interface
        .control_in(
            ControlIn {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0x01,
                value: feature_w_value(report_id),
                index: u16::from(interface_number),
                length,
            },
            timeout,
        )
        .wait()
        .map_err(|error| map_transfer_error(error, timeout))?;

    trace!(
        interface_number,
        report_id = format_args!("0x{report_id:02X}"),
        response_len = response.len(),
        response_hex = %format_hex_preview(&response, 32),
        "usb hid feature report receive"
    );

    Ok(response)
}

fn normalize_outgoing_packet(data: &[u8], endpoint_packet_size: usize) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }

    if data.len() == endpoint_packet_size {
        return data.to_vec();
    }

    if data.len() == endpoint_packet_size + 1 && data[0] == 0 {
        return data[1..].to_vec();
    }

    if data.len() <= endpoint_packet_size {
        let mut padded = vec![0_u8; endpoint_packet_size];
        padded[..data.len()].copy_from_slice(data);
        return padded;
    }

    data.to_vec()
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

fn feature_w_value(report_id: u8) -> u16 {
    (HID_REPORT_TYPE_FEATURE << 8) | u16::from(report_id)
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
