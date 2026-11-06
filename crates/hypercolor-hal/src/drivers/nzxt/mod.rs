//! NZXT lighting drivers (Spec 39).
//!
//! Phase 1 covers the Gen2 `0x22` stream family (Hue 2, Smart Device V2,
//! RGB and Fan Controller, Control Hub, NZXT motherboards, Kraken X3 and Z3)
//! and the Kraken Elite v2 pump ring. Nothing here has run on NZXT hardware;
//! every descriptor is unverified. Spec 39 §9.4 lists what is held back and
//! why.

pub mod devices;
pub mod gen2;
pub mod kraken_elite_v2;
pub mod kraken_modern;
pub mod motherboard;
pub mod packet;
pub mod topology;

/// NZXT USB vendor ID.
pub const NZXT_VID: u16 = 0x1E71;

pub use devices::{
    N7_B650E_FAMILY_HEADERS, N7_Z390_HEADERS, N7_Z490_FAMILY_HEADERS, NZXT_GEN2_INTERFACE,
    NZXT_GEN2_TRANSPORT_INTENT, NZXT_KRAKEN_ELITE_V2_TRANSPORT_INTENT,
    NZXT_KRAKEN_SCREEN_INTERFACE, NZXT_KRAKEN_Z3_TRANSPORT_INTENT, PID_CONTROL_HUB, PID_HUE_2,
    PID_HUE_2_AMBIENT, PID_KRAKEN_ELITE_V2, PID_KRAKEN_X3, PID_KRAKEN_X3_RGB, PID_KRAKEN_Z3,
    PID_N5_Z690, PID_N7_B550, PID_N7_B650E, PID_N7_Z390, PID_N7_Z490, PID_N7_Z590, PID_N7_Z690,
    PID_N7_Z790, PID_RGB_CONTROLLER_2012, PID_RGB_CONTROLLER_2021, PID_RGB_FAN_CONTROLLER_200E,
    PID_RGB_FAN_CONTROLLER_201F, PID_RGB_FAN_CONTROLLER_2009, PID_RGB_FAN_CONTROLLER_2010,
    PID_RGB_FAN_CONTROLLER_2011, PID_RGB_FAN_CONTROLLER_2019, PID_RGB_FAN_CONTROLLER_2020,
    PID_SMART_DEVICE_V2, PID_SMART_DEVICE_V2_CASE_200D, PID_SMART_DEVICE_V2_CASE_200F, descriptors,
};
pub use gen2::{ChannelDiscovery, GEN2_FRAME_INTERVAL, Gen2Profile, NzxtGen2Protocol};
pub use kraken_elite_v2::NzxtKrakenEliteV2Protocol;
pub use kraken_modern::{KRAKEN_EXTERNAL_LED_CAP, KrakenX3Logo, kraken_x3_logo_packet};
pub use motherboard::{AnalogColorOrder, MotherboardHeaders, analog_header_packet};
pub use topology::{
    ACCESSORY_CATALOG, Accessory, AccessoryShape, Gen2Channel, Gen2Topology, lookup_accessory,
    parse_topology_response,
};
