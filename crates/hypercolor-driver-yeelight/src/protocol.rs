//! Yeelight wire formats: the SSDP-style search, command lines, replies,
//! capabilities, and color to command conversion (Spec 94 §2 to §6).
//!
//! Commands are written by hand with a fixed key order so every line is
//! reproducible byte for byte.

use std::net::{IpAddr, SocketAddr};

use serde::Deserialize;
use thiserror::Error;

/// Multicast group and port the search goes to.
pub const SEARCH_MULTICAST: SocketAddr = SocketAddr::new(
    IpAddr::V4(std::net::Ipv4Addr::new(239, 255, 255, 250)),
    SEARCH_PORT,
);
/// UDP port devices answer searches on.
pub const SEARCH_PORT: u16 = 1982;
/// TCP control port in practice (the search reply's `Location` is
/// authoritative).
pub const CONTROL_PORT: u16 = 55443;
/// Longest line accepted from a device. The largest legitimate message is
/// a `props` notification or a `get_prop` result of a few hundred bytes.
pub const MAX_LINE_BYTES: usize = 4096;
/// Search request, every line ending CRLF.
pub const SEARCH_REQUEST: &str = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n";
/// Transition carried by `sudden` commands; devices ignore it but the
/// parameter is positional and must be at least 30 ms.
pub const SUDDEN_DURATION_MS: u32 = 30;

/// Errors decoding device messages.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    #[error("Yeelight search reply has no status line")]
    Empty,
    #[error("Yeelight search reply status '{0}' is not a search answer")]
    Status(String),
    #[error("Yeelight search reply lacks the {0} header")]
    MissingHeader(&'static str),
    #[error("Yeelight location '{0}' is not yeelight://ip:port")]
    Location(String),
    #[error("Yeelight reply is not JSON: {0}")]
    Json(String),
    #[error("Yeelight id '{0}' is not 0x followed by 1 to 16 hexadecimal digits")]
    InvalidId(String),
}

/// Longest `name` or `model` value kept from a search reply, in
/// characters. The specification caps device names at 64 bytes.
pub const MAX_TEXT_CHARS: usize = 64;

/// The canonical (lowercase) form of a device id, which the specification
/// defines as a 64-bit hexadecimal number written `0x...`.
///
/// # Errors
///
/// Returns [`ProtocolError::InvalidId`] for anything else.
pub fn canonical_id(raw: &str) -> Result<String, ProtocolError> {
    let invalid = || ProtocolError::InvalidId(raw.chars().take(24).collect());
    let digits = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .ok_or_else(invalid)?;
    if digits.is_empty()
        || digits.len() > 16
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    Ok(format!("0x{}", digits.to_ascii_lowercase()))
}

fn bounded_text(value: &str) -> Option<String> {
    let text: String = value.chars().take(MAX_TEXT_CHARS).collect();
    (!text.is_empty()).then_some(text)
}

/// One answer to the search (or a `NOTIFY` advertisement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchReply {
    /// TCP control endpoint from `Location`.
    pub location: SocketAddr,
    /// Device id, for example `0x000000000015243f`, lowercased.
    pub id: String,
    pub model: Option<String>,
    pub fw_ver: Option<String>,
    /// Every method the device accepts.
    pub support: Vec<String>,
    pub power: Option<String>,
    pub bright: Option<u8>,
    pub name: Option<String>,
}

/// Decode a search reply. Header names are matched case-insensitively and
/// split on the first colon; empty values (`Date:`) are fine.
///
/// # Errors
///
/// Returns an error when the status line, `Location`, or `id` is missing
/// or malformed.
pub fn decode_search_reply(datagram: &[u8]) -> Result<SearchReply, ProtocolError> {
    let text = String::from_utf8_lossy(datagram);
    let mut lines = text.split("\r\n").flat_map(|line| line.split('\n'));
    let status = lines
        .by_ref()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or(ProtocolError::Empty)?;
    if !(status.starts_with("HTTP/1.1 200") || status.starts_with("NOTIFY * HTTP/1.1")) {
        return Err(ProtocolError::Status(status.to_owned()));
    }
    let mut location = None;
    let mut reply = SearchReply {
        location: SocketAddr::from(([0, 0, 0, 0], CONTROL_PORT)),
        id: String::new(),
        model: None,
        fw_ver: None,
        support: Vec::new(),
        power: None,
        bright: None,
        name: None,
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let present = || bounded_text(value);
        match name.trim().to_ascii_lowercase().as_str() {
            "location" => location = Some(parse_location(value)?),
            "id" => reply.id = value.to_ascii_lowercase(),
            "model" => reply.model = present(),
            "fw_ver" => reply.fw_ver = present(),
            "support" => {
                reply.support = value.split_whitespace().map(ToOwned::to_owned).collect();
            }
            "power" => reply.power = present(),
            "bright" => reply.bright = value.parse().ok(),
            "name" => reply.name = present(),
            _ => {}
        }
    }
    reply.location = location.ok_or(ProtocolError::MissingHeader("Location"))?;
    if reply.id.is_empty() {
        return Err(ProtocolError::MissingHeader("id"));
    }
    reply.id = canonical_id(&reply.id)?;
    Ok(reply)
}

fn parse_location(raw: &str) -> Result<SocketAddr, ProtocolError> {
    raw.strip_prefix("yeelight://")
        .and_then(|rest| rest.trim_end_matches('/').parse::<SocketAddr>().ok())
        .ok_or_else(|| ProtocolError::Location(raw.to_owned()))
}

/// Encode a search reply. Only fake devices in tests answer searches, but
/// the encoder pins the layout beside the decoder.
#[must_use]
pub fn encode_search_reply(reply: &SearchReply) -> String {
    let optional = |value: &Option<String>| value.clone().unwrap_or_default();
    format!(
        "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nDate: \r\nExt: \r\nLocation: yeelight://{}\r\nServer: POSIX UPnP/1.0 YGLC/1\r\nid: {}\r\nmodel: {}\r\nfw_ver: {}\r\nsupport: {}\r\npower: {}\r\nbright: {}\r\nname: {}\r\n",
        reply.location,
        reply.id,
        optional(&reply.model),
        optional(&reply.fw_ver),
        reply.support.join(" "),
        optional(&reply.power),
        reply
            .bright
            .map(|bright| bright.to_string())
            .unwrap_or_default(),
        optional(&reply.name),
    )
}

/// What a device can do, from its `support` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// `set_rgb`: full color.
    pub color: bool,
    /// `set_scene`: color and brightness in one command, even while off.
    pub scene: bool,
    /// `set_bright`: brightness.
    pub brightness: bool,
    /// `set_music`: quota-free streaming.
    pub music: bool,
}

impl Capabilities {
    #[must_use]
    pub fn from_support(support: &[String]) -> Self {
        let has = |method: &str| support.iter().any(|known| known == method);
        Self {
            color: has("set_rgb"),
            scene: has("set_scene"),
            brightness: has("set_bright"),
            music: has("set_music"),
        }
    }

    /// Whether Hypercolor can drive the main light at all.
    #[must_use]
    pub const fn drivable(self) -> bool {
        self.color || self.brightness
    }

    /// Light class for metadata and display.
    #[must_use]
    pub const fn class_name(self) -> &'static str {
        if self.color { "color" } else { "white" }
    }
}

/// One frame's worth of light state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightState {
    Off,
    /// Packed `0xRRGGBB` at full chroma plus brightness 1 to 100.
    Color {
        rgb: u32,
        bright: u8,
    },
    /// Brightness 1 to 100 on a white light.
    White {
        bright: u8,
    },
}

impl LightState {
    /// Convert one engine color. Color lights get the color normalized so
    /// its brightest channel is 255, with that channel's level as
    /// brightness; white lights take Rec. 709 luma. Levels that round to 0 %
    /// turn the light off.
    #[must_use]
    pub fn from_color([red, green, blue]: [u8; 3], capabilities: Capabilities) -> Self {
        if capabilities.color {
            let peak = red.max(green).max(blue);
            let Some(bright) = percent(f32::from(peak)) else {
                return Self::Off;
            };
            let scale = |channel: u8| {
                let scaled = (f32::from(channel) * 255.0 / f32::from(peak)).round();
                u32::from(scaled.clamp(0.0, 255.0) as u8)
            };
            Self::Color {
                rgb: (scale(red) << 16) | (scale(green) << 8) | scale(blue),
                bright,
            }
        } else {
            let luma =
                0.2126 * f32::from(red) + 0.7152 * f32::from(green) + 0.0722 * f32::from(blue);
            percent(luma).map_or(Self::Off, |bright| Self::White { bright })
        }
    }
}

fn percent(level: f32) -> Option<u8> {
    let pct = (level * 100.0 / 255.0).round().clamp(0.0, 100.0) as u8;
    (pct > 0).then_some(pct)
}

/// One command, without its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    PowerOn,
    PowerOff,
    /// `set_scene ["color", rgb, bright]`: color and brightness at once,
    /// turning the light on first if needed.
    SceneColor {
        rgb: u32,
        bright: u8,
    },
    SetRgb {
        rgb: u32,
    },
    SetBright {
        bright: u8,
    },
    /// `set_music [1, host, port]`.
    MusicStart {
        host: IpAddr,
        port: u16,
    },
    /// `set_music [0]`.
    MusicStop,
}

impl Command {
    /// Method name and positional params as JSON text.
    #[must_use]
    pub fn method_and_params(&self) -> (&'static str, String) {
        let sudden = format!("\"sudden\",{SUDDEN_DURATION_MS}");
        match self {
            Self::PowerOn => ("set_power", format!("[\"on\",{sudden}]")),
            Self::PowerOff => ("set_power", format!("[\"off\",{sudden}]")),
            Self::SceneColor { rgb, bright } => {
                ("set_scene", format!("[\"color\",{rgb},{bright}]"))
            }
            Self::SetRgb { rgb } => ("set_rgb", format!("[{rgb},{sudden}]")),
            Self::SetBright { bright } => ("set_bright", format!("[{bright},{sudden}]")),
            Self::MusicStart { host, port } => ("set_music", format!("[1,\"{host}\",{port}]")),
            Self::MusicStop => ("set_music", "[0]".to_owned()),
        }
    }

    /// One command line: JSON then CRLF.
    #[must_use]
    pub fn encode(&self, id: u32) -> String {
        let (method, params) = self.method_and_params();
        format!("{{\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}\r\n")
    }
}

/// Commands that move a light from `previous` to `next`, or none when
/// nothing would change.
#[must_use]
pub fn commands_for(
    previous: Option<LightState>,
    next: LightState,
    capabilities: Capabilities,
) -> Vec<Command> {
    if previous == Some(next) {
        return Vec::new();
    }
    let was_on = matches!(
        previous,
        Some(LightState::Color { .. } | LightState::White { .. })
    );
    match next {
        LightState::Off => vec![Command::PowerOff],
        LightState::Color { rgb, bright } if capabilities.scene => {
            vec![Command::SceneColor { rgb, bright }]
        }
        LightState::Color { rgb, bright } => {
            let mut commands = Vec::with_capacity(3);
            if !was_on {
                commands.push(Command::PowerOn);
            }
            let same_rgb =
                matches!(previous, Some(LightState::Color { rgb: old, .. }) if old == rgb);
            let same_bright =
                matches!(previous, Some(LightState::Color { bright: old, .. }) if old == bright);
            if !same_rgb {
                commands.push(Command::SetRgb { rgb });
            }
            if !same_bright {
                commands.push(Command::SetBright { bright });
            }
            commands
        }
        LightState::White { bright } => {
            let mut commands = Vec::with_capacity(2);
            if !was_on {
                commands.push(Command::PowerOn);
            }
            commands.push(Command::SetBright { bright });
            commands
        }
    }
}

/// Commands that re-assert `state` on a light already showing it, used as
/// the music-mode keepalive.
#[must_use]
pub fn refresh_commands(state: LightState, capabilities: Capabilities) -> Vec<Command> {
    match state {
        LightState::Off => vec![Command::PowerOff],
        LightState::Color { rgb, bright } if capabilities.scene => {
            vec![Command::SceneColor { rgb, bright }]
        }
        LightState::Color { rgb, bright } => {
            vec![Command::SetRgb { rgb }, Command::SetBright { bright }]
        }
        LightState::White { bright } => vec![Command::SetBright { bright }],
    }
}

/// Most commands one frame can take for a device: the normal-mode pacing
/// unit (Spec 94 §6.1).
#[must_use]
pub const fn worst_case_commands(capabilities: Capabilities) -> u32 {
    if capabilities.color && !capabilities.scene {
        3
    } else if capabilities.color {
        1
    } else {
        2
    }
}

/// A line the device sent on a control connection.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DeviceMessage {
    #[serde(default)]
    pub id: Option<u32>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<DeviceError>,
}

/// Error object in a reply.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceError {
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

impl DeviceError {
    /// Whether the device ran out of its per-minute command quota.
    #[must_use]
    pub fn is_quota_exceeded(&self) -> bool {
        self.message.contains("quota")
    }
}

/// Decode one control line.
///
/// # Errors
///
/// Returns an error when the line is not a JSON object.
pub fn decode_message(line: &[u8]) -> Result<DeviceMessage, ProtocolError> {
    serde_json::from_slice(line.trim_ascii())
        .map_err(|error| ProtocolError::Json(error.to_string()))
}
