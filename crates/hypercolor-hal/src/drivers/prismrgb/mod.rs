//! `PrismRGB` and Nollie protocol driver family.

pub mod devices;
pub mod protocol;

pub use devices::{
    PID_PRISM_MINI, PID_PRISM_S, PRISM_GCS_VENDOR_ID, build_prism_mini_protocol,
    build_prism_s_protocol, descriptors,
};
pub use protocol::{
    HID_REPORT_SIZE, LOW_POWER_THRESHOLD, PrismRgbModel, PrismRgbProtocol, PrismSConfig,
    PrismSGpuCable, apply_low_power_saver, compress_color_pair,
};
