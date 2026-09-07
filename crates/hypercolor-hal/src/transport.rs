//! Transport resolution and async byte-level I/O.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;

use crate::protocol::TransferType;
use crate::registry::{HidRawReportMode, TransportType};

/// Maximum quiet period between packets of one logical multi-packet reply.
///
/// Distinct from the response timeout, which bounds the wait for the first
/// packet. A reply whose length is an exact multiple of the endpoint packet
/// size has no short packet to terminate it, so it costs one gap timeout.
pub const DEFAULT_PACKET_GAP_TIMEOUT: Duration = Duration::from_millis(20);

pub mod bulk;
pub mod companion;
pub mod control;
pub mod hid;
pub mod hidapi;
#[cfg(target_os = "linux")]
pub mod hidraw;
pub mod midi;
pub mod serial;
pub mod smbus;
pub mod vendor;

/// Operating system used to resolve platform-free transport intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPlatform {
    /// Linux host.
    Linux,
    /// macOS host.
    MacOs,
    /// Windows host.
    Windows,
    /// A host without a supported transport backend.
    Other(&'static str),
}

impl TransportPlatform {
    /// Platform targeted by the current build.
    pub const CURRENT: Self = current_transport_platform();
}

impl fmt::Display for TransportPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Linux => f.write_str("Linux"),
            Self::MacOs => f.write_str("macOS"),
            Self::Windows => f.write_str("Windows"),
            Self::Other(name) => f.write_str(name),
        }
    }
}

const fn current_transport_platform() -> TransportPlatform {
    if cfg!(target_os = "linux") {
        TransportPlatform::Linux
    } else if cfg!(target_os = "macos") {
        TransportPlatform::MacOs
    } else if cfg!(target_os = "windows") {
        TransportPlatform::Windows
    } else {
        TransportPlatform::Other(std::env::consts::OS)
    }
}

/// How a HID device expects the host HID driver to be managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidAccessMode {
    /// Keep the host HID driver attached while sending reports.
    HostManaged,
    /// Claim the HID interface directly where the operating system allows it.
    Direct,
}

/// Platform-free HID requirements declared by a device descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidTransportIntent {
    /// Whether the host HID driver must remain attached.
    pub access: HidAccessMode,
    /// HID interface number.
    pub interface: u8,
    /// HID report ID.
    pub report_id: u8,
    /// Feature-report or output-report I/O mode.
    pub report_mode: HidRawReportMode,
    /// Full HID report buffer length, including a report ID prefix when used.
    pub max_report_len: usize,
    /// Optional HID usage page filter.
    pub usage_page: Option<u16>,
    /// Optional HID usage filter.
    pub usage: Option<u16>,
}

/// Platform-free transport requirements declared by a protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportIntent {
    /// HID report transport.
    Hid(HidTransportIntent),
    /// Local I2C/SMBus transport.
    I2cSmBus {
        /// 7-bit SMBus slave address.
        address: u16,
    },
}

/// Resolve platform-free transport requirements for an operating system.
///
/// # Errors
///
/// Returns [`TransportError::UnsupportedPlatform`] when the target has no
/// implementation for the requested transport.
pub const fn resolve_transport(
    intent: TransportIntent,
    platform: TransportPlatform,
) -> Result<TransportType, TransportError> {
    match intent {
        TransportIntent::Hid(intent) => resolve_hid_transport(intent, platform),
        TransportIntent::I2cSmBus { address } => match platform {
            TransportPlatform::Linux | TransportPlatform::Windows => {
                Ok(TransportType::I2cSmBus { address })
            }
            TransportPlatform::MacOs | TransportPlatform::Other(_) => {
                Err(TransportError::UnsupportedPlatform {
                    transport: "SMBus",
                    platform,
                })
            }
        },
    }
}

/// Device identity and HID collection filters for opening a host-managed
/// HID (hidraw) transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidRawOpenRequest {
    /// USB vendor identifier.
    pub vendor_id: u16,
    /// USB product identifier.
    pub product_id: u16,
    /// HID interface number.
    pub interface: u8,
    /// HID report ID.
    pub report_id: u8,
    /// Feature-report or output-report I/O mode.
    pub report_mode: HidRawReportMode,
    /// Optional serial number filter.
    pub serial: Option<String>,
    /// Optional USB topology path filter.
    pub usb_path: Option<String>,
    /// Optional HID usage page filter.
    pub usage_page: Option<u16>,
    /// Optional HID usage filter.
    pub usage: Option<u16>,
}

/// Open a host-managed HID transport that keeps the kernel HID driver
/// attached.
///
/// Linux resolves the request to a `/dev/hidraw*` node. Every other target
/// has no host-managed HID backend and reports
/// [`TransportError::UnsupportedPlatform`], so neutral callers dispatch here
/// without branching on the operating system.
///
/// # Errors
///
/// Returns [`TransportError`] when no matching device node exists, when the
/// node cannot be opened, or when the current target has no backend.
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        clippy::unused_async,
        reason = "the signature is the cross-platform contract; only Linux awaits"
    )
)]
pub async fn open_hid_raw_transport(
    request: HidRawOpenRequest,
) -> Result<Box<dyn Transport>, TransportError> {
    #[cfg(target_os = "linux")]
    {
        let transport = hidraw::UsbHidRawTransport::open(
            request.vendor_id,
            request.product_id,
            request.interface,
            request.report_id,
            request.report_mode,
            request.serial.as_deref(),
            request.usb_path.as_deref(),
            request.usage_page,
            request.usage,
        )
        .await?;
        Ok(Box::new(transport))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = request;
        Err(TransportError::UnsupportedPlatform {
            transport: "hidraw",
            platform: TransportPlatform::CURRENT,
        })
    }
}

/// Resolve transport requirements for the current build target.
///
/// # Errors
///
/// Returns [`TransportError::UnsupportedPlatform`] when the current target has
/// no implementation for the requested transport.
pub const fn resolve_current_transport(
    intent: TransportIntent,
) -> Result<TransportType, TransportError> {
    resolve_transport(intent, TransportPlatform::CURRENT)
}

const fn resolve_hid_transport(
    intent: HidTransportIntent,
    platform: TransportPlatform,
) -> Result<TransportType, TransportError> {
    match platform {
        TransportPlatform::Linux => match intent.access {
            HidAccessMode::HostManaged => Ok(TransportType::UsbHidRaw {
                interface: intent.interface,
                report_id: intent.report_id,
                report_mode: intent.report_mode,
                usage_page: intent.usage_page,
                usage: intent.usage,
            }),
            HidAccessMode::Direct => Ok(TransportType::UsbHid {
                interface: intent.interface,
            }),
        },
        TransportPlatform::MacOs | TransportPlatform::Windows => Ok(TransportType::UsbHidApi {
            interface: Some(intent.interface),
            report_id: intent.report_id,
            report_mode: intent.report_mode,
            max_report_len: intent.max_report_len,
            usage_page: intent.usage_page,
            usage: intent.usage,
        }),
        TransportPlatform::Other(_) => Err(TransportError::UnsupportedPlatform {
            transport: "HID",
            platform,
        }),
    }
}

/// Render a byte slice as spaced uppercase hex for trace logging, truncating
/// past `max_bytes`.
pub(crate) fn format_hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    use std::fmt::Write as _;

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

pub(crate) async fn spawn_blocking_transport_io<F, T>(
    operation_name: &'static str,
    operation: F,
) -> Result<T, TransportError>
where
    F: FnOnce() -> Result<T, TransportError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("{operation_name} task failed: {error}"),
        })?
}

/// Assemble one logical reply from a packet source.
///
/// `read_packet` is called with the timeout budget for that packet: the first
/// packet gets `first_timeout`, every later one gets `gap_timeout`. The reply
/// completes on the first of three conditions, in the order they are checked
/// per packet: a short packet (shorter than `max_packet_size`), `capacity`
/// bytes accumulated, or a timeout once at least one packet has arrived.
///
/// The gap case is a normal completion. A reply whose length is an exact
/// multiple of the packet size sends no short packet, so it can only end by
/// going quiet, and that costs exactly one gap timeout.
///
/// # Errors
///
/// Propagates the packet source's error when it fails before any bytes
/// arrive, and any non-timeout error mid-reply.
pub fn accumulate_logical_reply<F>(
    max_packet_size: usize,
    capacity: usize,
    first_timeout: Duration,
    gap_timeout: Duration,
    mut read_packet: F,
) -> Result<Vec<u8>, TransportError>
where
    F: FnMut(Duration) -> Result<Vec<u8>, TransportError>,
{
    if capacity == 0 || max_packet_size == 0 {
        return Ok(Vec::new());
    }

    let mut accumulated: Vec<u8> = Vec::with_capacity(capacity.min(MAX_LOGICAL_PREALLOC));
    let mut packet_timeout = first_timeout;

    while accumulated.len() < capacity {
        let packet = match read_packet(packet_timeout) {
            Ok(packet) => packet,
            Err(error) => {
                // A quiet endpoint ends a reply already in hand; before that,
                // the caller asked for a reply and got none.
                if accumulated.is_empty() || !matches!(error, TransportError::Timeout { .. }) {
                    return Err(error);
                }
                break;
            }
        };

        let is_short = packet.len() < max_packet_size;
        accumulated.extend_from_slice(&packet);
        if is_short {
            break;
        }
        packet_timeout = gap_timeout;
    }

    accumulated.truncate(capacity);
    Ok(accumulated)
}

/// Upper bound on the buffer reserved up front for a logical reply, so a
/// large declared capacity cannot turn one read into a large allocation.
const MAX_LOGICAL_PREALLOC: usize = 8 * 1024;

/// Async byte-level I/O transport.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Human-readable transport name.
    fn name(&self) -> &'static str;

    /// Whether independent transfer paths can be driven concurrently.
    fn supports_parallel_transfer_lanes(&self) -> bool {
        false
    }

    /// Send raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when I/O fails.
    async fn send(&self, data: &[u8]) -> Result<(), TransportError>;

    /// Send raw bytes over a specific transport path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the requested transfer type is not
    /// supported or I/O fails.
    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        if transfer_type != TransferType::Primary {
            return Err(TransportError::UnsupportedTransfer {
                transport: self.name().to_owned(),
                transfer_type,
            });
        }

        self.send(data).await
    }

    /// Send owned bytes over a specific transport path.
    ///
    /// Implementations can override this to move packet ownership into the
    /// transport layer without cloning.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the requested transfer type is not
    /// supported or I/O fails.
    async fn send_owned_with_type(
        &self,
        data: Vec<u8>,
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        self.send_with_type(&data, transfer_type).await
    }

    /// Receive raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when I/O fails.
    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError>;

    /// Receive raw bytes over a specific transport path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the requested transfer type is not
    /// supported or I/O fails.
    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        if transfer_type != TransferType::Primary {
            return Err(TransportError::UnsupportedTransfer {
                transport: self.name().to_owned(),
                transfer_type,
            });
        }

        self.receive(timeout).await
    }

    /// Send then receive in one helper operation.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when send/receive fails.
    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        self.receive(timeout).await
    }

    /// Send then receive over a specific transport path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the requested transfer type is not
    /// supported or I/O fails.
    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        if transfer_type != TransferType::Primary {
            return Err(TransportError::UnsupportedTransfer {
                transport: self.name().to_owned(),
                transfer_type,
            });
        }

        self.send_receive(data, timeout).await
    }

    /// Receive one logical reply, accumulating up to `capacity` bytes.
    ///
    /// `capacity` is an upper bound, not an expected length. `None` means one
    /// transport-default read, which is what every transport did before this
    /// seam existed, so the default implementation ignores the bound.
    ///
    /// A transport whose reply can span several packets overrides this and
    /// completes the logical read on the first of: a short packet, the
    /// capacity being reached, or an inter-packet gap of
    /// [`DEFAULT_PACKET_GAP_TIMEOUT`]. A gap-terminated read returns the bytes
    /// accumulated so far as a normal reply, since a reply whose length is an
    /// exact multiple of the packet size has no short packet to end it.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the requested transfer type is not
    /// supported or I/O fails before any bytes arrive.
    async fn receive_logical(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
        _capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, transfer_type).await
    }

    /// Send then receive one logical reply over a specific transport path.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the requested transfer type is not
    /// supported or I/O fails.
    async fn send_receive_logical(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
        _capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_receive_with_type(data, timeout, transfer_type)
            .await
    }

    /// Close transport and release resources.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when close fails.
    async fn close(&self) -> Result<(), TransportError>;
}

/// Transport-level errors.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Device could not be found.
    #[error("device not found: {detail}")]
    NotFound {
        /// Human-readable detail.
        detail: String,
    },

    /// A transport endpoint exists conceptually but is not ready yet.
    #[error("transport endpoint not ready: {detail}")]
    NotReady {
        /// Human-readable detail.
        detail: String,
    },

    /// Generic I/O failure.
    #[error("USB I/O error: {detail}")]
    IoError {
        /// Human-readable detail.
        detail: String,
    },

    /// I/O timeout.
    #[error("transport timeout after {timeout_ms}ms")]
    Timeout {
        /// Timeout budget used for the operation.
        timeout_ms: u64,
    },

    /// The remote device disconnected while the transport was active.
    #[error("transport disconnected: {detail}")]
    Disconnected {
        /// Transport-specific disconnect detail.
        detail: String,
    },

    /// Transport already closed.
    #[error("transport closed")]
    Closed,

    /// Access denied by OS policy or udev rules.
    #[error("permission denied: {detail}")]
    PermissionDenied {
        /// Human-readable detail.
        detail: String,
    },

    /// Requested transport has no backend on this operating system.
    #[error("{transport} transport is not supported on {platform}")]
    UnsupportedPlatform {
        /// Human-readable transport name.
        transport: &'static str,
        /// Operating system without a transport backend.
        platform: TransportPlatform,
    },

    /// Requested transfer path is not implemented by this transport.
    #[error("transport '{transport}' does not support {transfer_type:?} transfers")]
    UnsupportedTransfer {
        /// Human-readable transport name.
        transport: String,
        /// Unsupported transfer type.
        transfer_type: TransferType,
    },
}
