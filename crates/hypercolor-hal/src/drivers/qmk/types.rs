//! QMK HID RGB protocol constants and type definitions.

/// Fixed HID report size for all QMK HID RGB packets.
pub const PACKET_SIZE: usize = 65;

/// HID read timeout in milliseconds.
pub const HID_READ_TIMEOUT_MS: u64 = 50;

/// QMK vendor-defined HID usage page.
pub const USAGE_PAGE: u16 = 0xFF60;

/// QMK vendor-defined HID usage ID.
pub const USAGE_ID: u16 = 0x0061;

// ── Command IDs ──────────────────────────────────────────────────────────

/// Protocol command identifiers sent as byte 1 of each HID report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    GetProtocolVersion = 0x01,
    GetQmkVersion = 0x02,
    GetDeviceInfo = 0x03,
    GetModeInfo = 0x04,
    GetLedInfo = 0x05,
    GetEnabledModes = 0x06,
    SetMode = 0x08,
    DirectModeSetSingleLed = 0x09,
    DirectModeSetLeds = 0x0A,
}

impl Command {
    /// Wire byte for this command.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub const fn byte(self) -> u8 {
        self as u8
    }
}

// ── Status codes ─────────────────────────────────────────────────────────

/// Response status sentinel values.
pub const STATUS_FAILURE: u8 = 25;
pub const STATUS_SUCCESS: u8 = 50;
pub const STATUS_END_OF_MESSAGE: u8 = 100;

// ── Protocol revisions ───────────────────────────────────────────────────

/// QMK HID RGB protocol revision negotiated during init.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRevision {
    /// Revision 9 — single-LED queries, 3 bytes/LED batch, max 20 LEDs.
    Rev9,
    /// Revision B — batch LED info (8/query), 3 bytes/LED batch, max 20 LEDs.
    RevB,
    /// Revision D — batch LED info (8/query), 4 bytes/LED batch (indexed),
    /// max 15 LEDs, underglow separation.
    RevD,
}

impl ProtocolRevision {
    /// Parse a protocol version byte from the device response.
    #[must_use]
    pub const fn from_version_byte(byte: u8) -> Option<Self> {
        match byte {
            0x09 => Some(Self::Rev9),
            0x0B | 0x0C => Some(Self::RevB),
            0x0D | 0x0E => Some(Self::RevD),
            _ => None,
        }
    }

    /// Maximum LEDs per batch-set packet.
    #[must_use]
    pub const fn max_leds_per_update(self) -> usize {
        match self {
            Self::Rev9 | Self::RevB => 20,
            Self::RevD => 15,
        }
    }

    /// Bytes consumed per LED in a `DIRECT_MODE_SET_LEDS` packet payload.
    #[must_use]
    pub const fn bytes_per_led(self) -> usize {
        match self {
            Self::Rev9 | Self::RevB => 3,
            Self::RevD => 4,
        }
    }

    /// LEDs queried per `GET_LED_INFO` batch request.
    #[must_use]
    pub const fn leds_per_info_query(self) -> usize {
        match self {
            Self::Rev9 => 1,
            Self::RevB | Self::RevD => 8,
        }
    }
}

// ── QMK RGB mode IDs ─────────────────────────────────────────────────────

/// Built-in QMK RGB lighting effect modes.
///
/// Mode 1 (`Direct`) is the one we use for per-LED control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)]
pub enum QmkMode {
    Direct = 1,
    SolidColor = 2,
    AlphaMod = 3,
    GradientUpDown = 4,
    GradientLeftRight = 5,
    Breathing = 6,
    BandSat = 7,
    BandVal = 8,
    BandPinwheelSat = 9,
    BandPinwheelVal = 10,
    BandSpiralSat = 11,
    BandSpiralVal = 12,
    CycleAll = 13,
    CycleLeftRight = 14,
    CycleUpDown = 15,
    CycleOutIn = 16,
    CycleOutInDual = 17,
    RainbowMovingChevron = 18,
    CyclePinwheel = 19,
    CycleSpiral = 20,
    DualBeacon = 21,
    RainbowBeacon = 22,
    RainbowPinwheels = 23,
    Raindrops = 24,
    JellybeanRaindrops = 25,
    HueBreathing = 26,
    HuePendulum = 27,
    HueWave = 28,
    TypingHeatmap = 29,
    DigitalRain = 30,
    SolidReactiveSimple = 31,
    SolidReactive = 32,
    SolidReactiveWide = 33,
    SolidReactiveMultiwide = 34,
    SolidReactiveCross = 35,
    SolidReactiveMulticross = 36,
    SolidReactiveNexus = 37,
    SolidReactiveMultinexus = 38,
    Splash = 39,
    Multisplash = 40,
    SolidSplash = 41,
    SolidMultisplash = 42,
    PixelRain = 43,
    PixelFlow = 44,
    PixelFractal = 45,
}

// ── Speed constants ──────────────────────────────────────────────────────

/// Default speed value (midpoint).
pub const SPEED_NORMAL: u8 = 0x7F;
