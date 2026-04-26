//! Nollie OEM USB HID protocol driver family.

pub mod devices;
mod gen1;
mod gen2;
pub mod protocol;

pub use devices::{
    NOLLIE_GEN2_VENDOR_ID, NOLLIE_VENDOR_ID, PID_NOLLIE_1, PID_NOLLIE_8_V2, PID_NOLLIE_16_V3,
    PID_NOLLIE_28_12_A, PID_NOLLIE_28_12_B, PID_NOLLIE_28_12_C, PID_NOLLIE_32, PID_PRISM_8,
    PRISM_VENDOR_ID, build_nollie_1_protocol, build_nollie_8_v2_protocol,
    build_nollie_16_v3_protocol, build_nollie_28_12_protocol, build_nollie_32_protocol,
    build_prism_8_protocol, descriptors,
};
pub use protocol::{
    GEN1_HID_REPORT_SIZE, GEN2_COLOR_REPORT_SIZE, GEN2_SETTINGS_REPORT_SIZE, GpuCableType,
    Nollie32Config, NollieModel, NollieProtocol, ProtocolVersion,
};
