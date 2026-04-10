//! Razer keyboard descriptors.
//!
//! Huntsman, `BlackWidow`, Cynosa, Ornata, `DeathStalker` keyboard family, and
//! tenkeyless / analog variants. Keypad hybrids like Tartarus live in
//! [`super::peripherals`].

use crate::registry::DeviceDescriptor;

use super::{
    HID_REPORT_ID_DEFAULT, PID_HUNTSMAN_V2, RAZER_CONSUMER_USAGE, RAZER_CONSUMER_USAGE_PAGE,
    build_huntsman_v2_protocol,
    build_matrix_extended_extended_5x15_backlight_protocol,
    build_matrix_extended_extended_6x17_backlight_protocol,
    build_matrix_extended_extended_6x18_backlight_protocol,
    build_matrix_extended_extended_6x19_backlight_protocol,
    build_matrix_extended_extended_6x22_backlight_protocol,
    build_matrix_extended_extended_9x22_backlight_protocol,
    build_matrix_extended_modern_5x15_backlight_protocol,
    build_matrix_extended_modern_5x16_backlight_protocol,
    build_matrix_extended_modern_6x17_zero_protocol,
    build_matrix_extended_modern_6x18_backlight_protocol,
    build_matrix_extended_modern_6x22_backlight_protocol,
    build_matrix_extended_modern_8x23_backlight_protocol, build_matrix_linear_extended_1x12_backlight_protocol,
    build_matrix_standard_extended_6x22_backlight_protocol,
    build_matrix_wireless_5x16_backlight_protocol, build_matrix_wireless_6x17_zero_protocol,
    build_matrix_wireless_6x22_zero_protocol, hidapi_descriptor, push_hidapi_group,
};

#[expect(
    clippy::too_many_lines,
    reason = "Device registry tables stay in one place per family for auditability against shared matrix builders"
)]
pub(super) fn push_all(descriptors: &mut Vec<DeviceDescriptor>) {
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

    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-linear-3f-1x12",
        build_matrix_linear_extended_1x12_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0204, "Razer DeathStalker Chroma")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-5x15-backlight",
        build_matrix_extended_extended_5x15_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0257, "Razer Huntsman Mini")],
    );
    push_hidapi_group(
        descriptors,
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
        descriptors,
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
        descriptors,
        "razer/matrix-extended-3f-9x22-backlight",
        build_matrix_extended_extended_9x22_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0226, "Razer Huntsman Elite")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-6x17-zero",
        build_matrix_extended_modern_6x17_zero_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0298, "Razer DeathStalker V2 Pro TKL (Wired)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-6x18-backlight",
        build_matrix_extended_modern_6x18_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x02B3, "Razer Blackwidow V4 Pro 75% (Wired)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-6x18-backlight",
        build_matrix_extended_modern_6x18_backlight_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x02B4, "Razer Blackwidow V4 Pro 75% (Wireless)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-6x22-backlight",
        build_matrix_extended_modern_6x22_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0295, "Razer Deathstalker V2")],
    );
    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-extended-3f-6x17-backlight",
        build_matrix_extended_extended_6x17_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x026B, "Razer Huntsman V2 TKL")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-3f-6x19-backlight",
        build_matrix_extended_extended_6x19_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x02A7, "Razer Huntsman V3 Pro TKL White")],
    );
    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-extended-3f-9x22-backlight",
        build_matrix_extended_extended_9x22_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0266, "Razer Huntsman V2 Analog")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-5x15-backlight",
        build_matrix_extended_modern_5x15_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0282, "Razer Huntsman Mini Analog")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-5x16-backlight",
        build_matrix_extended_modern_5x16_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0258, "Razer BlackWidow V3 Mini (Wired)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-wireless-5x16-backlight",
        build_matrix_wireless_5x16_backlight_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0271, "Razer BlackWidow V3 Mini (Wireless)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-wireless-6x17-zero",
        build_matrix_wireless_6x17_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0296, "Razer DeathStalker V2 Pro TKL (Wireless)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-wireless-6x22-zero",
        build_matrix_wireless_6x22_zero_protocol,
        Some(2),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0290, "Razer DeathStalker V2 Pro (Wireless)")],
    );
    push_hidapi_group(
        descriptors,
        "razer/matrix-wireless-6x22-zero",
        build_matrix_wireless_6x22_zero_protocol,
        Some(3),
        HID_REPORT_ID_DEFAULT,
        Some(RAZER_CONSUMER_USAGE_PAGE),
        Some(RAZER_CONSUMER_USAGE),
        &[(0x0292, "Razer DeathStalker V2 Pro (Wired)")],
    );
}
