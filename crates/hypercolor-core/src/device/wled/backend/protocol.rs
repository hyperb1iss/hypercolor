//! Wire-format encoders and per-device send state machines for DDP and E1.31.
//!
//! [`WledDevice`] owns a UDP socket plus per-protocol sequence counters and
//! exposes [`WledDevice::send_frame`] / [`WledDevice::send_frame_forced`] as
//! the protocol-neutral entry point for pushing pixel data. Fuzzy frame
//! deduplication and per-protocol fragmentation live here too.
//!
//! Also houses the `/json/cfg` realtime-receiver parsing and mismatch
//! checks — they are protocol-adjacent validation, not lifecycle plumbing.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::net::UdpSocket;

use super::cache::WledDeviceInfo;
use crate::device::wled::ddp::{DDP_DTYPE_RGB8, DDP_DTYPE_RGBW8, DdpSequence, build_ddp_frame};
use crate::device::wled::e131::{
    E131_CHANNELS_PER_UNIVERSE, E131_PORT, E131_PRIORITY, E131Packet, E131SequenceTracker,
};
use crate::types::device::DeviceId;

/// Interval between mandatory keepalive frames. Frames closer together
/// than this may be suppressed by [`WledDevice::send_frame`] dedup.
pub(super) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);

// ── Protocol Selection ──────────────────────────────────────────────────

/// Protocol transport for a WLED device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
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
    pub(super) socket: Arc<UdpSocket>,

    /// DDP sequence counter.
    pub(super) ddp_sequence: DdpSequence,
    /// E1.31 per-universe sequence counters.
    pub(super) e131_sequences: E131SequenceTracker,
    /// E1.31 sender CID (stable UUID per Hypercolor instance).
    pub(super) e131_cid: uuid::Uuid,
    /// Starting E1.31 universe number for this device.
    pub(super) e131_start_universe: u16,
    /// Per-device threshold for fuzzy frame deduplication.
    pub(super) dedup_threshold: u8,
    /// Last pixel data successfully sent.
    pub(super) last_sent_pixels: Option<Vec<u8>>,
    /// Rolling count of consecutive send failures.
    pub(super) consecutive_failures: u32,
    /// Timestamp of the last successful send.
    pub(super) last_success_at: Option<Instant>,
    /// Timestamp of the most recent frame-size mismatch warning.
    pub(super) last_size_mismatch_warn_at: Option<Instant>,
    /// Whether WLED realtime mode has been enabled for this session.
    pub(super) realtime_mode_active: bool,
    /// Whether the device's output stream has been initialized.
    pub(super) stream_initialized: bool,

    /// Frames successfully sent.
    pub frames_sent: u64,
    /// Last successful frame timestamp.
    pub last_frame_at: Option<Instant>,
}

impl WledDevice {
    pub(super) fn ddp_wire_format(&self) -> WledColorFormat {
        match self.color_format {
            // Preserve hue fidelity for WLED RGBW strips in DDP mode by
            // sending RGB-only payloads and letting WLED handle white-channel
            // behavior locally. WLED accepts RGB24 DDP for RGBW outputs.
            WledColorFormat::Rgbw | WledColorFormat::Rgb => WledColorFormat::Rgb,
        }
    }

    pub(super) fn black_frame(&self) -> Vec<u8> {
        vec![
            0_u8;
            usize::from(self.led_count)
                * match self.protocol {
                    WledProtocol::Ddp => self.ddp_wire_format().bytes_per_pixel(),
                    WledProtocol::E131 => self.color_format.bytes_per_pixel(),
                }
        ]
    }

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
            && self.last_sent_pixels.as_deref().is_some_and(|last| {
                self.dedup_threshold > 0
                    && pixels_match_with_threshold(last, pixel_data, self.dedup_threshold)
            })
        {
            return Ok(());
        }

        self.send_frame_forced(pixel_data).await
    }

    /// Send a frame immediately, bypassing deduplication.
    ///
    /// # Errors
    ///
    /// Returns an error if the UDP send fails.
    pub(super) async fn send_frame_forced(&mut self, pixel_data: &[u8]) -> Result<()> {
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
            self.ddp_wire_format().ddp_data_type(),
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

// ── Encoding + Dedup Helpers ────────────────────────────────────────────

pub(super) fn pixels_match_with_threshold(
    previous: &[u8],
    current: &[u8],
    threshold: u8,
) -> bool {
    previous.len() == current.len()
        && previous
            .iter()
            .zip(current.iter())
            .all(|(left, right)| left.abs_diff(*right) <= threshold)
}

pub(super) fn encode_colors(
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

// ── Realtime Receiver Config Parsing ────────────────────────────────────

/// Minimal realtime receiver settings from WLED `/json/cfg`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WledLiveReceiverConfig {
    /// Whether realtime receive is enabled.
    pub enabled: bool,
    /// Whether realtime mode is enabled in WLED.
    pub realtime_mode_enabled: bool,
    /// UDP port for the selected realtime receiver.
    pub port: u16,
    /// DMX start address (1-based for E1.31/Art-Net).
    pub dmx_address: Option<u16>,
    /// Starting E1.31 universe.
    pub dmx_universe: Option<u16>,
    /// WLED DMX mode enum.
    pub dmx_mode: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct WledCfgRoot {
    #[serde(rename = "if")]
    interfaces: Option<WledCfgInterfaces>,
}

#[derive(Debug, Deserialize)]
struct WledCfgInterfaces {
    live: Option<WledCfgLive>,
}

#[derive(Debug, Deserialize)]
struct WledCfgLive {
    #[serde(default)]
    en: bool,
    #[serde(default)]
    rlm: bool,
    #[serde(default)]
    port: u16,
    dmx: Option<WledCfgLiveDmx>,
}

#[derive(Debug, Deserialize)]
struct WledCfgLiveDmx {
    uni: Option<u16>,
    addr: Option<u16>,
    mode: Option<u8>,
}

/// Parse WLED realtime receiver settings from `/json/cfg`.
///
/// # Errors
///
/// Returns an error if the JSON cannot be parsed into the expected shape.
pub fn parse_wled_live_receiver_config(
    json: &serde_json::Value,
) -> Result<Option<WledLiveReceiverConfig>> {
    let cfg: WledCfgRoot =
        serde_json::from_value(json.clone()).context("Failed to parse WLED /json/cfg")?;

    let Some(live) = cfg.interfaces.and_then(|interfaces| interfaces.live) else {
        return Ok(None);
    };

    Ok(Some(WledLiveReceiverConfig {
        enabled: live.en,
        realtime_mode_enabled: live.rlm,
        port: live.port,
        dmx_address: live.dmx.as_ref().and_then(|dmx| dmx.addr),
        dmx_universe: live.dmx.as_ref().and_then(|dmx| dmx.uni),
        dmx_mode: live.dmx.and_then(|dmx| dmx.mode),
    }))
}

/// Compare WLED realtime receiver settings with Hypercolor's stream settings.
///
/// WLED documents DDP as a fixed-port receiver on `4048`, but `/json/cfg`
/// exposes the shared live-sync port and DMX settings used for E1.31/Art-Net.
/// In DDP mode those fields are not authoritative, so Hypercolor only validates
/// the generic realtime flags and skips the E1.31-specific checks.
#[must_use]
pub fn wled_receiver_config_mismatches(
    config: &WledLiveReceiverConfig,
    protocol: WledProtocol,
    color_format: WledColorFormat,
    e131_start_universe: u16,
) -> Vec<String> {
    let mut mismatches = Vec::new();

    if !config.enabled {
        mismatches.push("live receiver disabled".to_owned());
    }
    if !config.realtime_mode_enabled {
        mismatches.push("realtime mode disabled".to_owned());
    }

    if protocol == WledProtocol::E131 {
        let expected_mode = expected_wled_e131_mode(color_format);
        if config.port != E131_PORT {
            mismatches.push(format!(
                "expected E1.31 port {E131_PORT}, WLED is set to {}",
                config.port
            ));
        }
        match config.dmx_universe {
            Some(actual) if actual != e131_start_universe => mismatches.push(format!(
                "expected start universe {e131_start_universe}, WLED is set to {actual}"
            )),
            None => mismatches.push("missing E1.31 start universe".to_owned()),
            _ => {}
        }
        match config.dmx_address {
            Some(1) => {}
            Some(actual) => mismatches.push(format!(
                "expected DMX start address 1, WLED is set to {actual}"
            )),
            None => mismatches.push("missing DMX start address".to_owned()),
        }
        match config.dmx_mode {
            Some(actual) if actual != expected_mode => mismatches.push(format!(
                "expected DMX mode {} ({}), WLED is set to {} ({})",
                expected_mode,
                wled_e131_mode_name(expected_mode),
                actual,
                wled_e131_mode_name(actual)
            )),
            None => mismatches.push("missing E1.31 DMX mode".to_owned()),
            _ => {}
        }
    }

    mismatches
}

pub(super) const fn expected_wled_e131_mode(color_format: WledColorFormat) -> u8 {
    match color_format {
        WledColorFormat::Rgb => 4,
        WledColorFormat::Rgbw => 6,
    }
}

pub(super) const fn wled_e131_mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "disabled",
        1 => "single_rgb",
        2 => "single_drgb",
        3 => "effect",
        4 => "multiple_rgb",
        5 => "multiple_drgb",
        6 => "multiple_rgbw",
        7 => "effect_w",
        8 => "effect_segment",
        9 => "effect_segment_w",
        10 => "preset",
        _ => "unknown",
    }
}
