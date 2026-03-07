//! WLED backend — implements [`DeviceBackend`] for WLED LED controllers over UDP.
//!
//! Manages per-device UDP sockets, protocol selection (DDP vs E1.31),
//! and WLED JSON API parsing for device enrichment.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::device::discovery::TransportScanner;
use crate::device::traits::{BackendInfo, DeviceBackend};
use crate::types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo,
};

use super::ddp::{DDP_DTYPE_RGB8, DDP_DTYPE_RGBW8, DDP_PORT, DdpSequence, build_ddp_frame};
use super::e131::{
    E131_CHANNELS_PER_UNIVERSE, E131_PORT, E131_PRIORITY, E131Packet, E131SequenceTracker,
};

const REALTIME_HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const REALTIME_PRIME_FRAMES: usize = 3;
const REALTIME_PRIME_DELAY: Duration = Duration::from_millis(50);
const DEDUP_THRESHOLD: u8 = 2;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_secs(60);

// ── Protocol Selection ──────────────────────────────────────────────────

/// Protocol transport for a WLED device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WledProtocol {
    /// Distributed Display Protocol (preferred).
    #[default]
    Ddp,
    /// E1.31 / Streaming ACN (fallback).
    E131,
}

// ── Color Format ────────────────────────────────────────────────────────

/// Color format reported by the WLED device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WledColorFormat {
    /// 3 bytes per pixel: red, green, blue.
    Rgb,
    /// 4 bytes per pixel: red, green, blue, white.
    Rgbw,
}

impl WledColorFormat {
    /// Bytes per pixel for this color format.
    #[must_use]
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgbw => 4,
        }
    }

    /// DDP data type byte for this color format.
    #[must_use]
    pub fn ddp_data_type(self) -> u8 {
        match self {
            Self::Rgb => DDP_DTYPE_RGB8,
            Self::Rgbw => DDP_DTYPE_RGBW8,
        }
    }
}

// ── WLED Device Info ────────────────────────────────────────────────────

/// Information enriched from WLED's `/json/info` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WledDeviceInfo {
    /// Firmware version string (e.g., "0.15.3").
    pub firmware_version: String,
    /// Firmware build ID in YYMMDDB format.
    pub build_id: u32,
    /// Hardware MAC address, lowercase hex without colons.
    pub mac: String,
    /// Human-friendly name configured in WLED.
    pub name: String,
    /// Total LED count across all segments.
    pub led_count: u16,
    /// Whether the device has RGBW LEDs.
    pub rgbw: bool,
    /// Maximum number of segments supported.
    pub max_segments: u8,
    /// Current frames per second reported by WLED.
    pub fps: u8,
    /// Current power draw in milliamps (if ABL is enabled).
    pub power_draw_ma: u16,
    /// Maximum power budget in milliamps.
    pub max_power_ma: u16,
    /// Free heap memory in bytes (health indicator).
    pub free_heap: u32,
    /// Uptime in seconds.
    pub uptime_secs: u32,
    /// Architecture string (e.g., "esp32").
    pub arch: String,
    /// Whether the controller is currently connected over WiFi.
    #[serde(default)]
    pub is_wifi: bool,
    /// Number of built-in effects.
    pub effect_count: u8,
    /// Number of palettes.
    pub palette_count: u16,
}

impl WledDeviceInfo {
    /// Negotiate a practical streaming FPS from WLED's reported capabilities.
    #[must_use]
    pub fn negotiated_target_fps(&self) -> u32 {
        if self.fps > 0 {
            return u32::from(self.fps).clamp(15, 60);
        }

        match (usize::from(self.led_count), self.is_wifi) {
            (0..=300, _) => 40,
            (301..=600, true) => 30,
            (301..=600, false) => 40,
            (_, true) => 25,
            (_, false) => 35,
        }
    }
}

// ── WLED Segment Info ───────────────────────────────────────────────────

/// A WLED segment as reported by `/json/state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WledSegmentInfo {
    /// Segment index (0-based).
    pub id: u8,
    /// Start LED index (inclusive).
    pub start: u16,
    /// Stop LED index (exclusive).
    pub stop: u16,
    /// LED grouping factor.
    pub grouping: u8,
    /// Spacing between groups.
    pub spacing: u8,
    /// Whether the segment is currently on.
    pub on: bool,
    /// Segment brightness (0-255).
    pub brightness: u8,
    /// Whether the LEDs in this segment support RGBW.
    pub rgbw: bool,
    /// Virtual light capabilities bitfield.
    pub light_capabilities: u8,
}

impl WledSegmentInfo {
    /// Logical LED count accounting for grouping and spacing.
    #[must_use]
    pub fn pixel_count(&self) -> u16 {
        let raw = self.stop.saturating_sub(self.start);
        if self.grouping == 0 || self.spacing == 0 {
            return raw;
        }
        let group_size = u16::from(self.grouping) + u16::from(self.spacing);
        if group_size == 0 {
            return raw;
        }
        raw / group_size * u16::from(self.grouping)
    }
}

// ── Per-Device Handle ───────────────────────────────────────────────────

/// Runtime handle for a single connected WLED device.
///
/// Owns its UDP socket and maintains per-device protocol state.
pub struct WledDevice {
    /// Device identity.
    pub device_id: DeviceId,
    /// Resolved IP address.
    pub ip: IpAddr,
    /// Which protocol to use for pixel data.
    pub protocol: WledProtocol,
    /// Socket address for pixel data.
    pub pixel_addr: SocketAddr,
    /// Color format (RGB or RGBW).
    pub color_format: WledColorFormat,
    /// Total LED count on this device.
    pub led_count: u16,
    /// Enriched device info from `/json/info`.
    pub info: WledDeviceInfo,

    /// UDP socket for sending pixel data.
    socket: Arc<UdpSocket>,

    /// DDP sequence counter.
    ddp_sequence: DdpSequence,
    /// E1.31 per-universe sequence counters.
    e131_sequences: E131SequenceTracker,
    /// E1.31 sender CID (stable UUID per Hypercolor instance).
    e131_cid: uuid::Uuid,
    /// Starting E1.31 universe number for this device.
    e131_start_universe: u16,
    /// Last pixel data successfully sent.
    last_sent_pixels: Option<Vec<u8>>,
    /// Rolling count of consecutive send failures.
    consecutive_failures: u32,
    /// Timestamp of the last successful send.
    last_success_at: Option<Instant>,
    /// Timestamp of the most recent frame-size mismatch warning.
    last_size_mismatch_warn_at: Option<Instant>,

    /// Frames successfully sent.
    pub frames_sent: u64,
    /// Last successful frame timestamp.
    pub last_frame_at: Option<Instant>,
}

impl WledDevice {
    /// Send a frame of raw pixel bytes to this device.
    ///
    /// # Errors
    ///
    /// Returns an error if the UDP send fails.
    pub async fn send_frame(&mut self, pixel_data: &[u8]) -> Result<()> {
        let force_send = self
            .last_frame_at
            .is_none_or(|last_frame_at| last_frame_at.elapsed() >= KEEPALIVE_INTERVAL);

        if !force_send
            && self
                .last_sent_pixels
                .as_deref()
                .is_some_and(|last| pixels_match_with_threshold(last, pixel_data, DEDUP_THRESHOLD))
        {
            return Ok(());
        }

        let send_result = match self.protocol {
            WledProtocol::Ddp => self.send_ddp(pixel_data).await,
            WledProtocol::E131 => self.send_e131(pixel_data).await,
        };

        match &send_result {
            Ok(()) => {
                let now = Instant::now();
                self.frames_sent += 1;
                self.last_frame_at = Some(now);
                self.last_success_at = Some(now);
                self.last_sent_pixels = Some(pixel_data.to_vec());
                self.consecutive_failures = 0;
            }
            Err(_) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            }
        }

        send_result
    }

    /// Send pixel data via DDP, fragmenting if necessary.
    async fn send_ddp(&mut self, pixel_data: &[u8]) -> Result<()> {
        let packets = build_ddp_frame(
            pixel_data,
            self.color_format.ddp_data_type(),
            &mut self.ddp_sequence,
        );

        for packet in &packets {
            self.socket
                .send_to(packet.as_bytes(), self.pixel_addr)
                .await
                .context("DDP UDP send failed")?;
        }

        Ok(())
    }

    /// Send pixel data via E1.31, splitting across universes.
    async fn send_e131(&mut self, pixel_data: &[u8]) -> Result<()> {
        let bpp = self.color_format.bytes_per_pixel();
        let pixels_per_universe = E131_CHANNELS_PER_UNIVERSE / bpp;
        let bytes_per_universe = pixels_per_universe * bpp;

        for (i, chunk) in pixel_data.chunks(bytes_per_universe).enumerate() {
            #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
            let universe = self.e131_start_universe + i as u16;
            let sequence = self.e131_sequences.advance(universe);

            let mut packet = E131Packet::new("Hypercolor", self.e131_cid, universe, E131_PRIORITY);
            packet.set_channels(chunk, sequence);

            self.socket
                .send_to(packet.as_bytes(), (self.ip, E131_PORT))
                .await
                .context("E1.31 UDP send failed")?;
        }

        Ok(())
    }
}

// ── JSON API Parsing ────────────────────────────────────────────────────

/// Parse the WLED `/json/info` response into a `WledDeviceInfo`.
///
/// # Errors
///
/// Returns an error if required fields are missing or malformed.
pub fn parse_wled_info(json: &serde_json::Value) -> Result<WledDeviceInfo> {
    // Validate that this looks like a WLED info response
    json.get("ver")
        .and_then(serde_json::Value::as_str)
        .context("WLED /json/info missing 'ver' field")?;

    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    Ok(WledDeviceInfo {
        firmware_version: json["ver"].as_str().unwrap_or("unknown").to_owned(),
        build_id: json["vid"].as_u64().unwrap_or(0) as u32,
        mac: json["mac"].as_str().unwrap_or("").to_owned(),
        name: json["name"].as_str().unwrap_or("WLED").to_owned(),
        led_count: json["leds"]["count"].as_u64().unwrap_or(0) as u16,
        rgbw: json["leds"]["rgbw"].as_bool().unwrap_or(false),
        max_segments: json["leds"]["maxseg"].as_u64().unwrap_or(1) as u8,
        fps: json["leds"]["fps"].as_u64().unwrap_or(0) as u8,
        power_draw_ma: json["leds"]["pwr"].as_u64().unwrap_or(0) as u16,
        max_power_ma: json["leds"]["maxpwr"].as_u64().unwrap_or(0) as u16,
        free_heap: json["freeheap"].as_u64().unwrap_or(0) as u32,
        uptime_secs: json["uptime"].as_u64().unwrap_or(0) as u32,
        arch: json["arch"].as_str().unwrap_or("unknown").to_owned(),
        is_wifi: json["wifi"]["bssid"]
            .as_str()
            .is_some_and(|bssid| !bssid.trim().is_empty()),
        effect_count: json["fxcount"].as_u64().unwrap_or(0) as u8,
        palette_count: json["palcount"].as_u64().unwrap_or(0) as u16,
    })
}

/// Parse segments from the WLED `/json/state` response.
///
/// # Errors
///
/// Returns an error if the `seg` array is missing or malformed.
pub fn parse_wled_segments(json: &serde_json::Value) -> Result<Vec<WledSegmentInfo>> {
    let segments = json["seg"]
        .as_array()
        .context("Missing 'seg' array in WLED state response")?;

    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    let result: Vec<WledSegmentInfo> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            // Determine RGBW capability from light capability bitfield
            // Bit 0 = RGB, Bit 1 = White
            let lc = seg["lc"].as_u64().unwrap_or(1) as u8;
            let has_white = (lc & 0x02) != 0;

            WledSegmentInfo {
                id: seg["id"].as_u64().unwrap_or(i as u64) as u8,
                start: seg["start"].as_u64().unwrap_or(0) as u16,
                stop: seg["stop"].as_u64().unwrap_or(0) as u16,
                grouping: seg["grp"].as_u64().unwrap_or(1) as u8,
                spacing: seg["spc"].as_u64().unwrap_or(0) as u8,
                on: seg["on"].as_bool().unwrap_or(true),
                brightness: seg["bri"].as_u64().unwrap_or(255) as u8,
                rgbw: has_white,
                light_capabilities: lc,
            }
        })
        .collect();

    Ok(result)
}

/// Build a [`DeviceInfo`] from parsed WLED data.
fn build_device_info(device_id: DeviceId, wled_info: &WledDeviceInfo, _ip: IpAddr) -> DeviceInfo {
    let color_format = if wled_info.rgbw {
        DeviceColorFormat::Rgbw
    } else {
        DeviceColorFormat::Rgb
    };

    DeviceInfo {
        id: device_id,
        name: wled_info.name.clone(),
        vendor: "WLED".to_owned(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: u32::from(wled_info.led_count),
            topology: DeviceTopologyHint::Strip,
            color_format,
        }],
        firmware_version: Some(wled_info.firmware_version.clone()),
        capabilities: DeviceCapabilities {
            led_count: u32::from(wled_info.led_count),
            supports_direct: true,
            supports_brightness: true,
            max_fps: wled_info.negotiated_target_fps(),
        },
    }
}

async fn post_realtime_state(ip: IpAddr, body: serde_json::Value) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(REALTIME_HTTP_TIMEOUT)
        .build()
        .context("Failed to build WLED HTTP client")?;

    client
        .post(format!("http://{ip}/json/state"))
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Failed to update WLED realtime state for {ip}"))?;

    Ok(())
}

async fn enter_realtime_mode(ip: IpAddr) -> Result<()> {
    post_realtime_state(
        ip,
        serde_json::json!({
            "lor": 1,
            "live": true,
            "transition": 0,
        }),
    )
    .await
}

async fn exit_realtime_mode(ip: IpAddr) -> Result<()> {
    post_realtime_state(
        ip,
        serde_json::json!({
            "live": false,
            "lor": 0,
            "transition": 7,
        }),
    )
    .await
}

async fn prime_device(device: &mut WledDevice) -> Result<()> {
    let black_frame =
        vec![0_u8; usize::from(device.led_count) * device.color_format.bytes_per_pixel()];

    for _ in 0..REALTIME_PRIME_FRAMES {
        device.send_frame(&black_frame).await?;
        tokio::time::sleep(REALTIME_PRIME_DELAY).await;
    }

    Ok(())
}

fn pixels_match_with_threshold(previous: &[u8], current: &[u8], threshold: u8) -> bool {
    previous.len() == current.len()
        && previous
            .iter()
            .zip(current.iter())
            .all(|(left, right)| left.abs_diff(*right) <= threshold)
}

fn encode_colors(
    colors: &[[u8; 3]],
    color_format: WledColorFormat,
    expected_led_count: usize,
) -> Vec<u8> {
    match color_format {
        WledColorFormat::Rgb => {
            let mut pixel_data = Vec::with_capacity(expected_led_count * 3);
            for index in 0..expected_led_count {
                let color = colors.get(index).copied().unwrap_or([0, 0, 0]);
                pixel_data.extend_from_slice(&color);
            }
            pixel_data
        }
        WledColorFormat::Rgbw => {
            let mut pixel_data = Vec::with_capacity(expected_led_count * 4);
            for index in 0..expected_led_count {
                let color = colors.get(index).copied().unwrap_or([0, 0, 0]);
                let white = color[0].min(color[1]).min(color[2]);
                pixel_data.extend_from_slice(&[
                    color[0] - white,
                    color[1] - white,
                    color[2] - white,
                    white,
                ]);
            }
            pixel_data
        }
    }
}

fn wled_fingerprint(
    ip: IpAddr,
    hostname: Option<&str>,
    wled_info: &WledDeviceInfo,
) -> DeviceFingerprint {
    if !wled_info.mac.is_empty() {
        return DeviceFingerprint(format!("net:{}", wled_info.mac.to_ascii_lowercase()));
    }

    if let Some(hostname) = hostname {
        return DeviceFingerprint(format!("net:wled:{}", hostname.to_ascii_lowercase()));
    }

    DeviceFingerprint(format!("net:wled:{ip}"))
}

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

    /// E1.31 sender CID (stable UUID per backend instance).
    e131_cid: uuid::Uuid,

    /// Next available E1.31 universe number for auto-allocation.
    next_e131_universe: u16,
}

impl WledBackend {
    /// Create a new WLED backend with known device IPs for discovery.
    ///
    /// These IPs are probed via HTTP during `discover()`. For
    /// zero-config discovery, use the [`WledScanner`](super::scanner::WledScanner)
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

        let url = format!("http://{ip}/json/info");
        let client = reqwest::Client::builder()
            .timeout(REALTIME_HTTP_TIMEOUT)
            .build()
            .context("Failed to build WLED HTTP client")?;

        Ok(client.get(url).send().await.is_ok())
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
        if self.realtime_http_enabled {
            enter_realtime_mode(ip)
                .await
                .with_context(|| format!("Failed to enter realtime mode for WLED device {id}"))?;
        }

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

        let mut device = WledDevice {
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
            last_sent_pixels: None,
            consecutive_failures: 0,
            last_success_at: None,
            last_size_mismatch_warn_at: None,
            frames_sent: 0,
            last_frame_at: None,
        };

        if self.realtime_http_enabled {
            prime_device(&mut device)
                .await
                .with_context(|| format!("Failed to prime WLED device {id}"))?;
        }

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
        if let Some(device) = self.devices.remove(id) {
            if self.realtime_http_enabled
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

        let pixel_data = encode_colors(colors, device.color_format, expected_led_count);

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
