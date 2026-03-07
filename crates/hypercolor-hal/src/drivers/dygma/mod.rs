//! Dygma Focus-protocol driver family.

pub mod devices;
pub mod protocol;

pub use devices::{
    DYGMA_VENDOR_ID, PID_DEFY_WIRED, PID_DEFY_WIRELESS, build_defy_wired_protocol,
    build_defy_wireless_protocol, descriptors,
};
pub use protocol::{DygmaProtocol, DygmaVariant, FocusColorMode, rgb_to_rgbw};
