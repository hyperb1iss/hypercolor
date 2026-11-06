//! Wooting per-key RGB driver (Spec 90).
//!
//! Drives the analog keyboards and the UwU keypad through the vendor
//! configuration HID collection, the same one Wooting's RGB SDK uses. Three
//! wire generations exist: v1 firmware on the Atmel VID, v2 firmware on the
//! `0x1337` usage page, and firmware 2.12 and newer on the multi-report
//! `0xFF55` usage page.

pub mod packet;
pub mod protocol;
pub mod types;
pub mod v1;

pub use protocol::WootingProtocol;
pub use types::{
    CONFIG_USAGE_PAGE, GamepadMode, MULTI_REPORT_USAGE_PAGE, WOOTING_LEGACY_VID, WOOTING_VID,
    WootingInterface, WootingModel,
};
