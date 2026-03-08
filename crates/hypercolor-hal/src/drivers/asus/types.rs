//! Shared ASUS Aura protocol constants and enums.

/// `ASUSTek` Computer Inc.
pub const ASUS_VID: u16 = 0x0B05;

/// ASUS Aura HID report ID used by motherboard-class controllers.
pub const AURA_REPORT_ID: u8 = 0xEC;

/// ASUS Aura payload length excluding the HID report ID byte.
pub const AURA_REPORT_PAYLOAD_LEN: usize = 64;

/// Maximum direct-mode LED payload per packet.
pub const AURA_DIRECT_LED_CHUNK: usize = 20;

/// Default LED budget per motherboard ARGB header.
pub const AURA_DIRECT_LED_MAX: u32 = 120;

/// Static LED count for each Aura Terminal ARGB channel.
pub const AURA_TERMINAL_CHANNEL_LEDS: u32 = 90;

/// Logical channel index used for motherboard fixed LEDs.
pub const MAINBOARD_DIRECT_IDX: u8 = 0x04;

/// ASUS Aura USB command IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuraCommand {
    FirmwareVersion = 0x82,
    ConfigTable = 0xB0,
    SetMode = 0x35,
    SetEffectColor = 0x36,
    SetAddressableMode = 0x3B,
    Commit = 0x3F,
    DirectControl = 0x40,
    DisableGen2 = 0x52,
}

impl From<AuraCommand> for u8 {
    fn from(command: AuraCommand) -> Self {
        match command {
            AuraCommand::FirmwareVersion => 0x82,
            AuraCommand::ConfigTable => 0xB0,
            AuraCommand::SetMode => 0x35,
            AuraCommand::SetEffectColor => 0x36,
            AuraCommand::SetAddressableMode => 0x3B,
            AuraCommand::Commit => 0x3F,
            AuraCommand::DirectControl => 0x40,
            AuraCommand::DisableGen2 => 0x52,
        }
    }
}

/// ASUS Aura motherboard controller variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraControllerGen {
    /// PIDs `0x1867`, `0x1872`, `0x18A3`, `0x18A5`.
    AddressableOnly,
    /// PIDs `0x18F3`, `0x1939`, `0x19AF`, `0x1AA6`, `0x1BED`.
    Motherboard,
    /// PID `0x1889`.
    Terminal,
}

impl AuraControllerGen {
    /// Whether the controller discovers its topology from firmware/config
    /// responses rather than using a fixed static layout.
    #[must_use]
    pub const fn uses_runtime_discovery(self) -> bool {
        !matches!(self, Self::Terminal)
    }
}

/// Color byte ordering for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuraColorOrder {
    #[default]
    Rgb,
    Rbg,
    Grb,
    Gbr,
    Brg,
    Bgr,
}

impl AuraColorOrder {
    /// Permute an RGB triple into the configured wire order.
    #[must_use]
    pub const fn permute(self, r: u8, g: u8, b: u8) -> [u8; 3] {
        match self {
            Self::Rgb => [r, g, b],
            Self::Rbg => [r, b, g],
            Self::Grb => [g, r, b],
            Self::Gbr => [g, b, r],
            Self::Brg => [b, r, g],
            Self::Bgr => [b, g, r],
        }
    }
}

/// Runtime init phase for ASUS topology discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraInitPhase {
    /// No response-driven discovery has completed yet.
    PreInit,
    /// Firmware response was parsed.
    FirmwareReceived,
    /// Config response was parsed and topology is stable.
    Configured,
}

/// Build the 16-bit LED mask used by the effect-color packet.
#[must_use]
pub const fn led_mask(start: u8, count: u8) -> u16 {
    if count == 0 || start >= 16 {
        return 0;
    }

    let remaining = 16_u8 - start;
    let used = if count > remaining { remaining } else { count };

    if used >= 16 {
        u16::MAX
    } else {
        ((1_u16 << used) - 1) << start
    }
}
