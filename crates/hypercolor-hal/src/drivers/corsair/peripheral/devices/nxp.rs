//! Corsair NXP/CUE peripheral descriptors.
//!
//! Every descriptor here binds one vendor lighting collection (usage page
//! `0xFFC2`) on a pinned interface through HIDAPI. That path rides the OS
//! HID stack, so it never claims a USB interface or detaches `usbhid`, and
//! it has no fallback to a direct interrupt-endpoint transport. A keyboard
//! or mouse keeps typing and pointing while Hypercolor drives its lighting.
//!
//! The interface and usage are pinned, not just the page: NXP keyboards and
//! mice also expose an input-only `0xFFC2` collection on their composite
//! input interface, and the Polaris carries a keyboard collection on its
//! second interface.

use hypercolor_types::device::{DeviceFamily, DriverStability};

use crate::drivers::corsair::CORSAIR_VID;
use crate::drivers::corsair::peripheral::nxp::CorsairNxpProtocol;
use crate::drivers::corsair::peripheral::types::{
    CorsairPeripheralClass, CorsairPeripheralTopology, NXP_KEYBOARD_LED_SLOTS,
    NXP_MOUSE_ZONE_SLOTS, NXP_MOUSEPAD_ZONE_SLOTS, NXP_PACKET_SIZE, NxpDeviceConfig,
    NxpLightingKind,
};
use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding, TransportType};

/// Vendor-defined HID usage page of the NXP/CUE lighting collection.
pub const NXP_USAGE_PAGE: u16 = 0xFFC2;
/// Usage of the NXP command collection on keyboards and mice. The composite
/// input interface carries an input-only `0xFFC2` collection with usage
/// `0x0003`, which this filter excludes.
pub const NXP_COMMAND_USAGE: u16 = 0x0004;
/// Usage of the Polaris command collection.
pub const NXP_POLARIS_USAGE: u16 = 0x0001;
/// HID interface that carries the command collection on NXP keyboards and
/// mice. Interface 0 holds their boot-keyboard, NKRO, consumer, and pointer
/// collections.
pub const NXP_INTERFACE: u8 = 1;
/// HID interface that carries the command collection on the Polaris and
/// the ST100.
pub const NXP_ACCESSORY_INTERFACE: u8 = 0;
/// NXP packets carry no report ID; HIDAPI sends report number zero.
pub const NXP_REPORT_ID: u8 = 0x00;
/// HIDAPI buffer length: the report ID byte plus one 64-byte packet.
pub const NXP_MAX_REPORT_LEN: usize = NXP_PACKET_SIZE + 1;

pub const PID_K55_RGB: u16 = 0x1B3D;
pub const PID_K65_RGB: u16 = 0x1B17;
pub const PID_K65_LUX_RGB: u16 = 0x1B37;
pub const PID_K70_RGB: u16 = 0x1B13;
pub const PID_K70_LUX_RGB: u16 = 0x1B33;
pub const PID_K70_RGB_MK2: u16 = 0x1B49;
pub const PID_K95_RGB: u16 = 0x1B11;
pub const PID_K95_RGB_PLATINUM: u16 = 0x1B2D;
pub const PID_STRAFE_RGB: u16 = 0x1B20;
pub const PID_M65_RGB: u16 = 0x1B12;
pub const PID_SCIMITAR_RGB: u16 = 0x1B1E;
pub const PID_MM800_POLARIS: u16 = 0x1B3B;
pub const PID_ST100_RGB: u16 = 0x0A34;

/// Keyboards and mice: the command collection on interface 1.
pub const NXP_INPUT_DEVICE_TRANSPORT: TransportType =
    nxp_transport(NXP_INTERFACE, Some(NXP_COMMAND_USAGE));
/// Polaris: the command collection on interface 0 (interface 1 is a
/// keyboard collection).
pub const NXP_POLARIS_TRANSPORT: TransportType =
    nxp_transport(NXP_ACCESSORY_INTERFACE, Some(NXP_POLARIS_USAGE));
/// ST100: a single vendor-only interface.
pub const NXP_ST100_TRANSPORT: TransportType = nxp_transport(NXP_ACCESSORY_INTERFACE, None);

const fn nxp_transport(interface: u8, usage: Option<u16>) -> TransportType {
    TransportType::UsbHidApi {
        interface: Some(interface),
        report_id: NXP_REPORT_ID,
        report_mode: HidRawReportMode::OutputReport,
        max_report_len: NXP_MAX_REPORT_LEN,
        usage_page: Some(NXP_USAGE_PAGE),
        usage,
    }
}

const fn keyboard(name: &'static str, lighting_kind: NxpLightingKind) -> NxpDeviceConfig {
    NxpDeviceConfig::new(
        name,
        CorsairPeripheralClass::Keyboard,
        lighting_kind,
        NXP_KEYBOARD_LED_SLOTS,
        CorsairPeripheralTopology::Custom,
    )
}

const fn mouse(name: &'static str) -> NxpDeviceConfig {
    NxpDeviceConfig::new(
        name,
        CorsairPeripheralClass::Mouse,
        NxpLightingKind::Mouse,
        NXP_MOUSE_ZONE_SLOTS,
        CorsairPeripheralTopology::Strip,
    )
}

const fn accessory(name: &'static str, class: CorsairPeripheralClass) -> NxpDeviceConfig {
    NxpDeviceConfig::new(
        name,
        class,
        NxpLightingKind::Mousepad,
        NXP_MOUSEPAD_ZONE_SLOTS,
        CorsairPeripheralTopology::Strip,
    )
    .with_max_fps(30)
}

const K55_RGB: NxpDeviceConfig = NxpDeviceConfig::new(
    "Corsair K55 RGB",
    CorsairPeripheralClass::Keyboard,
    NxpLightingKind::ZonedKeyboard,
    3,
    CorsairPeripheralTopology::KeyboardMatrix { rows: 1, cols: 3 },
);
const K65_RGB: NxpDeviceConfig = keyboard("Corsair K65 RGB", NxpLightingKind::Packed512Keyboard);
const K65_LUX_RGB: NxpDeviceConfig =
    keyboard("Corsair K65 Lux RGB", NxpLightingKind::FullRangeKeyboard);
const K70_RGB: NxpDeviceConfig = keyboard("Corsair K70 RGB", NxpLightingKind::Packed512Keyboard);
const K70_LUX_RGB: NxpDeviceConfig =
    keyboard("Corsair K70 Lux RGB", NxpLightingKind::FullRangeKeyboard);
const K70_RGB_MK2: NxpDeviceConfig =
    keyboard("Corsair K70 RGB MK.2", NxpLightingKind::FullRangeKeyboard);
const K95_RGB: NxpDeviceConfig =
    keyboard("Corsair K95 RGB", NxpLightingKind::Packed512Keyboard).with_unclean_exit();
/// K95 Platinum light bar: the full-range plane slot of each bar LED, from
/// the leftmost LED to the rightmost (Spec 53 section 7.4).
pub const K95_PLATINUM_LIGHT_BAR_SLOTS: [u8; 19] = [
    0x90, 0x91, 0x92, 0x9E, 0xA0, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0x9B, 0x9F, 0xA2,
    0xA1, 0x9C, 0x9D,
];

const K95_RGB_PLATINUM: NxpDeviceConfig = keyboard(
    "Corsair K95 RGB Platinum",
    NxpLightingKind::FullRangeKeyboard,
)
.with_light_bar(&K95_PLATINUM_LIGHT_BAR_SLOTS);
const STRAFE_RGB: NxpDeviceConfig =
    keyboard("Corsair Strafe RGB", NxpLightingKind::FullRangeKeyboard);
const M65_RGB: NxpDeviceConfig = mouse("Corsair M65 RGB");
const SCIMITAR_RGB: NxpDeviceConfig = mouse("Corsair Scimitar RGB");
const MM800_POLARIS: NxpDeviceConfig = accessory(
    "Corsair MM800 RGB Polaris",
    CorsairPeripheralClass::Mousepad,
);
const ST100_RGB: NxpDeviceConfig =
    accessory("Corsair ST100 RGB", CorsairPeripheralClass::HeadsetStand);

fn build_protocol(config: NxpDeviceConfig) -> Box<dyn Protocol> {
    Box::new(CorsairNxpProtocol::new(config))
}

pub fn build_k55_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(K55_RGB)
}

pub fn build_k65_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(K65_RGB)
}

pub fn build_k65_lux_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(K65_LUX_RGB)
}

pub fn build_k70_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(K70_RGB)
}

pub fn build_k70_lux_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(K70_LUX_RGB)
}

pub fn build_k70_rgb_mk2_protocol() -> Box<dyn Protocol> {
    build_protocol(K70_RGB_MK2)
}

pub fn build_k95_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(K95_RGB)
}

pub fn build_k95_rgb_platinum_protocol() -> Box<dyn Protocol> {
    build_protocol(K95_RGB_PLATINUM)
}

pub fn build_strafe_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(STRAFE_RGB)
}

pub fn build_m65_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(M65_RGB)
}

pub fn build_scimitar_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(SCIMITAR_RGB)
}

pub fn build_mm800_polaris_protocol() -> Box<dyn Protocol> {
    build_protocol(MM800_POLARIS)
}

pub fn build_st100_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(ST100_RGB)
}

macro_rules! nxp_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        transport: $transport:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: CORSAIR_VID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::new_static("corsair", "Corsair"),
            transport: $transport,
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
            serial_quirk: None,
            stability: DriverStability::Experimental,
        }
    };
}

pub(super) static NXP_DESCRIPTORS: &[DeviceDescriptor] = &[
    nxp_descriptor!(
        pid: PID_K55_RGB,
        name: "Corsair K55 RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k55-rgb",
        builder: build_k55_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_K65_RGB,
        name: "Corsair K65 RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k65-rgb",
        builder: build_k65_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_K65_LUX_RGB,
        name: "Corsair K65 Lux RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k65-lux-rgb",
        builder: build_k65_lux_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_K70_RGB,
        name: "Corsair K70 RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k70-rgb",
        builder: build_k70_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_K70_LUX_RGB,
        name: "Corsair K70 Lux RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k70-lux-rgb",
        builder: build_k70_lux_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_K70_RGB_MK2,
        name: "Corsair K70 RGB MK.2",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k70-rgb-mk2",
        builder: build_k70_rgb_mk2_protocol
    ),
    nxp_descriptor!(
        pid: PID_K95_RGB,
        name: "Corsair K95 RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k95-rgb",
        builder: build_k95_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_K95_RGB_PLATINUM,
        name: "Corsair K95 RGB Platinum",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-k95-rgb-platinum",
        builder: build_k95_rgb_platinum_protocol
    ),
    nxp_descriptor!(
        pid: PID_STRAFE_RGB,
        name: "Corsair Strafe RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-strafe-rgb",
        builder: build_strafe_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_M65_RGB,
        name: "Corsair M65 RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-m65-rgb",
        builder: build_m65_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_SCIMITAR_RGB,
        name: "Corsair Scimitar RGB",
        transport: NXP_INPUT_DEVICE_TRANSPORT,
        protocol_id: "corsair/nxp-scimitar-rgb",
        builder: build_scimitar_rgb_protocol
    ),
    nxp_descriptor!(
        pid: PID_MM800_POLARIS,
        name: "Corsair MM800 RGB Polaris",
        transport: NXP_POLARIS_TRANSPORT,
        protocol_id: "corsair/nxp-mm800-polaris",
        builder: build_mm800_polaris_protocol
    ),
    nxp_descriptor!(
        pid: PID_ST100_RGB,
        name: "Corsair ST100 RGB",
        transport: NXP_ST100_TRANSPORT,
        protocol_id: "corsair/nxp-st100-rgb",
        builder: build_st100_rgb_protocol
    ),
];
