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
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::device::discovery::TransportScanner;
use crate::device::traits::{BackendInfo, DeviceBackend};
use crate::types::device::{DeviceId, DeviceInfo};

use cache::{build_device_info, wled_fingerprint};
use health::{
    clear_device, enter_realtime_mode, exit_realtime_mode, prime_device, probe_device_reachable,
    validate_wled_receiver_config,
};
use protocol::encode_colors;

use super::ddp::{DDP_PORT, DdpSequence};
use super::e131::{E131_CHANNELS_PER_UNIVERSE, E131_PORT, E131SequenceTracker};

// ── Re-exports: preserve `backend::Foo` public paths ────────────────────

pub use cache::{WledDeviceInfo, WledSegmentInfo, parse_wled_info, parse_wled_segments};
pub use protocol::{
    WledColorFormat, WledDevice, WledLiveReceiverConfig, WledProtocol,
    parse_wled_live_receiver_config, wled_receiver_config_mismatches,
};

const DEFAULT_DEDUP_THRESHOLD: u8 = 2;
const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_secs(60);

// ── WledBackend ─────────────────────────────────────────────────────────

/// WLED device backend implementing [`DeviceBackend`].
///
/// Manages discovery via HTTP probing and per-device UDP streaming
/// over DDP or E1.31.
pub struct WledBackend {
    /// Known device IPs for HTTP-based discovery (no mDNS required).
    known_ips: Vec<IpAddr>,

    /// Whether to run an mDNS fallback scan when no known IPs are configured.
    mdns_fallback: bool,

    /// Connected devices, keyed by `DeviceId`.
    devices: HashMap<DeviceId, WledDevice>,

    /// Maps `DeviceId` to IP for lookup during connect.
    device_ips: HashMap<DeviceId, IpAddr>,

    /// Maps `DeviceId` to parsed info for lazy connect.
    device_infos: HashMap<DeviceId, WledDeviceInfo>,

    /// Default protocol for new connections.
    default_protocol: WledProtocol,

    /// Shared UDP socket used by all connected WLED devices.
    shared_socket: Option<Arc<UdpSocket>>,

    /// Whether connect/disconnect should manage WLED realtime mode over HTTP.
    realtime_http_enabled: bool,

    /// Global fuzzy deduplication threshold for connected devices.
    dedup_threshold: u8,

    /// E1.31 sender CID (stable UUID per backend instance).
    e131_cid: uuid::Uuid,

    /// Next available E1.31 universe number for auto-allocation.
    next_e131_universe: u16,
}

impl WledBackend {
    /// Create a new WLED backend with known device IPs for discovery.
    ///
    /// These IPs are probed via HTTP during `discover()`. For
    /// zero-config discovery, use the [`WledScanner`](super::super::scanner::WledScanner)
    /// which uses mDNS.
    #[must_use]
    pub fn new(known_ips: Vec<IpAddr>) -> Self {
        Self::with_mdns_fallback(known_ips, false)
    }

    /// Create a backend with explicit mDNS fallback behavior.
    #[must_use]
    pub fn with_mdns_fallback(known_ips: Vec<IpAddr>, mdns_fallback: bool) -> Self {
        Self {
            known_ips,
            mdns_fallback,
            devices: HashMap::new(),
            device_ips: HashMap::new(),
            device_infos: HashMap::new(),
            default_protocol: WledProtocol::default(),
            shared_socket: None,
            realtime_http_enabled: true,
            dedup_threshold: DEFAULT_DEDUP_THRESHOLD,
            e131_cid: uuid::Uuid::now_v7(),
            next_e131_universe: 1,
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

    /// Seed the backend with a discovered device entry.
    pub fn remember_device(&mut self, device_id: DeviceId, ip: IpAddr, info: WledDeviceInfo) {
        self.device_ips.insert(device_id, ip);
        self.device_infos.insert(device_id, info);
    }

    /// The local address of the shared UDP socket, if initialized.
    #[must_use]
    pub fn shared_socket_local_addr(&self) -> Option<SocketAddr> {
        self.shared_socket
            .as_ref()
            .and_then(|socket| socket.local_addr().ok())
    }

    /// The local UDP socket used by a connected device, if available.
    #[must_use]
    pub fn connected_socket_local_addr(&self, id: &DeviceId) -> Option<SocketAddr> {
        self.devices
            .get(id)
            .and_then(|device| device.socket.local_addr().ok())
    }

    /// The starting E1.31 universe assigned to a connected device.
    #[must_use]
    pub fn connected_e131_start_universe(&self, id: &DeviceId) -> Option<u16> {
        self.devices
            .get(id)
            .map(|device| device.e131_start_universe)
    }

    /// Probe a single IP for a WLED device via HTTP.
    async fn probe_ip(ip: IpAddr) -> Result<WledDeviceInfo> {
        super::fetch_wled_info(ip).await
    }

    async fn ensure_shared_socket(&mut self) -> Result<Arc<UdpSocket>> {
        if let Some(socket) = &self.shared_socket {
            return Ok(Arc::clone(socket));
        }

        let socket = Arc::new(
            UdpSocket::bind("0.0.0.0:0")
                .await
                .context("Failed to bind shared WLED UDP socket")?,
        );
        self.shared_socket = Some(Arc::clone(&socket));
        Ok(socket)
    }

    async fn ensure_device_ready_for_output(&mut self, id: &DeviceId) -> Result<()> {
        let realtime_http_enabled = self.realtime_http_enabled;
        let device = self
            .devices
            .get_mut(id)
            .with_context(|| format!("WLED device {id} is not connected"))?;

        if device.stream_initialized {
            return Ok(());
        }

        if realtime_http_enabled {
            enter_realtime_mode(device.ip)
                .await
                .with_context(|| format!("Failed to enter realtime mode for WLED device {id}"))?;
            device.realtime_mode_active = true;
            validate_wled_receiver_config(
                device.ip,
                device.protocol,
                device.color_format,
                device.e131_start_universe,
            )
            .await;
            if let Err(error) = prime_device(device).await {
                if let Err(exit_error) = exit_realtime_mode(device.ip).await {
                    debug!(
                        device_id = %id,
                        ip = %device.ip,
                        error = %exit_error,
                        "best-effort exit from WLED realtime mode failed after priming error"
                    );
                }
                device.realtime_mode_active = false;
                return Err(error).with_context(|| format!("Failed to prime WLED device {id}"));
            }
        }

        device.stream_initialized = true;
        Ok(())
    }

    fn allocate_e131_start_universe(
        &mut self,
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
        let start_universe = self.next_e131_universe;
        self.next_e131_universe = self
            .next_e131_universe
            .saturating_add(universes_needed.max(1));
        start_universe
    }

    /// Check if a device is reachable over WLED's HTTP API.
    ///
    /// # Errors
    ///
    /// Returns an error when the device ID is unknown or the client cannot be built.
    pub async fn health_check(&self, id: &DeviceId) -> Result<bool> {
        let ip = self
            .device_ips
            .get(id)
            .copied()
            .with_context(|| format!("Unknown WLED device {id}"))?;

        probe_device_reachable(ip).await
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

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let mut discovered = Vec::new();
        let mut candidates: HashMap<IpAddr, Option<String>> = self
            .known_ips
            .iter()
            .copied()
            .map(|ip| (ip, None))
            .collect();

        if candidates.is_empty() && self.mdns_fallback {
            let mut scanner = super::scanner::WledScanner::with_timeout(Duration::from_secs(2));
            match scanner.scan().await {
                Ok(scanner_devices) => {
                    for device in scanner_devices {
                        let Some(ip_raw) = device.metadata.get("ip") else {
                            continue;
                        };
                        let Ok(ip) = ip_raw.parse::<IpAddr>() else {
                            continue;
                        };
                        let hostname = device
                            .metadata
                            .get("hostname")
                            .map(|value| value.trim_end_matches('.').to_ascii_lowercase());
                        candidates.entry(ip).or_insert(hostname);
                    }
                }
                Err(error) => {
                    debug!(error = %error, "WLED backend mDNS fallback scan failed");
                }
            }
        }

        self.device_ips.clear();
        self.device_infos.clear();

        for (ip, hostname) in candidates {
            match Self::probe_ip(ip).await {
                Ok(wled_info) => {
                    let fingerprint = wled_fingerprint(ip, hostname.as_deref(), &wled_info);
                    let device_id = fingerprint.stable_device_id();

                    info!(
                        ip = %ip,
                        name = %wled_info.name,
                        leds = wled_info.led_count,
                        firmware = %wled_info.firmware_version,
                        "Discovered WLED device"
                    );

                    let device_info = build_device_info(device_id, &wled_info, ip);
                    self.device_ips.insert(device_id, ip);
                    self.device_infos.insert(device_id, wled_info);
                    discovered.push(device_info);
                }
                Err(e) => {
                    debug!(ip = %ip, error = %e, "Failed to probe WLED device");
                }
            }
        }

        Ok(discovered)
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let known_ip_ids = self
            .device_ips
            .keys()
            .take(4)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let Some(ip) = self.device_ips.get(id).copied() else {
            bail!(
                "WLED device IP not found for {id}; cache_size={}, sample_ids=[{}]. discover() likely returned different IDs",
                self.device_ips.len(),
                known_ip_ids
            );
        };

        let known_info_ids = self
            .device_infos
            .keys()
            .take(4)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let Some(wled_info) = self.device_infos.get(id).cloned() else {
            bail!(
                "WLED device info not found for {id}; cache_size={}, sample_ids=[{}]. discover() likely returned different IDs",
                self.device_infos.len(),
                known_info_ids
            );
        };
        let socket = self.ensure_shared_socket().await?;

        let port = match self.default_protocol {
            WledProtocol::Ddp => DDP_PORT,
            WledProtocol::E131 => E131_PORT,
        };
        let pixel_addr = SocketAddr::new(ip, port);

        let color_format = if wled_info.rgbw {
            WledColorFormat::Rgbw
        } else {
            WledColorFormat::Rgb
        };
        let protocol = self.default_protocol;
        let e131_start_universe =
            self.allocate_e131_start_universe(color_format, wled_info.led_count, protocol);

        let device = WledDevice {
            device_id: *id,
            ip,
            protocol,
            pixel_addr,
            color_format,
            led_count: wled_info.led_count,
            info: wled_info,
            socket,
            ddp_sequence: DdpSequence::default(),
            e131_sequences: E131SequenceTracker::default(),
            e131_cid: self.e131_cid,
            e131_start_universe,
            dedup_threshold: self.dedup_threshold,
            last_sent_pixels: None,
            consecutive_failures: 0,
            last_success_at: None,
            last_size_mismatch_warn_at: None,
            realtime_mode_active: false,
            stream_initialized: false,
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

        self.devices.insert(*id, device);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if let Some(mut device) = self.devices.remove(id) {
            if device.last_sent_pixels.is_some()
                && let Err(error) = clear_device(&mut device).await
            {
                debug!(
                    device_id = %id,
                    ip = %device.ip,
                    error = %error,
                    "best-effort WLED clear frame failed during disconnect"
                );
            }
            if device.realtime_mode_active
                && let Err(error) = exit_realtime_mode(device.ip).await
            {
                debug!(
                    device_id = %id,
                    ip = %device.ip,
                    error = %error,
                    "best-effort exit from WLED realtime mode failed"
                );
            }
            info!(device_id = %id, "Disconnected from WLED device");
            Ok(())
        } else {
            bail!("WLED device {id} is not connected")
        }
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        self.ensure_device_ready_for_output(id).await?;
        let device = self
            .devices
            .get_mut(id)
            .with_context(|| format!("WLED device {id} is not connected"))?;
        let expected_led_count = usize::from(device.led_count);

        if colors.len() != expected_led_count {
            let should_warn = device
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);

            if should_warn {
                warn!(
                    device_id = %id,
                    expected_led_count,
                    actual_led_count = colors.len(),
                    "WLED frame size mismatch; truncating or padding to match device"
                );
                device.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let wire_format = match device.protocol {
            WledProtocol::Ddp => device.ddp_wire_format(),
            WledProtocol::E131 => device.color_format,
        };
        let pixel_data = encode_colors(colors, wire_format, expected_led_count);

        device.send_frame(&pixel_data).await
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.devices
            .get(id)
            .map(|device| device.info.negotiated_target_fps())
            .or_else(|| {
                self.device_infos
                    .get(id)
                    .map(WledDeviceInfo::negotiated_target_fps)
            })
    }
}
