//! Lian Li device registry entries.

use std::sync::LazyLock;

use hypercolor_types::device::DeviceFamily;

use crate::protocol::Protocol;
use crate::registry::{
    DeviceDescriptor, HidRawReportMode, ProtocolBinding, SerialQuirk, TransportLifecycleHints,
    TransportType, UsbTransportBinding, UsbTransportKind,
};
use crate::transport::{
    HidAccessMode, HidTransportIntent, TransportIntent, resolve_current_transport,
};

use super::common::{LianLiHubVariant, TL_REPORT_ID};
use super::ene::Ene6k77Protocol;
use super::lcd::{TL_LCD_PACKET_LEN, TL_LCD_REPORT_ID, TlLcdProtocol};
use super::legacy::LegacyUniHubProtocol;
use super::tl::{TL_PACKET_LEN, TlFanProtocol};
use super::wireless::WirelessControllerProtocol;
use super::wireless::lcd::{
    PID_SL_WIRELESS_LCD, PID_TL_WIRELESS_LCD, WIRELESS_LCD_VENDOR_ID, WirelessLcdProtocol,
};
use super::wireless::transport::{PID_WIRELESS_TX, WIRELESS_VENDOR_ID, open_wireless_controller};

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
/// Original UNI Hub PID.
pub const PID_UNI_HUB_ORIGINAL: u16 = 0x7750;
/// TL Fan Hub PID.
pub const PID_TL_FAN_HUB: u16 = 0x7372;
/// Wired Uni Fan TL LCD panel PID.
pub const PID_TL_LCD: u16 = 0x7393;

/// Wired TL LCD vendor ID.
///
/// Not the Lian Li VIDs above: the panel enumerates under a borrowed
/// vendor ID, which is why its udev rules stay scoped to this product.
pub const LIANLI_TL_LCD_VENDOR_ID: u16 = 0x04FC;

/// Serial every wired TL LCD panel reports from stock firmware, which makes
/// it a model string rather than an identity (spec 80 section 5.7).
const TL_LCD_PLACEHOLDER_SERIALS: &[&str] = &["TL_LCDV0.1"];

/// The panel speaks HID output reports carrying its own report ID.
const TL_LCD_TRANSPORT_INTENT: TransportIntent = TransportIntent::Hid(HidTransportIntent {
    access: HidAccessMode::Direct,
    interface: 0,
    report_id: TL_LCD_REPORT_ID,
    report_mode: HidRawReportMode::OutputReportWithReportId,
    max_report_len: TL_LCD_PACKET_LEN,
    usage_page: None,
    usage: None,
});

fn firmware_matches(candidate: &str, expected: &str) -> bool {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return false;
    }

    let token = trimmed.rsplit('-').next().unwrap_or(trimmed).trim();
    let token = token.strip_prefix(['v', 'V']).unwrap_or(token);
    token.eq_ignore_ascii_case(expected)
}

fn is_al_hid_firmware(candidate: &str) -> bool {
    firmware_matches(candidate, "1.7")
}

fn is_al10_firmware(candidate: &str) -> bool {
    firmware_matches(candidate, "1.0")
}

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

/// Build a wired Uni Fan TL LCD protocol instance.
#[must_use]
pub fn build_tl_lcd_protocol() -> Box<dyn Protocol> {
    Box::new(TlLcdProtocol::new())
}

/// Build an L-Wireless controller protocol instance.
#[must_use]
pub fn build_wireless_controller_protocol() -> Box<dyn Protocol> {
    Box::new(WirelessControllerProtocol::new())
}

/// Build a wireless LCD receiver protocol instance.
#[must_use]
pub fn build_wireless_lcd_protocol() -> Box<dyn Protocol> {
    Box::new(WirelessLcdProtocol::new())
}

/// Build an original UNI Hub protocol instance.
pub fn build_uni_hub_original_protocol() -> Box<dyn Protocol> {
    Box::new(LegacyUniHubProtocol::original())
}

/// Build an AL10 fallback protocol instance.
pub fn build_uni_hub_al10_protocol() -> Box<dyn Protocol> {
    Box::new(LegacyUniHubProtocol::al10())
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
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbHid {
                interface: LIANLI_ENE_INTERFACE,
            },
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
            serial_quirk: None,
        }
    };
}

static LIANLI_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    vec![
        ene_descriptor!(
            pid: PID_UNI_HUB_SL,
            name: "Lian Li Uni Hub - SL",
            protocol_id: "lianli/sl",
            builder: build_uni_hub_sl_protocol
        ),
        DeviceDescriptor {
            vendor_id: LIANLI_ENE_VENDOR_ID,
            product_id: PID_UNI_HUB_AL,
            name: "Lian Li Uni Hub - AL",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbHid {
                interface: LIANLI_ENE_INTERFACE,
            },
            protocol: ProtocolBinding {
                id: "lianli/al",
                build: build_uni_hub_al_protocol,
            },
            firmware_predicate: Some(is_al_hid_firmware),
            serial_quirk: None,
        },
        DeviceDescriptor {
            vendor_id: LIANLI_ENE_VENDOR_ID,
            product_id: PID_UNI_HUB_AL,
            name: "Lian Li Uni Hub - AL10",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbVendor,
            protocol: ProtocolBinding {
                id: "lianli/al10",
                build: build_uni_hub_al10_protocol,
            },
            firmware_predicate: Some(is_al10_firmware),
            serial_quirk: None,
        },
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
            vendor_id: LIANLI_ENE_VENDOR_ID,
            product_id: PID_UNI_HUB_ORIGINAL,
            name: "Lian Li Uni Hub",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbVendor,
            protocol: ProtocolBinding {
                id: "lianli/original",
                build: build_uni_hub_original_protocol,
            },
            firmware_predicate: None,
            serial_quirk: None,
        },
        DeviceDescriptor {
            vendor_id: LIANLI_TL_VENDOR_ID,
            product_id: PID_TL_FAN_HUB,
            name: "Lian Li TL Fan Hub",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbHidApi {
                interface: None,
                report_id: TL_REPORT_ID,
                report_mode: HidRawReportMode::OutputReportWithReportId,
                max_report_len: TL_PACKET_LEN,
                usage_page: Some(LIANLI_TL_USAGE_PAGE),
                usage: None,
            },
            protocol: ProtocolBinding {
                id: "lianli/tl-fan",
                build: build_tl_fan_protocol,
            },
            firmware_predicate: None,
            serial_quirk: None,
        },
        DeviceDescriptor {
            vendor_id: LIANLI_TL_LCD_VENDOR_ID,
            product_id: PID_TL_LCD,
            name: "Lian Li Uni Fan TL LCD",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: resolve_current_transport(TL_LCD_TRANSPORT_INTENT)
                .expect("wired TL LCD HID transport should support the current platform"),
            protocol: ProtocolBinding {
                id: "lianli/tl-lcd",
                build: build_tl_lcd_protocol,
            },
            firmware_predicate: None,
            serial_quirk: Some(SerialQuirk::PlaceholderValues(TL_LCD_PLACEHOLDER_SERIALS)),
        },
        DeviceDescriptor {
            vendor_id: WIRELESS_VENDOR_ID,
            product_id: PID_WIRELESS_TX,
            name: "Lian Li L-Wireless Controller",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            // The TX is the device discovery binds; its factory opens the RX
            // sibling behind the same hub and pairs the two.
            transport: TransportType::DriverUsb {
                binding: UsbTransportBinding {
                    id: "lianli/wireless",
                    kind: UsbTransportKind::Usb,
                    lifecycle: TransportLifecycleHints::default(),
                    open: open_wireless_controller,
                },
            },
            protocol: ProtocolBinding {
                id: "lianli/wireless",
                build: build_wireless_controller_protocol,
            },
            firmware_predicate: None,
            serial_quirk: None,
        },
        DeviceDescriptor {
            vendor_id: WIRELESS_LCD_VENDOR_ID,
            product_id: PID_TL_WIRELESS_LCD,
            name: "Lian Li Uni Fan TL Wireless LCD",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbBulk {
                interface: 0,
                report_id: 0,
            },
            protocol: ProtocolBinding {
                id: "lianli/wireless-lcd",
                build: build_wireless_lcd_protocol,
            },
            firmware_predicate: None,
            // Serial uniqueness across receivers is unverified on hardware;
            // a placeholder list goes here the day one is observed.
            serial_quirk: None,
        },
        DeviceDescriptor {
            vendor_id: WIRELESS_LCD_VENDOR_ID,
            product_id: PID_SL_WIRELESS_LCD,
            name: "Lian Li Uni Fan SL Wireless LCD",
            family: DeviceFamily::new_static("lianli", "Lian Li"),
            transport: TransportType::UsbBulk {
                interface: 0,
                report_id: 0,
            },
            protocol: ProtocolBinding {
                id: "lianli/wireless-lcd",
                build: build_wireless_lcd_protocol,
            },
            firmware_predicate: None,
            serial_quirk: None,
        },
    ]
});

/// Static Lian Li descriptors for HAL registration.
#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    &LIANLI_DESCRIPTORS
}
