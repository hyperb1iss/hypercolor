//! Composite USB MIDI + bulk transport used by Ableton Push 2-class devices.

use std::fmt::Write as _;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use alsa::{Direction, Rawmidi, seq::Seq};
use async_trait::async_trait;
use midir::{
    ConnectError, Ignore, InitError, MidiIO, MidiInput, MidiInputConnection, MidiOutput,
    MidiOutputConnection, SendError,
};
use nusb::transfer::{Buffer, Bulk, Out, TransferError};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tracing::{debug, trace, warn};

use crate::protocol::TransferType;
use crate::transport::{Transport, TransportError, spawn_blocking_transport_io};

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(1);
const PUSH2_MIDI_SHORT_PACKET_SPACING: Duration = Duration::from_micros(500);
const PUSH2_MIDI_SYSEX_PACKET_SPACING: Duration = Duration::from_millis(1);
#[cfg(target_os = "linux")]
const PUSH2_RAWMIDI_OPEN_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const PUSH2_RAWMIDI_OPEN_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const SYSEX_QUEUE_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    midi_out: Push2MidiOutput,
    midi_in: MidiInputConnection<()>,
}

enum Push2MidiOutput {
    Midir(MidiOutputConnection),
    #[cfg(target_os = "linux")]
    Raw(Rawmidi),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Push2MidiOutputPath {
    Sequencer,
    #[cfg(target_os = "linux")]
    RawMidi,
}

impl Push2MidiOutput {
    fn output_path(&self) -> Push2MidiOutputPath {
        match self {
            Self::Midir(_) => Push2MidiOutputPath::Sequencer,
            #[cfg(target_os = "linux")]
            Self::Raw(_) => Push2MidiOutputPath::RawMidi,
        }
    }

    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        match self {
            Self::Midir(midi_out) => midi_out.send(data).map_err(map_midi_send_error),
            #[cfg(target_os = "linux")]
            Self::Raw(rawmidi) => {
                let mut io = rawmidi.io();
                std::io::Write::write_all(&mut io, data).map_err(map_rawmidi_send_error)
            }
        }
    }
}

/// Composite transport that routes `Primary` traffic over MIDI and `Bulk`
/// traffic over a claimed USB bulk endpoint.
pub struct Push2Transport {
    _device: nusb::Device,
    _display_interface: nusb::Interface,
    bulk_endpoint_address: u8,
    bulk_endpoint: Arc<Mutex<nusb::Endpoint<Bulk, Out>>>,
    bulk_buffer: Arc<Mutex<Option<Buffer>>>,
    midi_out: Arc<Mutex<Push2MidiOutput>>,
    midi_next_send_at: AsyncMutex<Option<tokio::time::Instant>>,
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
        let serial_for_midi = serial.map(ToOwned::to_owned);
        let usb_path_for_midi = usb_path.map(ToOwned::to_owned);
        let midi_connections = spawn_blocking_transport_io("push2 midi open", move || {
            open_push2_midi_connections(
                expected_role,
                tx,
                vendor_id,
                product_id,
                serial_for_midi.as_deref(),
                usb_path_for_midi.as_deref(),
            )
        })
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
            midi_input = midi_connections.input_name,
            midi_output = midi_connections.output_name,
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
            midi_out: Arc::new(Mutex::new(midi_connections.midi_out)),
            midi_next_send_at: AsyncMutex::new(None),
            _midi_in: Mutex::new(Some(midi_connections.midi_in)),
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

        if self.midi_output_requires_pacing()? {
            self.pace_midi_send(data.len()).await;
        }

        let midi_out = Arc::clone(&self.midi_out);
        let packet = data.to_vec();
        spawn_blocking_transport_io("push2 midi send", move || {
            lock_mutex(midi_out.as_ref(), "MIDI output")?.send(packet.as_slice())
        })
        .await
    }

    fn midi_output_requires_pacing(&self) -> Result<bool, TransportError> {
        let midi_out = lock_mutex(self.midi_out.as_ref(), "MIDI output")?;
        Ok(midi_output_path_requires_pacing(midi_out.output_path()))
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

fn open_push2_midi_connections(
    expected_role: Push2MidiPortRole,
    tx: mpsc::Sender<Vec<u8>>,
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
                if message.first() == Some(&0xF0) {
                    let _ = tx.blocking_send(message.to_vec());
                }
            },
            (),
        )
        .map_err(|error| map_midi_connect_error(&error, "input"))?;
    let midi_out = open_push2_midi_output(midi_out, &output_port, &output_name)?;

    Ok(Push2MidiConnections {
        input_name,
        output_name,
        midi_out,
        midi_in,
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
        || Rawmidi::new(rawmidi_name, Direction::Playback, false),
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

fn midi_output_path_requires_pacing(path: Push2MidiOutputPath) -> bool {
    match path {
        Push2MidiOutputPath::Sequencer => true,
        #[cfg(target_os = "linux")]
        Push2MidiOutputPath::RawMidi => false,
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

#[doc(hidden)]
#[must_use]
pub fn midi_output_path_requires_pacing_for_testing(path: &str) -> Option<bool> {
    match path {
        "sequencer" => Some(midi_output_path_requires_pacing(
            Push2MidiOutputPath::Sequencer,
        )),
        #[cfg(target_os = "linux")]
        "rawmidi" => Some(midi_output_path_requires_pacing(
            Push2MidiOutputPath::RawMidi,
        )),
        _ => None,
    }
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
