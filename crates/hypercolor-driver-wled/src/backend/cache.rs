//! Device identity and metadata cache for WLED backends.
//!
//! Holds the types parsed from `/json/info` and `/json/state`, the
//! fingerprinting logic used to produce stable [`DeviceId`]s across
//! rediscoveries, and the translation into a generic [`DeviceInfo`].

use std::net::IpAddr;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

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
    /// Whether the controller is currently connected over `WiFi`.
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
            (301..=600, true) => 30,
            (0..=300, _) | (301..=600, false) => 40,
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

// ── JSON API Parsing ────────────────────────────────────────────────────

/// Parse the WLED `/json/info` response into a [`WledDeviceInfo`].
///
/// # Errors
///
/// Returns an error if required fields are missing or malformed.
pub fn parse_wled_info(json: &serde_json::Value) -> Result<WledDeviceInfo> {
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
            // Bit 0 = RGB, Bit 1 = White in WLED's light capability bitfield.
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

// ── Fingerprint / DeviceInfo translation ────────────────────────────────

/// Produce a stable [`DeviceFingerprint`] for a WLED device, preferring
/// the MAC address when available and falling back to hostname or IP.
pub(super) fn wled_fingerprint(
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

/// Build a generic [`DeviceInfo`] from parsed WLED data.
pub(super) fn build_device_info(
    device_id: DeviceId,
    wled_info: &WledDeviceInfo,
    _ip: IpAddr,
) -> DeviceInfo {
    let color_format = if wled_info.rgbw {
        DeviceColorFormat::Rgbw
    } else {
        DeviceColorFormat::Rgb
    };

    DeviceInfo {
        id: device_id,
        name: wled_info.name.clone(),
        vendor: "WLED".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: u32::from(wled_info.led_count),
            topology: DeviceTopologyHint::Strip,
            color_format,
            layout_hint: None,
        }],
        firmware_version: Some(wled_info.firmware_version.clone()),
        capabilities: DeviceCapabilities {
            led_count: u32::from(wled_info.led_count),
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: wled_info.negotiated_target_fps(),
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}
