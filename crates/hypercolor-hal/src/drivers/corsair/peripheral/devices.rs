//! Corsair peripheral device descriptors.

use hypercolor_types::device::DeviceFamily;

use crate::drivers::corsair::{CORSAIR_USAGE_PAGE, CORSAIR_VID};
use crate::protocol::Protocol;
use crate::registry::{DeviceDescriptor, HidRawReportMode, ProtocolBinding, TransportType};

use super::bragi::CorsairBragiProtocol;
use super::types::{
    BRAGI_JUMBO_PACKET_SIZE, BRAGI_LARGE_PACKET_SIZE, BRAGI_MAGIC, BRAGI_PACKET_SIZE,
    BragiDeviceConfig, CorsairPeripheralClass, CorsairPeripheralTopology,
};

pub const BRAGI_REPORT_ID: u8 = BRAGI_MAGIC;
pub const BRAGI_INTERFACE: u8 = 1;

pub const PID_K55_RGB_PRO: u16 = 0x1BA4;
pub const PID_K60_PRO_RGB: u16 = 0x1BA0;
pub const PID_K60_PRO_RGB_LOW_PROFILE: u16 = 0x1BAD;
pub const PID_K60_PRO_RGB_SE: u16 = 0x1B8D;
pub const PID_K60_PRO_MONO: u16 = 0x1B83;
pub const PID_K60_PRO_TKL: u16 = 0x1BC7;
pub const PID_K60_PRO_TKL_WHITE: u16 = 0x1BED;
pub const PID_K65_MINI: u16 = 0x1BAF;
pub const PID_K70_TKL: u16 = 0x1B73;
pub const PID_K70_TKL_CHAMPION_OPTICAL: u16 = 0x1BB9;
pub const PID_K70_RGB_PRO: u16 = 0x1BC4;
pub const PID_K70_PRO: u16 = 0x1BB3;
pub const PID_K70_PRO_OPTICAL: u16 = 0x1BD4;
pub const PID_K70_CORE_RGB: u16 = 0x2B0A;
pub const PID_K70_CORE_RGB_VARIANT_2: u16 = 0x1BFF;
pub const PID_K70_CORE_RGB_VARIANT_3: u16 = 0x1BFD;
pub const PID_K70_CORE_RGB_TKL: u16 = 0x2B01;
pub const PID_K95_PLATINUM_XT: u16 = 0x1B89;
pub const PID_K100_OPTICAL_V1: u16 = 0x1B7C;
pub const PID_K100_OPTICAL_V2: u16 = 0x1BC5;
pub const PID_K100_MX_RED: u16 = 0x1B7D;
pub const PID_DARK_CORE_RGB_SE_WIRED: u16 = 0x1B4B;
pub const PID_DARK_CORE_RGB_PRO_SE_WIRED: u16 = 0x1B7E;
pub const PID_HARPOON_WIRELESS_WIRED: u16 = 0x1B5E;
pub const PID_IRONCLAW_WIRELESS_WIRED: u16 = 0x1B4C;
pub const PID_M55_RGB_PRO: u16 = 0x1B70;
pub const PID_KATAR_PRO: u16 = 0x1B93;
pub const PID_KATAR_PRO_V2: u16 = 0x1BBA;
pub const PID_KATAR_PRO_XT: u16 = 0x1BAC;
pub const PID_M65_RGB_ULTRA_WIRED: u16 = 0x1B9E;
pub const PID_M65_RGB_ULTRA_WIRELESS_WIRED: u16 = 0x1BB5;
pub const PID_M75_GAMING_MOUSE: u16 = 0x1BF0;
pub const PID_SCIMITAR_ELITE_BRAGI: u16 = 0x1BE3;
pub const PID_MM700: u16 = 0x1B9B;
pub const PID_MM700_3XL: u16 = 0x1BC9;

const KEYBOARD_6: CorsairPeripheralTopology =
    CorsairPeripheralTopology::KeyboardMatrix { rows: 1, cols: 6 };
const KEYBOARD_102: CorsairPeripheralTopology =
    CorsairPeripheralTopology::KeyboardMatrix { rows: 6, cols: 17 };
const KEYBOARD_123: CorsairPeripheralTopology =
    CorsairPeripheralTopology::KeyboardMatrix { rows: 6, cols: 21 };
const KEYBOARD_156: CorsairPeripheralTopology =
    CorsairPeripheralTopology::KeyboardMatrix { rows: 6, cols: 26 };
const KEYBOARD_193: CorsairPeripheralTopology =
    CorsairPeripheralTopology::KeyboardMatrix { rows: 7, cols: 28 };
const KEYBOARD_288: CorsairPeripheralTopology =
    CorsairPeripheralTopology::KeyboardMatrix { rows: 12, cols: 24 };

fn bragi_keyboard_config(
    name: &'static str,
    packet_size: usize,
    led_count: usize,
    topology: CorsairPeripheralTopology,
) -> BragiDeviceConfig {
    BragiDeviceConfig::new(
        name,
        CorsairPeripheralClass::Keyboard,
        packet_size,
        led_count,
        topology,
    )
}

fn bragi_pointer_config(
    name: &'static str,
    class: CorsairPeripheralClass,
    packet_size: usize,
    led_count: usize,
) -> BragiDeviceConfig {
    let topology = if led_count == 1 {
        CorsairPeripheralTopology::Point
    } else {
        CorsairPeripheralTopology::Strip
    };
    BragiDeviceConfig::new(name, class, packet_size, led_count, topology)
}

fn build_protocol(config: BragiDeviceConfig) -> Box<dyn Protocol> {
    Box::new(CorsairBragiProtocol::new(config))
}

pub fn build_k60_pro_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K60 Pro RGB",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k60_pro_rgb_low_profile_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K60 Pro RGB Low Profile",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k60_pro_rgb_se_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K60 Pro RGB SE",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k60_pro_mono_protocol() -> Box<dyn Protocol> {
    build_protocol(
        bragi_keyboard_config("Corsair K60 Pro Mono", BRAGI_PACKET_SIZE, 123, KEYBOARD_123)
            .monochrome(),
    )
}

pub fn build_k60_pro_tkl_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K60 Pro TKL",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k55_rgb_pro_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K55 RGB Pro",
        BRAGI_PACKET_SIZE,
        6,
        KEYBOARD_6,
    ))
}

pub fn build_k60_pro_tkl_white_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K60 Pro TKL White",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k65_mini_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K65 Mini",
        BRAGI_JUMBO_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k70_tkl_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 TKL",
        BRAGI_JUMBO_PACKET_SIZE,
        193,
        KEYBOARD_193,
    ))
}

pub fn build_k70_tkl_champion_optical_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 TKL Champion Optical",
        BRAGI_JUMBO_PACKET_SIZE,
        193,
        KEYBOARD_193,
    ))
}

pub fn build_k70_rgb_pro_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 RGB Pro",
        BRAGI_JUMBO_PACKET_SIZE,
        193,
        KEYBOARD_193,
    ))
}

pub fn build_k70_pro_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 RGB Pro V2",
        BRAGI_JUMBO_PACKET_SIZE,
        193,
        KEYBOARD_193,
    ))
}

pub fn build_k70_pro_optical_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 Pro Optical",
        BRAGI_JUMBO_PACKET_SIZE,
        193,
        KEYBOARD_193,
    ))
}

pub fn build_k70_core_rgb_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 Core RGB",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k70_core_rgb_variant_2_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 Core RGB",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k70_core_rgb_variant_3_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 Core RGB",
        BRAGI_PACKET_SIZE,
        123,
        KEYBOARD_123,
    ))
}

pub fn build_k70_core_rgb_tkl_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K70 Core RGB TKL",
        BRAGI_PACKET_SIZE,
        102,
        KEYBOARD_102,
    ))
}

pub fn build_k95_platinum_xt_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_keyboard_config(
        "Corsair K95 Platinum XT",
        BRAGI_PACKET_SIZE,
        156,
        KEYBOARD_156,
    ))
}

pub fn build_k100_optical_v1_protocol() -> Box<dyn Protocol> {
    build_protocol(
        bragi_keyboard_config(
            "Corsair K100 RGB Optical",
            BRAGI_JUMBO_PACKET_SIZE,
            193,
            KEYBOARD_288,
        )
        .alternate_rgb(),
    )
}

pub fn build_k100_optical_v2_protocol() -> Box<dyn Protocol> {
    build_protocol(
        bragi_keyboard_config(
            "Corsair K100 RGB Optical",
            BRAGI_JUMBO_PACKET_SIZE,
            193,
            KEYBOARD_288,
        )
        .alternate_rgb(),
    )
}

pub fn build_k100_mx_red_protocol() -> Box<dyn Protocol> {
    build_protocol(
        bragi_keyboard_config(
            "Corsair K100 MX Red",
            BRAGI_JUMBO_PACKET_SIZE,
            193,
            KEYBOARD_288,
        )
        .alternate_rgb(),
    )
}

pub fn build_dark_core_rgb_se_wired_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Dark Core RGB SE (Wired)",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        12,
    ))
}

pub fn build_dark_core_rgb_pro_se_wired_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Dark Core RGB Pro SE (Wired)",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        12,
    ))
}

pub fn build_harpoon_wireless_wired_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Harpoon Wireless (Wired)",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_ironclaw_wireless_wired_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Ironclaw Wireless (Wired)",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        6,
    ))
}

pub fn build_m55_rgb_pro_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair M55 RGB Pro",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_katar_pro_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Katar Pro",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_katar_pro_v2_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Katar Pro V2",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_katar_pro_xt_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Katar Pro XT",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_m65_rgb_ultra_wired_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair M65 RGB Ultra Wired",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        3,
    ))
}

pub fn build_m65_rgb_ultra_wireless_wired_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair M65 RGB Ultra Wireless (Wired)",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_m75_gaming_mouse_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair M75 Gaming Mouse",
        CorsairPeripheralClass::Mouse,
        BRAGI_PACKET_SIZE,
        2,
    ))
}

pub fn build_scimitar_elite_bragi_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair Scimitar Elite",
        CorsairPeripheralClass::Mouse,
        BRAGI_LARGE_PACKET_SIZE,
        5,
    ))
}

pub fn build_mm700_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair MM700",
        CorsairPeripheralClass::Mousepad,
        BRAGI_PACKET_SIZE,
        3,
    ))
}

pub fn build_mm700_3xl_protocol() -> Box<dyn Protocol> {
    build_protocol(bragi_pointer_config(
        "Corsair MM700 3XL",
        CorsairPeripheralClass::Mousepad,
        BRAGI_PACKET_SIZE,
        3,
    ))
}

const fn bragi_transport(packet_size: usize) -> TransportType {
    TransportType::UsbHidApi {
        interface: Some(BRAGI_INTERFACE),
        report_id: BRAGI_REPORT_ID,
        report_mode: HidRawReportMode::OutputReportWithReportId,
        max_report_len: packet_size,
        usage_page: Some(CORSAIR_USAGE_PAGE),
        usage: None,
    }
}

macro_rules! bragi_descriptor {
    (
        pid: $pid:expr,
        name: $name:expr,
        packet_size: $packet_size:expr,
        protocol_id: $protocol_id:expr,
        builder: $builder:path
    ) => {
        DeviceDescriptor {
            vendor_id: CORSAIR_VID,
            product_id: $pid,
            name: $name,
            family: DeviceFamily::new_static("corsair", "Corsair"),
            transport: bragi_transport($packet_size),
            protocol: ProtocolBinding {
                id: $protocol_id,
                build: $builder,
            },
            firmware_predicate: None,
        }
    };
}

static PERIPHERAL_DESCRIPTORS: &[DeviceDescriptor] = &[
    bragi_descriptor!(
        pid: PID_K55_RGB_PRO,
        name: "Corsair K55 RGB Pro",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k55-rgb-pro",
        builder: build_k55_rgb_pro_protocol
    ),
    bragi_descriptor!(
        pid: PID_K60_PRO_RGB,
        name: "Corsair K60 Pro RGB",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k60-pro-rgb",
        builder: build_k60_pro_rgb_protocol
    ),
    bragi_descriptor!(
        pid: PID_K60_PRO_RGB_LOW_PROFILE,
        name: "Corsair K60 Pro RGB Low Profile",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k60-pro-rgb-low-profile",
        builder: build_k60_pro_rgb_low_profile_protocol
    ),
    bragi_descriptor!(
        pid: PID_K60_PRO_RGB_SE,
        name: "Corsair K60 Pro RGB SE",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k60-pro-rgb-se",
        builder: build_k60_pro_rgb_se_protocol
    ),
    bragi_descriptor!(
        pid: PID_K60_PRO_MONO,
        name: "Corsair K60 Pro Mono",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k60-pro-mono",
        builder: build_k60_pro_mono_protocol
    ),
    bragi_descriptor!(
        pid: PID_K60_PRO_TKL,
        name: "Corsair K60 Pro TKL",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k60-pro-tkl",
        builder: build_k60_pro_tkl_protocol
    ),
    bragi_descriptor!(
        pid: PID_K60_PRO_TKL_WHITE,
        name: "Corsair K60 Pro TKL White",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k60-pro-tkl-white",
        builder: build_k60_pro_tkl_white_protocol
    ),
    bragi_descriptor!(
        pid: PID_K65_MINI,
        name: "Corsair K65 Mini",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k65-mini",
        builder: build_k65_mini_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_TKL,
        name: "Corsair K70 TKL",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-tkl",
        builder: build_k70_tkl_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_TKL_CHAMPION_OPTICAL,
        name: "Corsair K70 TKL Champion Optical",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-tkl-champion-optical",
        builder: build_k70_tkl_champion_optical_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_RGB_PRO,
        name: "Corsair K70 RGB Pro",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-rgb-pro",
        builder: build_k70_rgb_pro_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_PRO,
        name: "Corsair K70 RGB Pro V2",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-rgb-pro-v2",
        builder: build_k70_pro_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_PRO_OPTICAL,
        name: "Corsair K70 Pro Optical",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-pro-optical",
        builder: build_k70_pro_optical_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_CORE_RGB,
        name: "Corsair K70 Core RGB",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-core-rgb",
        builder: build_k70_core_rgb_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_CORE_RGB_VARIANT_2,
        name: "Corsair K70 Core RGB",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-core-rgb-v2",
        builder: build_k70_core_rgb_variant_2_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_CORE_RGB_VARIANT_3,
        name: "Corsair K70 Core RGB",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-core-rgb-v3",
        builder: build_k70_core_rgb_variant_3_protocol
    ),
    bragi_descriptor!(
        pid: PID_K70_CORE_RGB_TKL,
        name: "Corsair K70 Core RGB TKL",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k70-core-rgb-tkl",
        builder: build_k70_core_rgb_tkl_protocol
    ),
    bragi_descriptor!(
        pid: PID_K95_PLATINUM_XT,
        name: "Corsair K95 Platinum XT",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-k95-platinum-xt",
        builder: build_k95_platinum_xt_protocol
    ),
    bragi_descriptor!(
        pid: PID_K100_OPTICAL_V1,
        name: "Corsair K100 RGB Optical",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k100-rgb-optical-v1",
        builder: build_k100_optical_v1_protocol
    ),
    bragi_descriptor!(
        pid: PID_K100_OPTICAL_V2,
        name: "Corsair K100 RGB Optical",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k100-rgb-optical-v2",
        builder: build_k100_optical_v2_protocol
    ),
    bragi_descriptor!(
        pid: PID_K100_MX_RED,
        name: "Corsair K100 MX Red",
        packet_size: BRAGI_JUMBO_PACKET_SIZE,
        protocol_id: "corsair/bragi-k100-mx-red",
        builder: build_k100_mx_red_protocol
    ),
    bragi_descriptor!(
        pid: PID_DARK_CORE_RGB_SE_WIRED,
        name: "Corsair Dark Core RGB SE (Wired)",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-dark-core-rgb-se-wired",
        builder: build_dark_core_rgb_se_wired_protocol
    ),
    bragi_descriptor!(
        pid: PID_DARK_CORE_RGB_PRO_SE_WIRED,
        name: "Corsair Dark Core RGB Pro SE (Wired)",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-dark-core-rgb-pro-se-wired",
        builder: build_dark_core_rgb_pro_se_wired_protocol
    ),
    bragi_descriptor!(
        pid: PID_HARPOON_WIRELESS_WIRED,
        name: "Corsair Harpoon Wireless (Wired)",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-harpoon-wireless-wired",
        builder: build_harpoon_wireless_wired_protocol
    ),
    bragi_descriptor!(
        pid: PID_IRONCLAW_WIRELESS_WIRED,
        name: "Corsair Ironclaw Wireless (Wired)",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-ironclaw-wireless-wired",
        builder: build_ironclaw_wireless_wired_protocol
    ),
    bragi_descriptor!(
        pid: PID_M55_RGB_PRO,
        name: "Corsair M55 RGB Pro",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-m55-rgb-pro",
        builder: build_m55_rgb_pro_protocol
    ),
    bragi_descriptor!(
        pid: PID_KATAR_PRO,
        name: "Corsair Katar Pro",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-katar-pro",
        builder: build_katar_pro_protocol
    ),
    bragi_descriptor!(
        pid: PID_KATAR_PRO_V2,
        name: "Corsair Katar Pro V2",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-katar-pro-v2",
        builder: build_katar_pro_v2_protocol
    ),
    bragi_descriptor!(
        pid: PID_KATAR_PRO_XT,
        name: "Corsair Katar Pro XT",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-katar-pro-xt",
        builder: build_katar_pro_xt_protocol
    ),
    bragi_descriptor!(
        pid: PID_M65_RGB_ULTRA_WIRED,
        name: "Corsair M65 RGB Ultra Wired",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-m65-rgb-ultra-wired",
        builder: build_m65_rgb_ultra_wired_protocol
    ),
    bragi_descriptor!(
        pid: PID_M65_RGB_ULTRA_WIRELESS_WIRED,
        name: "Corsair M65 RGB Ultra Wireless (Wired)",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-m65-rgb-ultra-wireless-wired",
        builder: build_m65_rgb_ultra_wireless_wired_protocol
    ),
    bragi_descriptor!(
        pid: PID_M75_GAMING_MOUSE,
        name: "Corsair M75 Gaming Mouse",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-m75-gaming-mouse",
        builder: build_m75_gaming_mouse_protocol
    ),
    bragi_descriptor!(
        pid: PID_SCIMITAR_ELITE_BRAGI,
        name: "Corsair Scimitar Elite",
        packet_size: BRAGI_LARGE_PACKET_SIZE,
        protocol_id: "corsair/bragi-scimitar-elite",
        builder: build_scimitar_elite_bragi_protocol
    ),
    bragi_descriptor!(
        pid: PID_MM700,
        name: "Corsair MM700",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-mm700",
        builder: build_mm700_protocol
    ),
    bragi_descriptor!(
        pid: PID_MM700_3XL,
        name: "Corsair MM700 3XL",
        packet_size: BRAGI_PACKET_SIZE,
        protocol_id: "corsair/bragi-mm700-3xl",
        builder: build_mm700_3xl_protocol
    ),
];

#[must_use]
pub fn descriptors() -> &'static [DeviceDescriptor] {
    PERIPHERAL_DESCRIPTORS
}
