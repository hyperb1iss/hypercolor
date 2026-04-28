//! Nollie OEM device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::{NollieModel, NollieProtocol, ProtocolVersion};

pub const PRISM_VENDOR_ID: u16 = 0x16D5;
pub const NOLLIE_VENDOR_ID: u16 = 0x16D2;
pub const NOLLIE_GEN2_VENDOR_ID: u16 = 0x3061;

pub const PID_PRISM_8: u16 = 0x1F01;
pub const PID_NOLLIE_1: u16 = 0x1F11;
pub const PID_NOLLIE_8_V2: u16 = 0x1F01;
pub const PID_NOLLIE_28_12_A: u16 = 0x1616;
pub const PID_NOLLIE_28_12_B: u16 = 0x1617;
pub const PID_NOLLIE_28_12_C: u16 = 0x1618;
pub const PID_NOLLIE_16_V3: u16 = 0x4716;
pub const PID_NOLLIE_32: u16 = 0x4714;

pub fn build_prism_8_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Prism8))
}

pub fn build_nollie_1_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie1))
}

pub fn build_nollie_8_v2_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie8))
}

pub fn build_nollie_28_12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie28_12))
}

pub fn build_nollie_16_v3_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie16v3))
}

pub fn build_nollie_32_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie32 {
        protocol_version: ProtocolVersion::V2,
    }))
}

macro_rules! nollie_descriptor {
    (
        vid: $vid:expr,
        pid: $pid:expr,
        name: $name:expr,
        family: $family:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: $vid,
            product_id: $pid,
            name: $name,
            family: $family,
            transport: TransportType::UsbHid { interface: 0 },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static NOLLIE_DESCRIPTORS: &[DeviceDescriptor] = &[
    nollie_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_PRISM_8,
        name: "PrismRGB Prism 8",
        family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
        protocol_id: "nollie/prism-8",
        builder: build_prism_8_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_1,
        name: "Nollie 1",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-1",
        builder: build_nollie_1_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_8_V2,
        name: "Nollie 8 v2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-8-v2",
        builder: build_nollie_8_v2_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_28_12_A,
        name: "Nollie 28/12",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-12",
        builder: build_nollie_28_12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_28_12_B,
        name: "Nollie 28/12 (rev B)",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-12-b",
        builder: build_nollie_28_12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_28_12_C,
        name: "Nollie 28/12 (rev C)",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-12-c",
        builder: build_nollie_28_12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_GEN2_VENDOR_ID,
        pid: PID_NOLLIE_16_V3,
        name: "Nollie 16 v3",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-v3",
        builder: build_nollie_16_v3_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_GEN2_VENDOR_ID,
        pid: PID_NOLLIE_32,
        name: "Nollie 32",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-32",
        builder: build_nollie_32_protocol
    ),
];

#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    NOLLIE_DESCRIPTORS
}
