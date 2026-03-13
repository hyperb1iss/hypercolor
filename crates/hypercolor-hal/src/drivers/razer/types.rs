//! Shared Razer protocol types and constants.

/// Device configuration command family used for non-lighting features.
pub const COMMAND_CLASS_DEVICE: u8 = 0x02;

/// Set tactile vs free-spin scroll mode.
pub const COMMAND_SET_SCROLL_MODE: u8 = 0x14;

/// Query the current scroll mode.
pub const COMMAND_GET_SCROLL_MODE: u8 = 0x94;

/// Set Smart Reel auto-switching.
pub const COMMAND_SET_SCROLL_SMART_REEL: u8 = 0x17;

/// Query Smart Reel state.
pub const COMMAND_GET_SCROLL_SMART_REEL: u8 = 0x97;

/// Set scroll acceleration.
pub const COMMAND_SET_SCROLL_ACCELERATION: u8 = 0x16;

/// Query scroll acceleration state.
pub const COMMAND_GET_SCROLL_ACCELERATION: u8 = 0x96;

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

    /// Special modern devices using `transaction_id = 0x08`.
    Special08,

    /// Kraken V4 family devices using `transaction_id = 0x60`.
    KrakenV4,
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
            Self::Special08 => 0x08,
            Self::KrakenV4 => 0x60,
        }
    }
}

/// Lighting command family used for color/effect/brightness operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RazerLightingCommandSet {
    /// Standard LED/effect commands under class `0x03`.
    Standard,

    /// Extended matrix/effect commands under class `0x0F`.
    Extended,
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
