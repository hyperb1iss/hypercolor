//! Self-contained Razer device registry entries.
//!
//! This registry imports shared-matrix Razer devices that map cleanly onto
//! Hypercolor's shared `RazerProtocol` transport model.
//! Specialty controller families that require different packet formats or
//! multi-interface coordination remain explicitly excluded until they are
//! ported as dedicated drivers.

use std::sync::LazyLock;
use std::time::Duration;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{
    DeviceDescriptor, HidRawReportMode, ProtocolBinding, ProtocolFactory, TransportType,
};

use super::protocol::RazerProtocol;
use super::seiren_v3::SeirenV3Protocol;
use super::types::{
    EFFECT_CUSTOM_FRAME, LED_ID_BACKLIGHT, LED_ID_ZERO, NOSTORE, RazerLightingCommandSet,
    RazerMatrixType, RazerProtocolVersion, VARSTORE,
};

/// Razer vendor ID.
pub const RAZER_VENDOR_ID: u16 = 0x1532;

const RAZER_CONSUMER_USAGE_PAGE: u16 = 0x000C;
const RAZER_CONSUMER_USAGE: u16 = 0x0001;
const RAZER_VENDOR_USAGE_PAGE: u16 = 0xFF53;
const RAZER_VENDOR_USAGE: u16 = 0x0004;
const HID_REPORT_ID_DEFAULT: u8 = 0x00;
const HID_REPORT_ID_ALT_0X07: u8 = 0x07;

/// Razer Huntsman V2 (full-size).
pub const PID_HUNTSMAN_V2: u16 = 0x026C;

/// Razer Basilisk V3.
pub const PID_BASILISK_V3: u16 = 0x0099;

/// Razer Tartarus Chroma.
pub const PID_TARTARUS_CHROMA: u16 = 0x0208;

/// Razer Mamba Elite.
pub const PID_MAMBA_ELITE: u16 = 0x006C;

/// Razer Seiren Emote.
pub const PID_SEIREN_EMOTE: u16 = 0x0F1B;

/// Razer Seiren V3 Chroma.
pub const PID_SEIREN_V3_CHROMA: u16 = 0x056F;

/// Razer Blade 14 (2021).
pub const PID_BLADE_14_2021: u16 = 0x0270;

/// Razer Blade Pro (2016).
pub const PID_BLADE_PRO_2016: u16 = 0x0210;

/// Razer Blade 15 (Late 2021 Advanced).
pub const PID_BLADE_15_LATE_2021_ADVANCED: u16 = 0x0276;

/// Razer Blade 15 (2022).
pub const PID_BLADE_15_2022: u16 = 0x028A;

/// Razer Blade 14 (2023).
pub const PID_BLADE_14_2023: u16 = 0x029D;

/// Build a Huntsman V2 protocol instance.
pub fn build_huntsman_v2_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Extended,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (6, 22),
            LED_ID_BACKLIGHT,
        )
        .with_init_custom_effect()
        .with_write_only_frame_uploads(),
    )
}

/// Build a Basilisk V3 protocol instance.
pub fn build_basilisk_v3_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (1, 11),
            LED_ID_ZERO,
        )
        .without_device_mode_commands()
        .with_extended_custom_effect_activation(
            0x0C,
            [NOSTORE, LED_ID_ZERO, EFFECT_CUSTOM_FRAME, 0x00, 0x00],
            3,
        )
        .with_write_only_custom_effect_activation(Duration::ZERO)
        .with_scroll_features()
        .with_write_only_frame_uploads(),
    )
}

/// Build a Basilisk V3 X `HyperSpeed` protocol instance.
pub fn build_basilisk_v3_x_hyperspeed_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (1, 1),
            LED_ID_ZERO,
        )
        .without_device_mode_commands()
        .with_write_only_frame_uploads()
        .with_write_only_custom_effect_activation(Duration::ZERO)
        .with_scroll_features(),
    )
}

/// Build a Mamba Elite protocol instance.
pub fn build_mamba_elite_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (1, 20),
            LED_ID_ZERO,
        )
        .with_init_custom_effect()
        .with_write_only_frame_uploads(),
    )
}

/// Build a Tartarus Chroma protocol instance.
pub fn build_tartarus_chroma_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::None,
            (1, 1),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_init_custom_effect()
        .with_standard_led_effect_activation(NOSTORE, LED_ID_BACKLIGHT, 0x00)
        .with_write_only_custom_effect_activation(Duration::ZERO)
        .with_write_only_frame_uploads()
        .without_brightness(),
    )
}

/// Build a Seiren Emote protocol instance.
pub fn build_seiren_emote_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Extended,
            RazerLightingCommandSet::Extended,
            RazerMatrixType::Extended,
            (4, 16),
            LED_ID_ZERO,
        )
        .with_reported_matrix_size((8, 8))
        .with_init_custom_effect()
        .with_write_only_frame_uploads(),
    )
}

/// Build a Seiren V3 Chroma protocol instance.
pub fn build_seiren_v3_protocol() -> Box<dyn Protocol> {
    Box::new(SeirenV3Protocol)
}

/// Build a Blade 15 (Late 2021 Advanced) protocol instance.
pub fn build_blade_15_late_2021_advanced_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads(),
    )
}

/// Build a Blade 14 (2021) protocol instance.
pub fn build_blade_14_2021_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Extended,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads()
        .with_device_mode_keepalive(Duration::from_millis(2_500)),
    )
}

/// Build a Blade Pro (2016) protocol instance.
pub fn build_blade_pro_2016_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Legacy,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 25),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0x80)
        .with_write_only_frame_uploads(),
    )
}

/// Build a Blade 15 (2022) protocol instance.
pub fn build_blade_15_2022_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads(),
    )
}

/// Build a Blade 14 (2023) protocol instance.
pub fn build_blade_14_2023_protocol() -> Box<dyn Protocol> {
    Box::new(
        RazerProtocol::new(
            RazerProtocolVersion::Modern,
            RazerLightingCommandSet::Standard,
            RazerMatrixType::Standard,
            (6, 16),
            LED_ID_BACKLIGHT,
        )
        .without_device_mode_commands()
        .with_standard_storage(VARSTORE)
        .with_frame_transaction_id(0xFF)
        .with_write_only_frame_uploads(),
    )
}

macro_rules! razer_matrix_builder {
    (
        $name:ident,
        $version:expr,
        $command_set:expr,
        $matrix_type:expr,
        ($rows:expr, $cols:expr),
        $led_id:expr
        $(, .$modifier:ident ( $($args:expr),* ) )* $(,)?
    ) => {
        fn $name() -> Box<dyn Protocol> {
            let protocol = RazerProtocol::new(
                $version,
                $command_set,
                $matrix_type,
                ($rows, $cols),
                $led_id,
            )
            .without_device_mode_commands()
            .with_write_only_frame_uploads()
            .with_write_only_custom_effect_activation(Duration::ZERO);
            $(let protocol = protocol.$modifier($($args),*);)*
            Box::new(protocol)
        }
    };
}

macro_rules! razer_matrix_device_mode_builder {
    (
        $name:ident,
        $version:expr,
        $command_set:expr,
        $matrix_type:expr,
        ($rows:expr, $cols:expr),
        $led_id:expr
        $(, .$modifier:ident ( $($args:expr),* ) )* $(,)?
    ) => {
        fn $name() -> Box<dyn Protocol> {
            let protocol = RazerProtocol::new(
                $version,
                $command_set,
                $matrix_type,
                ($rows, $cols),
                $led_id,
            )
            .with_write_only_frame_uploads()
            .with_write_only_custom_effect_activation(Duration::ZERO);
            $(let protocol = protocol.$modifier($($args),*);)*
            Box::new(protocol)
        }
    };
}

macro_rules! razer_laptop_builder {
    (
        $name:ident,
        $version:expr,
        ($rows:expr, $cols:expr)
        $(, .$modifier:ident ( $($args:expr),* ) )* $(,)?
    ) => {
        fn $name() -> Box<dyn Protocol> {
            let protocol = RazerProtocol::new(
                $version,
                RazerLightingCommandSet::Standard,
                RazerMatrixType::Standard,
                ($rows, $cols),
                LED_ID_BACKLIGHT,
            )
            .without_device_mode_commands()
            .with_standard_storage(VARSTORE)
            .with_frame_transaction_id(0xFF)
            .with_write_only_frame_uploads();
            $(let protocol = protocol.$modifier($($args),*);)*
            Box::new(protocol)
        }
    };
}

razer_matrix_builder!(
    build_matrix_extended_modern_1x1_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 1),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x1_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 1),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x2_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 2),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x3_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 3),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x3_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 3),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x4_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 4),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x8_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 8),
    LED_ID_ZERO
);
razer_matrix_device_mode_builder!(
    build_matrix_extended_modern_1x10_zero_device_mode_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 10),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x11_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 11),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x13_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 13),
    LED_ID_ZERO,
    .with_scroll_features()
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x14_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 14),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x15_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 15),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_1x17_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 17),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_5x15_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (5, 15),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_5x16_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (5, 16),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_2x9_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (2, 9),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_4x16_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (4, 16),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_4x6_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (4, 6),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_6x17_zero_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 17),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_modern_6x18_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 18),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_6x22_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 22),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_modern_8x23_backlight_protocol,
    RazerProtocolVersion::Modern,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (8, 23),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x1_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 1),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x1_zero_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 1),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x2_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 2),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x2_zero_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 2),
    LED_ID_ZERO
);
razer_matrix_device_mode_builder!(
    build_matrix_extended_extended_1x14_zero_device_mode_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 14),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x15_zero_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 15),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x16_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 16),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x17_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 17),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x19_zero_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 19),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_extended_2x8_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (2, 8),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_2x16_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (2, 16),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_2x24_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (2, 24),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_4x16_zero_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (4, 16),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_extended_extended_5x15_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (5, 15),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_6x17_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 17),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_6x18_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 18),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_6x19_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 19),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_6x22_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 22),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_9x22_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (9, 22),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_linear_extended_1x15_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Linear,
    (1, 15),
    LED_ID_BACKLIGHT
);
razer_matrix_device_mode_builder!(
    build_matrix_linear_extended_1x15_backlight_device_mode_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Linear,
    (1, 15),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_linear_extended_1x16_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Linear,
    (1, 16),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_linear_extended_1x21_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Linear,
    (1, 21),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_linear_extended_1x12_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Linear,
    (1, 12),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_linear_extended_1x3_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Linear,
    (1, 3),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_standard_extended_1x3_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Standard,
    (1, 3),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_standard_extended_1x9_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Standard,
    (1, 9),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_standard_extended_6x22_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Standard,
    RazerMatrixType::Standard,
    (6, 22),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_extended_extended_1x12_backlight_protocol,
    RazerProtocolVersion::Extended,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 12),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_legacy_extended_1x9_zero_protocol,
    RazerProtocolVersion::Legacy,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (1, 9),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_wireless_5x16_backlight_protocol,
    RazerProtocolVersion::WirelessKb,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (5, 16),
    LED_ID_BACKLIGHT
);
razer_matrix_builder!(
    build_matrix_wireless_6x17_zero_protocol,
    RazerProtocolVersion::WirelessKb,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 17),
    LED_ID_ZERO
);
razer_matrix_builder!(
    build_matrix_wireless_6x22_zero_protocol,
    RazerProtocolVersion::WirelessKb,
    RazerLightingCommandSet::Extended,
    RazerMatrixType::Extended,
    (6, 22),
    LED_ID_ZERO
);
razer_laptop_builder!(
    build_matrix_standard_extended_1x1_laptop_protocol,
    RazerProtocolVersion::Extended,
    (1, 1)
);
razer_laptop_builder!(
    build_matrix_standard_extended_1x16_laptop_protocol,
    RazerProtocolVersion::Extended,
    (1, 16)
);
razer_laptop_builder!(
    build_matrix_standard_extended_6x16_laptop_protocol,
    RazerProtocolVersion::Extended,
    (6, 16)
);
razer_laptop_builder!(
    build_matrix_standard_extended_6x25_laptop_protocol,
    RazerProtocolVersion::Extended,
    (6, 25)
);
razer_laptop_builder!(
    build_matrix_standard_modern_1x16_laptop_protocol,
    RazerProtocolVersion::Modern,
    (1, 16)
);
razer_laptop_builder!(
    build_matrix_standard_modern_6x16_laptop_keepalive_protocol,
    RazerProtocolVersion::Modern,
    (6, 16),
    .with_device_mode_keepalive(Duration::from_millis(2_500))
);

#[expect(
    clippy::too_many_arguments,
    reason = "Descriptor helpers keep transport identity and protocol binding together for large static device catalogs"
)]
fn hidapi_descriptor(
    product_id: u16,
    name: &'static str,
    protocol_id: &'static str,
    interface: Option<u8>,
    report_id: u8,
    usage_page: Option<u16>,
    usage: Option<u16>,
    build: ProtocolFactory,
) -> DeviceDescriptor {
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id,
        name,
        family: DeviceFamily::Razer,
        transport: TransportType::UsbHidApi {
            interface,
            report_id,
            report_mode: HidRawReportMode::FeatureReport,
            usage_page,
            usage,
        },
        protocol: ProtocolBinding {
            id: protocol_id,
            build,
        },
        firmware_predicate: None,
    }
}

fn control_descriptor(
    product_id: u16,
    name: &'static str,
    protocol_id: &'static str,
    build: ProtocolFactory,
) -> DeviceDescriptor {
    DeviceDescriptor {
        vendor_id: RAZER_VENDOR_ID,
        product_id,
        name,
        family: DeviceFamily::Razer,
        transport: TransportType::UsbControl {
            interface: 2,
            report_id: HID_REPORT_ID_DEFAULT,
        },
        protocol: ProtocolBinding {
            id: protocol_id,
            build,
        },
        firmware_predicate: None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Grouped HID registrations share the full transport tuple across many product IDs"
)]
fn push_hidapi_group(
    descriptors: &mut Vec<DeviceDescriptor>,
    protocol_id: &'static str,
    build: ProtocolFactory,
    interface: Option<u8>,
    report_id: u8,
    usage_page: Option<u16>,
    usage: Option<u16>,
    devices: &[(u16, &'static str)],
) {
    descriptors.extend(devices.iter().map(|&(product_id, name)| {
        hidapi_descriptor(
            product_id,
            name,
            protocol_id,
            interface,
            report_id,
            usage_page,
            usage,
            build,
        )
    }));
}

fn push_control_group(
    descriptors: &mut Vec<DeviceDescriptor>,
    protocol_id: &'static str,
    build: ProtocolFactory,
    devices: &[(u16, &'static str)],
) {
    descriptors.extend(
        devices
            .iter()
            .map(|&(product_id, name)| control_descriptor(product_id, name, protocol_id, build)),
    );
}

static RAZER_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::with_capacity(128);

    descriptors.push(hidapi_descriptor(
        PID_HUNTSMAN_V2,
        "Razer Huntsman V2",
        "razer/huntsman-v2",
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        build_huntsman_v2_protocol,
    ));
    descriptors.push(hidapi_descriptor(
        PID_BASILISK_V3,
        "Razer Basilisk V3",
        "razer/basilisk-v3",
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        build_basilisk_v3_protocol,
    ));
    descriptors.push(hidapi_descriptor(
        PID_TARTARUS_CHROMA,
        "Razer Tartarus Chroma",
        "razer/tartarus-chroma",
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        build_tartarus_chroma_protocol,
    ));
    descriptors.push(hidapi_descriptor(
        PID_MAMBA_ELITE,
        "Razer Mamba Elite",
        "razer/mamba-elite",
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        build_mamba_elite_protocol,
    ));
    descriptors.push(hidapi_descriptor(
        PID_SEIREN_EMOTE,
        "Razer Seiren Emote",
        "razer/seiren-emote",
        Some(3),
        HID_REPORT_ID_DEFAULT,
        None,
        None,
        build_seiren_emote_protocol,
    ));
    descriptors.push(hidapi_descriptor(
        PID_SEIREN_V3_CHROMA,
        "Razer Seiren V3 Chroma",
        "razer/seiren-v3-chroma",
        Some(3),
        HID_REPORT_ID_ALT_0X07,
        Some(RAZER_VENDOR_USAGE_PAGE),
        Some(RAZER_VENDOR_USAGE),
        build_seiren_v3_protocol,
    ));
    descriptors.push(control_descriptor(
        PID_BLADE_14_2021,
        "Razer Blade 14 (2021)",
        "razer/blade-14-2021",
        build_blade_14_2021_protocol,
    ));
    descriptors.push(control_descriptor(
        PID_BLADE_PRO_2016,
        "Razer Blade Pro (2016)",
        "razer/blade-pro-2016",
        build_blade_pro_2016_protocol,
    ));
    descriptors.push(control_descriptor(
        PID_BLADE_15_LATE_2021_ADVANCED,
        "Razer Blade 15 (Late 2021 Advanced)",
        "razer/blade-15-late-2021-advanced",
        build_blade_15_late_2021_advanced_protocol,
    ));
    descriptors.push(control_descriptor(
        PID_BLADE_15_2022,
        "Razer Blade 15 (2022)",
        "razer/blade-15-2022",
        build_blade_15_2022_protocol,
    ));
    descriptors.push(control_descriptor(
        PID_BLADE_14_2023,
        "Razer Blade 14 (2023)",
        "razer/blade-14-2023",
        build_blade_14_2023_protocol,
    ));

    push_control_group(
        &mut descriptors,
        "razer/matrix-standard-3f-laptop-6x16",
        build_matrix_standard_extended_6x16_laptop_protocol,
        &[
            (0x020F, "Razer Blade (2016)"),
            (0x0224, "Razer Blade (Late 2016)"),
            (0x0233, "Razer Blade 15 (2018 Advanced)"),
            (0x023B, "Razer Blade 15 (2018 Base)"),
            (0x0240, "Razer Blade 15 (2018 Mercury)"),
            (0x023A, "Razer Blade 15 (2019 Advanced)"),
            (0x0245, "Razer Blade 15 (2019 Mercury)"),
            (0x024D, "Razer Blade 15 (2019 Studio)"),
            (0x0253, "Razer Blade 15 (2020 Advanced)"),
            (0x0268, "Razer Blade (Late 2020)"),
            (0x026D, "Razer Blade 15 (2021 Advanced)"),
            (0x026A, "Razer Book 13 (2020)"),
            (0x0234, "Razer Blade Pro (2019)"),
            (0x024C, "Razer Blade Pro (Late 2019)"),
            (0x0256, "Razer Blade Pro 17 (2020)"),
            (0x0279, "Razer Blade Pro 17 (2021)"),
            (0x0205, "Razer Blade Stealth (2016)"),
            (0x0220, "Razer Blade Stealth (Late 2016)"),
            (0x022D, "Razer Blade Stealth (2017)"),
            (0x0232, "Razer Blade Stealth (Late 2017)"),
            (0x0239, "Razer Blade Stealth (2019)"),
        ],
    );
    push_control_group(
        &mut descriptors,
        "razer/matrix-standard-3f-laptop-6x25",
        build_matrix_standard_extended_6x25_laptop_protocol,
        &[
            (0x0225, "Razer Blade Pro (2017)"),
            (0x022F, "Razer Blade Pro (2017 FullHD)"),
        ],
    );
    push_control_group(
        &mut descriptors,
        "razer/matrix-standard-3f-laptop-1x1",
        build_matrix_standard_extended_1x1_laptop_protocol,
        &[
            (0x0246, "Razer Blade 15 (2019 Base)"),
            (0x024A, "Razer Blade Stealth (Late 2019)"),
            (0x0252, "Razer Blade Stealth (2020)"),
        ],
    );
    push_control_group(
        &mut descriptors,
        "razer/matrix-standard-3f-laptop-1x16",
        build_matrix_standard_extended_1x16_laptop_protocol,
        &[
            (0x0255, "Razer Blade 15 (2020 Base)"),
            (0x026F, "Razer Blade 15 (2021 Base)"),
            (0x0259, "Razer Blade Stealth (Late 2020)"),
        ],
    );
    push_control_group(
        &mut descriptors,
        "razer/matrix-standard-1f-laptop-1x16",
        build_matrix_standard_modern_1x16_laptop_protocol,
        &[(0x027A, "Razer Blade 15 (2021 Base)")],
    );
    push_control_group(
        &mut descriptors,
        "razer/matrix-standard-1f-laptop-6x16",
        build_matrix_standard_modern_6x16_laptop_keepalive_protocol,
        &[(0x028C, "Razer Blade 14 (2022)")],
    );

    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-standard-3f-6x22",
        build_matrix_standard_extended_6x22_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0203, "Razer BlackWidow Chroma"),
            (0x0211, "Razer Blackwidow Chroma Overwatch"),
            (0x0209, "Razer BlackWidow Chroma Tournament Edition"),
            (0x0221, "Razer BlackWidow Chroma V2"),
            (0x0216, "Razer BlackWidow X Chroma"),
            (0x021A, "Razer BlackWidow X Chroma Tournament Edition"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-standard-3f-1x3",
        build_matrix_standard_extended_1x3_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0050, "Razer Naga Hex V2")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-standard-3f-1x9",
        build_matrix_standard_extended_1x9_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0xFF00),
        Some(0x0001),
        &[(0x0215, "Razer Core")],
    );

    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-linear-3f-1x3",
        build_matrix_linear_extended_1x3_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0053, "Razer Naga Chroma")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-linear-3f-1x12",
        build_matrix_linear_extended_1x12_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0204, "Razer DeathStalker Chroma")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-linear-3f-1x15",
        build_matrix_linear_extended_1x15_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0044, "Razer Mamba 2015 (Wired)"),
            (0x0045, "Razer Mamba (Wireless)"),
            (0x0C00, "Razer Firefly"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-linear-3f-1x15-device-mode",
        build_matrix_linear_extended_1x15_backlight_device_mode_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F07, "Razer Chroma Mug Holder")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-linear-3f-1x16",
        build_matrix_linear_extended_1x16_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0046, "Razer Mamba Tournament Edition")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-linear-3f-1x21",
        build_matrix_linear_extended_1x21_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x004C, "Razer Diamondback Chroma")],
    );

    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x1-backlight",
        build_matrix_extended_extended_1x1_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x006A, "Razer Abyssus Elite DVa Edition"),
            (0x006B, "Razer Abyssus Essential"),
            (0x0065, "Razer Basilisk Essential"),
            (0x007C, "Razer DeathAdder V2 (Wired)"),
            (0x007D, "Razer DeathAdder V2 (Wireless)"),
            (0x008A, "Razer Viper Mini"),
            (0x007A, "Razer Viper Ultimate (Wired)"),
            (0x007B, "Razer Viper Ultimate (Wireless)"),
            (0x0078, "Razer Viper"),
            (0x007E, "Razer Mouse Dock Chroma"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x1-zero",
        build_matrix_extended_extended_1x1_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0C01, "Razer Goliathus"),
            (0x0C02, "Razer Goliathus Extended"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x2-backlight",
        build_matrix_extended_extended_1x2_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0064, "Razer Basilisk"),
            (0x0085, "Razer Basilisk V2"),
            (0x006E, "Razer DeathAdder Essential"),
            (0x0071, "Razer DeathAdder Essential (White Edition)"),
            (0x0073, "Razer Mamba 2018 (Wired)"),
            (0x0072, "Razer Mamba 2018 (Wireless)"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x2-zero",
        build_matrix_extended_extended_1x2_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x005C, "Razer DeathAdder Elite"),
            (0x0069, "Razer Mamba Hyperflux (Wired)"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x14-zero-device-mode",
        build_matrix_extended_extended_1x14_zero_device_mode_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0068, "Razer Firefly Hyperflux")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x15-zero",
        build_matrix_extended_extended_1x15_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F08, "Razer Base Station Chroma")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x16-backlight",
        build_matrix_extended_extended_1x16_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0059, "Razer Lancehead 2017 (Wired)"),
            (0x005A, "Razer Lancehead 2017 (Wireless)"),
            (0x0070, "Razer Lancehead 2019 (Wired)"),
            (0x006F, "Razer Lancehead 2019 (Wireless)"),
            (0x0060, "Razer Lancehead Tournament Edition"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x19-zero",
        build_matrix_extended_extended_1x19_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0C04, "Razer Firefly V2"),
            (0x0C05, "Razer Strider Chroma"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-2x8-backlight",
        build_matrix_extended_extended_2x8_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0000),
        &[(0x0518, "Razer Nommo Pro")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-2x24-backlight",
        build_matrix_extended_extended_2x24_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0000),
        &[(0x0517, "Razer Nommo Chroma")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-2x16-backlight",
        build_matrix_extended_extended_2x16_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F0E, "Razer Chroma PC Case Lighting Kit")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-4x16-zero",
        build_matrix_extended_extended_4x16_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F09, "Razer Chroma HDK")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-5x15-backlight",
        build_matrix_extended_extended_5x15_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0257, "Razer Huntsman Mini")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-6x18-backlight",
        build_matrix_extended_extended_6x18_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0A24, "Razer BlackWidow V3 TKL"),
            (0x0243, "Razer Huntsman Tournament Edition"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-6x22-backlight",
        build_matrix_extended_extended_6x22_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x025A, "Razer BlackWidow V3 Pro (Wired)"),
            (0x022A, "Razer Cynosa Chroma"),
            (0x021E, "Razer Ornata Chroma"),
            (0x0227, "Razer Huntsman"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-9x22-backlight",
        build_matrix_extended_extended_9x22_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0226, "Razer Huntsman Elite")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x17-backlight",
        build_matrix_extended_extended_1x17_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        None,
        None,
        &[(0x0F03, "Razer Tiamat 7.1 V2")],
    );

    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x1-backlight",
        build_matrix_extended_modern_1x1_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x00A3, "Razer Cobra"), (0x0091, "Razer Viper 8kHz")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/basilisk-v3-x-hyperspeed",
        build_basilisk_v3_x_hyperspeed_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x00B9, "Razer Basilisk V3 X HyperSpeed")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x1-zero",
        build_matrix_extended_modern_1x1_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x008C, "Razer DeathAdder V2 Mini"),
            (0x0C06, "Razer Goliathus Chroma 3XL"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x2-zero",
        build_matrix_extended_modern_1x2_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0098, "Razer DeathAdder Essential V2"),
            (0x0084, "Razer DeathAdder V2"),
            (0x00A7, "Razer Naga Pro V2 (Wired)"),
            (0x00A8, "Razer Naga Pro V2 (Wireless)"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x3-backlight",
        build_matrix_extended_modern_1x3_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x008F, "Razer Naga Pro (Wired)"),
            (0x0090, "Razer Naga Pro (Wireless)"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x3-zero",
        build_matrix_extended_modern_1x3_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0093, "Razer Naga Classic"),
            (0x008D, "Razer Naga Left Handed"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x8-zero",
        build_matrix_extended_modern_1x8_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0F1D, "Razer Mouse Bungee V3 Chroma"),
            (0x0F20, "Razer Base Station V2 Chroma"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x10-zero-device-mode",
        build_matrix_extended_modern_1x10_zero_device_mode_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x000C),
        Some(0x0001),
        &[(0x0F26, "Razer Charging Pad Chroma")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x11-zero",
        build_matrix_extended_modern_1x11_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x00AF, "Razer Cobra Pro (Wired)"),
            (0x00B0, "Razer Cobra Pro (Wireless)"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x13-zero",
        build_matrix_extended_modern_1x13_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x00AA, "Razer Basilisk V3 Pro (Wired)"),
            (0x00AB, "Razer Basilisk V3 Pro (Wireless)"),
            (0x00CC, "Razer Basilisk V3 Pro 35K (Wired)"),
            (0x00CD, "Razer Basilisk V3 Pro 35K (Wireless)"),
            (
                0x00D6,
                "Razer Basilisk V3 Pro 35K Phantom Green Edition (Wired)",
            ),
            (
                0x00D7,
                "Razer Basilisk V3 Pro 35K Phantom Green Edition (Wireless)",
            ),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x14-zero",
        build_matrix_extended_modern_1x14_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0086, "Razer Basilisk Ultimate"),
            (0x0088, "Razer Basilisk Ultimate (Wireless)"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x15-zero",
        build_matrix_extended_modern_1x15_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0F0D, "Razer Laptop Stand Chroma"),
            (0x0F2B, "Razer Laptop Stand Chroma V2"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x17-zero",
        build_matrix_extended_modern_1x17_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0C08, "Razer Firefly V2 Pro")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-2x9-zero",
        build_matrix_extended_modern_2x9_zero_protocol,
        Some(2),
        HID_REPORT_ID_ALT_0X07,
        Some(0x000C),
        Some(0x0001),
        &[(0x0532, "Razer Leviathan V2")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-4x16-zero",
        build_matrix_extended_modern_4x16_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F13, "Lian Li O11 Dynamic - Razer Edition")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-4x6-zero",
        build_matrix_extended_modern_4x6_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x022B, "Razer Tartarus V2")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-6x17-zero",
        build_matrix_extended_modern_6x17_zero_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0298, "Razer DeathStalker V2 Pro TKL (Wired)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-6x18-backlight",
        build_matrix_extended_modern_6x18_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x02B3, "Razer Blackwidow V4 Pro 75% (Wired)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-6x18-backlight",
        build_matrix_extended_modern_6x18_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x02B4, "Razer Blackwidow V4 Pro 75% (Wireless)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-6x22-backlight",
        build_matrix_extended_modern_6x22_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0295, "Razer Deathstalker V2")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-8x23-backlight",
        build_matrix_extended_modern_8x23_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0000),
        &[
            (0x0287, "Razer Blackwidow V4"),
            (0x028D, "Razer Blackwidow V4 Pro"),
        ],
    );

    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-6x17-backlight",
        build_matrix_extended_extended_6x17_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x026B, "Razer Huntsman V2 TKL")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-6x19-backlight",
        build_matrix_extended_extended_6x19_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x02A7, "Razer Huntsman V3 Pro TKL White")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-6x22-backlight",
        build_matrix_extended_extended_6x22_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[
            (0x024E, "Razer Blackwidow V3"),
            (0x025C, "Razer BlackWidow V3 Pro (Wireless)"),
            (0x02A6, "Razer Huntsman V3 Pro"),
        ],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-9x22-backlight",
        build_matrix_extended_extended_9x22_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0266, "Razer Huntsman V2 Analog")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x11-zero-basilisk",
        build_basilisk_v3_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x00CB, "Razer Basilisk V3 35K")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-5x15-backlight",
        build_matrix_extended_modern_5x15_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0282, "Razer Huntsman Mini Analog")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-5x16-backlight",
        build_matrix_extended_modern_5x16_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0258, "Razer BlackWidow V3 Mini (Wired)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-wireless-5x16-backlight",
        build_matrix_wireless_5x16_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0271, "Razer BlackWidow V3 Mini (Wireless)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-wireless-6x17-zero",
        build_matrix_wireless_6x17_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0296, "Razer DeathStalker V2 Pro TKL (Wireless)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-wireless-6x22-zero",
        build_matrix_wireless_6x22_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0290, "Razer DeathStalker V2 Pro (Wireless)")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-wireless-6x22-zero",
        build_matrix_wireless_6x22_zero_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0292, "Razer DeathStalker V2 Pro (Wired)")],
    );

    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x14-zero",
        build_matrix_extended_modern_1x14_zero_protocol,
        Some(0),
        HID_REPORT_ID_ALT_0X07,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x054A, "Razer Leviathan V2 X")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-1f-1x4-backlight",
        build_matrix_extended_modern_1x4_backlight_protocol,
        Some(1),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0003),
        &[(0x0F19, "Razer Kraken Kitty Edition")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-legacy-1x9-zero",
        build_matrix_legacy_extended_1x9_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x00A4, "Razer Mouse Dock Pro")],
    );
    push_hidapi_group(
        &mut descriptors,
        "razer/matrix-extended-3f-1x12-backlight",
        build_matrix_extended_extended_1x12_backlight_protocol,
        None,
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[
            (0x0F21, "Razer Thunderbolt 4 Dock Chroma"),
            (0x0F52, "Razer Thunderbolt 5 Dock Chroma"),
        ],
    );

    // Excluded for now:
    // - Custom-matrix generic devices (DeathAdder Chroma, Naga Epic Chroma,
    //   Mamba 2012, Orbweaver Chroma) because they need
    //   device-specific LED/effect packet routing instead of shared matrix I/O.
    // - Chroma ARGB Controller, Kraken classic/V3/V4, Hanbo, and other
    //   specialty controller families because they use dedicated controllers.
    // - Kraken Kitty Black Edition (VID/PID 1532:0F21) because it collides
    //   with Thunderbolt 4 Dock Chroma and Hypercolor's descriptor selection is
    //   still VID/PID-first.

    descriptors
});

/// Static Razer descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    RAZER_DESCRIPTORS.as_slice()
}
