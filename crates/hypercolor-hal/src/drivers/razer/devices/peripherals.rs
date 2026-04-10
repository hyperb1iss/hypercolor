//! Razer peripheral descriptors.
//!
//! Keypads (Tartarus), headsets (Kraken, Tiamat), microphones (Seiren),
//! speakers (Nommo, Leviathan), docks and hubs (Mouse Dock, Base Station,
//! Thunderbolt Dock, Charging Pad, Mouse Bungee, Laptop Stand, Mug Holder),
//! enclosures (Core, Chroma PC Case Lighting Kit, Chroma HDK), and
//! case-mounted Razer Edition gear.

use crate::registry::DeviceDescriptor;

use super::{
    HID_REPORT_ID_ALT_0X07, HID_REPORT_ID_DEFAULT, PID_SEIREN_EMOTE, PID_SEIREN_V3_CHROMA,
    PID_TARTARUS_CHROMA, RAZER_CONSUMER_USAGE, RAZER_CONSUMER_USAGE_PAGE, RAZER_VENDOR_USAGE,
    RAZER_VENDOR_USAGE_PAGE, build_matrix_extended_extended_1x15_zero_protocol,
    build_matrix_extended_extended_1x17_backlight_protocol,
    build_matrix_extended_extended_2x8_backlight_protocol,
    build_matrix_extended_extended_2x16_backlight_protocol,
    build_matrix_extended_extended_2x24_backlight_protocol,
    build_matrix_extended_extended_4x16_zero_protocol,
    build_matrix_extended_extended_1x12_backlight_protocol,
    build_matrix_extended_modern_1x4_backlight_protocol,
    build_matrix_extended_modern_1x8_zero_protocol,
    build_matrix_extended_modern_1x10_zero_device_mode_protocol,
    build_matrix_extended_modern_1x14_zero_protocol,
    build_matrix_extended_modern_1x15_zero_protocol,
    build_matrix_extended_modern_2x9_zero_protocol,
    build_matrix_extended_modern_4x6_zero_protocol,
    build_matrix_extended_modern_4x16_zero_protocol,
    build_matrix_linear_extended_1x15_backlight_device_mode_protocol,
    build_matrix_standard_extended_1x9_backlight_protocol, build_seiren_emote_protocol,
    build_seiren_v3_protocol, build_tartarus_chroma_protocol, hidapi_descriptor, push_hidapi_group,
};

#[expect(
    clippy::too_many_lines,
    reason = "Device registry tables stay in one place per family for auditability against shared matrix builders"
)]
pub(super) fn push_all(descriptors: &mut Vec<DeviceDescriptor>) {
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

    push_hidapi_group(
        descriptors,
        "razer/matrix-standard-3f-1x9",
        build_matrix_standard_extended_1x9_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0xFF00),
        Some(0x0001),
        &[(0x0215, "Razer Core")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-linear-3f-1x15-device-mode",
        build_matrix_linear_extended_1x15_backlight_device_mode_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F07, "Razer Chroma Mug Holder")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-1x15-zero",
        build_matrix_extended_extended_1x15_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F08, "Razer Base Station Chroma")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-2x8-backlight",
        build_matrix_extended_extended_2x8_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0000),
        &[(0x0518, "Razer Nommo Pro")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-2x24-backlight",
        build_matrix_extended_extended_2x24_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0000),
        &[(0x0517, "Razer Nommo Chroma")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-2x16-backlight",
        build_matrix_extended_extended_2x16_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F0E, "Razer Chroma PC Case Lighting Kit")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-4x16-zero",
        build_matrix_extended_extended_4x16_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F09, "Razer Chroma HDK")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-1x17-backlight",
        build_matrix_extended_extended_1x17_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        None,
        None,
        &[(0x0F03, "Razer Tiamat 7.1 V2")],
    );

    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-extended-1f-1x10-zero-device-mode",
        build_matrix_extended_modern_1x10_zero_device_mode_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x000C),
        Some(0x0001),
        &[(0x0F26, "Razer Charging Pad Chroma")],
    );
    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-extended-1f-2x9-zero",
        build_matrix_extended_modern_2x9_zero_protocol,
        Some(2),
        HID_REPORT_ID_ALT_0X07,
        Some(0x000C),
        Some(0x0001),
        &[(0x0532, "Razer Leviathan V2")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-4x16-zero",
        build_matrix_extended_modern_4x16_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0F13, "Lian Li O11 Dynamic - Razer Edition")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-4x6-zero",
        build_matrix_extended_modern_4x6_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x022B, "Razer Tartarus V2")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-1x14-zero",
        build_matrix_extended_modern_1x14_zero_protocol,
        Some(0),
        HID_REPORT_ID_ALT_0X07,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x054A, "Razer Leviathan V2 X")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-1x4-backlight",
        build_matrix_extended_modern_1x4_backlight_protocol,
        Some(1),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0003),
        &[(0x0F19, "Razer Kraken Kitty Edition")],
    );

    push_hidapi_group(
        descriptors,
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
}
