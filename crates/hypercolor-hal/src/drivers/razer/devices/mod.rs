//! Self-contained Razer device registry entries.
//!
//! This registry imports shared-matrix Razer devices that map cleanly onto
//! Hypercolor's shared `RazerProtocol` transport model.
//! Specialty controller families that require different packet formats or
//! multi-interface coordination remain explicitly excluded until they are
//! ported as dedicated drivers.
//!
//! The descriptors themselves live in per-family modules ([`keyboards`],
//! [`mice`], [`peripherals`], [`laptops`], [`mousepads`]). This module owns
//! the shared protocol builders, descriptor helpers, and the single
//! [`static@RAZER_DESCRIPTORS`] that assembles the final registry.

mod keyboards;
mod laptops;
mod mice;
mod mousepads;
mod peripherals;

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
    keyboards::push_all(&mut descriptors);
    mice::push_all(&mut descriptors);
    peripherals::push_all(&mut descriptors);
    laptops::push_all(&mut descriptors);
    mousepads::push_all(&mut descriptors);

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
