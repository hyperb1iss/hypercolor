//! WLED backend — implements [`DeviceBackend`] for WLED LED controllers over UDP.
//!
//! Manages per-device UDP sockets, protocol selection (DDP vs E1.31),
//! and WLED JSON API parsing for device enrichment.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::{debug, info};

use crate::device::traits::{BackendInfo, DeviceBackend};
use crate::types::device::{
    ColorFormat, ConnectionType, DeviceCapabilities, DeviceFamily, DeviceId, DeviceInfo,
    LedTopology, ZoneInfo,
};

use super::ddp::{DDP_DTYPE_RGB8, DDP_DTYPE_RGBW8, DDP_PORT, DdpSequence, build_ddp_frame};
use super::e131::{
    E131_CHANNELS_PER_UNIVERSE, E131_PORT, E131_PRIORITY, E131Packet, E131SequenceTracker,
};

// ── Protocol Selection ──────────────────────────────────────────────────

/// Protocol transport for a WLED device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WledProtocol {
    /// Distributed Display Protocol (preferred).
    Ddp,
    /// E1.31 / Streaming ACN (fallback).
    E131,
}

impl Default for WledProtocol {
    fn default() -> Self {
        Self::Ddp
    }
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
    /// Number of built-in effects.
    pub effect_count: u8,
    /// Number of palettes.
    pub palette_count: u16,
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
        match self.protocol {
            WledProtocol::Ddp => self.send_ddp(pixel_data).await,
            WledProtocol::E131 => self.send_e131(pixel_data).await,
        }?;

        self.frames_sent += 1;
        self.last_frame_at = Some(Instant::now());
        Ok(())
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
        ColorFormat::Rgbw
    } else {
        ColorFormat::Rgb
    };

    DeviceInfo {
        id: device_id,
        name: wled_info.name.clone(),
        vendor: "WLED".to_owned(),
        family: DeviceFamily::Wled,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: u32::from(wled_info.led_count),
            topology: LedTopology::Strip,
            color_format,
        }],
        firmware_version: Some(wled_info.firmware_version.clone()),
        capabilities: DeviceCapabilities {
            led_count: u32::from(wled_info.led_count),
            supports_direct: true,
            supports_brightness: true,
            max_fps: u32::from(wled_info.fps).max(60),
        },
    }
}

// ── WledBackend ─────────────────────────────────────────────────────────

/// WLED device backend implementing [`DeviceBackend`].
///
/// Manages discovery via HTTP probing and per-device UDP streaming
/// over DDP or E1.31.
pub struct WledBackend {
    /// Known device IPs for HTTP-based discovery (no mDNS required).
    known_ips: Vec<IpAddr>,

    /// Connected devices, keyed by `DeviceId`.
    devices: HashMap<DeviceId, WledDevice>,

    /// Maps `DeviceId` to IP for lookup during connect.
    device_ips: HashMap<DeviceId, IpAddr>,

    /// Maps `DeviceId` to parsed info for lazy connect.
    device_infos: HashMap<DeviceId, WledDeviceInfo>,

    /// Default protocol for new connections.
    default_protocol: WledProtocol,

    /// E1.31 sender CID (stable UUID per backend instance).
    e131_cid: uuid::Uuid,
}

impl WledBackend {
    /// Create a new WLED backend with known device IPs for discovery.
    ///
    /// These IPs are probed via HTTP during `discover()`. For
    /// zero-config discovery, use the [`WledScanner`](super::scanner::WledScanner)
    /// which uses mDNS.
    #[must_use]
    pub fn new(known_ips: Vec<IpAddr>) -> Self {
        Self {
            known_ips,
            devices: HashMap::new(),
            device_ips: HashMap::new(),
            device_infos: HashMap::new(),
            default_protocol: WledProtocol::default(),
            e131_cid: uuid::Uuid::now_v7(),
        }
    }

    /// Set the default protocol for new connections.
    pub fn set_protocol(&mut self, protocol: WledProtocol) {
        self.default_protocol = protocol;
    }

    /// Probe a single IP for a WLED device via HTTP.
    async fn probe_ip(ip: IpAddr) -> Result<WledDeviceInfo> {
        super::fetch_wled_info(ip).await
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

        for &ip in &self.known_ips.clone() {
            match Self::probe_ip(ip).await {
                Ok(wled_info) => {
                    let device_id = DeviceId::new();

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
        let ip = self
            .device_ips
            .get(id)
            .copied()
            .context("Device IP not found — was discover() called?")?;

        let wled_info = self
            .device_infos
            .get(id)
            .cloned()
            .context("Device info not found — was discover() called?")?;

        // Bind a UDP socket for this device
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket")?;

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

        let device = WledDevice {
            device_id: *id,
            ip,
            protocol: self.default_protocol,
            pixel_addr,
            color_format,
            led_count: wled_info.led_count,
            info: wled_info,
            socket: Arc::new(socket),
            ddp_sequence: DdpSequence::default(),
            e131_sequences: E131SequenceTracker::default(),
            e131_cid: self.e131_cid,
            e131_start_universe: 1,
            frames_sent: 0,
            last_frame_at: None,
        };

        info!(
            device_id = %id,
            ip = %ip,
            protocol = ?self.default_protocol,
            leds = device.led_count,
            "Connected to WLED device"
        );

        self.devices.insert(*id, device);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if self.devices.remove(id).is_some() {
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

        // Convert RGB triplets to flat pixel data
        let pixel_data: Vec<u8> = match device.color_format {
            WledColorFormat::Rgb => colors.iter().flat_map(|c| *c).collect(),
            WledColorFormat::Rgbw => colors
                .iter()
                .flat_map(|c| {
                    // Naive white extraction: min(R,G,B) becomes the white channel
                    let w = c[0].min(c[1]).min(c[2]);
                    [c[0] - w, c[1] - w, c[2] - w, w]
                })
                .collect(),
        };

        device.send_frame(&pixel_data).await
    }
}
