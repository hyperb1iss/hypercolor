//! Descriptor registration for Corsair LCD devices.

use hypercolor_types::device::DeviceFamily;

use crate::drivers::corsair::CORSAIR_VID;
use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};

use super::protocol::CorsairLcdProtocol;

/// Shared HID report ID used by current Corsair LCD devices.
pub const CORSAIR_LCD_REPORT_ID: u8 = 0x03;
/// Shared interface number used by current Corsair LCD devices.
pub const CORSAIR_LCD_INTERFACE: u8 = 0;

/// Elite Capellix LCD PID.
pub const PID_ELITE_CAPELLIX_LCD: u16 = 0x0C39;
/// Alternate Elite Capellix LCD PID.
pub const PID_ELITE_CAPELLIX_LCD_ALT: u16 = 0x0C33;
/// iCUE LINK LCD PID.
pub const PID_ICUE_LINK_LCD: u16 = 0x0C4E;
/// Nautilus RS LCD PID.
pub const PID_NAUTILUS_RS_LCD: u16 = 0x0C55;
/// XC7 RGB Elite LCD PID.
pub const PID_XC7_RGB_ELITE_LCD: u16 = 0x0C42;
/// XD6 Elite LCD PID.
pub const PID_XD6_ELITE_LCD: u16 = 0x0C43;

fn build_protocol(
    name: &'static str,
    data_zone_byte: u8,
    keepalive_zone_byte: u8,
) -> Box<dyn Protocol> {
    Box::new(CorsairLcdProtocol::new(
        name,
        480,
        480,
        data_zone_byte,
        keepalive_zone_byte,
        true,
        0,
    ))
}

/// Build an Elite Capellix LCD protocol instance.
pub fn build_elite_capellix_lcd_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair Elite Capellix LCD", 0x40, 0x40)
}

/// Build an iCUE LINK LCD protocol instance.
pub fn build_icue_link_lcd_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair iCUE LINK LCD", 0x40, 0x40)
}

/// Build a Nautilus RS LCD protocol instance.
pub fn build_nautilus_rs_lcd_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair Nautilus RS LCD", 0x40, 0x40)
}

/// Build an XC7 RGB Elite LCD protocol instance.
pub fn build_xc7_rgb_elite_lcd_protocol() -> Box<dyn Protocol> {
    Box::new(CorsairLcdProtocol::new_xc7("Corsair XC7 RGB Elite LCD"))
}

/// Build an XD6 Elite LCD protocol instance.
pub fn build_xd6_elite_lcd_protocol() -> Box<dyn Protocol> {
    build_protocol("Corsair XD6 Elite LCD", 0x01, 0x40)
}

macro_rules! lcd_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: CORSAIR_VID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::Corsair,
            transport: TransportType::UsbHid {
                interface: CORSAIR_LCD_INTERFACE,
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static LCD_DESCRIPTORS: &[DeviceDescriptor] = &[
    lcd_descriptor!(
        pid: PID_ELITE_CAPELLIX_LCD,
        name: "Corsair Elite Capellix LCD",
        protocol_id: "corsair/elite-capellix-lcd",
        builder: build_elite_capellix_lcd_protocol
    ),
    lcd_descriptor!(
        pid: PID_ELITE_CAPELLIX_LCD_ALT,
        name: "Corsair Elite Capellix LCD",
        protocol_id: "corsair/elite-capellix-lcd",
        builder: build_elite_capellix_lcd_protocol
    ),
    lcd_descriptor!(
        pid: PID_ICUE_LINK_LCD,
        name: "Corsair iCUE LINK LCD",
        protocol_id: "corsair/icue-link-lcd",
        builder: build_icue_link_lcd_protocol
    ),
    lcd_descriptor!(
        pid: PID_NAUTILUS_RS_LCD,
        name: "Corsair Nautilus RS LCD",
        protocol_id: "corsair/nautilus-rs-lcd",
        builder: build_nautilus_rs_lcd_protocol
    ),
    lcd_descriptor!(
        pid: PID_XC7_RGB_ELITE_LCD,
        name: "Corsair XC7 RGB Elite LCD",
        protocol_id: "corsair/xc7-rgb-elite-lcd",
        builder: build_xc7_rgb_elite_lcd_protocol
    ),
    lcd_descriptor!(
        pid: PID_XD6_ELITE_LCD,
        name: "Corsair XD6 Elite LCD",
        protocol_id: "corsair/xd6-elite-lcd",
        builder: build_xd6_elite_lcd_protocol
    ),
];

/// Static LCD descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    LCD_DESCRIPTORS
}
