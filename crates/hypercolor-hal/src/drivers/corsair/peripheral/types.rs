//! Shared Corsair peripheral descriptor types.

use std::time::Duration;

use hypercolor_types::device::DeviceTopologyHint;

pub const BRAGI_MAGIC: u8 = 0x08;
pub const BRAGI_PACKET_SIZE: usize = 64;
pub const BRAGI_LARGE_PACKET_SIZE: usize = 128;
pub const BRAGI_JUMBO_PACKET_SIZE: usize = 1_024;
pub const BRAGI_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
pub const BRAGI_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(50);
pub const BRAGI_KEYBOARD_FRAME_INTERVAL: Duration = Duration::from_millis(33);
pub const BRAGI_POINTER_FRAME_INTERVAL: Duration = Duration::from_millis(22);
pub const NXP_PACKET_SIZE: usize = 64;
pub const NXP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
pub const NXP_KEYBOARD_FRAME_INTERVAL: Duration = Duration::from_millis(33);
pub const NXP_POINTER_FRAME_INTERVAL: Duration = Duration::from_millis(22);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsairPeripheralClass {
    Keyboard,
    Mouse,
    Mousepad,
    HeadsetStand,
    Dongle,
}

impl CorsairPeripheralClass {
    #[must_use]
    pub const fn zone_name(self) -> &'static str {
        match self {
            Self::Keyboard => "Keyboard",
            Self::Mouse => "Mouse",
            Self::Mousepad => "Mousepad",
            Self::HeadsetStand => "Headset Stand",
            Self::Dongle => "Receiver",
        }
    }

    #[must_use]
    pub const fn default_max_fps(self) -> u32 {
        match self {
            Self::Mouse | Self::Mousepad => 45,
            _ => 30,
        }
    }

    #[must_use]
    pub const fn default_frame_interval(self) -> Duration {
        match self {
            Self::Mouse | Self::Mousepad => BRAGI_POINTER_FRAME_INTERVAL,
            _ => BRAGI_KEYBOARD_FRAME_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragiLightingFormat {
    RgbPlanar,
    Monochrome,
    AlternateRgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragiCommand {
    Set = 0x01,
    Get = 0x02,
    CloseHandle = 0x05,
    WriteData = 0x06,
    ContinueWrite = 0x07,
    ReadData = 0x08,
    ProbeHandle = 0x09,
    OpenHandle = 0x0D,
    Poll = 0x12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragiProperty {
    Brightness = 0x02,
    Mode = 0x03,
    AppVersion = 0x13,
    BootloaderVersion = 0x14,
    BrightnessCoarse = 0x44,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragiLightingMode {
    Hardware = 0x01,
    Software = 0x02,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragiResource {
    Lighting = 0x0001,
    LightingMonochrome = 0x0010,
    AlternateLighting = 0x0022,
    LightingExtra = 0x002E,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragiHandle {
    Lighting = 0x00,
    Generic = 0x01,
    SecondLighting = 0x02,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsairPeripheralTopology {
    None,
    KeyboardMatrix { rows: u32, cols: u32 },
    Strip,
    Point,
}

impl CorsairPeripheralTopology {
    #[must_use]
    pub const fn hint(self) -> DeviceTopologyHint {
        match self {
            Self::None | Self::Strip => DeviceTopologyHint::Strip,
            Self::KeyboardMatrix { rows, cols } => DeviceTopologyHint::Matrix { rows, cols },
            Self::Point => DeviceTopologyHint::Point,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BragiDeviceConfig {
    pub name: &'static str,
    pub class: CorsairPeripheralClass,
    pub packet_size: usize,
    pub led_count: usize,
    pub topology: CorsairPeripheralTopology,
    pub lighting_format: BragiLightingFormat,
    pub max_fps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxpCommand {
    Set = 0x07,
    Get = 0x0E,
    WriteBulk = 0x7F,
    ReadBulk = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxpField {
    Ident = 0x01,
    Reset = 0x02,
    Special = 0x04,
    Lighting = 0x05,
    PollRate = 0x0A,
    FirmwareStart = 0x0C,
    FirmwareData = 0x0D,
    Mouse = 0x13,
    KeyboardHardwareColor = 0x14,
    MouseProfileId = 0x15,
    MouseProfileName = 0x16,
    KeyboardHardwareAnimation = 0x17,
    MouseColor = 0x22,
    KeyboardZoneColor = 0x25,
    KeyboardPackedColor = 0x27,
    KeyboardColor = 0x28,
    KeyInput = 0x40,
    Battery = 0x50,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxpLightingMode {
    Hardware = 0x01,
    Software = 0x02,
    Sidelight = 0x08,
    WinLock = 0x09,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxpColorSelector {
    Red = 0x01,
    Green = 0x02,
    Blue = 0x03,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxpLightingKind {
    FullRangeKeyboard,
    Packed512Keyboard,
    ZonedKeyboard,
    MonochromeKeyboard,
    Mouse,
    Mousepad,
    NoLights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NxpDeviceConfig {
    pub name: &'static str,
    pub class: CorsairPeripheralClass,
    pub lighting_kind: NxpLightingKind,
    pub led_count: usize,
    pub topology: CorsairPeripheralTopology,
    pub max_fps: u32,
    pub requires_unclean_exit: bool,
}

impl NxpDeviceConfig {
    #[must_use]
    pub const fn new(
        name: &'static str,
        class: CorsairPeripheralClass,
        lighting_kind: NxpLightingKind,
        led_count: usize,
        topology: CorsairPeripheralTopology,
    ) -> Self {
        Self {
            name,
            class,
            lighting_kind,
            led_count,
            topology,
            max_fps: class.default_max_fps(),
            requires_unclean_exit: false,
        }
    }

    #[must_use]
    pub const fn with_max_fps(mut self, max_fps: u32) -> Self {
        self.max_fps = max_fps;
        self
    }

    #[must_use]
    pub const fn with_unclean_exit(mut self) -> Self {
        self.requires_unclean_exit = true;
        self
    }

    #[must_use]
    pub const fn supports_direct(self) -> bool {
        !matches!(self.lighting_kind, NxpLightingKind::NoLights) && self.led_count > 0
    }

    #[must_use]
    pub const fn frame_interval(self) -> Duration {
        match self.class {
            CorsairPeripheralClass::Mouse => NXP_POINTER_FRAME_INTERVAL,
            _ => NXP_KEYBOARD_FRAME_INTERVAL,
        }
    }
}

impl BragiDeviceConfig {
    #[must_use]
    pub const fn new(
        name: &'static str,
        class: CorsairPeripheralClass,
        packet_size: usize,
        led_count: usize,
        topology: CorsairPeripheralTopology,
    ) -> Self {
        Self {
            name,
            class,
            packet_size,
            led_count,
            topology,
            lighting_format: BragiLightingFormat::RgbPlanar,
            max_fps: class.default_max_fps(),
        }
    }

    #[must_use]
    pub const fn monochrome(mut self) -> Self {
        self.lighting_format = BragiLightingFormat::Monochrome;
        self
    }

    #[must_use]
    pub const fn alternate_rgb(mut self) -> Self {
        self.lighting_format = BragiLightingFormat::AlternateRgb;
        self
    }

    #[must_use]
    pub const fn with_max_fps(mut self, max_fps: u32) -> Self {
        self.max_fps = max_fps;
        self
    }

    #[must_use]
    pub const fn resource(self) -> BragiResource {
        match self.lighting_format {
            BragiLightingFormat::RgbPlanar => BragiResource::Lighting,
            BragiLightingFormat::Monochrome => BragiResource::LightingMonochrome,
            BragiLightingFormat::AlternateRgb => BragiResource::AlternateLighting,
        }
    }
}
