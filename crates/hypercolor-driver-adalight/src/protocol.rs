//! Adalight-family wire framing (Spec 91).
//!
//! Pure encoding, no I/O. Every frame is a six-byte header followed by the
//! LED payload. AWA frames add an optional four-byte white calibration
//! block and a three-byte Fletcher-style trailer. One [`FrameEncoder`] per
//! device writes the header once and rewrites only the payload and
//! trailer on each frame.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::color::ColorOrder;

/// Header length shared by every variant: magic (3), count (2), check (1).
pub const HEADER_LEN: usize = 6;

/// Classic Adalight magic, `"Ada"`.
pub const ADA_MAGIC: [u8; 3] = *b"Ada";

/// AWA magic for RGB frames, `"Awa"`.
pub const AWA_MAGIC: [u8; 3] = *b"Awa";

/// AWA magic for RGB frames followed by a white calibration block, `"AwA"`.
pub const AWA_CALIBRATED_MAGIC: [u8; 3] = *b"AwA";

/// AWA magic for frames carrying four bytes per LED, `"AWa"`.
pub const AWA_RGBW_MAGIC: [u8; 3] = *b"AWa";

/// Constant mixed into the header check byte.
pub const HEADER_CHECK_KEY: u8 = 0x55;

/// AWA trailer length: two Fletcher sums and the extended sum.
pub const AWA_TRAILER_LEN: usize = 3;

/// AWA white calibration block length: gain, red, green, blue.
pub const AWA_CALIBRATION_LEN: usize = 4;

/// Extended-sum value the trailer never sends, because it is `'A'`.
pub const AWA_EXT_FORBIDDEN: u8 = 0x41;

/// Value sent in place of [`AWA_EXT_FORBIDDEN`].
pub const AWA_EXT_SUBSTITUTE: u8 = 0xAA;

/// Most LEDs the 16-bit count field can describe (it carries count - 1).
pub const ADA_MAX_LED_COUNT: u32 = 65_536;

/// Most LEDs HyperSerial firmware renders; it drops larger frames.
pub const AWA_MAX_LED_COUNT: u32 = 4_096;

/// Line bits per byte with 8N1 framing: start bit, eight data bits, stop bit.
pub const BITS_PER_BYTE: u64 = 10;

/// Wire protocol variant chosen in config.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum AdalightProtocol {
    /// Classic Adalight: `"Ada"` header and raw RGB payload.
    #[default]
    Ada,
    /// HyperSerial AWA: `"Awa"` family header, payload, Fletcher trailer.
    Awa,
}

impl AdalightProtocol {
    /// Every supported variant, in display order.
    pub const ALL: [Self; 2] = [Self::Ada, Self::Awa];

    /// Stable lowercase identifier used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ada => "ada",
            Self::Awa => "awa",
        }
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ada => "Adalight",
            Self::Awa => "AWA (HyperSerial)",
        }
    }

    /// Most LEDs one frame of this variant may carry.
    #[must_use]
    pub const fn max_led_count(self) -> u32 {
        match self {
            Self::Ada => ADA_MAX_LED_COUNT,
            Self::Awa => AWA_MAX_LED_COUNT,
        }
    }

    /// Parse a variant name, accepting common spellings in any case.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ada" | "adalight" => Some(Self::Ada),
            "awa" | "hyperserial" => Some(Self::Awa),
            _ => None,
        }
    }
}

impl fmt::Display for AdalightProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<String> for AdalightProtocol {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
            .ok_or_else(|| format!("unknown protocol '{value}' (expected ada or awa)"))
    }
}

impl From<AdalightProtocol> for String {
    fn from(value: AdalightProtocol) -> Self {
        value.as_str().to_owned()
    }
}

/// White calibration that HyperSerial RGBW builds apply when they derive
/// the white channel themselves. Omitted fields take the cold-white
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct WhiteCalibration {
    /// Scale applied to the derived white channel, 0-255.
    pub gain: u8,
    /// Red correction used while deriving white, 0-255.
    pub red: u8,
    /// Green correction used while deriving white, 0-255.
    pub green: u8,
    /// Blue correction used while deriving white, 0-255.
    pub blue: u8,
}

impl WhiteCalibration {
    /// HyperSerial's own default for cold-white strips.
    pub const COLD_WHITE: Self = Self {
        gain: 0xFF,
        red: 0xA0,
        green: 0xA0,
        blue: 0xA0,
    };

    /// Calibration block in wire order.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; AWA_CALIBRATION_LEN] {
        [self.gain, self.red, self.green, self.blue]
    }
}

impl Default for WhiteCalibration {
    fn default() -> Self {
        Self::COLD_WHITE
    }
}

/// How an AWA frame handles the white channel of RGBW strips.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AwaWhite {
    /// Plain RGB frames (`"Awa"`).
    #[default]
    Off,
    /// RGB frames plus a calibration block (`"AwA"`); the firmware derives
    /// white from RGB using these values.
    Firmware(WhiteCalibration),
    /// Four bytes per LED (`"AWa"`); Hypercolor derives white and sends it
    /// after the three color bytes.
    Host,
}

/// Header, payload shape, and trailer of one variant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Framing {
    /// Classic Adalight.
    #[default]
    Ada,
    /// HyperSerial AWA.
    Awa(AwaWhite),
}

impl Framing {
    /// Wire protocol this framing belongs to.
    #[must_use]
    pub const fn protocol(self) -> AdalightProtocol {
        match self {
            Self::Ada => AdalightProtocol::Ada,
            Self::Awa(_) => AdalightProtocol::Awa,
        }
    }

    /// Magic bytes that open every frame.
    #[must_use]
    pub const fn magic(self) -> [u8; 3] {
        match self {
            Self::Ada => ADA_MAGIC,
            Self::Awa(AwaWhite::Off) => AWA_MAGIC,
            Self::Awa(AwaWhite::Firmware(_)) => AWA_CALIBRATED_MAGIC,
            Self::Awa(AwaWhite::Host) => AWA_RGBW_MAGIC,
        }
    }

    /// Payload bytes per LED.
    #[must_use]
    pub const fn bytes_per_led(self) -> usize {
        match self {
            Self::Awa(AwaWhite::Host) => 4,
            Self::Ada | Self::Awa(_) => 3,
        }
    }

    /// Bytes after the payload: calibration block and trailer.
    #[must_use]
    pub const fn suffix_len(self) -> usize {
        match self {
            Self::Ada => 0,
            Self::Awa(AwaWhite::Firmware(_)) => AWA_CALIBRATION_LEN + AWA_TRAILER_LEN,
            Self::Awa(_) => AWA_TRAILER_LEN,
        }
    }
}

/// Why a frame layout cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameLayoutError {
    /// The LED count is zero or above the variant's limit.
    #[error("LED count {led_count} is outside 1-{max} for {protocol}")]
    LedCount {
        /// Requested LED count.
        led_count: u32,
        /// Variant limit.
        max: u32,
        /// Variant.
        protocol: AdalightProtocol,
    },
}

/// Validated frame shape for one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameLayout {
    framing: Framing,
    led_count: u32,
}

impl FrameLayout {
    /// Validate a frame shape.
    ///
    /// # Errors
    ///
    /// Returns [`FrameLayoutError::LedCount`] when `led_count` is zero or
    /// above the variant's limit.
    pub const fn new(framing: Framing, led_count: u32) -> Result<Self, FrameLayoutError> {
        let max = framing.protocol().max_led_count();
        if led_count == 0 || led_count > max {
            return Err(FrameLayoutError::LedCount {
                led_count,
                max,
                protocol: framing.protocol(),
            });
        }
        Ok(Self { framing, led_count })
    }

    /// Variant framing.
    #[must_use]
    pub const fn framing(&self) -> Framing {
        self.framing
    }

    /// LEDs per frame.
    #[must_use]
    pub const fn led_count(&self) -> u32 {
        self.led_count
    }

    /// Value of the 16-bit count field: LED count minus one.
    #[must_use]
    pub const fn count_field(&self) -> u16 {
        // `new` bounds led_count to 1..=65_536, so this always fits.
        (self.led_count - 1) as u16
    }

    /// Six-byte header.
    #[must_use]
    pub const fn header(&self) -> [u8; HEADER_LEN] {
        header(self.framing.magic(), self.count_field())
    }

    /// Payload length in bytes.
    #[must_use]
    pub const fn payload_len(&self) -> usize {
        self.led_count as usize * self.framing.bytes_per_led()
    }

    /// Total bytes per frame.
    #[must_use]
    pub const fn frame_len(&self) -> usize {
        HEADER_LEN + self.payload_len() + self.framing.suffix_len()
    }

    /// Serial line budget of this layout at `baud_rate`.
    #[must_use]
    pub const fn bandwidth(&self, baud_rate: u32) -> Bandwidth {
        Bandwidth::new(self.frame_len(), baud_rate)
    }
}

/// Build a six-byte header from magic bytes and the raw count field.
///
/// `count_field` is the LED count minus one, sent big-endian. The check
/// byte is `hi ^ lo ^ 0x55`.
#[must_use]
pub const fn header(magic: [u8; 3], count_field: u16) -> [u8; HEADER_LEN] {
    let [hi, lo] = count_field.to_be_bytes();
    [
        magic[0],
        magic[1],
        magic[2],
        hi,
        lo,
        hi ^ lo ^ HEADER_CHECK_KEY,
    ]
}

/// AWA trailer over every byte between the header and the trailer.
///
/// Three accumulators start at zero and are reduced modulo 255 after each
/// byte: `s1 += byte`, then `s2 += s1`, then `ext += byte ^ position`,
/// where `position` is the byte's offset truncated to eight bits. The
/// trailer is `s1, s2, ext`, except that an `ext` of `0x41` is sent as
/// `0xAA`.
#[must_use]
pub fn awa_trailer(data: &[u8]) -> [u8; AWA_TRAILER_LEN] {
    let mut sum1: u16 = 0;
    let mut sum2: u16 = 0;
    let mut ext: u16 = 0;
    for (index, &byte) in data.iter().enumerate() {
        let position = (index & 0xFF) as u8;
        sum1 = (sum1 + u16::from(byte)) % 255;
        sum2 = (sum2 + sum1) % 255;
        ext = (ext + u16::from(byte ^ position)) % 255;
    }
    let ext = if ext as u8 == AWA_EXT_FORBIDDEN {
        AWA_EXT_SUBSTITUTE
    } else {
        ext as u8
    };
    [sum1 as u8, sum2 as u8, ext]
}

/// Split RGB into RGB plus white: white is the shared minimum, which is
/// then removed from each color channel.
#[must_use]
pub const fn extract_white(rgb: [u8; 3]) -> ([u8; 3], u8) {
    let [red, green, blue] = rgb;
    let mut white = red;
    if green < white {
        white = green;
    }
    if blue < white {
        white = blue;
    }
    ([red - white, green - white, blue - white], white)
}

/// What a serial line can carry for one frame shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bandwidth {
    /// Bytes per frame.
    pub frame_bytes: usize,
    /// Line rate in bits per second.
    pub baud_rate: u32,
}

impl Bandwidth {
    /// Budget for `frame_bytes`-byte frames at `baud_rate`.
    #[must_use]
    pub const fn new(frame_bytes: usize, baud_rate: u32) -> Self {
        Self {
            frame_bytes,
            baud_rate,
        }
    }

    /// Line bits one frame occupies with 8N1 framing.
    #[must_use]
    pub const fn frame_bits(&self) -> u64 {
        self.frame_bytes as u64 * BITS_PER_BYTE
    }

    /// Time one frame occupies the line, rounded up to the microsecond.
    #[must_use]
    pub const fn frame_time(&self) -> Duration {
        if self.baud_rate == 0 {
            return Duration::MAX;
        }
        Duration::from_micros((self.frame_bits() * 1_000_000).div_ceil(self.baud_rate as u64))
    }

    /// Whole frames per second the line carries back to back.
    #[must_use]
    pub const fn max_fps(&self) -> u32 {
        let bits = self.frame_bits();
        if bits == 0 {
            return u32::MAX;
        }
        let fps = self.baud_rate as u64 / bits;
        if fps > u32::MAX as u64 {
            u32::MAX
        } else {
            fps as u32
        }
    }
}

/// Reusable frame buffer for one device.
#[derive(Debug, Clone)]
pub struct FrameEncoder {
    layout: FrameLayout,
    color_order: ColorOrder,
    buffer: Vec<u8>,
}

impl FrameEncoder {
    /// Build an encoder for `layout` that writes colors in `color_order`.
    ///
    /// The buffer starts as a valid all-black frame.
    #[must_use]
    pub fn new(layout: FrameLayout, color_order: ColorOrder) -> Self {
        let mut buffer = vec![0; layout.frame_len()];
        buffer[..HEADER_LEN].copy_from_slice(&layout.header());
        if let Framing::Awa(AwaWhite::Firmware(calibration)) = layout.framing() {
            let start = HEADER_LEN + layout.payload_len();
            buffer[start..start + AWA_CALIBRATION_LEN].copy_from_slice(&calibration.to_bytes());
        }
        let mut encoder = Self {
            layout,
            color_order,
            buffer,
        };
        encoder.seal();
        encoder
    }

    /// Frame layout.
    #[must_use]
    pub const fn layout(&self) -> FrameLayout {
        self.layout
    }

    /// Encode `colors` and return the frame bytes.
    ///
    /// The frame always carries the configured LED count, because the
    /// firmware sizes its read from the header: missing LEDs are sent
    /// black and extra input is ignored.
    pub fn encode(&mut self, colors: &[[u8; 3]]) -> &[u8] {
        let bytes_per_led = self.layout.framing().bytes_per_led();
        let host_white = matches!(self.layout.framing(), Framing::Awa(AwaWhite::Host));
        let payload_end = HEADER_LEN + self.layout.payload_len();
        let payload = &mut self.buffer[HEADER_LEN..payload_end];
        for (index, led) in payload.chunks_exact_mut(bytes_per_led).enumerate() {
            let rgb = colors.get(index).copied().unwrap_or_default();
            if host_white {
                let (color, white) = extract_white(rgb);
                led[..3].copy_from_slice(&self.color_order.apply(color));
                led[3] = white;
            } else {
                led.copy_from_slice(&self.color_order.apply(rgb));
            }
        }
        self.seal();
        &self.buffer
    }

    /// Encode an all-black frame and return its bytes.
    pub fn encode_black(&mut self) -> &[u8] {
        self.encode(&[])
    }

    /// Frame bytes as last encoded.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer
    }

    fn seal(&mut self) {
        if !matches!(self.layout.framing(), Framing::Awa(_)) {
            return;
        }
        let trailer_start = self.buffer.len() - AWA_TRAILER_LEN;
        let trailer = awa_trailer(&self.buffer[HEADER_LEN..trailer_start]);
        self.buffer[trailer_start..].copy_from_slice(&trailer);
    }
}
