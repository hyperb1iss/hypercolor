//! Lian Li UNI Hub driver family.

pub mod devices;
pub mod protocol;

pub use devices::{
    LIANLI_ENE_INTERFACE, LIANLI_ENE_VENDOR_ID, LIANLI_TL_USAGE_PAGE, LIANLI_TL_VENDOR_ID,
    PID_TL_FAN_HUB, PID_UNI_HUB_AL, PID_UNI_HUB_AL_V2, PID_UNI_HUB_SL, PID_UNI_HUB_SL_INFINITY,
    PID_UNI_HUB_SL_REDRAGON, PID_UNI_HUB_SL_V2, PID_UNI_HUB_SL_V2A, build_tl_fan_protocol,
    build_uni_hub_al_protocol, build_uni_hub_al_v2_protocol, build_uni_hub_sl_infinity_protocol,
    build_uni_hub_sl_protocol, build_uni_hub_sl_redragon_protocol, build_uni_hub_sl_v2_protocol,
    descriptors,
};
pub use protocol::{
    ENE_COMMAND_DELAY, ENE_REPORT_ID, Ene6k77Protocol, LianLiHubVariant, TL_REPORT_ID,
    TlFanProtocol, apply_al_white_limit, apply_sum_white_limit, firmware_version_from_fine_tune,
};
