use crate::error::{OpenRgbError, Result};

/// RGBColor wire value. OpenRGB documents this as 32-bit `0x00BBGGRR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    /// Number of bytes in an OpenRGB RGBColor wire value.
    pub const WIRE_SIZE: usize = 4;

    /// Create an RGB color.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Decode from OpenRGB little-endian `0x00BBGGRR` bytes.
    #[must_use]
    pub const fn from_wire_bytes(bytes: [u8; 4]) -> Self {
        Self {
            red: bytes[0],
            green: bytes[1],
            blue: bytes[2],
        }
    }

    /// Encode to OpenRGB little-endian `0x00BBGGRR` bytes.
    #[must_use]
    pub const fn to_wire_bytes(self) -> [u8; 4] {
        [self.red, self.green, self.blue, 0]
    }
}

/// OpenRGB device type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceType {
    Motherboard,
    Dram,
    Gpu,
    Cooler,
    LedStrip,
    Keyboard,
    Mouse,
    Mousemat,
    Headset,
    HeadsetStand,
    Gamepad,
    Light,
    Speaker,
    Virtual,
    Storage,
    Case,
    Microphone,
    Accessory,
    Keypad,
    Laptop,
    Monitor,
    Unknown,
    Other(i32),
}

impl DeviceType {
    /// Convert an OpenRGB raw device type into a typed value.
    #[must_use]
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Motherboard,
            1 => Self::Dram,
            2 => Self::Gpu,
            3 => Self::Cooler,
            4 => Self::LedStrip,
            5 => Self::Keyboard,
            6 => Self::Mouse,
            7 => Self::Mousemat,
            8 => Self::Headset,
            9 => Self::HeadsetStand,
            10 => Self::Gamepad,
            11 => Self::Light,
            12 => Self::Speaker,
            13 => Self::Virtual,
            14 => Self::Storage,
            15 => Self::Case,
            16 => Self::Microphone,
            17 => Self::Accessory,
            18 => Self::Keypad,
            19 => Self::Laptop,
            20 => Self::Monitor,
            21 => Self::Unknown,
            other => Self::Other(other),
        }
    }
}

/// OpenRGB mode color mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorMode {
    None,
    PerLed,
    ModeSpecific,
    Random,
    Other(u32),
}

impl ColorMode {
    /// Convert an OpenRGB raw color mode into a typed value.
    #[must_use]
    pub const fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::None,
            1 => Self::PerLed,
            2 => Self::ModeSpecific,
            3 => Self::Random,
            other => Self::Other(other),
        }
    }

    /// Raw OpenRGB color mode value.
    #[must_use]
    pub const fn raw(self) -> u32 {
        match self {
            Self::None => 0,
            Self::PerLed => 1,
            Self::ModeSpecific => 2,
            Self::Random => 3,
            Self::Other(value) => value,
        }
    }
}

/// Public OpenRGB mode flag bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeFlag {
    Speed,
    LeftRightDirection,
    UpDownDirection,
    HorizontalVerticalDirection,
    Brightness,
    PerLedColor,
    ModeSpecificColor,
    RandomColor,
}

impl ModeFlag {
    /// Bit mask for this mode flag.
    #[must_use]
    pub const fn mask(self) -> u32 {
        match self {
            Self::Speed => 1 << 0,
            Self::LeftRightDirection => 1 << 1,
            Self::UpDownDirection => 1 << 2,
            Self::HorizontalVerticalDirection => 1 << 3,
            Self::Brightness => 1 << 4,
            Self::PerLedColor => 1 << 5,
            Self::ModeSpecificColor => 1 << 6,
            Self::RandomColor => 1 << 7,
        }
    }
}

/// Policy for selecting realtime writable modes from documented flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeFlagPolicy {
    pub per_led_color_mask: u32,
    pub persistent_mask: u32,
}

impl Default for ModeFlagPolicy {
    fn default() -> Self {
        Self {
            per_led_color_mask: ModeFlag::PerLedColor.mask(),
            persistent_mask: 0,
        }
    }
}

/// OpenRGB controller mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerMode {
    pub name: String,
    pub value: i32,
    pub flags: u32,
    pub speed_min: u32,
    pub speed_max: u32,
    pub brightness_min: Option<u32>,
    pub brightness_max: Option<u32>,
    pub colors_min: u32,
    pub colors_max: u32,
    pub speed: u32,
    pub brightness: Option<u32>,
    pub direction: u32,
    pub color_mode: ColorMode,
    pub colors: Vec<RgbColor>,
}

impl ControllerMode {
    /// Whether this mode is suitable for realtime per-LED writes under policy.
    #[must_use]
    pub fn is_realtime_writable(&self, policy: ModeFlagPolicy) -> bool {
        self.flags & policy.per_led_color_mask != 0
            && self.flags & policy.persistent_mask == 0
            && self.color_mode == ColorMode::PerLed
    }

    /// Encode this mode as an OpenRGB Mode Data block.
    ///
    /// # Errors
    ///
    /// Returns an error when string or color counts cannot fit the wire format.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        write_c_string(&mut bytes, &self.name)?;
        bytes.extend_from_slice(&self.value.to_le_bytes());
        bytes.extend_from_slice(&self.flags.to_le_bytes());
        bytes.extend_from_slice(&self.speed_min.to_le_bytes());
        bytes.extend_from_slice(&self.speed_max.to_le_bytes());
        if let (Some(min), Some(max)) = (self.brightness_min, self.brightness_max) {
            bytes.extend_from_slice(&min.to_le_bytes());
            bytes.extend_from_slice(&max.to_le_bytes());
        }
        bytes.extend_from_slice(&self.colors_min.to_le_bytes());
        bytes.extend_from_slice(&self.colors_max.to_le_bytes());
        bytes.extend_from_slice(&self.speed.to_le_bytes());
        if let Some(brightness) = self.brightness {
            bytes.extend_from_slice(&brightness.to_le_bytes());
        }
        bytes.extend_from_slice(&self.direction.to_le_bytes());
        bytes.extend_from_slice(&self.color_mode.raw().to_le_bytes());
        let color_count =
            u16::try_from(self.colors.len()).map_err(|_| OpenRgbError::CountOverflow {
                count: self.colors.len(),
                element_size: RgbColor::WIRE_SIZE,
            })?;
        bytes.extend_from_slice(&color_count.to_le_bytes());
        for color in &self.colors {
            bytes.extend_from_slice(&color.to_wire_bytes());
        }
        Ok(bytes)
    }
}

/// OpenRGB zone type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZoneType {
    Single,
    Linear,
    Matrix,
    Other(i32),
}

impl ZoneType {
    /// Convert an OpenRGB raw zone type into a typed value.
    #[must_use]
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Single,
            1 => Self::Linear,
            2 => Self::Matrix,
            other => Self::Other(other),
        }
    }
}

/// OpenRGB matrix mapping data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixMap {
    pub height: u32,
    pub width: u32,
    pub values: Vec<u32>,
}

/// OpenRGB zone segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentData {
    pub name: String,
    pub segment_type: ZoneType,
    pub start_index: u32,
    pub leds_count: u32,
}

/// OpenRGB controller zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerZone {
    pub name: String,
    pub zone_type: ZoneType,
    pub leds_min: u32,
    pub leds_max: u32,
    pub leds_count: u32,
    pub matrix: Option<MatrixMap>,
    pub segments: Vec<SegmentData>,
    pub flags: Option<u32>,
}

/// OpenRGB LED entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedData {
    pub name: String,
    pub value: u32,
}

/// OpenRGB controller data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerData {
    pub device_type: DeviceType,
    pub name: String,
    pub vendor: String,
    pub description: String,
    pub version: String,
    pub serial: String,
    pub location: String,
    pub active_mode: i32,
    pub modes: Vec<ControllerMode>,
    pub zones: Vec<ControllerZone>,
    pub leds: Vec<LedData>,
    pub colors: Vec<RgbColor>,
    pub led_alt_names: Vec<String>,
    pub flags: Option<u32>,
}

fn write_c_string(bytes: &mut Vec<u8>, value: &str) -> Result<()> {
    let len = value
        .len()
        .checked_add(1)
        .ok_or(OpenRgbError::CountOverflow {
            count: value.len(),
            element_size: 1,
        })?;
    let len_u16 = u16::try_from(len).map_err(|_| OpenRgbError::CountOverflow {
        count: len,
        element_size: 1,
    })?;
    bytes.extend_from_slice(&len_u16.to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    Ok(())
}
