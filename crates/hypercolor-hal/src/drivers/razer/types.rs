//! Shared Razer protocol types and constants.

/// Razer protocol generation selected by HID transaction ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RazerProtocolVersion {
    /// Legacy protocol (`transaction_id = 0xFF`).
    Legacy,

    /// Extended protocol (`transaction_id = 0x3F`).
    Extended,

    /// Modern protocol (`transaction_id = 0x1F`).
    Modern,

    /// Wireless keyboard protocol (`transaction_id = 0x9F`).
    WirelessKb,
}

impl RazerProtocolVersion {
    /// Transaction ID byte written into packet offset `1`.
    #[must_use]
    pub const fn transaction_id(self) -> u8 {
        match self {
            Self::Legacy => 0xFF,
            Self::Extended => 0x3F,
            Self::Modern => 0x1F,
            Self::WirelessKb => 0x9F,
        }
    }

    /// Command class used for frame/effect operations.
    #[must_use]
    pub const fn command_class(self) -> u8 {
        match self {
            Self::Legacy => 0x03,
            _ => 0x0F,
        }
    }

    /// Whether this version uses the extended frame format.
    #[must_use]
    pub const fn uses_extended_fx(self) -> bool {
        !matches!(self, Self::Legacy)
    }
}

/// Matrix addressing mode for a Razer device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RazerMatrixType {
    /// No matrix addressing support.
    None,

    /// Legacy matrix command layout.
    Standard,

    /// Extended matrix command layout.
    Extended,

    /// Legacy linear command layout.
    Linear,

    /// Extended ARGB channel layout.
    ExtendedArgb,
}

/// Do not persist commands to device flash.
pub const NOSTORE: u8 = 0x00;

/// Persist command/effect to device flash.
pub const VARSTORE: u8 = 0x01;

/// LED ID for keyboard backlight matrices.
pub const LED_ID_BACKLIGHT: u8 = 0x05;

/// LED ID for default mouse lighting zone.
pub const LED_ID_ZERO: u8 = 0x00;

/// LED ID for scroll wheel zone.
pub const LED_ID_SCROLL_WHEEL: u8 = 0x01;

/// LED ID for logo zone.
pub const LED_ID_LOGO: u8 = 0x04;

/// Effect ID used to activate custom-frame mode.
pub const EFFECT_CUSTOM_FRAME: u8 = 0x08;
