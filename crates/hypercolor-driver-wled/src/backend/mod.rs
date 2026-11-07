//! WLED backend — implements [`DeviceBackend`] for WLED LED controllers over UDP.
//!
//! This module is split across a few focused submodules:
//!
//! - [`protocol`] holds the wire-format send state machine ([`WledDevice`])
//!   plus per-protocol dedup, encoding, and `/json/cfg` realtime-receiver
//!   validation helpers.
//! - [`cache`] holds metadata parsed from `/json/info` and `/json/state`,
//!   fingerprinting, and translation into the generic [`DeviceInfo`].
//! - [`health`] holds the realtime-mode HTTP lifecycle (enter/exit/prime/
//!   clear/validate) and a cheap reachability probe.
//!
//! [`WledBackend`] stitches the three together and implements
//! [`DeviceBackend`].

mod cache;
mod health;
mod protocol;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::net::UdpSocket;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice,
    OutputCadence,
};
use hypercolor_driver_support::{DeviceSession, FrameDelivery, Session, SessionTable};
use hypercolor_types::device::{DeviceColorFormat, DeviceError, DeviceId, DeviceInfo};

use health::{
    REALTIME_HTTP_TIMEOUT, clear_device, enter_realtime_mode, exit_realtime_mode, prime_device,
    validate_wled_receiver_config,
};
use protocol::{KEEPALIVE_INTERVAL, encode_colors};

use hypercolor_pixel_protocols::ddp::{DDP_PORT, DdpSequence};
use hypercolor_pixel_protocols::e131::{
    E131_CHANNELS_PER_UNIVERSE, E131_PORT, E131SequenceTracker,
};

// ── Re-exports: preserve `backend::Foo` public paths ────────────────────

pub use cache::{WledDeviceInfo, WledSegmentInfo, parse_wled_info, parse_wled_segments};
pub use protocol::{
    WledColorFormat, WledDevice, WledLiveReceiverConfig, WledProtocol,
    parse_wled_live_receiver_config, wled_receiver_config_mismatches,
};

const DEFAULT_DEDUP_THRESHOLD: u8 = 2;
/// WLED's JSON API, unless discovery says otherwise.
const DEFAULT_API_PORT: u16 = 80;
const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Longest the clear frame may take, so a stuck one leaves the realtime exit
/// its whole budget.
const CLEAR_FRAME_BOUND: Duration = Duration::from_millis(500);

/// What a connect fixed about one session.
pub struct WledFacts {
    api_addr: SocketAddr,
    target_fps: u32,
    e131_start_universe: u16,
    local_addr: Option<SocketAddr>,
}

impl DeviceSession for WledDevice {
    type Facts = WledFacts;

    /// The first frame can be entering realtime mode over HTTP. Cancelling
    /// that would leave the device in live override with nobody to exit it.
    const SETTLE: Duration = REALTIME_HTTP_TIMEOUT;
    /// One HTTP exit from realtime mode, plus the clear frame before it.
    const CLEANUP: Duration = REALTIME_HTTP_TIMEOUT.saturating_add(CLEAR_FRAME_BOUND);

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> std::result::Result<DeviceWriteOutcome, DeviceError> {
        let device_id = self.device_id;
        self.ensure_ready_for_output()
            .await
            .map_err(|error| DeviceError::write(device_id, error))?;
        let expected_led_count = usize::from(self.led_count);

        if colors.len() != expected_led_count {
            let should_warn = self
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);

            if should_warn {
                warn!(
                    device_id = %device_id,
                    expected_led_count,
                    actual_led_count = colors.len(),
                    "WLED frame size mismatch; truncating or padding to match device"
                );
                self.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let wire_format = match self.protocol {
            WledProtocol::Ddp => self.ddp_wire_format(),
            WledProtocol::E131 => self.color_format,
        };
        let pixel_data = encode_colors(colors, wire_format, expected_led_count);

        self.send_frame_outcome_observed(&pixel_data, delivery)
            .await
            .map_err(|error| DeviceError::write(device_id, error))
    }

    /// Blank the strip and hand it back to WLED's own effects. Both steps
    /// are best effort: the session is ending either way.
    async fn finish(&mut self) -> std::result::Result<(), DeviceError> {
        if self.last_sent_pixels.is_some() {
            let cleared = tokio::time::timeout(CLEAR_FRAME_BOUND, clear_device(self)).await;
            if !matches!(cleared, Ok(Ok(()))) {
                debug!(
                    device_id = %self.device_id,
                    ip = %self.ip,
                    "best-effort WLED clear frame failed during disconnect"
                );
            }
        }
        if self.realtime_mode_active
            && let Err(error) = exit_realtime_mode(self.api_addr).await
        {
            debug!(
                device_id = %self.device_id,
                ip = %self.ip,
                error = %error,
                "best-effort exit from WLED realtime mode failed"
            );
        }
        Ok(())
    }
}

impl WledDevice {
    async fn ensure_ready_for_output(&mut self) -> Result<()> {
        if self.stream_initialized {
            return Ok(());
        }

        let id = self.device_id;
        if self.realtime_http_enabled {
            enter_realtime_mode(self.api_addr)
                .await
                .with_context(|| format!("Failed to enter realtime mode for WLED device {id}"))?;
            self.realtime_mode_active = true;
            validate_wled_receiver_config(
                self.api_addr,
                self.protocol,
                self.color_format,
                self.e131_start_universe,
            )
            .await;
            if let Err(error) = prime_device(self).await {
                if let Err(exit_error) = exit_realtime_mode(self.api_addr).await {
                    debug!(
                        device_id = %id,
                        ip = %self.ip,
                        error = %exit_error,
                        "best-effort exit from WLED realtime mode failed after priming error"
                    );
                }
                self.realtime_mode_active = false;
                return Err(error).with_context(|| format!("Failed to prime WLED device {id}"));
            }
        }

        self.stream_initialized = true;
        Ok(())
    }
}

// ── WledBackend ─────────────────────────────────────────────────────────

/// WLED device backend implementing [`DeviceBackend`].
///
/// Manages adopted devices and per-device UDP streaming over DDP or E1.31.
pub struct WledBackend {
    /// Open sessions, keyed by `DeviceId`. Each holds a reference to the
    /// shared socket, so closing one leaves the others sending.
    sessions: SessionTable<WledDevice>,

    /// Maps `DeviceId` to its JSON API address for lookup during connect.
    device_addrs: StdRwLock<HashMap<DeviceId, SocketAddr>>,

    /// Maps `DeviceId` to parsed info for lazy connect.
    device_infos: StdRwLock<HashMap<DeviceId, WledDeviceInfo>>,

    /// Default protocol for new connections.
    default_protocol: WledProtocol,

    /// Output state every session of this backend shares.
    shared: Arc<WledShared>,

    /// Whether connect/disconnect should manage WLED realtime mode over HTTP.
    realtime_http_enabled: bool,

    /// Global fuzzy deduplication threshold for connected devices.
    dedup_threshold: u8,

    /// E1.31 sender CID (stable UUID per backend instance).
    e131_cid: uuid::Uuid,
}

/// Output state every session of one backend shares.
struct WledShared {
    /// UDP socket used by all connected WLED devices.
    socket: OnceCell<Arc<UdpSocket>>,

    /// Next available E1.31 universe number for auto-allocation.
    next_e131_universe: StdMutex<u16>,
}

impl WledShared {
    async fn ensure_socket(&self) -> Result<Arc<UdpSocket>> {
        self.socket
            .get_or_try_init(|| async {
                UdpSocket::bind("0.0.0.0:0")
                    .await
                    .map(Arc::new)
                    .context("Failed to bind shared WLED UDP socket")
            })
            .await
            .map(Arc::clone)
    }

    fn allocate_e131_start_universe(
        &self,
        color_format: WledColorFormat,
        led_count: u16,
        protocol: WledProtocol,
    ) -> u16 {
        if protocol != WledProtocol::E131 {
            return 1;
        }

        let pixels_per_universe = E131_CHANNELS_PER_UNIVERSE / color_format.bytes_per_pixel();
        let universes_needed = usize::from(led_count)
            .div_ceil(pixels_per_universe)
            .clamp(1, usize::from(u16::MAX));
        let universes_needed = u16::try_from(universes_needed).unwrap_or(u16::MAX);
        let mut next_universe = self
            .next_e131_universe
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let start_universe = *next_universe;
        *next_universe = next_universe.saturating_add(universes_needed.max(1));
        start_universe
    }
}

impl WledBackend {
    /// Create a new WLED backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: SessionTable::new(),
            device_addrs: StdRwLock::new(HashMap::new()),
            device_infos: StdRwLock::new(HashMap::new()),
            default_protocol: WledProtocol::default(),
            shared: Arc::new(WledShared {
                socket: OnceCell::new(),
                next_e131_universe: StdMutex::new(1),
            }),
            realtime_http_enabled: true,
            dedup_threshold: DEFAULT_DEDUP_THRESHOLD,
            e131_cid: uuid::Uuid::now_v7(),
        }
    }

    /// Set the default protocol for new connections.
    pub fn set_protocol(&mut self, protocol: WledProtocol) {
        self.default_protocol = protocol;
    }

    /// Enable or disable HTTP realtime-mode lifecycle calls.
    pub fn set_realtime_http_enabled(&mut self, enabled: bool) {
        self.realtime_http_enabled = enabled;
    }

    /// Set the global fuzzy dedup threshold for newly connected devices.
    pub fn set_dedup_threshold(&mut self, threshold: u8) {
        self.dedup_threshold = threshold;
    }

    /// The local address of the shared UDP socket, if initialized.
    #[must_use]
    pub fn shared_socket_local_addr(&self) -> Option<SocketAddr> {
        self.shared
            .socket
            .get()
            .and_then(|socket| socket.local_addr().ok())
    }

    /// The local UDP socket used by a connected device, if available.
    #[must_use]
    pub fn connected_socket_local_addr(&self, id: &DeviceId) -> Option<SocketAddr> {
        self.sessions.facts(id).and_then(|facts| facts.local_addr)
    }

    /// The starting E1.31 universe assigned to a connected device.
    #[must_use]
    pub fn connected_e131_start_universe(&self, id: &DeviceId) -> Option<u16> {
        self.sessions
            .facts(id)
            .map(|facts| facts.e131_start_universe)
    }
}

impl Default for WledBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DeviceBackend for WledBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "wled".to_owned(),
            name: "WLED (DDP/E1.31)".to_owned(),
            description: "Network-attached WLED LED controllers over UDP".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let Some(ip) = discovered
            .metadata
            .get("ip")
            .and_then(|ip| ip.parse::<IpAddr>().ok())
        else {
            return Err(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            });
        };
        let rgbw = discovered
            .info
            .segments
            .first()
            .is_some_and(|segment| matches!(segment.color_format, DeviceColorFormat::Rgbw));
        let mac = discovered.metadata.get("mac").cloned().unwrap_or_default();
        let info = WledDeviceInfo {
            firmware_version: discovered
                .info
                .firmware_version
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            build_id: 0,
            mac,
            name: discovered.info.name.clone(),
            led_count: u16::try_from(discovered.info.total_led_count()).unwrap_or(u16::MAX),
            rgbw,
            max_segments: 1,
            fps: u8::try_from(discovered.info.capabilities.max_fps).unwrap_or(u8::MAX),
            power_draw_ma: 0,
            max_power_ma: 0,
            free_heap: 0,
            uptime_secs: 0,
            arch: discovered
                .metadata
                .get("arch")
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            is_wifi: true,
            effect_count: 0,
            palette_count: 0,
        };
        let api_port = discovered
            .metadata
            .get("api_port")
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(DEFAULT_API_PORT);
        self.device_addrs
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, SocketAddr::new(ip, api_port));
        self.device_infos
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, info);
        Ok(())
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let api_addr = {
            let device_addrs = self
                .device_addrs
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            device_addrs
                .get(id)
                .copied()
                .ok_or(DeviceError::NotAdopted { device_id: *id })?
        };
        let ip = api_addr.ip();
        let id = *id;
        // A device that moved closes first, so its realtime exit cannot land
        // on the session replacing it.
        self.sessions
            .open_unless(
                &id,
                |open| open.api_addr == api_addr,
                || {
                    let wled_info = self
                        .device_infos
                        .read()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&id)
                        .cloned();
                    let shared = Arc::clone(&self.shared);
                    let protocol = self.default_protocol;
                    let e131_cid = self.e131_cid;
                    let dedup_threshold = self.dedup_threshold;
                    let realtime_http_enabled = self.realtime_http_enabled;
                    async move {
                        let wled_info =
                            wled_info.ok_or(DeviceError::NotAdopted { device_id: id })?;
                        let socket = shared
                            .ensure_socket()
                            .await
                            .map_err(|error| DeviceError::connection(id, error))?;

                        let port = match protocol {
                            WledProtocol::Ddp => DDP_PORT,
                            WledProtocol::E131 => E131_PORT,
                        };
                        let pixel_addr = SocketAddr::new(ip, port);

                        let color_format = if wled_info.rgbw {
                            WledColorFormat::Rgbw
                        } else {
                            WledColorFormat::Rgb
                        };
                        let e131_start_universe = shared.allocate_e131_start_universe(
                            color_format,
                            wled_info.led_count,
                            protocol,
                        );

                        let device = WledDevice {
                            device_id: id,
                            ip,
                            api_addr,
                            protocol,
                            pixel_addr,
                            color_format,
                            led_count: wled_info.led_count,
                            info: wled_info,
                            socket,
                            ddp_sequence: DdpSequence::default(),
                            e131_sequences: E131SequenceTracker::default(),
                            e131_cid,
                            e131_start_universe,
                            dedup_threshold,
                            last_sent_pixels: None,
                            consecutive_failures: 0,
                            last_success_at: None,
                            last_size_mismatch_warn_at: None,
                            realtime_mode_active: false,
                            stream_initialized: false,
                            realtime_http_enabled,
                            frames_sent: 0,
                            last_frame_at: None,
                        };

                        info!(
                            device_id = %id,
                            ip = %ip,
                            protocol = ?protocol,
                            leds = device.led_count,
                            start_universe = device.e131_start_universe,
                            "Connected to WLED device"
                        );

                        let facts = WledFacts {
                            api_addr,
                            target_fps: device.info.negotiated_target_fps(),
                            e131_start_universe: device.e131_start_universe,
                            local_addr: device.socket.local_addr().ok(),
                        };
                        Ok(Session::new(id, facts, device))
                    }
                },
            )
            .await
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected from WLED device");
        Ok(())
    }

    async fn retire(&self) -> std::result::Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await.map(|_| ())
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.sessions
            .facts(id)
            .map(|facts| facts.target_fps)
            .or_else(|| {
                self.device_infos
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(id)
                    .map(WledDeviceInfo::negotiated_target_fps)
            })
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.target_fps(id).map(|target_fps| {
            OutputCadence::from_fps(target_fps).with_max_frame_silence(KEEPALIVE_INTERVAL)
        })
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}
