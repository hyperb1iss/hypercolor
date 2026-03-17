//! Composite USB MIDI + bulk transport used by Ableton Push 2-class devices.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use midir::{
    ConnectError, Ignore, InitError, MidiIO, MidiInput, MidiInputConnection, MidiOutput,
    MidiOutputConnection, SendError,
};
use nusb::transfer::{Buffer, Bulk, Out, TransferError};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tracing::{debug, trace};

use crate::protocol::TransferType;
use crate::transport::{Transport, TransportError, spawn_blocking_transport_io};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(1_000);
const SYSEX_QUEUE_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Push2MidiPortRole {
    Live,
    User,
}

/// Composite transport that routes `Primary` traffic over MIDI and `Bulk`
/// traffic over a claimed USB bulk endpoint.
pub struct Push2Transport {
    _device: nusb::Device,
    _display_interface: nusb::Interface,
    bulk_endpoint_address: u8,
    bulk_endpoint: Arc<Mutex<nusb::Endpoint<Bulk, Out>>>,
    bulk_buffer: Arc<Mutex<Option<Buffer>>>,
    midi_out: AsyncMutex<MidiOutputConnection>,
    _midi_in: Mutex<Option<MidiInputConnection<()>>>,
    sysex_rx: AsyncMutex<mpsc::Receiver<Vec<u8>>>,
    closed: AtomicBool,
}

impl Push2Transport {
    /// Open the Push 2 transport, binding the MIDI user port and the display
    /// bulk endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the MIDI ports or bulk endpoint cannot
    /// be opened.
    #[expect(
        clippy::too_many_arguments,
        reason = "transport open needs both USB and MIDI identity plus endpoint metadata"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "setup requires sequential USB + MIDI negotiation"
    )]
    pub async fn new(
        device: nusb::Device,
        vendor_id: u16,
        product_id: u16,
        serial: Option<&str>,
        usb_path: Option<&str>,
        midi_interface: u8,
        display_interface: u8,
        display_endpoint: u8,
    ) -> Result<Self, TransportError> {
        let expected_role = match midi_interface {
            1 => Push2MidiPortRole::Live,
            _ => Push2MidiPortRole::User,
        };

        let (tx, rx) = mpsc::channel(SYSEX_QUEUE_DEPTH);
        let mut midi_in = MidiInput::new("hypercolor-push2-input").map_err(map_midi_init_error)?;
        midi_in.ignore(Ignore::None);
        let midi_out = MidiOutput::new("hypercolor-push2-output").map_err(map_midi_init_error)?;

        let input_port = find_push2_port(
            &midi_in,
            expected_role,
            "input",
            vendor_id,
            product_id,
            serial,
            usb_path,
        )?;
        let output_port = find_push2_port(
            &midi_out,
            expected_role,
            "output",
            vendor_id,
            product_id,
            serial,
            usb_path,
        )?;
        let input_name = midi_in
            .port_name(&input_port)
            .unwrap_or_else(|_| "<unknown>".to_owned());
        let output_name = midi_out
            .port_name(&output_port)
            .unwrap_or_else(|_| "<unknown>".to_owned());
        let midi_in_connection = midi_in
            .connect(
                &input_port,
                "hypercolor-push2-sysex",
                move |_timestamp, message, _state| {
                    if message.first() == Some(&0xF0) {
                        let _ = tx.blocking_send(message.to_vec());
                    }
                },
                (),
            )
            .map_err(|error| map_midi_connect_error(&error, "input"))?;
        let midi_out_connection = midi_out
            .connect(&output_port, "hypercolor-push2-output")
            .map_err(|error| map_midi_connect_error(&error, "output"))?;

        #[cfg(target_os = "linux")]
        let display_interface_handle = device
            .detach_and_claim_interface(display_interface)
            .await
            .map_err(|error| map_nusb_error(&error))?;

        #[cfg(not(target_os = "linux"))]
        let display_interface_handle = device
            .claim_interface(display_interface)
            .await
            .map_err(|error| map_nusb_error(&error))?;

        let descriptor =
            display_interface_handle
                .descriptor()
                .ok_or_else(|| TransportError::NotFound {
                    detail: format!(
                        "display interface {display_interface} has no active descriptor"
                    ),
                })?;
        let out_max_packet_size = descriptor
            .endpoints()
            .find(|endpoint| {
                endpoint.transfer_type() == nusb::descriptors::TransferType::Bulk
                    && endpoint.address() == display_endpoint
                    && endpoint.address() & 0x80 == 0
            })
            .map(|endpoint| endpoint.max_packet_size())
            .ok_or_else(|| TransportError::NotFound {
                detail: format!(
                    "bulk OUT endpoint 0x{display_endpoint:02X} not found on interface {display_interface}"
                ),
            })?;
        let bulk_endpoint = display_interface_handle
            .endpoint::<Bulk, Out>(display_endpoint)
            .map_err(|error| map_nusb_error(&error))?;

        debug!(
            vendor_id = format_args!("{vendor_id:04X}"),
            product_id = format_args!("{product_id:04X}"),
            serial = serial.unwrap_or("<none>"),
            usb_path = usb_path.unwrap_or("<unknown>"),
            midi_role = ?expected_role,
            midi_input = input_name,
            midi_output = output_name,
            display_interface,
            display_endpoint = format_args!("0x{display_endpoint:02X}"),
            out_max_packet_size,
            "opened Push 2 MIDI + bulk transport"
        );

        Ok(Self {
            _device: device,
            _display_interface: display_interface_handle,
            bulk_endpoint_address: display_endpoint,
            bulk_endpoint: Arc::new(Mutex::new(bulk_endpoint)),
            bulk_buffer: Arc::new(Mutex::new(Some(Buffer::new(out_max_packet_size)))),
            midi_out: AsyncMutex::new(midi_out_connection),
            _midi_in: Mutex::new(Some(midi_in_connection)),
            sysex_rx: AsyncMutex::new(rx),
            closed: AtomicBool::new(false),
        })
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }

        Ok(())
    }

    async fn send_midi(&self, data: &[u8]) -> Result<(), TransportError> {
        trace!(
            packet_len = data.len(),
            packet_hex = %format_hex_preview(data, 32),
            "push2 midi send"
        );

        self.midi_out
            .lock()
            .await
            .send(data)
            .map_err(map_midi_send_error)
    }

    async fn receive_sysex(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let mut rx = self.sysex_rx.lock().await;
        tokio::time::timeout(timeout, rx.recv())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .ok_or(TransportError::Closed)
    }
}

#[async_trait]
impl Transport for Push2Transport {
    fn name(&self) -> &'static str {
        "USB MIDI + Bulk"
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

        match transfer_type {
            TransferType::Primary => self.send_midi(data).await,
            TransferType::Bulk => {
                let endpoint = Arc::clone(&self.bulk_endpoint);
                let scratch = Arc::clone(&self.bulk_buffer);
                let endpoint_address = self.bulk_endpoint_address;
                let packet = data.to_vec();
                spawn_blocking_transport_io("push2 bulk send", move || {
                    send_bulk_locked(
                        endpoint.as_ref(),
                        scratch.as_ref(),
                        endpoint_address,
                        &packet,
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

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, TransferType::Primary).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        match transfer_type {
            TransferType::Primary => self.receive_sysex(timeout).await,
            TransferType::Bulk | TransferType::HidReport => {
                Err(TransportError::UnsupportedTransfer {
                    transport: self.name().to_owned(),
                    transfer_type,
                })
            }
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

        match transfer_type {
            TransferType::Primary => {
                self.send_midi(data).await?;
                self.receive_sysex(timeout).await
            }
            TransferType::Bulk | TransferType::HidReport => {
                Err(TransportError::UnsupportedTransfer {
                    transport: self.name().to_owned(),
                    transfer_type,
                })
            }
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn send_bulk_locked(
    endpoint: &Mutex<nusb::Endpoint<Bulk, Out>>,
    scratch: &Mutex<Option<Buffer>>,
    endpoint_address: u8,
    data: &[u8],
) -> Result<(), TransportError> {
    let mut endpoint = lock_mutex(endpoint, "bulk OUT endpoint")?;
    let mut scratch = lock_mutex(scratch, "bulk OUT scratch buffer")?;
    let mut buffer = scratch.take().unwrap_or_else(|| Buffer::new(data.len()));
    if buffer.capacity() < data.len() {
        buffer = Buffer::new(data.len());
    }
    buffer.clear();
    buffer.set_requested_len(data.len());
    buffer.extend_from_slice(data);

    trace!(
        endpoint = format_args!("0x{endpoint_address:02X}"),
        packet_len = data.len(),
        packet_hex = %format_hex_preview(data, 32),
        "push2 bulk send"
    );

    let completion = endpoint.transfer_blocking(buffer, DEFAULT_IO_TIMEOUT);
    let mut returned_buffer = completion.buffer;
    returned_buffer.clear();
    *scratch = Some(returned_buffer);

    completion
        .status
        .map_err(|error| map_transfer_error(error, DEFAULT_IO_TIMEOUT))
}

fn find_push2_port<T: MidiIO>(
    io: &T,
    expected_role: Push2MidiPortRole,
    direction: &str,
    vendor_id: u16,
    product_id: u16,
    serial: Option<&str>,
    usb_path: Option<&str>,
) -> Result<T::Port, TransportError> {
    let identity = format_device_identity(vendor_id, product_id, serial, usb_path);
    let matches = io
        .ports()
        .into_iter()
        .filter_map(|port| {
            let name = io.port_name(&port).ok()?;
            let role = classify_push2_port(&name)?;
            (role == expected_role).then_some((port, name))
        })
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [(port, _name)] => Ok(port.clone()),
        [] => Err(TransportError::NotFound {
            detail: format!(
                "no Push 2 {direction} MIDI port found for {identity} ({expected_role:?})"
            ),
        }),
        _ => Err(TransportError::NotFound {
            detail: format!(
                "multiple Push 2 {direction} MIDI ports matched for {identity} ({expected_role:?}): {}",
                matches
                    .iter()
                    .map(|(_port, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

fn classify_push2_port(name: &str) -> Option<Push2MidiPortRole> {
    let normalized = name.to_ascii_lowercase();
    if !normalized.contains("push 2") {
        return None;
    }

    if normalized.contains("user") {
        return Some(Push2MidiPortRole::User);
    }
    if normalized.contains("live") {
        return Some(Push2MidiPortRole::Live);
    }

    let (_, suffix) = normalized.rsplit_once(':')?;
    match suffix.trim().parse::<u8>().ok()? {
        0 => Some(Push2MidiPortRole::Live),
        1 => Some(Push2MidiPortRole::User),
        _ => None,
    }
}

fn format_device_identity(
    vendor_id: u16,
    product_id: u16,
    serial: Option<&str>,
    usb_path: Option<&str>,
) -> String {
    format!(
        "{vendor_id:04X}:{product_id:04X} serial={} usb_path={}",
        serial.unwrap_or("<none>"),
        usb_path.unwrap_or("<unknown>")
    )
}

fn lock_mutex<'a, T>(
    mutex: &'a Mutex<T>,
    name: &str,
) -> Result<std::sync::MutexGuard<'a, T>, TransportError> {
    mutex.lock().map_err(|_| TransportError::IoError {
        detail: format!("{name} mutex poisoned"),
    })
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
        let _ = write!(rendered, " ... (+{} bytes)", bytes.len() - preview_len);
    }

    if rendered.is_empty() {
        "<empty>".to_owned()
    } else {
        rendered
    }
}

fn map_midi_init_error(error: InitError) -> TransportError {
    TransportError::IoError {
        detail: error.to_string(),
    }
}

fn map_midi_connect_error<T>(error: &ConnectError<T>, direction: &str) -> TransportError {
    TransportError::IoError {
        detail: format!("failed to connect MIDI {direction} port: {error}"),
    }
}

fn map_midi_send_error(error: SendError) -> TransportError {
    TransportError::IoError {
        detail: error.to_string(),
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
        TransferError::Fault
        | TransferError::Stall
        | TransferError::InvalidArgument
        | TransferError::Unknown(_) => TransportError::IoError {
            detail: error.to_string(),
        },
    }
}
