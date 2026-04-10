//! Razer mousepad descriptors.
//!
//! Firefly, Goliathus, Strider family.

use crate::registry::DeviceDescriptor;

use super::{
    HID_REPORT_ID_DEFAULT, build_matrix_extended_extended_1x1_zero_protocol,
    build_matrix_extended_extended_1x14_zero_device_mode_protocol,
    build_matrix_extended_extended_1x19_zero_protocol,
    build_matrix_extended_modern_1x1_zero_protocol,
    build_matrix_extended_modern_1x17_zero_protocol,
    build_matrix_linear_extended_1x15_backlight_protocol, push_hidapi_group,
};

pub(super) fn push_all(descriptors: &mut Vec<DeviceDescriptor>) {
    push_hidapi_group(
        descriptors,
        "razer/matrix-linear-3f-1x15",
        build_matrix_linear_extended_1x15_backlight_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0C00, "Razer Firefly")],
    );

    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-extended-3f-1x14-zero-device-mode",
        build_matrix_extended_extended_1x14_zero_device_mode_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0068, "Razer Firefly Hyperflux")],
    );

    push_hidapi_group(
        descriptors,
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
        descriptors,
        "razer/matrix-extended-1f-1x1-zero",
        build_matrix_extended_modern_1x1_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0C06, "Razer Goliathus Chroma 3XL")],
    );

    push_hidapi_group(
        descriptors,
        "razer/matrix-extended-1f-1x17-zero",
        build_matrix_extended_modern_1x17_zero_protocol,
        Some(0),
        HID_REPORT_ID_DEFAULT,
        Some(0x0001),
        Some(0x0002),
        &[(0x0C08, "Razer Firefly V2 Pro")],
    );
}
