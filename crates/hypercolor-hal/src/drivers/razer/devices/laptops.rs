//! Razer laptop descriptors.
//!
//! Blade 14 / 15 / 17 / Pro / Stealth family and Book 13. All use USB
//! control-transfer transport instead of HID.

use crate::registry::DeviceDescriptor;

use super::{
    PID_BLADE_14_2021, PID_BLADE_14_2023, PID_BLADE_15_2022, PID_BLADE_15_LATE_2021_ADVANCED,
    PID_BLADE_PRO_2016, build_blade_14_2021_protocol, build_blade_14_2023_protocol,
    build_blade_15_2022_protocol, build_blade_15_late_2021_advanced_protocol,
    build_blade_pro_2016_protocol, build_matrix_standard_extended_1x1_laptop_protocol,
    build_matrix_standard_extended_1x16_laptop_protocol,
    build_matrix_standard_extended_6x16_laptop_protocol,
    build_matrix_standard_extended_6x25_laptop_protocol,
    build_matrix_standard_modern_1x16_laptop_protocol,
    build_matrix_standard_modern_6x16_laptop_keepalive_protocol, control_descriptor,
    push_control_group,
};

pub(super) fn push_all(descriptors: &mut Vec<DeviceDescriptor>) {
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
        descriptors,
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
        descriptors,
        "razer/matrix-standard-3f-laptop-6x25",
        build_matrix_standard_extended_6x25_laptop_protocol,
        &[
            (0x0225, "Razer Blade Pro (2017)"),
            (0x022F, "Razer Blade Pro (2017 FullHD)"),
        ],
    );
    push_control_group(
        descriptors,
        "razer/matrix-standard-3f-laptop-1x1",
        build_matrix_standard_extended_1x1_laptop_protocol,
        &[
            (0x0246, "Razer Blade 15 (2019 Base)"),
            (0x024A, "Razer Blade Stealth (Late 2019)"),
            (0x0252, "Razer Blade Stealth (2020)"),
        ],
    );
    push_control_group(
        descriptors,
        "razer/matrix-standard-3f-laptop-1x16",
        build_matrix_standard_extended_1x16_laptop_protocol,
        &[
            (0x0255, "Razer Blade 15 (2020 Base)"),
            (0x026F, "Razer Blade 15 (2021 Base)"),
            (0x0259, "Razer Blade Stealth (Late 2020)"),
        ],
    );
    push_control_group(
        descriptors,
        "razer/matrix-standard-1f-laptop-1x16",
        build_matrix_standard_modern_1x16_laptop_protocol,
        &[(0x027A, "Razer Blade 15 (2021 Base)")],
    );
    push_control_group(
        descriptors,
        "razer/matrix-standard-1f-laptop-6x16",
        build_matrix_standard_modern_6x16_laptop_keepalive_protocol,
        &[(0x028C, "Razer Blade 14 (2022)")],
    );
}
