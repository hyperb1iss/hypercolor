use std::collections::BTreeSet;

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::asus::{
    ASUS_VID, AURA_REPORT_ID, PID_AURA_MOTHERBOARD_GEN3, PID_AURA_TERMINAL,
};
use hypercolor_hal::drivers::corsair::{
    CORSAIR_VID, PID_COMMANDER_PRO, PID_ELITE_CAPELLIX_LCD, PID_ELITE_CAPELLIX_LCD_ALT,
    PID_ICUE_LINK_LCD, PID_ICUE_LINK_SYSTEM_HUB, PID_LIGHTING_NODE_CORE, PID_LIGHTING_NODE_PRO,
    PID_NAUTILUS_RS_LCD, PID_XC7_RGB_ELITE_LCD, PID_XD6_ELITE_LCD,
};
use hypercolor_hal::drivers::dygma::{DYGMA_VENDOR_ID, PID_DEFY_WIRED, PID_DEFY_WIRELESS};
use hypercolor_hal::drivers::lianli::{
    LIANLI_ENE_INTERFACE, LIANLI_ENE_VENDOR_ID, LIANLI_TL_USAGE_PAGE, LIANLI_TL_VENDOR_ID,
    PID_TL_FAN_HUB, PID_UNI_HUB_AL, PID_UNI_HUB_ORIGINAL, PID_UNI_HUB_SL_INFINITY, TL_REPORT_ID,
};
use hypercolor_hal::drivers::nollie::{
    NOLLIE_GEN2_VENDOR_ID, NOLLIE_VENDOR_ID, PID_NOLLIE_1, PID_NOLLIE_8_V2, PID_NOLLIE_16_V3,
    PID_NOLLIE_28_12_A, PID_NOLLIE_32, PID_PRISM_8, PRISM_VENDOR_ID,
};
use hypercolor_hal::drivers::prismrgb::{PID_PRISM_MINI, PID_PRISM_S, PRISM_GCS_VENDOR_ID};
use hypercolor_hal::drivers::push2::{
    ABLETON_VENDOR_ID, PID_PUSH_2, PUSH2_DISPLAY_ENDPOINT, PUSH2_DISPLAY_INTERFACE,
    PUSH2_MIDI_INTERFACE,
};
use hypercolor_hal::drivers::razer::{
    PID_BASILISK_V3, PID_BLADE_14_2021, PID_BLADE_14_2023, PID_BLADE_15_2022,
    PID_BLADE_15_LATE_2021_ADVANCED, PID_BLADE_PRO_2016, PID_HUNTSMAN_V2, PID_MAMBA_ELITE,
    PID_SEIREN_EMOTE, PID_SEIREN_V3_CHROMA, PID_TARTARUS_CHROMA, RAZER_VENDOR_ID,
};
use hypercolor_hal::registry::{HidRawReportMode, TransportType};
use hypercolor_types::device::{
    DeviceFamily, DeviceTopologyHint, DriverModuleKind, DriverTransportKind,
};

const PID_BLADE_14_2022: u16 = 0x028C;
const PID_BLACKWIDOW_V3: u16 = 0x024E;
const PID_FIREFLY: u16 = 0x0C00;
const PID_LAPTOP_STAND_CHROMA: u16 = 0x0F0D;
const PID_THUNDERBOLT_4_DOCK_CHROMA: u16 = 0x0F21;

fn expected_razer_shared_hid_transport(
    interface: u8,
    report_id: u8,
    usage_page: Option<u16>,
    usage: Option<u16>,
) -> TransportType {
    TransportType::UsbHidApi {
        interface: Some(interface),
        report_id,
        report_mode: HidRawReportMode::FeatureReport,
        usage_page,
        usage,
    }
}

#[test]
fn lookup_returns_asus_motherboard_descriptor() {
    let descriptor = ProtocolDatabase::lookup(ASUS_VID, PID_AURA_MOTHERBOARD_GEN3)
        .expect("ASUS motherboard descriptor should exist");

    assert_eq!(descriptor.name, "ASUS Aura Motherboard (Gen 3)");
    assert_eq!(descriptor.family, DeviceFamily::new_static("asus", "ASUS"));
    assert_eq!(descriptor.protocol.id, "asus/motherboard-gen3");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidRaw {
            interface: 2,
            report_id: AURA_REPORT_ID,
            report_mode: HidRawReportMode::OutputReport,
            usage_page: None,
            usage: None,
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "ASUS Aura Motherboard");
}

#[test]
fn lookup_returns_asus_terminal_descriptor() {
    let descriptor = ProtocolDatabase::lookup(ASUS_VID, PID_AURA_TERMINAL)
        .expect("ASUS Aura Terminal descriptor should exist");

    assert_eq!(descriptor.name, "ASUS Aura Terminal");
    assert_eq!(descriptor.family, DeviceFamily::new_static("asus", "ASUS"));
    assert_eq!(descriptor.protocol.id, "asus/terminal");

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 361);
    assert_eq!(protocol.zones().len(), 5);
}

#[test]
fn lookup_returns_prism_8_descriptor() {
    let descriptor = ProtocolDatabase::lookup(PRISM_VENDOR_ID, PID_PRISM_8)
        .expect("Prism 8 descriptor should exist");

    assert_eq!(descriptor.name, "PrismRGB Prism 8");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("prismrgb", "PrismRGB")
    );
    assert_eq!(descriptor.driver_id(), "nollie");
    assert_eq!(descriptor.protocol.id, "nollie/prism-8");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "PrismRGB Prism 8");
    assert_eq!(protocol.total_leds(), 1_008);
    assert_eq!(protocol.zones().len(), 8);
}

#[test]
fn lookup_returns_lianli_sl_infinity_descriptor() {
    let descriptor = ProtocolDatabase::lookup(LIANLI_ENE_VENDOR_ID, PID_UNI_HUB_SL_INFINITY)
        .expect("Lian Li SL Infinity descriptor should exist");

    assert_eq!(descriptor.name, "Lian Li Uni Hub - SL Infinity");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("lianli", "Lian Li")
    );
    assert_eq!(descriptor.protocol.id, "lianli/sl-infinity");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHid {
            interface: LIANLI_ENE_INTERFACE,
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Lian Li UNI Hub SL Infinity");
    assert_eq!(protocol.total_leds(), 320);
    assert_eq!(protocol.zones().len(), 8);
}

#[test]
fn lookup_returns_lianli_tl_fan_descriptor() {
    let descriptor = ProtocolDatabase::lookup(LIANLI_TL_VENDOR_ID, PID_TL_FAN_HUB)
        .expect("Lian Li TL Fan descriptor should exist");

    assert_eq!(descriptor.name, "Lian Li TL Fan Hub");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("lianli", "Lian Li")
    );
    assert_eq!(descriptor.protocol.id, "lianli/tl-fan");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: None,
            report_id: TL_REPORT_ID,
            report_mode: HidRawReportMode::OutputReport,
            usage_page: Some(LIANLI_TL_USAGE_PAGE),
            usage: None,
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Lian Li TL Fan Hub");
    assert_eq!(protocol.total_leds(), 0);
    assert!(protocol.zones().is_empty());
}

#[test]
fn lookup_returns_lianli_original_descriptor() {
    let descriptor = ProtocolDatabase::lookup(LIANLI_ENE_VENDOR_ID, PID_UNI_HUB_ORIGINAL)
        .expect("original Lian Li UNI Hub descriptor should exist");

    assert_eq!(descriptor.name, "Lian Li Uni Hub");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("lianli", "Lian Li")
    );
    assert_eq!(descriptor.protocol.id, "lianli/original");
    assert_eq!(descriptor.transport, TransportType::UsbVendor);

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Lian Li UNI Hub");
    assert_eq!(protocol.total_leds(), 256);
    assert_eq!(protocol.zones().len(), 4);
}

#[test]
fn lookup_with_firmware_routes_al_pid_to_correct_protocol_family() {
    let al = ProtocolDatabase::lookup_with_firmware(
        LIANLI_ENE_VENDOR_ID,
        PID_UNI_HUB_AL,
        Some("LianLi-UNI FAN-AL-v1.7"),
    )
    .expect("AL HID descriptor should resolve from product string");
    assert_eq!(al.protocol.id, "lianli/al");
    assert_eq!(
        al.transport,
        TransportType::UsbHid {
            interface: LIANLI_ENE_INTERFACE,
        }
    );

    let al10 = ProtocolDatabase::lookup_with_firmware(
        LIANLI_ENE_VENDOR_ID,
        PID_UNI_HUB_AL,
        Some("LianLi-UNI FAN-AL-v1.0"),
    )
    .expect("AL10 fallback descriptor should resolve from product string");
    assert_eq!(al10.protocol.id, "lianli/al10");
    assert_eq!(al10.transport, TransportType::UsbVendor);
}

#[test]
fn lookup_returns_defy_wired_descriptor() {
    let descriptor = ProtocolDatabase::lookup(DYGMA_VENDOR_ID, PID_DEFY_WIRED)
        .expect("Dygma Defy descriptor should exist");

    assert_eq!(descriptor.name, "Dygma Defy");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("dygma", "Dygma")
    );
    assert_eq!(descriptor.protocol.id, "dygma/defy-wired");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbSerial { baud_rate: 115_200 }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Dygma Defy");
    assert_eq!(protocol.total_leds(), 176);
    assert_eq!(protocol.zones().len(), 4);
    assert!(!protocol.capabilities().supports_direct);
}

#[test]
fn lookup_returns_defy_wireless_descriptor() {
    let descriptor = ProtocolDatabase::lookup(DYGMA_VENDOR_ID, PID_DEFY_WIRELESS)
        .expect("Dygma Defy Wireless descriptor should exist");

    assert_eq!(descriptor.name, "Dygma Defy Wireless");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("dygma", "Dygma")
    );
    assert_eq!(descriptor.protocol.id, "dygma/defy-wireless");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbSerial { baud_rate: 115_200 }
    );
}

#[test]
fn lookup_returns_push2_descriptor() {
    let descriptor = ProtocolDatabase::lookup(ABLETON_VENDOR_ID, PID_PUSH_2)
        .expect("Ableton Push 2 descriptor should exist");

    assert_eq!(descriptor.name, "Ableton Push 2");
    assert_eq!(descriptor.family, DeviceFamily::named("Ableton"));
    assert_eq!(descriptor.protocol.id, "push2/push-2");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbMidi {
            midi_interface: PUSH2_MIDI_INTERFACE,
            display_interface: PUSH2_DISPLAY_INTERFACE,
            display_endpoint: PUSH2_DISPLAY_ENDPOINT,
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Ableton Push 2");
    assert_eq!(protocol.total_leds(), 160);
    assert_eq!(protocol.zones().len(), 8);
    assert_eq!(protocol.capabilities().display_resolution, Some((960, 160)));
}

#[test]
fn lookup_returns_icue_link_system_hub_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_ICUE_LINK_SYSTEM_HUB)
        .expect("Corsair iCUE LINK System Hub descriptor should exist");

    assert_eq!(descriptor.name, "Corsair iCUE LINK System Hub");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/icue-link-system-hub");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Corsair iCUE LINK System Hub");
    assert_eq!(protocol.total_leds(), 0);
    assert!(protocol.zones().is_empty());
}

#[test]
fn lookup_returns_lighting_node_core_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_LIGHTING_NODE_CORE)
        .expect("Corsair Lighting Node Core descriptor should exist");

    assert_eq!(descriptor.name, "Corsair Lighting Node Core");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/lighting-node-core");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Corsair Lighting Node Core");
    assert_eq!(protocol.total_leds(), 204);
    assert_eq!(protocol.zones().len(), 1);
}

#[test]
fn lookup_returns_lighting_node_pro_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_LIGHTING_NODE_PRO)
        .expect("Corsair Lighting Node Pro descriptor should exist");

    assert_eq!(descriptor.name, "Corsair Lighting Node Pro");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/lighting-node-pro");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Corsair Lighting Node Pro");
    assert_eq!(protocol.total_leds(), 408);
    assert_eq!(protocol.zones().len(), 2);
}

#[test]
fn lookup_returns_commander_pro_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_COMMANDER_PRO)
        .expect("Corsair Commander Pro descriptor should exist");

    assert_eq!(descriptor.name, "Corsair Commander Pro");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/commander-pro");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });
}

#[test]
fn lookup_returns_elite_capellix_lcd_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_ELITE_CAPELLIX_LCD)
        .expect("Corsair Elite Capellix LCD descriptor should exist");

    assert_eq!(descriptor.name, "Corsair Elite Capellix LCD");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/elite-capellix-lcd");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Corsair Elite Capellix LCD");
    assert_eq!(protocol.total_leds(), 0);
    assert_eq!(protocol.capabilities().display_resolution, Some((480, 480)));
}

#[test]
fn lookup_returns_icue_link_lcd_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_ICUE_LINK_LCD)
        .expect("Corsair iCUE LINK LCD descriptor should exist");

    assert_eq!(descriptor.name, "Corsair iCUE LINK LCD");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/icue-link-lcd");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });
}

#[test]
fn lookup_returns_xd6_elite_lcd_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_XD6_ELITE_LCD)
        .expect("Corsair XD6 Elite LCD descriptor should exist");

    assert_eq!(descriptor.name, "Corsair XD6 Elite LCD");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/xd6-elite-lcd");

    let protocol = (descriptor.protocol.build)();
    assert!(protocol.capabilities().has_display);
    assert_eq!(protocol.zones().len(), 1);
}

#[test]
fn lookup_returns_xc7_rgb_elite_lcd_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_XC7_RGB_ELITE_LCD)
        .expect("Corsair XC7 RGB Elite LCD descriptor should exist");

    assert_eq!(descriptor.name, "Corsair XC7 RGB Elite LCD");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("corsair", "Corsair")
    );
    assert_eq!(descriptor.protocol.id, "corsair/xc7-rgb-elite-lcd");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Corsair XC7 RGB Elite LCD");
    assert_eq!(protocol.total_leds(), 31);
    assert_eq!(protocol.zones().len(), 2);
    assert!(protocol.capabilities().has_display);
    assert!(protocol.capabilities().supports_direct);
    assert_eq!(protocol.capabilities().display_resolution, Some((480, 480)));
    assert_eq!(
        protocol.zones()[1].topology,
        DeviceTopologyHint::Ring { count: 31 }
    );
}

#[test]
fn lookup_returns_nollie_8_v2_descriptor() {
    let descriptor = ProtocolDatabase::lookup(NOLLIE_VENDOR_ID, PID_NOLLIE_8_V2)
        .expect("Nollie 8 v2 descriptor should exist");

    assert_eq!(descriptor.name, "Nollie 8 v2");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("nollie", "Nollie")
    );
    assert_eq!(descriptor.protocol.id, "nollie/nollie-8-v2");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });
}

#[test]
fn lookup_returns_nollie_1_descriptor() {
    let descriptor = ProtocolDatabase::lookup(NOLLIE_VENDOR_ID, PID_NOLLIE_1)
        .expect("Nollie 1 descriptor should exist");

    assert_eq!(descriptor.name, "Nollie 1");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("nollie", "Nollie")
    );
    assert_eq!(descriptor.protocol.id, "nollie/nollie-1");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 630);
    assert_eq!(protocol.zones().len(), 1);
}

#[test]
fn lookup_returns_nollie_28_12_descriptor() {
    let descriptor = ProtocolDatabase::lookup(NOLLIE_VENDOR_ID, PID_NOLLIE_28_12_A)
        .expect("Nollie 28/12 descriptor should exist");

    assert_eq!(descriptor.name, "Nollie 28/12");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("nollie", "Nollie")
    );
    assert_eq!(descriptor.protocol.id, "nollie/nollie-28-12");

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 504);
    assert_eq!(protocol.zones().len(), 12);
}

#[test]
fn lookup_returns_nollie_gen2_descriptors() {
    let nollie16 = ProtocolDatabase::lookup(NOLLIE_GEN2_VENDOR_ID, PID_NOLLIE_16_V3)
        .expect("Nollie 16 v3 descriptor should exist");
    assert_eq!(nollie16.name, "Nollie 16 v3");
    assert_eq!(
        nollie16.family,
        DeviceFamily::new_static("nollie", "Nollie")
    );
    assert_eq!(nollie16.protocol.id, "nollie/nollie-16-v3");

    let nollie32 = ProtocolDatabase::lookup(NOLLIE_GEN2_VENDOR_ID, PID_NOLLIE_32)
        .expect("Nollie 32 descriptor should exist");
    assert_eq!(nollie32.name, "Nollie 32");
    assert_eq!(
        nollie32.family,
        DeviceFamily::new_static("nollie", "Nollie")
    );
    assert_eq!(nollie32.protocol.id, "nollie/nollie-32");
}

#[test]
fn lookup_returns_prism_s_descriptor() {
    let descriptor = ProtocolDatabase::lookup(PRISM_GCS_VENDOR_ID, PID_PRISM_S)
        .expect("Prism S descriptor should exist");

    assert_eq!(descriptor.name, "PrismRGB Prism S");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("prismrgb", "PrismRGB")
    );
    assert_eq!(descriptor.protocol.id, "prismrgb/prism-s");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 2 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 282);
    assert_eq!(protocol.zones().len(), 2);
}

#[test]
fn lookup_returns_prism_mini_descriptor() {
    let descriptor = ProtocolDatabase::lookup(PRISM_GCS_VENDOR_ID, PID_PRISM_MINI)
        .expect("Prism Mini descriptor should exist");

    assert_eq!(descriptor.name, "PrismRGB Prism Mini");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("prismrgb", "PrismRGB")
    );
    assert_eq!(descriptor.protocol.id, "prismrgb/prism-mini");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 2 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 128);
    assert_eq!(protocol.zones().len(), 1);
}

#[test]
fn lookup_returns_huntsman_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_HUNTSMAN_V2)
        .expect("Huntsman V2 descriptor should exist");

    assert_eq!(descriptor.name, "Razer Huntsman V2");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(descriptor.protocol.id, "razer/huntsman-v2");
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(3, 0x00, Some(0x000C), Some(0x0001))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Extended");
    assert_eq!(protocol.total_leds(), 132);
}

#[test]
fn lookup_returns_basilisk_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");

    assert_eq!(descriptor.name, "Razer Basilisk V3");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(3, 0x00, Some(0x000C), Some(0x0001))
    );
}

#[test]
fn lookup_returns_mamba_elite_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_MAMBA_ELITE)
        .expect("Mamba Elite descriptor should exist");

    assert_eq!(descriptor.name, "Razer Mamba Elite");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(descriptor.protocol.id, "razer/mamba-elite");
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(0, 0x00, Some(0x0001), Some(0x0002))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Modern");
    assert_eq!(protocol.total_leds(), 20);
}

#[test]
fn lookup_returns_tartarus_chroma_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_TARTARUS_CHROMA)
        .expect("Tartarus Chroma descriptor should exist");

    assert_eq!(descriptor.name, "Razer Tartarus Chroma");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(descriptor.protocol.id, "razer/tartarus-chroma");
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(2, 0x00, Some(0x0001), Some(0x0002))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer 0x1F Standard");
    assert_eq!(protocol.total_leds(), 1);
    assert!(!protocol.capabilities().supports_brightness);
}

#[test]
fn lookup_returns_blade_15_late_2021_advanced_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_15_LATE_2021_ADVANCED)
        .expect("Blade descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade 15 (Late 2021 Advanced)");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(descriptor.protocol.id, "razer/blade-15-late-2021-advanced");

    assert_eq!(
        descriptor.transport,
        TransportType::UsbControl {
            interface: 2,
            report_id: 0x00
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer 0x1F Standard");
    assert_eq!(protocol.total_leds(), 96);
    assert!(protocol.init_sequence().is_empty());
    assert!(protocol.shutdown_sequence().is_empty());
}

#[test]
fn lookup_returns_blade_14_2021_descriptor_with_keepalive() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_14_2021)
        .expect("Blade 14 (2021) descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade 14 (2021)");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbControl {
            interface: 2,
            report_id: 0x00
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer 0x3F Standard");
    assert!(protocol.keepalive().is_some());
}

#[test]
fn lookup_returns_blade_pro_2016_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_PRO_2016)
        .expect("Blade Pro (2016) descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade Pro (2016)");
    assert_eq!(descriptor.protocol.id, "razer/blade-pro-2016");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbControl {
            interface: 2,
            report_id: 0x00
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Legacy");
    assert_eq!(protocol.total_leds(), 150);
    assert!(protocol.init_sequence().is_empty());
    assert!(protocol.shutdown_sequence().is_empty());
}

#[test]
fn lookup_returns_blade_15_2022_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_15_2022)
        .expect("Blade 15 (2022) descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade 15 (2022)");
    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer 0x1F Standard");
    assert_eq!(protocol.total_leds(), 96);
}

#[test]
fn lookup_returns_blade_14_2023_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_14_2023)
        .expect("Blade 14 (2023) descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade 14 (2023)");
    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer 0x1F Standard");
    assert_eq!(protocol.total_leds(), 96);
}

#[test]
fn lookup_returns_blade_14_2022_descriptor_with_keepalive() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_14_2022)
        .expect("Blade 14 (2022) descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade 14 (2022)");
    assert_eq!(
        descriptor.protocol.id,
        "razer/matrix-standard-1f-laptop-6x16"
    );
    assert_eq!(
        descriptor.transport,
        TransportType::UsbControl {
            interface: 2,
            report_id: 0x00
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer 0x1F Standard");
    assert_eq!(protocol.total_leds(), 96);
    assert!(protocol.keepalive().is_some());
}

#[test]
fn lookup_returns_blackwidow_v3_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLACKWIDOW_V3)
        .expect("BlackWidow V3 descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blackwidow V3");
    assert_eq!(
        descriptor.protocol.id,
        "razer/matrix-extended-3f-6x22-backlight"
    );
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(3, 0x00, Some(0x000C), Some(0x0001))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 132);
}

#[test]
fn lookup_returns_firefly_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_FIREFLY)
        .expect("Firefly descriptor should exist");

    assert_eq!(descriptor.name, "Razer Firefly");
    assert_eq!(descriptor.protocol.id, "razer/matrix-linear-3f-1x15");
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(0, 0x00, Some(0x0001), Some(0x0002))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 15);
}

#[test]
fn lookup_returns_laptop_stand_chroma_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_LAPTOP_STAND_CHROMA)
        .expect("Laptop Stand Chroma descriptor should exist");

    assert_eq!(descriptor.name, "Razer Laptop Stand Chroma");
    assert_eq!(descriptor.protocol.id, "razer/matrix-extended-1f-1x15-zero");
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(0, 0x00, Some(0x0001), Some(0x0002))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 15);
}

#[test]
fn lookup_returns_thunderbolt_4_dock_descriptor_with_interface_wildcard() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_THUNDERBOLT_4_DOCK_CHROMA)
        .expect("Thunderbolt 4 Dock Chroma descriptor should exist");

    assert_eq!(descriptor.name, "Razer Thunderbolt 4 Dock Chroma");
    assert_eq!(
        descriptor.protocol.id,
        "razer/matrix-extended-3f-1x12-backlight"
    );
    assert_eq!(
        descriptor.transport,
        TransportType::UsbHidApi {
            interface: None,
            report_id: 0x00,
            report_mode: HidRawReportMode::FeatureReport,
            usage_page: Some(0x000C),
            usage: Some(0x0001),
        }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.total_leds(), 12);
}

#[test]
fn lookup_returns_seiren_v3_chroma_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_SEIREN_V3_CHROMA)
        .expect("Seiren V3 Chroma descriptor should exist");

    assert_eq!(descriptor.name, "Razer Seiren V3 Chroma");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );
    assert_eq!(descriptor.protocol.id, "razer/seiren-v3-chroma");
    assert_eq!(
        descriptor.transport,
        expected_razer_shared_hid_transport(3, 0x07, Some(0xFF53), Some(0x0004))
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Seiren V3");
    assert_eq!(protocol.total_leds(), 10);
    assert_eq!(protocol.zones()[0].topology, DeviceTopologyHint::Custom);
}

#[test]
fn lookup_returns_seiren_emote_with_8x8_zone_topology() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_SEIREN_EMOTE)
        .expect("Seiren Emote descriptor should exist");

    assert_eq!(descriptor.name, "Razer Seiren Emote");
    assert_eq!(
        descriptor.family,
        DeviceFamily::new_static("razer", "Razer")
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Extended");
    assert_eq!(protocol.total_leds(), 64);
    assert_eq!(protocol.zones().len(), 1);

    match &protocol.zones()[0].topology {
        DeviceTopologyHint::Matrix { rows, cols } => assert_eq!((*rows, *cols), (8, 8)),
        other => panic!("expected matrix topology, got {other:?}"),
    }
}

#[test]
fn known_vid_pid_contains_razer_entries() {
    let pairs = ProtocolDatabase::known_vid_pids();
    assert!(pairs.contains(&(ABLETON_VENDOR_ID, PID_PUSH_2)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ELITE_CAPELLIX_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ELITE_CAPELLIX_LCD_ALT)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ICUE_LINK_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_NAUTILUS_RS_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_XC7_RGB_ELITE_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_XD6_ELITE_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ICUE_LINK_SYSTEM_HUB)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_LIGHTING_NODE_CORE)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_LIGHTING_NODE_PRO)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_COMMANDER_PRO)));
    assert!(pairs.contains(&(DYGMA_VENDOR_ID, PID_DEFY_WIRED)));
    assert!(pairs.contains(&(DYGMA_VENDOR_ID, PID_DEFY_WIRELESS)));
    assert!(pairs.contains(&(LIANLI_ENE_VENDOR_ID, PID_UNI_HUB_SL_INFINITY)));
    assert!(pairs.contains(&(LIANLI_TL_VENDOR_ID, PID_TL_FAN_HUB)));
    assert!(pairs.contains(&(PRISM_VENDOR_ID, PID_PRISM_8)));
    assert!(pairs.contains(&(NOLLIE_VENDOR_ID, PID_NOLLIE_8_V2)));
    assert!(pairs.contains(&(PRISM_GCS_VENDOR_ID, PID_PRISM_S)));
    assert!(pairs.contains(&(PRISM_GCS_VENDOR_ID, PID_PRISM_MINI)));
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_HUNTSMAN_V2)));
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_BASILISK_V3)));
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_BLADE_14_2021)));
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_BLADE_15_LATE_2021_ADVANCED)));
}

#[test]
fn by_vendor_returns_only_razer_entries() {
    let descriptors = ProtocolDatabase::by_vendor(RAZER_VENDOR_ID);
    assert!(!descriptors.is_empty());
    assert!(
        descriptors
            .iter()
            .all(|descriptor| descriptor.vendor_id == RAZER_VENDOR_ID)
    );
}

#[test]
fn lookup_filters_by_enabled_hal_driver_ids() {
    let nollie_only = BTreeSet::from(["nollie".to_owned()]);
    let razer_only = BTreeSet::from(["razer".to_owned()]);

    let enabled = ProtocolDatabase::lookup_with_firmware_for_driver_ids(
        NOLLIE_VENDOR_ID,
        PID_NOLLIE_1,
        None,
        Some(&nollie_only),
    )
    .expect("enabled Nollie descriptor should resolve");
    assert_eq!(enabled.family, DeviceFamily::new_static("nollie", "Nollie"));

    let prism_8 = ProtocolDatabase::lookup_with_firmware_for_driver_ids(
        PRISM_VENDOR_ID,
        PID_PRISM_8,
        None,
        Some(&nollie_only),
    )
    .expect("Nollie-owned Prism 8 descriptor should resolve");
    assert_eq!(
        prism_8.family,
        DeviceFamily::new_static("prismrgb", "PrismRGB")
    );
    assert_eq!(prism_8.driver_id(), "nollie");

    let disabled = ProtocolDatabase::lookup_with_firmware_for_driver_ids(
        NOLLIE_VENDOR_ID,
        PID_NOLLIE_1,
        None,
        Some(&razer_only),
    );
    assert!(disabled.is_none());
}

#[test]
fn count_matches_static_descriptor_count() {
    assert_eq!(ProtocolDatabase::count(), ProtocolDatabase::all().len());
    assert!(ProtocolDatabase::count() >= 28);
}

#[test]
fn module_descriptors_group_hal_protocols_by_family() {
    let modules = ProtocolDatabase::module_descriptors();
    let ids = modules
        .iter()
        .map(|module| module.id.as_str())
        .collect::<Vec<_>>();

    assert!(ids.contains(&"asus"));
    assert!(ids.contains(&"nollie"));
    assert!(ids.contains(&"prismrgb"));
    assert!(ids.contains(&"razer"));

    let nollie = modules
        .iter()
        .find(|module| module.id == "nollie")
        .expect("Nollie module descriptor should exist");
    assert_eq!(nollie.display_name, "Nollie");
    assert_eq!(nollie.module_kind, DriverModuleKind::Hal);
    assert_eq!(nollie.transports, vec![DriverTransportKind::Usb]);
    assert!(nollie.capabilities.protocol_catalog);
    assert!(!nollie.capabilities.output_backend);
    assert!(nollie.default_enabled);

    let asus = modules
        .iter()
        .find(|module| module.id == "asus")
        .expect("ASUS module descriptor should exist");
    assert_eq!(
        asus.transports,
        vec![DriverTransportKind::Usb, DriverTransportKind::Smbus]
    );

    let push2 = modules
        .iter()
        .find(|module| module.id == "push2")
        .expect("Push 2 module descriptor should exist");
    assert_eq!(push2.transports, vec![DriverTransportKind::Midi]);

    let dygma = modules
        .iter()
        .find(|module| module.id == "dygma")
        .expect("Dygma module descriptor should exist");
    assert_eq!(dygma.transports, vec![DriverTransportKind::Serial]);
}

#[test]
fn protocol_descriptors_expose_hal_catalog_entries() {
    let protocols = ProtocolDatabase::protocol_descriptors_for_driver("nollie");
    let nollie_8 = protocols
        .iter()
        .find(|protocol| protocol.protocol_id == "nollie/nollie-8-v2")
        .expect("Nollie 8 V2 protocol descriptor should exist");

    assert_eq!(nollie_8.driver_id, "nollie");
    assert_eq!(nollie_8.display_name, "Nollie 8 v2");
    assert_eq!(nollie_8.family_id, "nollie");
    assert_eq!(nollie_8.transport, DriverTransportKind::Usb);
    assert_eq!(nollie_8.route_backend_id, "usb");
    assert_eq!(nollie_8.vendor_id, Some(NOLLIE_VENDOR_ID));
    assert_eq!(nollie_8.product_id, Some(PID_NOLLIE_8_V2));

    let asus = ProtocolDatabase::protocol_descriptors_for_driver("asus");
    assert!(asus.iter().any(|protocol| {
        protocol.protocol_id == "asus/aura-smbus"
            && protocol.transport == DriverTransportKind::Smbus
            && protocol.route_backend_id == "smbus"
    }));
}
