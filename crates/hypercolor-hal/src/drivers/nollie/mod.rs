//! Nollie OEM USB HID protocol driver family.

pub mod devices;
mod gen1;
mod gen2;
mod legacy;
mod nos2;
pub mod protocol;
mod serial;
mod stream65;

pub use devices::{
    NOLLIE_GEN2_VENDOR_ID, NOLLIE_LEGACY_VENDOR_ID, NOLLIE_MATRIX_VENDOR_ID, NOLLIE_VENDOR_ID,
    PID_NOLLIE_1, PID_NOLLIE_4, PID_NOLLIE_8_V2, PID_NOLLIE_8_YOUTH, PID_NOLLIE_16_V3,
    PID_NOLLIE_28_12_A, PID_NOLLIE_32, PID_NOLLIE_CDC_1, PID_NOLLIE_CDC_8, PID_NOLLIE_L1_V12,
    PID_NOLLIE_L2_V12, PID_NOLLIE_LEGACY_2, PID_NOLLIE_LEGACY_8, PID_NOLLIE_LEGACY_16_1,
    PID_NOLLIE_LEGACY_16_2, PID_NOLLIE_LEGACY_28_12, PID_NOLLIE_LEGACY_28_L1,
    PID_NOLLIE_LEGACY_28_L2, PID_NOLLIE_LEGACY_TT, PID_NOLLIE_MATRIX, PID_NOLLIE_NOS2_16_V3_ALT,
    PID_NOLLIE_NOS2_32_ALT, PID_NOLLIE_V12_8, PID_NOLLIE_V12_16_1, PID_NOLLIE_V12_16_2,
    PID_PRISM_8, PRISM_VENDOR_ID, build_nollie_1_cdc_protocol, build_nollie_1_protocol,
    build_nollie_4_protocol, build_nollie_8_cdc_protocol, build_nollie_8_v2_protocol,
    build_nollie_8_v12_protocol, build_nollie_8_youth_protocol, build_nollie_16_1_v12_protocol,
    build_nollie_16_2_v12_protocol, build_nollie_16_v3_nos2_protocol, build_nollie_16_v3_protocol,
    build_nollie_28_12_protocol, build_nollie_32_nos2_protocol, build_nollie_32_protocol,
    build_nollie_l1_v12_protocol, build_nollie_l2_v12_protocol, build_nollie_legacy_2_protocol,
    build_nollie_legacy_8_protocol, build_nollie_legacy_16_1_protocol,
    build_nollie_legacy_16_2_protocol, build_nollie_legacy_28_12_protocol,
    build_nollie_legacy_28_l1_protocol, build_nollie_legacy_28_l2_protocol,
    build_nollie_legacy_tt_protocol, build_nollie_matrix_protocol, build_prism_8_protocol,
    descriptors,
};
pub use protocol::{
    CDC_SERIAL_REPORT_SIZE, GEN1_HID_REPORT_SIZE, GEN2_COLOR_REPORT_SIZE,
    GEN2_SETTINGS_REPORT_SIZE, GpuCableType, Nollie32Config, NollieModel, NollieProtocol,
    ProtocolVersion,
};
