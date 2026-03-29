//! Lian Li device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding, TransportType};

use super::protocol::{Ene6k77Protocol, LianLiHubVariant, TL_REPORT_ID, TlFanProtocol};

/// ENE-based Lian Li vendor ID.
pub const LIANLI_ENE_VENDOR_ID: u16 = 0x0CF2;
/// TL/Nuvoton Lian Li vendor ID.
pub const LIANLI_TL_VENDOR_ID: u16 = 0x0416;
/// ENE HID interface number used by modern UNI Hubs.
pub const LIANLI_ENE_INTERFACE: u8 = 1;
/// TL usage page used to select the correct HID collection.
pub const LIANLI_TL_USAGE_PAGE: u16 = 0xFF1B;

/// UNI FAN SL PID.
pub const PID_UNI_HUB_SL: u16 = 0xA100;
/// UNI FAN AL PID.
pub const PID_UNI_HUB_AL: u16 = 0xA101;
/// UNI FAN SL Infinity PID.
pub const PID_UNI_HUB_SL_INFINITY: u16 = 0xA102;
/// UNI FAN SL V2 PID.
pub const PID_UNI_HUB_SL_V2: u16 = 0xA103;
/// UNI FAN AL V2 PID.
pub const PID_UNI_HUB_AL_V2: u16 = 0xA104;
/// UNI FAN SL V2a PID.
pub const PID_UNI_HUB_SL_V2A: u16 = 0xA105;
/// UNI FAN SL Redragon PID.
pub const PID_UNI_HUB_SL_REDRAGON: u16 = 0xA106;
/// TL Fan Hub PID.
pub const PID_TL_FAN_HUB: u16 = 0x7372;

/// Build a UNI FAN SL protocol instance.
pub fn build_uni_hub_sl_protocol() -> Box<dyn Protocol> {
    Box::new(Ene6k77Protocol::new(LianLiHubVariant::Sl))
}

/// Build a UNI FAN AL protocol instance.
pub fn build_uni_hub_al_protocol() -> Box<dyn Protocol> {
    Box::new(Ene6k77Protocol::new(LianLiHubVariant::Al))
}

/// Build a UNI FAN SL Infinity protocol instance.
pub fn build_uni_hub_sl_infinity_protocol() -> Box<dyn Protocol> {
    Box::new(Ene6k77Protocol::new(LianLiHubVariant::SlInfinity))
}

/// Build a UNI FAN SL V2 protocol instance.
pub fn build_uni_hub_sl_v2_protocol() -> Box<dyn Protocol> {
    Box::new(Ene6k77Protocol::new(LianLiHubVariant::SlV2))
}

/// Build a UNI FAN AL V2 protocol instance.
pub fn build_uni_hub_al_v2_protocol() -> Box<dyn Protocol> {
    Box::new(Ene6k77Protocol::new(LianLiHubVariant::AlV2))
}

/// Build a UNI FAN SL Redragon protocol instance.
pub fn build_uni_hub_sl_redragon_protocol() -> Box<dyn Protocol> {
    Box::new(Ene6k77Protocol::new(LianLiHubVariant::SlRedragon))
}

/// Build a TL Fan protocol instance.
pub fn build_tl_fan_protocol() -> Box<dyn Protocol> {
    Box::new(TlFanProtocol::new())
}

macro_rules! ene_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: LIANLI_ENE_VENDOR_ID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::LianLi,
            transport: TransportType::UsbHid {
                interface: LIANLI_ENE_INTERFACE,
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static LIANLI_DESCRIPTORS: &[DeviceDescriptor] = &[
    ene_descriptor!(
        pid: PID_UNI_HUB_SL,
        name: "Lian Li Uni Hub - SL",
        protocol_id: "lianli/sl",
        builder: build_uni_hub_sl_protocol
    ),
    ene_descriptor!(
        pid: PID_UNI_HUB_AL,
        name: "Lian Li Uni Hub - AL",
        protocol_id: "lianli/al",
        builder: build_uni_hub_al_protocol
    ),
    ene_descriptor!(
        pid: PID_UNI_HUB_SL_INFINITY,
        name: "Lian Li Uni Hub - SL Infinity",
        protocol_id: "lianli/sl-infinity",
        builder: build_uni_hub_sl_infinity_protocol
    ),
    ene_descriptor!(
        pid: PID_UNI_HUB_SL_V2,
        name: "Lian Li Uni Hub - SL V2",
        protocol_id: "lianli/sl-v2",
        builder: build_uni_hub_sl_v2_protocol
    ),
    ene_descriptor!(
        pid: PID_UNI_HUB_AL_V2,
        name: "Lian Li Uni Hub - AL V2",
        protocol_id: "lianli/al-v2",
        builder: build_uni_hub_al_v2_protocol
    ),
    ene_descriptor!(
        pid: PID_UNI_HUB_SL_V2A,
        name: "Lian Li Uni Hub - SL V2a",
        protocol_id: "lianli/sl-v2",
        builder: build_uni_hub_sl_v2_protocol
    ),
    ene_descriptor!(
        pid: PID_UNI_HUB_SL_REDRAGON,
        name: "Lian Li Uni Hub - SL Redragon",
        protocol_id: "lianli/sl-redragon",
        builder: build_uni_hub_sl_redragon_protocol
    ),
    DeviceDescriptor {
        vendor_id: LIANLI_TL_VENDOR_ID,
        product_id: PID_TL_FAN_HUB,
        name: "Lian Li TL Fan Hub",
        family: DeviceFamily::LianLi,
        transport: TransportType::UsbHidApi {
            interface: None,
            report_id: TL_REPORT_ID,
            report_mode: HidRawReportMode::OutputReport,
            usage_page: Some(LIANLI_TL_USAGE_PAGE),
            usage: None,
        },
        protocol: ProtocolBinding {
            id: "lianli/tl-fan",
            build: build_tl_fan_protocol,
        },
        firmware_predicate: None,
    },
];

/// Static Lian Li descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    LIANLI_DESCRIPTORS
}
