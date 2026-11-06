//! Driver configuration: manually configured serial LED controllers.
//!
//! Adalight-family boards sit behind generic USB-serial chips that also
//! appear in unrelated hardware, so the driver never guesses which port is
//! which. Every device is an entry in `drivers.adalight.devices` naming its
//! port, line rate, protocol variant, and LED count.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::color::ColorOrder;
use crate::protocol::{AdalightProtocol, WhiteCalibration};

/// Most devices one driver config may list.
pub const MAX_DEVICES: usize = 64;

/// Longest device id, in bytes.
pub const MAX_DEVICE_ID_LEN: usize = 64;

/// Longest display name, in characters.
pub const MAX_NAME_LEN: usize = 128;

/// Longest serial port path, in bytes.
pub const MAX_PORT_LEN: usize = 255;

/// Lowest accepted line rate.
pub const MIN_BAUD_RATE: u32 = 1_200;

/// Highest accepted line rate.
pub const MAX_BAUD_RATE: u32 = 12_000_000;

/// Line rate the Adalight sketches default to.
pub const DEFAULT_ADA_BAUD_RATE: u32 = 115_200;

/// Line rate HyperSerial firmware defaults to.
pub const DEFAULT_AWA_BAUD_RATE: u32 = 2_000_000;

/// Longest configurable wait between opening the port and the first frame.
pub const MAX_STARTUP_DELAY_MS: u32 = 10_000;

/// Longest configurable idle gap after each frame.
pub const MAX_LATCH_MS: u32 = 1_000;

/// Highest configurable per-device frame rate cap.
pub const MAX_FPS_LIMIT: u32 = 1_000;

/// Who produces the white channel of an RGBW strip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum WhiteMode {
    /// RGB strip, or the firmware handles white without host input.
    #[default]
    Off,
    /// AWA only: send a white calibration block and let the firmware
    /// derive white from RGB.
    Firmware,
    /// AWA only: Hypercolor derives white and sends four bytes per LED.
    Host,
}

impl WhiteMode {
    /// Every mode, in display order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Firmware, Self::Host];

    /// Stable lowercase identifier used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Firmware => "firmware",
            Self::Host => "host",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off (RGB)",
            Self::Firmware => "Firmware derives white",
            Self::Host => "Hypercolor derives white",
        }
    }

    /// Whether the strip has a white channel.
    #[must_use]
    pub const fn has_white(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Parse a mode name, ignoring ASCII case.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "rgb" => Some(Self::Off),
            "firmware" => Some(Self::Firmware),
            "host" => Some(Self::Host),
            _ => None,
        }
    }
}

impl fmt::Display for WhiteMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for WhiteMode {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| {
            format!("unknown white mode '{value}' (expected off, firmware, or host)")
        })
    }
}

impl From<WhiteMode> for String {
    fn from(value: WhiteMode) -> Self {
        value.as_str().to_owned()
    }
}

/// Adalight driver configuration (`[drivers.adalight]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdalightConfig {
    /// Manually configured serial controllers.
    #[serde(default)]
    pub devices: Vec<AdalightDeviceConfig>,
}

/// One manually configured serial LED controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdalightDeviceConfig {
    /// Stable identifier. Device identity derives from it, so renaming the
    /// device or moving it to another port keeps layouts intact.
    pub id: String,

    /// Display name; defaults to the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Serial port path: `/dev/ttyUSB0`, `/dev/serial/by-id/...`,
    /// `/dev/cu.usbserial-...`, or `COM3`.
    pub port: String,

    /// Line rate in baud; defaults to 115200 for `ada` and 2000000 for
    /// `awa`, the firmware defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baud_rate: Option<u32>,

    /// Wire protocol variant.
    #[serde(default)]
    pub protocol: AdalightProtocol,

    /// LEDs on the strip. Must match what the firmware drives.
    pub led_count: u32,

    /// Byte order of each LED on the wire.
    #[serde(default)]
    pub color_order: ColorOrder,

    /// White channel handling for RGBW strips (AWA only).
    #[serde(default)]
    pub white: WhiteMode,

    /// Calibration sent with `white = "firmware"`; ignored otherwise.
    /// Defaults to HyperSerial's cold-white values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub white_calibration: Option<WhiteCalibration>,

    /// Wait after opening the port before the first frame, in ms. Boards
    /// that reset when the port opens (classic Arduinos) boot in this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_delay_ms: Option<u32>,

    /// Idle time after each frame, in ms, for firmware that cannot receive
    /// while it latches LEDs. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latch_ms: Option<u32>,

    /// Output frame rate cap. Unset or 0 sends as fast as the line allows,
    /// up to the render loop's rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<u32>,
}

impl AdalightDeviceConfig {
    /// Display name, falling back to the id.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    /// Line rate, falling back to the protocol's firmware default.
    #[must_use]
    pub const fn effective_baud_rate(&self) -> u32 {
        match self.baud_rate {
            Some(baud_rate) => baud_rate,
            None => match self.protocol {
                AdalightProtocol::Ada => DEFAULT_ADA_BAUD_RATE,
                AdalightProtocol::Awa => DEFAULT_AWA_BAUD_RATE,
            },
        }
    }
}

/// Validation failure for the Adalight driver config.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdalightConfigError {
    /// The config lists more devices than the driver accepts.
    #[error("{count} serial devices configured; the limit is {MAX_DEVICES}")]
    TooManyDevices {
        /// Configured device count.
        count: usize,
    },

    /// One device entry is invalid.
    #[error("serial device '{id}': {message}")]
    Device {
        /// Device id as written in config.
        id: String,
        /// What is wrong.
        message: String,
    },

    /// Two devices use the same id.
    #[error("serial device id '{id}' is used more than once")]
    DuplicateId {
        /// Duplicated id.
        id: String,
    },

    /// Two devices name the same port.
    #[error(
        "serial devices '{first}' and '{second}' both use port {port}; one port drives one strip"
    )]
    DuplicatePort {
        /// First device id.
        first: String,
        /// Second device id.
        second: String,
        /// Shared port path.
        port: String,
    },
}

impl AdalightConfigError {
    pub(crate) fn device(id: &str, message: impl Into<String>) -> Self {
        Self::Device {
            id: id.to_owned(),
            message: message.into(),
        }
    }
}

/// Whether `id` is a valid device id: 1-64 ASCII letters, digits, `-`, `_`.
#[must_use]
pub fn is_valid_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_DEVICE_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
