//! `PrismRGB` and Nollie protocol driver family.

pub mod devices;
pub mod protocol;

pub use devices::{
    NOLLIE_VENDOR_ID, PID_NOLLIE_8_V2, PID_PRISM_8, PID_PRISM_MINI, PID_PRISM_S,
    PRISM_GCS_VENDOR_ID, PRISM_VENDOR_ID, build_nollie_8_v2_protocol, build_prism_8_protocol,
    build_prism_mini_protocol, build_prism_s_protocol, descriptors,
};
pub use protocol::{
    HID_REPORT_SIZE, LOW_POWER_THRESHOLD, PrismRgbModel, PrismRgbProtocol, apply_low_power_saver,
    compress_color_pair,
};
