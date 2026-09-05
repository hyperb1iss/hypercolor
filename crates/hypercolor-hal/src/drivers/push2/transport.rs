//! Ableton Push 2 native MIDI and bulk-display transport.

use std::collections::HashSet;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

#[cfg(target_os = "linux")]
use alsa::poll::Descriptors as _;
#[cfg(target_os = "linux")]
use alsa::{Direction, Rawmidi, seq::Seq};
use async_trait::async_trait;
use midir::{
    ConnectError, Ignore, InitError, MidiIO, MidiInput, MidiInputConnection, MidiOutput,
    MidiOutputConnection, SendError,
};
use nusb::transfer::{Buffer, Bulk, Out, TransferError};
use tokio::sync::Mutex as AsyncMutex;
#[cfg(target_os = "linux")]
use tracing::warn;
use tracing::{debug, trace};

use crate::protocol::TransferType;
use crate::registry::{UsbTransportFuture, UsbTransportOpenRequest};
use crate::transport::midi::{
    MidiResponseIngress, MidiResponseToken, MidiWorkerClient, NativeMidiSession, OpenedMidiSession,
    close_input_before_output, open_native_midi_worker,
};
use crate::transport::{
    Transport, TransportError, format_hex_preview, spawn_blocking_transport_io,
};

use super::devices::{PUSH2_DISPLAY_ENDPOINT, PUSH2_DISPLAY_INTERFACE, PUSH2_MIDI_INTERFACE};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);
const PUSH2_MIDI_SHORT_PACKET_SPACING: Duration = Duration::from_micros(500);
const PUSH2_MIDI_SYSEX_PACKET_SPACING: Duration = Duration::from_millis(1);
const PUSH2_SYSEX_RESPONSE_QUEUE_DEPTH: usize = 2;
const PUSH2_RESPONSE_NONE: u32 = 0;
const PUSH2_RESPONSE_ANY_SYSEX: u32 = 1;
const PUSH2_RESPONSE_IDENTITY: u32 = 2;
const PUSH2_RESPONSE_MANUFACTURER: u32 = 1 << 31;
const PUSH2_RESPONSE_HAS_ARG: u32 = 1 << 30;
const PUSH2_GET_PALETTE_ENTRY_COMMAND: u8 = 0x04;
const PUSH2_MANUFACTURER_PREFIX: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];
#[cfg(target_os = "linux")]
const PUSH2_RAWMIDI_OPEN_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const PUSH2_RAWMIDI_OPEN_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Open the driver-owned Push 2 composite transport.
#[must_use]
pub fn open_push2_transport(request: UsbTransportOpenRequest) -> UsbTransportFuture {
    Box::pin(async move {
        let transport = Push2Transport::new(
            request.device,
            request.vendor_id,
            request.product_id,
            request.serial.as_deref(),
            request.usb_path.as_deref(),
            PUSH2_MIDI_INTERFACE,
            PUSH2_DISPLAY_INTERFACE,
            PUSH2_DISPLAY_ENDPOINT,
        )
        .await?;
        Ok(Box::new(transport) as Box<dyn Transport>)
    })
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Push2MidiPortRole {
    Live,
    User,
}

trait Push2PortIdentity {
    fn push2_port_id(&self) -> String;
}

impl Push2PortIdentity for midir::MidiInputPort {
    fn push2_port_id(&self) -> String {
        self.id()
    }
}

impl Push2PortIdentity for midir::MidiOutputPort {
    fn push2_port_id(&self) -> String {
        self.id()
    }
}

#[derive(Clone)]
struct Push2PortMatch<P> {
    port: P,
    name: String,
    port_id: String,
    usb_path: Option<String>,
}

struct Push2MidiConnections {
    input_name: String,
    output_name: String,
    midi_out: Option<Push2MidiOutput>,
    midi_in: Option<MidiInputConnection<()>>,
}

enum Push2MidiOutput {
    Midir(MidiOutputConnection),
    #[cfg(target_os = "linux")]
    Raw(Rawmidi),
}

impl Push2MidiOutput {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        match self {
            Self::Midir(midi_out) => midi_out.send(data).map_err(map_midi_send_error),
            #[cfg(target_os = "linux")]
            Self::Raw(rawmidi) => write_rawmidi_with_deadline(rawmidi, data, DEFAULT_IO_TIMEOUT),
        }
    }

    fn close(self) {
        match self {
            Self::Midir(midi_out) => {
                let _ = midi_out.close();
            }
            #[cfg(target_os = "linux")]
            Self::Raw(_rawmidi) => {}
        }
    }
}

impl NativeMidiSession for Push2MidiConnections {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.midi_out
            .as_mut()
            .ok_or(TransportError::Closed)?
            .send(data)
    }

    fn close(&mut self) {
        close_input_before_output(
            &mut self.midi_in,
            &mut self.midi_out,
            |midi_in| {
                let _ = midi_in.close();
            },
            Push2MidiOutput::close,
        );
    }
}

impl Drop for Push2MidiConnections {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Push2MidiIdentity {
    role: Push2MidiPortRole,
    vendor_id: u16,
    product_id: u16,
    serial: Option<String>,
    usb_path: Option<String>,
}

impl Push2MidiIdentity {
    fn describe(&self) -> String {
        format!(
            "{:04X}:{:04X} role={:?} serial={} usb_path={}",
            self.vendor_id,
            self.product_id,
            self.role,
            self.serial.as_deref().unwrap_or("<none>"),
            self.usb_path.as_deref().unwrap_or("<unknown>")
        )
    }
}

struct Push2MidiLease {
    identity: Push2MidiIdentity,
}

impl Push2MidiLease {
    fn acquire(identity: Push2MidiIdentity) -> Result<Self, TransportError> {
        let mut active = push2_midi_leases()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !active.insert(identity.clone()) {
            return Err(TransportError::IoError {
                detail: format!(
                    "Push 2 MIDI lifecycle already active for {}",
                    identity.describe()
                ),
            });
        }
        Ok(Self { identity })
    }
}

impl Drop for Push2MidiLease {
    fn drop(&mut self) {
        push2_midi_leases()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.identity);
    }
}

fn push2_midi_leases() -> &'static Mutex<HashSet<Push2MidiIdentity>> {
    static ACTIVE: OnceLock<Mutex<HashSet<Push2MidiIdentity>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn push2_response_selector(request: &[u8]) -> u32 {
    if request == [0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7] {
        return PUSH2_RESPONSE_IDENTITY;
    }
    if request.first() != Some(&0xF0) {
        return PUSH2_RESPONSE_NONE;
    }
    if request.len() < 8 || request[..6] != PUSH2_MANUFACTURER_PREFIX {
        return PUSH2_RESPONSE_ANY_SYSEX;
    }

    let mut selector = PUSH2_RESPONSE_MANUFACTURER | u32::from(request[6]);
    if request[6] == PUSH2_GET_PALETTE_ENTRY_COMMAND && request.len() > 8 {
        selector |= PUSH2_RESPONSE_HAS_ARG | (u32::from(request[7]) << 8);
    }
    selector
}

fn push2_sysex_matches(token: MidiResponseToken, message: &[u8]) -> bool {
    let selector = token.get();
    if selector == PUSH2_RESPONSE_NONE || message.first() != Some(&0xF0) {
        return false;
    }
    if selector == PUSH2_RESPONSE_ANY_SYSEX {
        return true;
    }
    if selector == PUSH2_RESPONSE_IDENTITY {
        return message.len() >= 5 && message[1..5] == [0x7E, 0x01, 0x06, 0x02];
    }
    if selector & PUSH2_RESPONSE_MANUFACTURER == 0
        || message.len() < 8
        || message[..6] != PUSH2_MANUFACTURER_PREFIX
        || u32::from(message[6]) != (selector & 0xFF)
    {
        return false;
    }

    selector & PUSH2_RESPONSE_HAS_ARG == 0
        || message
            .get(7)
            .is_some_and(|arg| u32::from(*arg) == (selector >> 8) & 0xFF)
}

/// Composite transport that routes `Primary` traffic over MIDI and `Bulk`
/// traffic over a claimed USB bulk endpoint.
pub struct Push2Transport {
    _device: nusb::Device,
    _display_interface: nusb::Interface,
    bulk_endpoint_address: u8,
    bulk_endpoint: Arc<Mutex<nusb::Endpoint<Bulk, Out>>>,
    bulk_buffer: Arc<Mutex<Option<Buffer>>>,
    midi: MidiWorkerClient,
    midi_next_send_at: AsyncMutex<Option<tokio::time::Instant>>,
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

        let serial_for_midi = serial.map(ToOwned::to_owned);
        let usb_path_for_midi = usb_path.map(ToOwned::to_owned);
        let midi = open_push2_midi_worker(
            expected_role,
            vendor_id,
            product_id,
            serial_for_midi,
            usb_path_for_midi,
        )
        .await?;

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
            midi_input = midi.input_name(),
            midi_output = midi.output_name(),
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
            midi,
            midi_next_send_at: AsyncMutex::new(None),
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

        // Push 2 firmware wedges under unpaced MIDI bursts until power cycled,
        // so every output path gets inter-message spacing, raw MIDI included.
        self.pace_midi_send(data.len()).await;

        let packet = data.to_vec();
        let response_token = MidiResponseToken::new(push2_response_selector(&packet));
        self.midi.send(packet, response_token).await
    }

    async fn pace_midi_send(&self, packet_len: usize) {
        let spacing = midi_packet_spacing(packet_len);
        let mut next_send_at = self.midi_next_send_at.lock().await;
        let now = tokio::time::Instant::now();

        if let Some(deadline) = *next_send_at
            && deadline > now
        {
            tokio::time::sleep_until(deadline).await;
        }

        *next_send_at = Some(tokio::time::Instant::now() + spacing);
    }
}

#[async_trait]
impl Transport for Push2Transport {
    fn name(&self) -> &'static str {
        "USB MIDI + Bulk"
    }

    fn supports_parallel_transfer_lanes(&self) -> bool {
        true
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
            TransferType::HidReport | TransferType::Companion => {
                Err(TransportError::UnsupportedTransfer {
                    transport: self.name().to_owned(),
                    transfer_type,
                })
            }
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
            TransferType::Primary => {
                let fallback_token = MidiResponseToken::new(PUSH2_RESPONSE_ANY_SYSEX)
                    .expect("Push 2 catch-all response token should be nonzero");
                self.midi.receive(timeout, fallback_token).await
            }
            TransferType::Bulk | TransferType::HidReport | TransferType::Companion => {
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
                trace!(
                    packet_len = data.len(),
                    packet_hex = %format_hex_preview(data, 32),
                    "push2 midi send_receive"
                );
                self.pace_midi_send(data.len()).await;
                let selector = match push2_response_selector(data) {
                    PUSH2_RESPONSE_NONE => PUSH2_RESPONSE_ANY_SYSEX,
                    selector => selector,
                };
                let response_token = MidiResponseToken::new(selector)
                    .expect("Push 2 response selector should be nonzero");
                self.midi
                    .send_receive(data.to_vec(), response_token, timeout)
                    .await
            }
            TransferType::Bulk | TransferType::HidReport | TransferType::Companion => {
                Err(TransportError::UnsupportedTransfer {
                    transport: self.name().to_owned(),
                    transfer_type,
                })
            }
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.midi.close().await
    }
}

async fn open_push2_midi_worker(
    expected_role: Push2MidiPortRole,
    vendor_id: u16,
    product_id: u16,
    serial: Option<String>,
    usb_path: Option<String>,
) -> Result<MidiWorkerClient, TransportError> {
    let identity = Push2MidiIdentity {
        role: expected_role,
        vendor_id,
        product_id,
        serial,
        usb_path,
    };
    let lease = Push2MidiLease::acquire(identity.clone())?;
    let worker_context = format!("Push 2 MIDI worker for {}", identity.describe());
    let open_identity = identity;
    open_native_midi_worker(
        "hypercolor-push2-midi".to_owned(),
        worker_context,
        lease,
        PUSH2_SYSEX_RESPONSE_QUEUE_DEPTH,
        push2_sysex_matches,
        move |ingress| {
            let connections = open_push2_midi_connections(
                open_identity.role,
                ingress,
                open_identity.vendor_id,
                open_identity.product_id,
                open_identity.serial.as_deref(),
                open_identity.usb_path.as_deref(),
            )?;
            let input_name = connections.input_name.clone();
            let output_name = connections.output_name.clone();
            Ok(OpenedMidiSession::new(connections, input_name, output_name))
        },
    )
    .await
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

fn open_push2_midi_connections(
    expected_role: Push2MidiPortRole,
    ingress: MidiResponseIngress,
    vendor_id: u16,
    product_id: u16,
    serial: Option<&str>,
    usb_path: Option<&str>,
) -> Result<Push2MidiConnections, TransportError> {
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
    let midi_in = midi_in
        .connect(
            &input_port,
            "hypercolor-push2-sysex",
            move |_timestamp, message, _state| {
                ingress.forward(message);
            },
            (),
        )
        .map_err(|error| map_midi_connect_error(&error, "input"))?;
    let midi_out = open_push2_midi_output(midi_out, &output_port, &output_name)?;

    Ok(Push2MidiConnections {
        input_name,
        output_name,
        midi_out: Some(midi_out),
        midi_in: Some(midi_in),
    })
}

#[cfg(target_os = "linux")]
fn open_push2_midi_output(
    midi_out: MidiOutput,
    output_port: &midir::MidiOutputPort,
    output_name: &str,
) -> Result<Push2MidiOutput, TransportError> {
    let output_port_id = output_port.push2_port_id();
    if let Some(rawmidi_name) = rawmidi_name_from_seq_port_id(&output_port_id) {
        match open_push2_rawmidi_with_retry(&rawmidi_name) {
            Ok((rawmidi, attempts, elapsed)) => {
                debug!(
                    midi_output = output_name,
                    midi_port_id = output_port_id,
                    rawmidi = %rawmidi_name,
                    attempts,
                    wait_ms = elapsed.as_millis(),
                    "opened Push 2 raw MIDI output"
                );
                return Ok(Push2MidiOutput::Raw(rawmidi));
            }
            Err(error) => {
                warn!(
                    midi_output = output_name,
                    midi_port_id = output_port_id,
                    rawmidi = %rawmidi_name,
                    error = %error,
                    retry_timeout_ms = PUSH2_RAWMIDI_OPEN_RETRY_TIMEOUT.as_millis(),
                    "failed to open Push 2 raw MIDI output after retry; falling back to sequencer output"
                );
            }
        }
    }

    midi_out
        .connect(output_port, "hypercolor-push2-output")
        .map(Push2MidiOutput::Midir)
        .map_err(|error| map_midi_connect_error(&error, "output"))
}

#[cfg(target_os = "linux")]
fn open_push2_rawmidi_with_retry(
    rawmidi_name: &str,
) -> Result<(Rawmidi, u32, Duration), alsa::Error> {
    retry_rawmidi_open(
        || Rawmidi::new(rawmidi_name, Direction::Playback, true),
        std::thread::sleep,
        {
            let started_at = Instant::now();
            move || started_at.elapsed()
        },
        PUSH2_RAWMIDI_OPEN_RETRY_TIMEOUT,
        PUSH2_RAWMIDI_OPEN_RETRY_INTERVAL,
    )
}

#[cfg(target_os = "linux")]
fn retry_rawmidi_open<T, E>(
    mut open: impl FnMut() -> Result<T, E>,
    mut sleep: impl FnMut(Duration),
    mut elapsed: impl FnMut() -> Duration,
    timeout: Duration,
    retry_interval: Duration,
) -> Result<(T, u32, Duration), E> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match open() {
            Ok(rawmidi) => return Ok((rawmidi, attempts, elapsed())),
            Err(error) => {
                let waited = elapsed();
                if waited >= timeout {
                    return Err(error);
                }
                sleep(retry_interval.min(timeout.saturating_sub(waited)));
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn write_rawmidi_with_deadline(
    rawmidi: &Rawmidi,
    data: &[u8],
    timeout: Duration,
) -> Result<(), TransportError> {
    let started_at = Instant::now();
    let mut io = rawmidi.io();
    write_with_deadline(
        |chunk| std::io::Write::write(&mut io, chunk),
        |remaining| wait_rawmidi_writable(rawmidi, remaining),
        move || started_at.elapsed(),
        data,
        timeout,
    )
}

#[cfg(target_os = "linux")]
fn wait_rawmidi_writable(rawmidi: &Rawmidi, timeout: Duration) -> Result<bool, TransportError> {
    let mut fds = rawmidi.get().map_err(|error| TransportError::IoError {
        detail: format!("rawmidi poll descriptors unavailable: {error}"),
    })?;
    let timeout_ms = i32::try_from(timeout.as_millis())
        .unwrap_or(i32::MAX)
        .max(1);
    let ready =
        alsa::poll::poll(&mut fds, timeout_ms).map_err(|error| TransportError::IoError {
            detail: format!("rawmidi poll failed: {error}"),
        })?;
    Ok(ready > 0)
}

#[cfg(target_os = "linux")]
fn write_with_deadline(
    mut write: impl FnMut(&[u8]) -> std::io::Result<usize>,
    mut wait_writable: impl FnMut(Duration) -> Result<bool, TransportError>,
    mut elapsed: impl FnMut() -> Duration,
    data: &[u8],
    timeout: Duration,
) -> Result<(), TransportError> {
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let mut offset = 0;
    while offset < data.len() {
        match write(&data[offset..]) {
            Ok(0) => {
                return Err(TransportError::IoError {
                    detail: "raw MIDI write made no progress".to_owned(),
                });
            }
            Ok(written) => offset += written,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let waited = elapsed();
                if waited >= timeout || !wait_writable(timeout.saturating_sub(waited))? {
                    return Err(TransportError::Timeout { timeout_ms });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                if elapsed() >= timeout {
                    return Err(TransportError::Timeout { timeout_ms });
                }
            }
            Err(error) => return Err(map_rawmidi_send_error(error)),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn open_push2_midi_output(
    midi_out: MidiOutput,
    output_port: &midir::MidiOutputPort,
    _output_name: &str,
) -> Result<Push2MidiOutput, TransportError> {
    midi_out
        .connect(output_port, "hypercolor-push2-output")
        .map(Push2MidiOutput::Midir)
        .map_err(|error| map_midi_connect_error(&error, "output"))
}

fn find_push2_port<T: MidiIO>(
    io: &T,
    expected_role: Push2MidiPortRole,
    direction: &str,
    vendor_id: u16,
    product_id: u16,
    serial: Option<&str>,
    usb_path: Option<&str>,
) -> Result<T::Port, TransportError>
where
    T::Port: Push2PortIdentity,
{
    let identity = format_device_identity(vendor_id, product_id, serial, usb_path);
    let matches = io
        .ports()
        .into_iter()
        .filter_map(|port| {
            let name = io.port_name(&port).ok()?;
            let role = classify_push2_port(&name)?;
            if role != expected_role {
                return None;
            }

            let port_id = port.push2_port_id();
            Some(Push2PortMatch {
                usb_path: resolve_midi_port_usb_path(&port_id),
                port,
                name,
                port_id,
            })
        })
        .collect::<Vec<_>>();

    let matches = filter_push2_port_matches(matches, usb_path);
    match matches.as_slice() {
        [port] => Ok(port.port.clone()),
        [] => Err(TransportError::NotReady {
            detail: format!(
                "no Push 2 {direction} MIDI port found for {identity} ({expected_role:?})"
            ),
        }),
        _ => Err(TransportError::NotFound {
            detail: format!(
                "multiple Push 2 {direction} MIDI ports matched for {identity} ({expected_role:?}): {}",
                matches
                    .iter()
                    .map(describe_push2_port_match)
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

    if matches_windows_numbered_push2_user_port(&normalized) {
        return Some(Push2MidiPortRole::User);
    }
    if normalized.trim() == "ableton push 2" {
        return Some(Push2MidiPortRole::Live);
    }

    let (_, suffix) = normalized.rsplit_once(':')?;
    match suffix.trim().parse::<u8>().ok()? {
        0 => Some(Push2MidiPortRole::Live),
        1 => Some(Push2MidiPortRole::User),
        _ => None,
    }
}

fn matches_windows_numbered_push2_user_port(normalized: &str) -> bool {
    matches_windows_numbered_push2_port(normalized, "midiin2")
        || matches_windows_numbered_push2_port(normalized, "midiout2")
}

fn matches_windows_numbered_push2_port(normalized: &str, prefix: &str) -> bool {
    normalized
        .strip_prefix(prefix)
        .is_some_and(|suffix| suffix.starts_with(' ') || suffix.starts_with('('))
}

fn filter_push2_port_matches<P>(
    mut matches: Vec<Push2PortMatch<P>>,
    requested_usb_path: Option<&str>,
) -> Vec<Push2PortMatch<P>> {
    let Some(requested_usb_path) = requested_usb_path else {
        return matches;
    };

    let any_usb_paths = matches.iter().any(|candidate| candidate.usb_path.is_some());
    if any_usb_paths {
        matches.retain(|candidate| {
            candidate
                .usb_path
                .as_deref()
                .is_some_and(|candidate_path| usb_paths_match(candidate_path, requested_usb_path))
        });
    }

    matches
}

fn describe_push2_port_match<P>(candidate: &Push2PortMatch<P>) -> String {
    format!(
        "{}(id={}, usb_path={})",
        candidate.name,
        candidate.port_id,
        candidate.usb_path.as_deref().unwrap_or("<unknown>")
    )
}

#[cfg(target_os = "linux")]
fn resolve_midi_port_usb_path(port_id: &str) -> Option<String> {
    let (client, _port) = parse_seq_port_id(port_id)?;
    let seq = Seq::open(None, None, true).ok()?;
    let client_info = seq.get_any_client_info(client).ok()?;
    let card = client_info.get_card().ok()?;
    sound_card_usb_path(card)
}

#[cfg(not(target_os = "linux"))]
fn resolve_midi_port_usb_path(_port_id: &str) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn sound_card_usb_path(card: i32) -> Option<String> {
    let card_path = Path::new("/sys/class/sound").join(format!("card{card}"));
    let canonical = std::fs::canonicalize(card_path).ok()?;
    usb_path_from_sysfs_path(&canonical)
}

#[cfg(target_os = "linux")]
fn rawmidi_name_from_seq_port_id(port_id: &str) -> Option<String> {
    let (client, port) = parse_seq_port_id(port_id)?;
    let seq = Seq::open(None, None, true).ok()?;
    let client_info = seq.get_any_client_info(client).ok()?;
    let card = client_info.get_card().ok()?;
    rawmidi_name_from_sound_card_and_seq_port(card, port)
}

#[cfg(target_os = "linux")]
fn rawmidi_name_from_sound_card_and_seq_port(card: i32, seq_port: i32) -> Option<String> {
    if card < 0 || seq_port < 0 {
        return None;
    }

    Some(format!("hw:{card},0,{seq_port}"))
}

#[cfg(target_os = "linux")]
fn parse_seq_port_id(port_id: &str) -> Option<(i32, i32)> {
    let (client, port) = port_id.split_once(':')?;
    Some((client.parse().ok()?, port.parse().ok()?))
}

#[cfg(target_os = "linux")]
fn usb_path_from_sysfs_path(path: &Path) -> Option<String> {
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        let Some((usb_path, _interface_suffix)) = value.split_once(':') else {
            continue;
        };
        if usb_path.contains('-') {
            return Some(usb_path.to_owned());
        }
    }

    None
}

fn usb_paths_match(candidate: &str, requested: &str) -> bool {
    if candidate == requested {
        return true;
    }

    match (normalize_usb_path(candidate), normalize_usb_path(requested)) {
        (Some(candidate), Some(requested)) => candidate == requested,
        _ => false,
    }
}

fn normalize_usb_path(path: &str) -> Option<String> {
    let (bus, ports) = path.split_once('-')?;
    let bus = bus.parse::<u16>().ok()?;
    Some(format!("{bus}-{ports}"))
}

fn midi_packet_spacing(packet_len: usize) -> Duration {
    if packet_len <= 3 {
        PUSH2_MIDI_SHORT_PACKET_SPACING
    } else {
        PUSH2_MIDI_SYSEX_PACKET_SPACING
    }
}

#[doc(hidden)]
#[must_use]
pub fn classify_push2_port_for_testing(name: &str) -> Option<&'static str> {
    match classify_push2_port(name)? {
        Push2MidiPortRole::Live => Some("live"),
        Push2MidiPortRole::User => Some("user"),
    }
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
#[must_use]
pub fn midi_usb_path_from_sound_card_sysfs_for_testing(path: &str) -> Option<String> {
    usb_path_from_sysfs_path(Path::new(path))
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
#[must_use]
pub fn rawmidi_name_from_sound_card_and_seq_port_for_testing(
    card: i32,
    seq_port: i32,
) -> Option<String> {
    rawmidi_name_from_sound_card_and_seq_port(card, seq_port)
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
pub fn rawmidi_open_retry_for_testing(
    failures_before_success: usize,
    timeout: Duration,
    retry_interval: Duration,
) -> Result<(u32, Duration), String> {
    use std::cell::Cell;

    let attempts = Cell::new(0_u32);
    let elapsed = Cell::new(Duration::ZERO);
    retry_rawmidi_open(
        || {
            let next_attempt = attempts.get().saturating_add(1);
            attempts.set(next_attempt);
            if usize::try_from(next_attempt).unwrap_or(usize::MAX) > failures_before_success {
                Ok(())
            } else {
                Err("rawmidi not ready".to_owned())
            }
        },
        |delay| elapsed.set(elapsed.get().saturating_add(delay)),
        || elapsed.get(),
        timeout,
        retry_interval,
    )
    .map(|((), attempts, elapsed)| (attempts, elapsed))
}

#[doc(hidden)]
#[must_use]
pub fn midi_usb_paths_match_for_testing(candidate: &str, requested: &str) -> bool {
    usb_paths_match(candidate, requested)
}

#[doc(hidden)]
#[must_use]
pub fn midi_packet_spacing_for_testing(packet_len: usize) -> Duration {
    midi_packet_spacing(packet_len)
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
pub fn rawmidi_write_deadline_for_testing(
    write_results: &[Result<usize, std::io::ErrorKind>],
    poll_ready: bool,
    timeout: Duration,
    data_len: usize,
) -> Result<(), String> {
    use std::cell::Cell;

    let step = Cell::new(0_usize);
    let simulated_elapsed = Cell::new(Duration::ZERO);
    let data = vec![0_u8; data_len];
    write_with_deadline(
        |chunk| {
            let index = step.get().min(write_results.len().saturating_sub(1));
            step.set(step.get() + 1);
            match write_results[index] {
                Ok(written) => Ok(written.min(chunk.len())),
                Err(kind) => Err(std::io::Error::from(kind)),
            }
        },
        |_remaining| {
            simulated_elapsed.set(simulated_elapsed.get() + Duration::from_millis(100));
            Ok(poll_ready)
        },
        || simulated_elapsed.get(),
        &data,
        timeout,
    )
    .map_err(|error| match error {
        TransportError::Timeout { .. } => "timeout".to_owned(),
        other => other.to_string(),
    })
}

#[doc(hidden)]
pub fn select_push2_port_identity_for_testing(
    candidates: &[(&str, &str, Option<&str>)],
    expected_role: &str,
    requested_usb_path: Option<&str>,
) -> Result<String, String> {
    let expected_role = match expected_role {
        "live" => Push2MidiPortRole::Live,
        "user" => Push2MidiPortRole::User,
        other => return Err(format!("unknown expected role '{other}'")),
    };

    let matches = candidates
        .iter()
        .filter_map(|(name, port_id, usb_path)| {
            let role = classify_push2_port(name)?;
            if role != expected_role {
                return None;
            }

            Some(Push2PortMatch {
                port: (*port_id).to_owned(),
                name: (*name).to_owned(),
                port_id: (*port_id).to_owned(),
                usb_path: usb_path.map(ToOwned::to_owned),
            })
        })
        .collect::<Vec<_>>();
    let matches = filter_push2_port_matches(matches, requested_usb_path);

    match matches.as_slice() {
        [port] => Ok(port.port.clone()),
        [] => Err("no matching Push 2 test port".to_owned()),
        _ => Err(matches
            .iter()
            .map(describe_push2_port_match)
            .collect::<Vec<_>>()
            .join(", ")),
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

#[cfg(target_os = "linux")]
fn map_rawmidi_send_error(error: std::io::Error) -> TransportError {
    TransportError::IoError {
        detail: format!("raw MIDI write failed: {error}"),
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
        TransferError::Fault
        | TransferError::Stall
        | TransferError::InvalidArgument
        | TransferError::Unknown(_) => TransportError::IoError {
            detail: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity(serial: &str) -> Push2MidiIdentity {
        Push2MidiIdentity {
            role: Push2MidiPortRole::User,
            vendor_id: 0x2982,
            product_id: 0x1967,
            serial: Some(serial.to_owned()),
            usb_path: Some("test-path".to_owned()),
        }
    }

    #[test]
    fn midi_mode_selector_accepts_command_only_acknowledgement() {
        let selector =
            push2_response_selector(&[0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]);
        let command_only_reply = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0xF7];
        let token =
            MidiResponseToken::new(selector).expect("Push 2 MIDI mode selector should be nonzero");

        assert!(push2_sysex_matches(token, &command_only_reply));
    }

    #[test]
    fn midi_lifecycle_lease_blocks_duplicate_native_opens_until_release() {
        let identity = test_identity("lease-deduplication");
        let lease = Push2MidiLease::acquire(identity.clone()).expect("first lease is available");
        assert!(Push2MidiLease::acquire(identity.clone()).is_err());

        drop(lease);
        let reacquired = Push2MidiLease::acquire(identity).expect("released lease is reusable");
        drop(reacquired);
    }
}
