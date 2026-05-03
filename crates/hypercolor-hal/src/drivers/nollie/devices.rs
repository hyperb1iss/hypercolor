//! Nollie OEM device registry entries.

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
#[cfg(windows)]
use crate::registry::HidRawReportMode;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::{
    GEN1_HID_REPORT_SIZE, GEN2_COLOR_REPORT_SIZE, NollieModel, NollieProtocol, ProtocolVersion,
};

pub const PRISM_VENDOR_ID: u16 = 0x16D5;
pub const NOLLIE_LEGACY_VENDOR_ID: u16 = 0x16D1;
pub const NOLLIE_VENDOR_ID: u16 = 0x16D2;
pub const NOLLIE_MATRIX_VENDOR_ID: u16 = 0x16D3;
pub const NOLLIE_GEN2_VENDOR_ID: u16 = 0x3061;

pub const PID_PRISM_8: u16 = 0x1F01;
pub const PID_NOLLIE_1: u16 = 0x1F11;
pub const PID_NOLLIE_8_V2: u16 = 0x1F01;
pub const PID_NOLLIE_28_12_A: u16 = 0x1616;
pub const PID_NOLLIE_L1_V12: u16 = 0x1617;
pub const PID_NOLLIE_L2_V12: u16 = 0x1618;
pub const PID_NOLLIE_16_V3: u16 = 0x4716;
pub const PID_NOLLIE_32: u16 = 0x4714;
pub const PID_NOLLIE_4: u16 = 0x4711;
pub const PID_NOLLIE_8_YOUTH: u16 = 0x4712;
pub const PID_NOLLIE_CDC_1: u16 = 0x2A01;
pub const PID_NOLLIE_CDC_8: u16 = 0x2A08;
pub const PID_NOLLIE_NOS2_16_V3_ALT: u16 = 0x2A16;
pub const PID_NOLLIE_NOS2_32_ALT: u16 = 0x2A32;
pub const PID_NOLLIE_MATRIX: u16 = 0x0001;
pub const PID_NOLLIE_LEGACY_8: u16 = 0x1612;
pub const PID_NOLLIE_LEGACY_16_1: u16 = 0x1613;
pub const PID_NOLLIE_LEGACY_16_2: u16 = 0x1615;
pub const PID_NOLLIE_LEGACY_28_12: u16 = 0x1616;
pub const PID_NOLLIE_LEGACY_28_L1: u16 = 0x1617;
pub const PID_NOLLIE_LEGACY_28_L2: u16 = 0x1618;
pub const PID_NOLLIE_LEGACY_2: u16 = 0x1619;
pub const PID_NOLLIE_LEGACY_TT: u16 = 0x1620;
pub const PID_NOLLIE_V12_8: u16 = 0x1612;
pub const PID_NOLLIE_V12_16_1: u16 = 0x1613;
pub const PID_NOLLIE_V12_16_2: u16 = 0x1615;

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

pub fn build_nollie_1_cdc_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie1Cdc))
}

pub fn build_nollie_8_cdc_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie8Cdc))
}

pub fn build_nollie_16_v3_nos2_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie16v3Nos2))
}

pub fn build_nollie_32_nos2_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie32Nos2))
}

pub fn build_nollie_matrix_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieMatrix))
}

pub fn build_nollie_legacy_8_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy8))
}

pub fn build_nollie_legacy_2_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy2))
}

pub fn build_nollie_legacy_tt_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacyTt))
}

pub fn build_nollie_legacy_16_1_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy16_1))
}

pub fn build_nollie_legacy_16_2_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy16_2))
}

pub fn build_nollie_legacy_28_12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy28_12))
}

pub fn build_nollie_legacy_28_l1_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy28L1))
}

pub fn build_nollie_legacy_28_l2_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieLegacy28L2))
}

pub fn build_nollie_8_v12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie8V12))
}

pub fn build_nollie_16_1_v12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie16_1V12))
}

pub fn build_nollie_16_2_v12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie16_2V12))
}

pub fn build_nollie_l1_v12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieL1V12))
}

pub fn build_nollie_l2_v12_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::NollieL2V12))
}

pub fn build_nollie_4_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie4))
}

pub fn build_nollie_8_youth_protocol() -> Box<dyn Protocol> {
    Box::new(NollieProtocol::new(NollieModel::Nollie8Youth))
}

#[cfg(windows)]
const fn nollie_hid_transport(interface: u8, max_report_len: usize) -> TransportType {
    TransportType::UsbHidApi {
        interface: Some(interface),
        report_id: 0x00,
        report_mode: HidRawReportMode::OutputReportWithReportId,
        max_report_len,
        usage_page: None,
        usage: None,
    }
}

#[cfg(not(windows))]
const fn nollie_hid_transport(interface: u8, _max_report_len: usize) -> TransportType {
    TransportType::UsbHid { interface }
}

macro_rules! nollie_descriptor {
    (
        vid: $vid:expr,
        pid: $pid:expr,
        name: $name:expr,
        family: $family:expr,
        protocol_id: $protocol_id:expr,
        interface: $interface:expr,
        max_report_len: $max_report_len:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: $vid,
            product_id: $pid,
            name: $name,
            family: $family,
            transport: nollie_hid_transport($interface, $max_report_len),
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

macro_rules! nollie_serial_descriptor {
    (
        vid: $vid:expr,
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: $vid,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::new_static("nollie", "Nollie"),
            transport: TransportType::UsbSerial { baud_rate: 115_200 },
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
        interface: 0,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_prism_8_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_1,
        name: "Nollie 1",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-1",
        interface: 0,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_1_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_8_V2,
        name: "Nollie 8 v2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-8-v2",
        interface: 0,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_8_v2_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_28_12_A,
        name: "Nollie 28/12",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-12",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_28_12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_L1_V12,
        name: "Nollie L1 v1.2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-l1-v12",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_l1_v12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_L2_V12,
        name: "Nollie L2 v1.2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-l2-v12",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_l2_v12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_GEN2_VENDOR_ID,
        pid: PID_NOLLIE_16_V3,
        name: "Nollie 16 v3",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-v3",
        interface: 0,
        max_report_len: GEN2_COLOR_REPORT_SIZE,
        builder: build_nollie_16_v3_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_GEN2_VENDOR_ID,
        pid: PID_NOLLIE_32,
        name: "Nollie 32",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-32",
        interface: 0,
        max_report_len: GEN2_COLOR_REPORT_SIZE,
        builder: build_nollie_32_protocol
    ),
    nollie_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_1,
        name: "Nollie 1 NOS2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-1-nos2",
        interface: 0,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_1_protocol
    ),
    nollie_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_16_V3,
        name: "Nollie 16 v3 NOS2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-v3-nos2",
        interface: 0,
        max_report_len: GEN2_COLOR_REPORT_SIZE,
        builder: build_nollie_16_v3_nos2_protocol
    ),
    nollie_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_NOS2_16_V3_ALT,
        name: "Nollie 16 v3 NOS2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-v3-nos2-alt",
        interface: 0,
        max_report_len: GEN2_COLOR_REPORT_SIZE,
        builder: build_nollie_16_v3_nos2_protocol
    ),
    nollie_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_32,
        name: "Nollie 32 NOS2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-32-nos2",
        interface: 0,
        max_report_len: GEN2_COLOR_REPORT_SIZE,
        builder: build_nollie_32_nos2_protocol
    ),
    nollie_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_NOS2_32_ALT,
        name: "Nollie 32 NOS2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-32-nos2-alt",
        interface: 0,
        max_report_len: GEN2_COLOR_REPORT_SIZE,
        builder: build_nollie_32_nos2_protocol
    ),
    nollie_serial_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_CDC_1,
        name: "Nollie 1 CDC",
        protocol_id: "nollie/nollie-1-cdc",
        builder: build_nollie_1_cdc_protocol
    ),
    nollie_serial_descriptor!(
        vid: PRISM_VENDOR_ID,
        pid: PID_NOLLIE_CDC_8,
        name: "Nollie 8 CDC",
        protocol_id: "nollie/nollie-8-cdc",
        builder: build_nollie_8_cdc_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_MATRIX_VENDOR_ID,
        pid: PID_NOLLIE_MATRIX,
        name: "Nollie Matrix",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-matrix",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_matrix_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_8,
        name: "Nollie 8",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-8-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_8_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_2,
        name: "Nollie 2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-2-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_2_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_TT,
        name: "Nollie TT",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-tt-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_tt_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_16_1,
        name: "Nollie 16 #1",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-1-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_16_1_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_16_2,
        name: "Nollie 16 #2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-2-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_16_2_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_28_12,
        name: "Nollie 28/12 legacy",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-12-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_28_12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_28_L1,
        name: "Nollie 28 L1",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-l1-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_28_l1_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_LEGACY_VENDOR_ID,
        pid: PID_NOLLIE_LEGACY_28_L2,
        name: "Nollie 28 L2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-28-l2-legacy",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_legacy_28_l2_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_V12_8,
        name: "Nollie 8 v1.2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-8-v12",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_8_v12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_V12_16_1,
        name: "Nollie 16 #1 v1.2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-1-v12",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_16_1_v12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_VENDOR_ID,
        pid: PID_NOLLIE_V12_16_2,
        name: "Nollie 16 #2 v1.2",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-16-2-v12",
        interface: 2,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_16_2_v12_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_GEN2_VENDOR_ID,
        pid: PID_NOLLIE_4,
        name: "Nollie 4",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-4",
        interface: 3,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_4_protocol
    ),
    nollie_descriptor!(
        vid: NOLLIE_GEN2_VENDOR_ID,
        pid: PID_NOLLIE_8_YOUTH,
        name: "Nollie 8 Youth",
        family: DeviceFamily::new_static("nollie", "Nollie"),
        protocol_id: "nollie/nollie-8-youth",
        interface: 3,
        max_report_len: GEN1_HID_REPORT_SIZE,
        builder: build_nollie_8_youth_protocol
    ),
];

#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    NOLLIE_DESCRIPTORS
}
