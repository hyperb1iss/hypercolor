//! Shared Corsair protocol enums and endpoint definitions.

use hypercolor_types::device::DeviceTopologyHint;

/// Wire-level command bytes for the Corsair iCUE LINK hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkCommand {
    /// Open a data endpoint for reading.
    OpenEndpoint,
    /// Open the color-write endpoint.
    OpenColorEndpoint,
    /// Close the current endpoint.
    CloseEndpoint,
    /// Query firmware version.
    GetFirmware,
    /// Enter software control mode.
    SoftwareMode,
    /// Return control to hardware.
    HardwareMode,
    /// Write first chunk of standard data.
    Write,
    /// Write first chunk of color data.
    WriteColor,
    /// Write subsequent color chunks.
    WriteColorNext,
    /// Read from the current endpoint.
    Read,
    /// Query current device mode.
    GetDeviceMode,
}

impl LinkCommand {
    /// Static wire bytes for this command.
    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Self::OpenEndpoint => &[0x0D, 0x01],
            Self::OpenColorEndpoint => &[0x0D, 0x00],
            Self::CloseEndpoint => &[0x05, 0x01, 0x01],
            Self::GetFirmware => &[0x02, 0x13],
            Self::SoftwareMode => &[0x01, 0x03, 0x00, 0x02],
            Self::HardwareMode => &[0x01, 0x03, 0x00, 0x01],
            Self::Write => &[0x06, 0x01],
            Self::WriteColor => &[0x06, 0x00],
            Self::WriteColorNext => &[0x07, 0x00],
            Self::Read => &[0x08, 0x01],
            Self::GetDeviceMode => &[0x01, 0x08, 0x01],
        }
    }
}

/// LINK Hub endpoint addresses and typed payload markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointConfig {
    /// Endpoint address appended to open/close commands.
    pub address: u8,
    /// Typed payload marker used in LINK framed writes/responses.
    pub data_type: [u8; 2],
}

/// LINK endpoint used to enumerate downstream devices.
pub const EP_GET_DEVICES: EndpointConfig = EndpointConfig {
    address: 0x36,
    data_type: [0x21, 0x00],
};

/// LINK endpoint used to write color data.
pub const EP_SET_COLOR: EndpointConfig = EndpointConfig {
    address: 0x22,
    data_type: [0x12, 0x00],
};

/// Device type byte reported by a LINK hub during child enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkDeviceType {
    QxRgbFan,
    LxRgbFan,
    RxRgbMaxFan,
    RxMaxFan,
    CaseAdapter,
    CoolerPumpLcd,
    HSeriesAio,
    Xc7Elite,
    Xg3Hybrid,
    Xd5Elite,
    Xg7Rgb,
    Xd5EliteLcd,
    RxRgbFan,
    VrmCoolerModule,
    TitanAio,
    RxFan,
    OriginOa,
    Xd6Elite,
}

impl LinkDeviceType {
    /// Convert a raw type byte into a known LINK device type.
    #[must_use]
    pub const fn from_byte(raw: u8) -> Option<Self> {
        match raw {
            0x01 => Some(Self::QxRgbFan),
            0x02 => Some(Self::LxRgbFan),
            0x03 => Some(Self::RxRgbMaxFan),
            0x04 => Some(Self::RxMaxFan),
            0x05 => Some(Self::CaseAdapter),
            0x06 => Some(Self::CoolerPumpLcd),
            0x07 => Some(Self::HSeriesAio),
            0x09 => Some(Self::Xc7Elite),
            0x0A => Some(Self::Xg3Hybrid),
            0x0C => Some(Self::Xd5Elite),
            0x0D => Some(Self::Xg7Rgb),
            0x0E => Some(Self::Xd5EliteLcd),
            0x0F => Some(Self::RxRgbFan),
            0x10 => Some(Self::VrmCoolerModule),
            0x11 => Some(Self::TitanAio),
            0x13 => Some(Self::RxFan),
            0x14 => Some(Self::OriginOa),
            0x19 => Some(Self::Xd6Elite),
            _ => None,
        }
    }

    /// Human-readable model name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::QxRgbFan => "iCUE LINK QX RGB",
            Self::LxRgbFan => "iCUE LINK LX RGB",
            Self::RxRgbMaxFan => "iCUE LINK RX RGB MAX",
            Self::RxMaxFan => "iCUE LINK RX MAX",
            Self::CaseAdapter => "iCUE LINK Case Adapter",
            Self::CoolerPumpLcd => "iCUE LINK Cooler Pump LCD",
            Self::HSeriesAio => "iCUE LINK H-Series AIO",
            Self::Xc7Elite => "iCUE LINK XC7 Elite",
            Self::Xg3Hybrid => "iCUE LINK XG3 Hybrid",
            Self::Xd5Elite => "iCUE LINK XD5 Elite",
            Self::Xg7Rgb => "iCUE LINK XG7 RGB",
            Self::Xd5EliteLcd => "iCUE LINK XD5 Elite LCD",
            Self::RxRgbFan => "iCUE LINK RX RGB",
            Self::VrmCoolerModule => "iCUE LINK VRM Cooler Module",
            Self::TitanAio => "iCUE LINK Titan AIO",
            Self::RxFan => "iCUE LINK RX",
            Self::OriginOa => "Origin OA",
            Self::Xd6Elite => "iCUE LINK XD6 Elite",
        }
    }

    /// Number of LEDs driven by this device type.
    #[must_use]
    pub const fn led_count(self, model: u8) -> u32 {
        match self {
            Self::QxRgbFan => 34,
            Self::LxRgbFan => 18,
            Self::RxRgbMaxFan | Self::RxRgbFan => 8,
            Self::CaseAdapter => match model {
                0x01 => 22,
                0x02 => 160,
                _ => 0,
            },
            Self::CoolerPumpLcd => 24,
            Self::HSeriesAio | Self::TitanAio => 20,
            Self::Xc7Elite => 24,
            Self::Xd5Elite | Self::Xd6Elite => 22,
            Self::Xg7Rgb => 16,
            Self::RxMaxFan
            | Self::Xg3Hybrid
            | Self::Xd5EliteLcd
            | Self::VrmCoolerModule
            | Self::RxFan
            | Self::OriginOa => 0,
        }
    }

    /// Whether this child should be hidden from direct RGB enumeration.
    #[must_use]
    pub const fn is_internal(self) -> bool {
        matches!(self, Self::Xd5EliteLcd)
    }

    /// Best-effort topology hint for this device type.
    #[must_use]
    pub const fn topology_hint(self, model: u8) -> DeviceTopologyHint {
        match self {
            Self::QxRgbFan
            | Self::LxRgbFan
            | Self::RxRgbMaxFan
            | Self::RxRgbFan
            | Self::CoolerPumpLcd
            | Self::HSeriesAio
            | Self::TitanAio
            | Self::Xc7Elite
            | Self::Xd5Elite
            | Self::Xd5EliteLcd
            | Self::Xd6Elite => DeviceTopologyHint::Ring {
                count: self.led_count(model),
            },
            Self::CaseAdapter | Self::Xg7Rgb => DeviceTopologyHint::Strip,
            Self::RxMaxFan
            | Self::Xg3Hybrid
            | Self::VrmCoolerModule
            | Self::RxFan
            | Self::OriginOa => DeviceTopologyHint::Custom,
        }
    }
}

/// Lighting Node command packet IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingNodePacketId {
    Firmware,
    Direct,
    Commit,
    Reset,
    PortState,
    Brightness,
}

impl LightingNodePacketId {
    /// Raw packet byte for this command.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Firmware => 0x02,
            Self::Direct => 0x32,
            Self::Commit => 0x33,
            Self::Reset => 0x37,
            Self::PortState => 0x38,
            Self::Brightness => 0x39,
        }
    }
}

/// Lighting Node planar color channel selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingNodeColorChannel {
    Red,
    Green,
    Blue,
}

impl LightingNodeColorChannel {
    /// Raw packet byte for this channel selector.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Red => 0x00,
            Self::Green => 0x01,
            Self::Blue => 0x02,
        }
    }
}

/// Lighting Node port state selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingNodePortState {
    Hardware,
    Software,
}

impl LightingNodePortState {
    /// Raw packet byte for this port state.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Hardware => 0x01,
            Self::Software => 0x02,
        }
    }
}
