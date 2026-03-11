//! QMK OpenRGB protocol driver for per-key RGB keyboards.
//!
//! Supports keyboards running QMK firmware with the OpenRGB feature enabled.
//! Protocol revisions 9, B/C, and D/E are implemented.

pub mod devices;
pub mod protocol;
pub mod types;

pub use devices::{
    VID_CANNONKEYS, VID_DROP, VID_GLORIOUS, VID_IDOBAO, VID_KBDFANS, VID_KEYCHRON, VID_SONIX,
    VID_WCH, VID_ZSA, descriptors,
};
pub use protocol::{QmkKeyboardConfig, QmkProtocol};
pub use types::{Command, PACKET_SIZE, ProtocolRevision, QmkMode, USAGE_ID, USAGE_PAGE};
