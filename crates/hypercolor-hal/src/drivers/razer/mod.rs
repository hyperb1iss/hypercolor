//! Razer protocol driver family.

pub mod crc;
pub mod devices;
pub mod protocol;
pub mod types;

pub use crc::{RAZER_REPORT_LEN, razer_crc};
pub use devices::{
    PID_BASILISK_V3, PID_HUNTSMAN_V2, PID_SEIREN_EMOTE, RAZER_VENDOR_ID,
};
pub use protocol::RazerProtocol;
pub use types::{
    EFFECT_CUSTOM_FRAME, LED_ID_BACKLIGHT, LED_ID_LOGO, LED_ID_SCROLL_WHEEL, LED_ID_ZERO,
    NOSTORE, RazerMatrixType, RazerProtocolVersion, VARSTORE,
};
