//! Twinkly wire formats: discovery datagrams, firmware versions, LED
//! profiles, and the real-time UDP frame protocol (Spec 92 §2 to §6).
//!
//! Everything here is pure encoding and decoding with no I/O, so every
//! byte layout is covered by golden tests.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use thiserror::Error;

/// UDP port devices listen on for the discovery datagram.
pub const DISCOVERY_PORT: u16 = 5555;
/// TCP port of the local REST API.
pub const HTTP_PORT: u16 = 80;
/// UDP port devices listen on for real-time frames.
pub const RT_PORT: u16 = 7777;

/// Discovery request: a version byte followed by the ASCII word `discover`.
pub const DISCOVERY_REQUEST: [u8; 9] = [0x01, b'd', b'i', b's', b'c', b'o', b'v', b'e', b'r'];

/// Length of the decoded authentication token carried in every frame.
pub const TOKEN_LEN: usize = 8;
/// Largest frame fragment a version 3 datagram carries.
pub const V3_FRAGMENT_PAYLOAD: usize = 900;
/// Largest LED count a version 1 header can describe.
pub const V1_MAX_LEDS: usize = u8::MAX as usize;

/// Errors decoding device replies.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    #[error("Twinkly discovery reply is {0} bytes, shorter than the 6-byte minimum")]
    DiscoveryTooShort(usize),
    #[error("Twinkly discovery reply lacks the OK marker")]
    DiscoveryMissingMarker,
    #[error("Twinkly firmware version '{0}' is not dotted numbers")]
    FirmwareVersion(String),
    #[error("Twinkly LED profile '{0}' is not RGB or RGBW")]
    LedProfile(String),
    #[error("Twinkly frame of {leds} LEDs exceeds the version 1 limit of 255")]
    V1TooManyLeds { leds: usize },
    #[error("Twinkly frame needs {fragments} fragments, more than one byte can number")]
    TooManyFragments { fragments: usize },
}

/// One reply to the discovery datagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryReply {
    /// Address the device reports for itself.
    pub ip: Ipv4Addr,
    /// Device id string, for example `Twinkly_33AE5C`.
    pub device_id: String,
}

/// Decode a discovery reply: four IP octets in reverse order, `OK`, the
/// device id, and a terminating zero byte.
///
/// # Errors
///
/// Returns an error when the datagram is too short or lacks the marker.
pub fn decode_discovery_reply(datagram: &[u8]) -> Result<DiscoveryReply, ProtocolError> {
    if datagram.len() < 6 {
        return Err(ProtocolError::DiscoveryTooShort(datagram.len()));
    }
    if &datagram[4..6] != b"OK" {
        return Err(ProtocolError::DiscoveryMissingMarker);
    }
    let ip = Ipv4Addr::new(datagram[3], datagram[2], datagram[1], datagram[0]);
    let id_bytes = &datagram[6..];
    let id_bytes = id_bytes
        .iter()
        .position(|byte| *byte == 0)
        .map_or(id_bytes, |end| &id_bytes[..end]);
    Ok(DiscoveryReply {
        ip,
        device_id: String::from_utf8_lossy(id_bytes).trim().to_owned(),
    })
}

/// Encode a discovery reply. Only the fake devices in tests answer
/// discovery, but keeping the encoder beside the decoder pins the layout.
#[must_use]
pub fn encode_discovery_reply(reply: &DiscoveryReply) -> Vec<u8> {
    let [a, b, c, d] = reply.ip.octets();
    let mut datagram = vec![d, c, b, a, b'O', b'K'];
    datagram.extend_from_slice(reply.device_id.as_bytes());
    datagram.push(0);
    datagram
}

/// Dotted firmware version, compared numerically part by part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FirmwareVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl FirmwareVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl FromStr for FirmwareVersion {
    type Err = ProtocolError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let error = || ProtocolError::FirmwareVersion(raw.to_owned());
        let mut parts = raw.trim().split('.');
        let mut next = |required: bool| -> Result<u16, ProtocolError> {
            match parts.next() {
                Some(part) => part.trim().parse::<u16>().map_err(|_| error()),
                None if required => Err(error()),
                None => Ok(0),
            }
        };
        let major = next(true)?;
        let minor = next(false)?;
        let patch = next(false)?;
        if parts.next().is_some() {
            return Err(error());
        }
        Ok(Self::new(major, minor, patch))
    }
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Bytes and channel order each LED takes in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LedProfile {
    /// Red, green, blue.
    Rgb,
    /// White first, then red, green, blue.
    Rgbw,
}

impl LedProfile {
    /// Parse the gestalt `led_profile` value.
    ///
    /// # Errors
    ///
    /// Returns an error for profiles other than RGB and RGBW.
    pub fn parse(raw: &str) -> Result<Self, ProtocolError> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "RGB" => Ok(Self::Rgb),
            "RGBW" => Ok(Self::Rgbw),
            _ => Err(ProtocolError::LedProfile(raw.to_owned())),
        }
    }

    /// Bytes one LED occupies.
    #[must_use]
    pub const fn bytes_per_led(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgbw => 4,
        }
    }

    /// Name used in metadata and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Rgbw => "RGBW",
        }
    }

    /// Append one LED in device channel order.
    ///
    /// RGBW devices get a zero white channel: the engine's color kernel
    /// does not extract white yet (`DevicePixelLayout::RgbwZeroWhite`).
    pub fn push_led(self, frame: &mut Vec<u8>, [red, green, blue]: [u8; 3]) {
        match self {
            Self::Rgb => frame.extend_from_slice(&[red, green, blue]),
            Self::Rgbw => frame.extend_from_slice(&[0, red, green, blue]),
        }
    }
}

impl fmt::Display for LedProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Real-time datagram format version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RtProtocol {
    /// Generation I: one datagram with an LED count byte.
    V1,
    /// Generation II through firmware 2.4.6: one datagram, zero byte.
    V2,
    /// Generation II from firmware 2.4.14: 900-byte numbered fragments.
    V3,
}

/// First firmware known to take version 3 datagrams.
pub const V3_MIN_FIRMWARE: FirmwareVersion = FirmwareVersion::new(2, 4, 14);
/// Firmware family of the first hardware generation.
pub const GEN1_FIRMWARE_FAMILY: &str = "D";

impl RtProtocol {
    /// Pick the datagram format for a device (Spec 92 §6.4).
    ///
    /// Generation I (firmware family "D", whose gestalt omits `fw_family`
    /// and whose firmware stops at 2.3.x) takes version 1. Generation II
    /// (families "F" and "G") takes version 2 before firmware 2.4.14 and
    /// version 3 from it. No firmware is known between 2.4.6 and 2.4.14;
    /// anything there takes version 2.
    #[must_use]
    pub fn for_device(fw_family: Option<&str>, firmware: FirmwareVersion) -> Self {
        let family = fw_family.map(str::trim).filter(|family| !family.is_empty());
        let generation_one = match family {
            Some(family) => family.eq_ignore_ascii_case(GEN1_FIRMWARE_FAMILY),
            None => firmware < V3_MIN_FIRMWARE,
        };
        if generation_one {
            Self::V1
        } else if firmware < V3_MIN_FIRMWARE {
            Self::V2
        } else {
            Self::V3
        }
    }

    /// Version byte that opens every datagram.
    #[must_use]
    pub const fn version_byte(self) -> u8 {
        match self {
            Self::V1 => 0x01,
            Self::V2 => 0x02,
            Self::V3 => 0x03,
        }
    }

    /// Name used in metadata and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2 => "v2",
            Self::V3 => "v3",
        }
    }

    /// Datagrams one frame of `frame_bytes` bytes takes.
    #[must_use]
    pub const fn datagrams_for(self, frame_bytes: usize) -> usize {
        match self {
            Self::V1 | Self::V2 => 1,
            Self::V3 => {
                if frame_bytes == 0 {
                    1
                } else {
                    frame_bytes.div_ceil(V3_FRAGMENT_PAYLOAD)
                }
            }
        }
    }
}

impl fmt::Display for RtProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Encodes engine frames into real-time datagrams, reusing its buffers.
#[derive(Debug, Clone)]
pub struct RtFrameEncoder {
    protocol: RtProtocol,
    profile: LedProfile,
    led_count: usize,
    token: [u8; TOKEN_LEN],
    frame: Vec<u8>,
    datagrams: Vec<Vec<u8>>,
}

impl RtFrameEncoder {
    /// Encoder for a device with `led_count` LEDs.
    ///
    /// # Errors
    ///
    /// Returns an error when the device cannot be described by the
    /// protocol: more than 255 LEDs on version 1, or more fragments than
    /// one byte can number on version 3.
    pub fn new(
        protocol: RtProtocol,
        profile: LedProfile,
        led_count: usize,
        token: [u8; TOKEN_LEN],
    ) -> Result<Self, ProtocolError> {
        let frame_bytes = led_count.saturating_mul(profile.bytes_per_led());
        match protocol {
            RtProtocol::V1 if led_count > V1_MAX_LEDS => {
                return Err(ProtocolError::V1TooManyLeds { leds: led_count });
            }
            RtProtocol::V3 if protocol.datagrams_for(frame_bytes) > usize::from(u8::MAX) + 1 => {
                return Err(ProtocolError::TooManyFragments {
                    fragments: protocol.datagrams_for(frame_bytes),
                });
            }
            _ => {}
        }
        Ok(Self {
            protocol,
            profile,
            led_count,
            token,
            frame: Vec::with_capacity(frame_bytes),
            datagrams: Vec::new(),
        })
    }

    /// Replace the token after a fresh login.
    pub const fn set_token(&mut self, token: [u8; TOKEN_LEN]) {
        self.token = token;
    }

    #[must_use]
    pub const fn protocol(&self) -> RtProtocol {
        self.protocol
    }

    #[must_use]
    pub const fn profile(&self) -> LedProfile {
        self.profile
    }

    #[must_use]
    pub const fn led_count(&self) -> usize {
        self.led_count
    }

    /// Datagrams one frame takes for this device.
    #[must_use]
    pub const fn datagrams_per_frame(&self) -> usize {
        self.protocol
            .datagrams_for(self.led_count.saturating_mul(self.profile.bytes_per_led()))
    }

    /// Encode one frame. Short input is padded with black and long input
    /// is truncated to the device's LED count.
    pub fn encode(&mut self, colors: &[[u8; 3]]) -> &[Vec<u8>] {
        self.frame.clear();
        for index in 0..self.led_count {
            let color = colors.get(index).copied().unwrap_or([0, 0, 0]);
            self.profile.push_led(&mut self.frame, color);
        }

        let header_len = match self.protocol {
            RtProtocol::V1 | RtProtocol::V2 => 1 + TOKEN_LEN + 1,
            RtProtocol::V3 => 1 + TOKEN_LEN + 3,
        };
        let chunks: Vec<&[u8]> = match self.protocol {
            RtProtocol::V1 | RtProtocol::V2 => vec![self.frame.as_slice()],
            RtProtocol::V3 if self.frame.is_empty() => vec![&[][..]],
            RtProtocol::V3 => self.frame.chunks(V3_FRAGMENT_PAYLOAD).collect(),
        };
        self.datagrams.resize_with(chunks.len(), Vec::new);
        for (index, (datagram, chunk)) in self.datagrams.iter_mut().zip(chunks).enumerate() {
            datagram.clear();
            datagram.reserve(header_len + chunk.len());
            datagram.push(self.protocol.version_byte());
            datagram.extend_from_slice(&self.token);
            match self.protocol {
                RtProtocol::V1 => {
                    datagram.push(u8::try_from(self.led_count).unwrap_or(u8::MAX));
                }
                RtProtocol::V2 => datagram.push(0),
                RtProtocol::V3 => {
                    datagram.extend_from_slice(&[0, 0]);
                    datagram.push(u8::try_from(index).unwrap_or(u8::MAX));
                }
            }
            datagram.extend_from_slice(chunk);
        }
        &self.datagrams
    }

    /// Datagrams from the last [`Self::encode`].
    #[must_use]
    pub fn datagrams(&self) -> &[Vec<u8>] {
        &self.datagrams
    }
}
