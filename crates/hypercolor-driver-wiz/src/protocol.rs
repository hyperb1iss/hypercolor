//! WiZ wire formats: JSON request bytes, reply decoding, module classes,
//! color to pilot conversion, and request signing (Spec 93 §2 to §7).
//!
//! Requests are written by hand rather than through a JSON map so the
//! bytes, and therefore signatures, are fixed regardless of how the
//! workspace configures `serde_json` key ordering.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// UDP port every WiZ device listens on.
pub const DEFAULT_PORT: u16 = 38899;
/// Lowest `dimming` most devices accept when `getModelConfig` is silent.
pub const DEFAULT_MIN_DIMMING: u8 = 10;
/// Length of a home signing key in bytes (32 hex characters).
pub const SIGNING_KEY_LEN: usize = 16;

/// Discovery request: a registration with `register` false, which asks
/// every device to answer without subscribing the host to pushes.
pub const DISCOVERY_REQUEST: &str = r#"{"method":"registration","params":{"phoneMac":"AAAAAAAAAAAA","register":false,"phoneIp":"1.2.3.4","id":"1"}}"#;
/// System configuration query: MAC, module name, firmware.
pub const GET_SYSTEM_CONFIG: &str = r#"{"method":"getSystemConfig","params":{}}"#;
/// Model configuration query (firmware 1.22 and later).
pub const GET_MODEL_CONFIG: &str = r#"{"method":"getModelConfig","params":{}}"#;
/// Current state query.
pub const GET_PILOT: &str = r#"{"method":"getPilot","params":{}}"#;

/// JSON-RPC style error codes devices return.
pub mod error_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const SECURITY_ERROR: i64 = -32605;
}

/// Errors decoding device replies or configuration.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    #[error("WiZ reply is not JSON: {0}")]
    Json(String),
    #[error("WiZ reply answers '{actual}', not '{expected}'")]
    UnexpectedMethod { expected: String, actual: String },
    #[error("WiZ device answered {method} with error {code}: {message}")]
    Device {
        method: String,
        code: i64,
        message: String,
    },
    #[error("WiZ reply to {0} carries no result")]
    MissingResult(String),
    #[error("WiZ signing key must be 32 hexadecimal characters")]
    SigningKey,
}

/// Envelope every reply shares.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<ReplyError>,
}

/// Error object inside a reply.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReplyError {
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

/// Parse a reply datagram.
///
/// # Errors
///
/// Returns an error when the datagram is not a JSON object.
pub fn decode_reply(datagram: &[u8]) -> Result<Reply, ProtocolError> {
    let trimmed = datagram
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(&datagram[..0], |end| &datagram[..=end]);
    serde_json::from_slice(trimmed).map_err(|error| ProtocolError::Json(error.to_string()))
}

/// Extract the typed result of a reply to `method`.
///
/// # Errors
///
/// Returns an error for a reply to another method, a device error, or a
/// result that does not match `T`.
pub fn reply_result<T: serde::de::DeserializeOwned>(
    reply: &Reply,
    method: &str,
) -> Result<T, ProtocolError> {
    if !reply.method.is_empty() && reply.method != method {
        return Err(ProtocolError::UnexpectedMethod {
            expected: method.to_owned(),
            actual: reply.method.clone(),
        });
    }
    if let Some(error) = &reply.error {
        return Err(ProtocolError::Device {
            method: method.to_owned(),
            code: error.code,
            message: error.message.clone(),
        });
    }
    let result = reply
        .result
        .clone()
        .ok_or_else(|| ProtocolError::MissingResult(method.to_owned()))?;
    serde_json::from_value(result).map_err(|error| ProtocolError::Json(error.to_string()))
}

/// Result of a discovery registration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RegistrationResult {
    pub mac: String,
    #[serde(default)]
    pub success: Option<bool>,
}

/// Result of `getSystemConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SystemConfig {
    pub mac: String,
    #[serde(default, rename = "moduleName")]
    pub module_name: Option<String>,
    #[serde(default, rename = "fwVersion")]
    pub fw_version: Option<String>,
    #[serde(default, rename = "homeId")]
    pub home_id: Option<i64>,
    #[serde(default, rename = "typeId")]
    pub type_id: Option<i64>,
}

/// The parts of `getModelConfig` the driver uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelConfig {
    #[serde(default, rename = "minDimLevel")]
    pub min_dim_level: Option<u8>,
    #[serde(default)]
    pub nowc: Option<u8>,
    #[serde(default, rename = "cctRange")]
    pub cct_range: Option<Vec<u32>>,
}

/// Light class decoded from the module name (Spec 93 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleClass {
    /// Color bulbs and strips.
    Rgb,
    /// Tunable white.
    TunableWhite,
    /// Dimmable white, including fan lights.
    DimmableWhite,
    /// Smart plugs: no light to drive.
    Socket,
}

impl ModuleClass {
    /// Classify a module name such as `ESP01_SHRGB1C_31`.
    ///
    /// The identifier is the second underscore-separated token; substrings
    /// are checked in a fixed order and the first match wins. A missing
    /// module name (very old firmware) is treated as dimmable white.
    #[must_use]
    pub fn from_module_name(module_name: Option<&str>) -> Self {
        let Some(name) = module_name.map(str::trim).filter(|name| !name.is_empty()) else {
            return Self::DimmableWhite;
        };
        let identifier = name.split('_').nth(1).unwrap_or(name).to_ascii_uppercase();
        if identifier.contains("RGB") {
            Self::Rgb
        } else if identifier.contains("DDTW") || identifier.contains("TW") {
            Self::TunableWhite
        } else if identifier.contains("SOCKET") {
            Self::Socket
        } else {
            Self::DimmableWhite
        }
    }

    /// Whether the device has a light Hypercolor can drive.
    #[must_use]
    pub const fn is_light(self) -> bool {
        !matches!(self, Self::Socket)
    }

    /// Whether hue and saturation reach the light.
    #[must_use]
    pub const fn color_capable(self) -> bool {
        matches!(self, Self::Rgb)
    }

    /// Name used in metadata and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rgb => "rgb",
            Self::TunableWhite => "tunable_white",
            Self::DimmableWhite => "dimmable_white",
            Self::Socket => "socket",
        }
    }
}

impl fmt::Display for ModuleClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One frame's worth of light state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pilot {
    /// Turn the light off.
    Off,
    /// Color at full chroma plus a dimming percentage.
    Color { r: u8, g: u8, b: u8, dimming: u8 },
    /// White light at a dimming percentage.
    White { dimming: u8 },
}

impl Pilot {
    /// Convert one engine color for a light of `class`.
    ///
    /// Color lights take the color normalized so its brightest channel is
    /// 255, with that channel's level as `dimming`: the device keeps hue
    /// and saturation in `r`, `g`, `b` and brightness in `dimming`. White
    /// lights take the color's luma as `dimming`. Levels that round to 0 %
    /// turn the light off; levels below `min_dimming` are raised to it,
    /// the dimmest the device renders.
    #[must_use]
    pub fn from_color([red, green, blue]: [u8; 3], class: ModuleClass, min_dimming: u8) -> Self {
        let floor = min_dimming.clamp(1, 100);
        if class.color_capable() {
            let peak = red.max(green).max(blue);
            let Some(dimming) = percent(f32::from(peak)) else {
                return Self::Off;
            };
            let scale = |channel: u8| {
                let scaled = (f32::from(channel) * 255.0 / f32::from(peak)).round();
                scaled.clamp(0.0, 255.0) as u8
            };
            Self::Color {
                r: scale(red),
                g: scale(green),
                b: scale(blue),
                dimming: dimming.max(floor),
            }
        } else {
            let luma =
                0.2126 * f32::from(red) + 0.7152 * f32::from(green) + 0.0722 * f32::from(blue);
            match percent(luma) {
                Some(dimming) => Self::White {
                    dimming: dimming.max(floor),
                },
                None => Self::Off,
            }
        }
    }

    /// `setPilot` parameters, without braces, with `state` true prepended
    /// when the light may be off.
    #[must_use]
    pub fn params(self, power_on: bool) -> String {
        let state = if power_on { "\"state\":true," } else { "" };
        match self {
            Self::Off => "\"state\":false".to_owned(),
            Self::Color { r, g, b, dimming } => {
                format!("{state}\"r\":{r},\"g\":{g},\"b\":{b},\"dimming\":{dimming}")
            }
            Self::White { dimming } => format!("{state}\"dimming\":{dimming}"),
        }
    }
}

fn percent(level: f32) -> Option<u8> {
    let pct = (level * 100.0 / 255.0).round().clamp(0.0, 100.0) as u8;
    (pct > 0).then_some(pct)
}

/// Home signing key exported from the WiZ app (Spec 93 §5).
#[derive(Clone, PartialEq, Eq)]
pub struct SigningKey([u8; SIGNING_KEY_LEN]);

impl SigningKey {
    /// Parse the 32-character hexadecimal key.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not 32 hexadecimal characters.
    pub fn parse(hex: &str) -> Result<Self, ProtocolError> {
        let hex = hex.trim();
        if hex.len() != SIGNING_KEY_LEN * 2 {
            return Err(ProtocolError::SigningKey);
        }
        let mut key = [0_u8; SIGNING_KEY_LEN];
        for (index, byte) in key.iter_mut().enumerate() {
            let pair = hex
                .get(index * 2..index * 2 + 2)
                .ok_or(ProtocolError::SigningKey)?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| ProtocolError::SigningKey)?;
        }
        Ok(Self(key))
    }

    /// Base64 HMAC-SHA256 of `message` under this key.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> String {
        STANDARD.encode(hmac_sha256(&self.0, message))
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SigningKey(redacted)")
    }
}

/// HMAC-SHA256 (RFC 2104) over SHA-256.
#[must_use]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(Sha256::digest(key).as_slice());
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.as_slice());
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(outer.finalize().as_slice());
    digest
}

/// Encode a `setPilot` request.
///
/// Unsigned: `{"method":"setPilot","params":{...}}`. Signed: `sigTs` is
/// appended inside `params`, the compact `params` object is signed, and the
/// request carries `env` and `hmac` as the device expects.
#[must_use]
pub fn encode_set_pilot(
    pilot: Pilot,
    power_on: bool,
    signing: Option<(&SigningKey, u64)>,
) -> String {
    let params = pilot.params(power_on);
    match signing {
        None => format!("{{\"method\":\"setPilot\",\"params\":{{{params}}}}}"),
        Some((key, timestamp)) => {
            let params = format!("{{{params},\"sigTs\":{timestamp}}}");
            let hmac = key.sign(params.as_bytes());
            format!(
                "{{\"method\":\"setPilot\",\"env\":\"pro\",\"params\":{params},\"hmac\":\"{hmac}\"}}"
            )
        }
    }
}

/// Lowercase, colon-separated MAC from the 12-character device form.
#[must_use]
pub fn canonical_mac(raw: &str) -> Option<String> {
    let hex: String = raw
        .chars()
        .filter(|character| !matches!(character, ':' | '-' | '.'))
        .collect();
    if hex.len() != 12 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    let lower = hex.to_ascii_lowercase();
    let octets: Vec<&str> = (0..6)
        .map(|index| &lower[index * 2..index * 2 + 2])
        .collect();
    Some(octets.join(":"))
}
