//! Wooting wire constants, model geometry, and protocol generations
//! (Spec 90 §2, §3).

/// Vendor ID used by v1 firmware, which borrowed Atmel's VID.
pub const WOOTING_LEGACY_VID: u16 = 0x03EB;

/// Wooting's own vendor ID, used by every keyboard on v2 firmware or newer.
pub const WOOTING_VID: u16 = 0x31E3;

/// Usage page of the vendor configuration collection before firmware 2.12.
///
/// v1 and v2 firmware expose lighting here.
pub const CONFIG_USAGE_PAGE: u16 = 0x1337;

/// Usage page of the multi-report configuration collection on firmware 2.12
/// and newer.
pub const MULTI_REPORT_USAGE_PAGE: u16 = 0xFF55;

/// Rows in the colour matrix every v2 and v3 colour report carries.
pub const MATRIX_ROWS: usize = 6;

/// Columns in the colour matrix every v2 and v3 colour report carries.
///
/// Models narrower than 21 columns still use this stride on the wire.
pub const MATRIX_COLS: usize = 21;

/// Cells in the wire colour matrix.
pub const MATRIX_CELLS: usize = MATRIX_ROWS * MATRIX_COLS;

/// Second magic byte shared by every report.
pub const MAGIC_TAIL: u8 = 0xDA;

/// First magic byte on the v1 and v2 configuration collection.
pub const MAGIC_HEAD: u8 = 0xD0;

/// First magic byte on the multi-report collection.
pub const MAGIC_HEAD_MULTI_REPORT: u8 = 0xD1;

/// Report ID of v1 and v2 traffic (the collection has no numbered reports).
pub const UNNUMBERED_REPORT_ID: u8 = 0x00;

/// Feature report ID for commands on the multi-report collection.
pub const MULTI_REPORT_FEATURE_REPORT_ID: u8 = 0x01;

/// Output report ID that carries a colour frame on the multi-report
/// collection (510 data bytes, the smallest report the frame fits).
pub const MULTI_REPORT_COLOR_REPORT_ID: u8 = 0x05;

/// Feature command length, report ID byte included.
pub const FEATURE_COMMAND_LEN: usize = 8;

/// v1 colour report length, report ID byte included.
pub const V1_REPORT_LEN: usize = 129;

/// v2 colour report length, report ID byte included.
pub const V2_REPORT_LEN: usize = 257;

/// Payload bytes per v2 small-packet chunk (the report ID byte is extra).
pub const V2_SMALL_CHUNK_PAYLOAD: usize = 64;

/// Chunks a v2 small-packet frame splits into.
pub const V2_SMALL_CHUNK_COUNT: usize = 4;

/// v2 small-packet chunk length, report ID byte included.
pub const V2_SMALL_CHUNK_LEN: usize = V2_SMALL_CHUNK_PAYLOAD + 1;

/// Multi-report colour write length, report ID byte included.
///
/// The longest output report on the multi-report collection is 2046 bytes,
/// and the vendor SDK writes exactly that plus the report ID.
pub const V3_REPORT_LEN: usize = 2047;

/// Bytes in the colour payload a v1 colour report carries per part.
pub const V1_PART_PAYLOAD_LEN: usize = 96;

/// Command: raw colour buffer upload (the third byte of every colour report).
pub const COMMAND_RAW_COLORS: u8 = 0x0B;

/// Command: read the device configuration block (layout lives here).
pub const COMMAND_DEVICE_CONFIG: u8 = 0x13;

/// Command: set one key's colour.
pub const COMMAND_SINGLE_COLOR: u8 = 0x1E;

/// Command: return one key to its profile colour.
pub const COMMAND_SINGLE_RESET: u8 = 0x1F;

/// Command: return every key to the active profile's lighting.
pub const COMMAND_RESET_ALL: u8 = 0x20;

/// Command: enter host colour control.
pub const COMMAND_COLOR_INIT: u8 = 0x21;

/// Gamepad emulation mode, which the firmware folds into the low nibble of
/// the product ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadMode {
    /// Xbox controller emulation (`base | 0x0`).
    Xinput,
    /// Classic `DirectInput` controller emulation (`base | 0x1`).
    DirectInput,
    /// Controller emulation disabled (`base | 0x2`).
    Disabled,
}

impl GamepadMode {
    /// Every gamepad mode, in product ID order.
    pub const ALL: [Self; 3] = [Self::Xinput, Self::DirectInput, Self::Disabled];

    /// Low-nibble offset this mode adds to a model's base product ID.
    #[must_use]
    pub const fn pid_offset(self) -> u16 {
        match self {
            Self::Xinput => 0x0,
            Self::DirectInput => 0x1,
            Self::Disabled => 0x2,
        }
    }
}

/// Wire protocol generation of a Wooting configuration interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WootingInterface {
    /// v1 firmware on the Atmel VID: per-driver-chip PWM buffers in four or
    /// five CRC-protected reports.
    V1,
    /// v2 firmware before 2.12: one RGB565 matrix in a 256-byte report, or
    /// four 64-byte reports on boards whose collection declares 64-byte
    /// reports.
    V2 {
        /// Whether the collection takes 64-byte output reports.
        small_packets: bool,
    },
    /// Firmware 2.12 and newer: numbered reports on the multi-report
    /// collection, one RGB565 matrix per frame.
    V3,
}

/// Wooting keyboard and keypad models with per-key lighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WootingModel {
    /// Wooting One (tenkeyless).
    One,
    /// Wooting Two (full size).
    Two,
    /// Wooting Two Lekker Edition (full size).
    TwoLekker,
    /// Wooting Two HE (full size).
    TwoHe,
    /// Wooting Two HE with the ARM controller (full size).
    TwoHeArm,
    /// Wooting 60HE.
    SixtyHe,
    /// Wooting 60HE with the ARM controller.
    SixtyHeArm,
    /// Wooting 60HE+.
    SixtyHePlus,
    /// Wooting 60HE v2.
    SixtyHeV2,
    /// Wooting 80HE.
    EightyHe,
    /// Wooting UwU RGB three-key keypad.
    UwuRgb,
}

impl WootingModel {
    /// Rows of the colour matrix this model populates.
    #[must_use]
    pub const fn rows(self) -> usize {
        match self {
            Self::SixtyHe
            | Self::SixtyHeArm
            | Self::SixtyHePlus
            | Self::SixtyHeV2
            | Self::UwuRgb => 5,
            _ => MATRIX_ROWS,
        }
    }

    /// First wire-matrix row this model populates.
    ///
    /// The 60% boards have no function row, so their number row lands on
    /// wire row 1 and wire row 0 stays dark.
    #[must_use]
    pub const fn first_row(self) -> usize {
        match self {
            Self::SixtyHe | Self::SixtyHeArm | Self::SixtyHePlus | Self::SixtyHeV2 => 1,
            _ => 0,
        }
    }

    /// Columns of the colour matrix this model populates.
    #[must_use]
    pub const fn cols(self) -> usize {
        match self {
            Self::One | Self::EightyHe => 17,
            Self::Two | Self::TwoLekker | Self::TwoHe | Self::TwoHeArm => MATRIX_COLS,
            Self::SixtyHe | Self::SixtyHeArm | Self::SixtyHePlus | Self::SixtyHeV2 => 14,
            Self::UwuRgb => 7,
        }
    }

    /// Whether the model's v2 collection takes 64-byte output reports when
    /// its report descriptor cannot be consulted.
    #[must_use]
    pub const fn v2_small_packets(self) -> bool {
        matches!(
            self,
            Self::TwoHeArm | Self::SixtyHeArm | Self::SixtyHePlus | Self::UwuRgb
        )
    }

    /// v1 colour reports per frame: four driver-chip halves on the
    /// tenkeyless board, a fifth for the Two's numpad.
    #[must_use]
    pub const fn v1_part_count(self) -> usize {
        match self {
            Self::Two => 5,
            _ => 4,
        }
    }

    /// Highest v1 LED index the board populates.
    #[must_use]
    pub const fn v1_led_index_max(self) -> u8 {
        match self {
            Self::Two => 116,
            _ => 95,
        }
    }

    /// Zone name for the model's lighting.
    #[must_use]
    pub const fn zone_name(self) -> &'static str {
        match self {
            Self::UwuRgb => "Keypad",
            _ => "Keyboard",
        }
    }
}
