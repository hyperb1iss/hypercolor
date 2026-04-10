//! Razer mouse descriptors.
//!
//! Basilisk, `DeathAdder`, Mamba, Viper, Naga, Lancehead, Orochi, Abyssus,
//! Cobra, Diamondback family.

use crate::registry::DeviceDescriptor;

use super::{
    HID_REPORT_ID_DEFAULT, PID_BASILISK_V3, PID_MAMBA_ELITE, RAZER_CONSUMER_USAGE,
    RAZER_CONSUMER_USAGE_PAGE, build_basilisk_v3_protocol, build_basilisk_v3_x_hyperspeed_protocol,
    build_mamba_elite_protocol, build_matrix_extended_extended_1x1_backlight_protocol,
    build_matrix_extended_extended_1x2_backlight_protocol,
    build_matrix_extended_extended_1x2_zero_protocol,
    build_matrix_extended_extended_1x16_backlight_protocol,
    build_matrix_extended_modern_1x1_backlight_protocol,
    build_matrix_extended_modern_1x1_zero_protocol, build_matrix_extended_modern_1x2_zero_protocol,
    build_matrix_extended_modern_1x3_backlight_protocol,
    build_matrix_extended_modern_1x3_zero_protocol,
    build_matrix_extended_modern_1x11_zero_protocol,
    build_matrix_extended_modern_1x13_zero_protocol,
    build_matrix_extended_modern_1x14_zero_protocol,
    build_matrix_legacy_extended_1x9_zero_protocol,
    build_matrix_linear_extended_1x3_backlight_protocol,
    build_matrix_linear_extended_1x15_backlight_protocol,
    build_matrix_linear_extended_1x16_backlight_protocol,
    build_matrix_linear_extended_1x21_backlight_protocol,
    build_matrix_standard_extended_1x3_backlight_protocol, hidapi_descriptor, push_hidapi_group,
};

#[expect(
    clippy::too_many_lines,
    reason = "Device registry tables stay in one place per family for auditability against shared matrix builders"
)]
pub(super) fn push_all(descriptors: &mut Vec<DeviceDescriptor>) {
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
        PID_MAMBA_ELITE,
        "Razer Mamba Elite",
        "razer/mamba-elite",
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        build_mamba_elite_protocol,
    ));

    push_hidapi_group(
        descriptors,
        "razer/matrix-standard-3f-1x3",
        build_matrix_standard_extended_1x3_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0050, "Razer Naga Hex V2")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-linear-3f-1x3",
        build_matrix_linear_extended_1x3_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0053, "Razer Naga Chroma")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-linear-3f-1x15",
        build_matrix_linear_extended_1x15_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[
            (0x0044, "Razer Mamba 2015 (Wired)"),
            (0x0045, "Razer Mamba (Wireless)"),
        ],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-linear-3f-1x16",
        build_matrix_linear_extended_1x16_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0046, "Razer Mamba Tournament Edition")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-linear-3f-1x21",
        build_matrix_linear_extended_1x21_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x004C, "Razer Diamondback Chroma")],
    );

    push_hidapi_group(
        descriptors,
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
        descriptors,
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
        descriptors,
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
        descriptors,
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
        descriptors,
        "razer/matrix-extended-1f-1x1-backlight",
        build_matrix_extended_modern_1x1_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x00A3, "Razer Cobra"), (0x0091, "Razer Viper 8kHz")],
    );
    push_hidapi_group(
        descriptors,
        "razer/basilisk-v3-x-hyperspeed",
        build_basilisk_v3_x_hyperspeed_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x00B9, "Razer Basilisk V3 X HyperSpeed")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-1x1-zero",
        build_matrix_extended_modern_1x1_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x008C, "Razer DeathAdder V2 Mini")],
    );
    push_hidapi_group(
        descriptors,
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
        descriptors,
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
        descriptors,
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
        descriptors,
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
        descriptors,
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
        descriptors,
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
        descriptors,
        "razer/matrix-extended-3f-1x11-zero-basilisk",
        build_basilisk_v3_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x00CB, "Razer Basilisk V3 35K")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-legacy-1x9-zero",
        build_matrix_legacy_extended_1x9_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x00A4, "Razer Mouse Dock Pro")],
    );
}
