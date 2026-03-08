//! ASUS Aura protocol driver family.

pub mod devices;
pub mod protocol;
pub mod types;

pub use devices::{
    PID_AURA_ADDRESSABLE_GEN1, PID_AURA_ADDRESSABLE_GEN2, PID_AURA_ADDRESSABLE_GEN3,
    PID_AURA_ADDRESSABLE_GEN4, PID_AURA_MOTHERBOARD_GEN1, PID_AURA_MOTHERBOARD_GEN2,
    PID_AURA_MOTHERBOARD_GEN3, PID_AURA_MOTHERBOARD_GEN4, PID_AURA_MOTHERBOARD_GEN5,
    PID_AURA_TERMINAL, build_aura_addressable_gen1_protocol, build_aura_addressable_gen2_protocol,
    build_aura_addressable_gen3_protocol, build_aura_addressable_gen4_protocol,
    build_aura_motherboard_gen1_protocol, build_aura_motherboard_gen2_protocol,
    build_aura_motherboard_gen3_protocol, build_aura_motherboard_gen4_protocol,
    build_aura_motherboard_gen5_protocol, build_aura_terminal_protocol, descriptors,
};
pub use protocol::{AuraUsbProtocol, build_effect_color_payload};
pub use types::{
    ASUS_VID, AURA_DIRECT_LED_CHUNK, AURA_DIRECT_LED_MAX, AURA_REPORT_ID, AURA_REPORT_PAYLOAD_LEN,
    AURA_TERMINAL_CHANNEL_LEDS, AuraColorOrder, AuraCommand, AuraControllerGen, AuraInitPhase,
    MAINBOARD_DIRECT_IDX, led_mask,
};
