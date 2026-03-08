//! Razer protocol driver family.

pub mod crc;
pub mod devices;
pub mod protocol;
pub mod seiren_v3;
pub mod types;

pub use crc::{RAZER_REPORT_LEN, razer_crc};
pub use devices::{
    PID_BASILISK_V3, PID_BLADE_14_2021, PID_BLADE_14_2023, PID_BLADE_15_2022,
    PID_BLADE_15_LATE_2021_ADVANCED, PID_HUNTSMAN_V2, PID_SEIREN_EMOTE, PID_SEIREN_V3_CHROMA,
    RAZER_VENDOR_ID, build_basilisk_v3_protocol, build_blade_14_2021_protocol,
    build_blade_14_2023_protocol, build_blade_15_2022_protocol,
    build_blade_15_late_2021_advanced_protocol, build_huntsman_v2_protocol,
    build_seiren_emote_protocol, build_seiren_v3_protocol, descriptors,
};
pub use protocol::RazerProtocol;
pub use seiren_v3::SeirenV3Protocol;
pub use types::{
    EFFECT_CUSTOM_FRAME, LED_ID_BACKLIGHT, LED_ID_LOGO, LED_ID_SCROLL_WHEEL, LED_ID_ZERO, NOSTORE,
    RazerLightingCommandSet, RazerMatrixType, RazerProtocolVersion, VARSTORE,
};
