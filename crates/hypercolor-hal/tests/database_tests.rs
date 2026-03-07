use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::corsair::{
    CORSAIR_VID, PID_COMMANDER_PRO, PID_ELITE_CAPELLIX_LCD, PID_ELITE_CAPELLIX_LCD_ALT,
    PID_ICUE_LINK_LCD, PID_ICUE_LINK_SYSTEM_HUB, PID_LIGHTING_NODE_CORE, PID_LIGHTING_NODE_PRO,
    PID_NAUTILUS_RS_LCD, PID_XD6_ELITE_LCD,
};
use hypercolor_hal::drivers::dygma::{DYGMA_VENDOR_ID, PID_DEFY_WIRED, PID_DEFY_WIRELESS};
use hypercolor_hal::drivers::prismrgb::{
    NOLLIE_VENDOR_ID, PID_NOLLIE_8_V2, PID_PRISM_8, PID_PRISM_MINI, PID_PRISM_S,
    PRISM_GCS_VENDOR_ID, PRISM_VENDOR_ID,
};
use hypercolor_hal::drivers::razer::{
    PID_BASILISK_V3, PID_BLADE_14_2021, PID_BLADE_14_2023, PID_BLADE_15_2022,
    PID_BLADE_15_LATE_2021_ADVANCED, PID_HUNTSMAN_V2, PID_SEIREN_EMOTE, RAZER_VENDOR_ID,
};
use hypercolor_hal::registry::TransportType;
use hypercolor_types::device::{DeviceFamily, DeviceTopologyHint};

#[test]
fn lookup_returns_prism_8_descriptor() {
    let descriptor = ProtocolDatabase::lookup(PRISM_VENDOR_ID, PID_PRISM_8)
        .expect("Prism 8 descriptor should exist");

    assert_eq!(descriptor.name, "PrismRGB Prism 8");
    assert_eq!(descriptor.family, DeviceFamily::PrismRgb);
    assert_eq!(descriptor.protocol.id, "prismrgb/prism-8");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "PrismRGB Prism 8");
    assert_eq!(protocol.total_leds(), 1_008);
    assert_eq!(protocol.zones().len(), 8);
}

#[test]
fn lookup_returns_defy_wired_descriptor() {
    let descriptor = ProtocolDatabase::lookup(DYGMA_VENDOR_ID, PID_DEFY_WIRED)
        .expect("Dygma Defy descriptor should exist");

    assert_eq!(descriptor.name, "Dygma Defy");
    assert_eq!(descriptor.family, DeviceFamily::Dygma);
    assert_eq!(descriptor.protocol.id, "dygma/defy-wired");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbSerial { baud_rate: 115_200 }
    );

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Dygma Defy");
    assert_eq!(protocol.total_leds(), 176);
    assert_eq!(protocol.zones().len(), 4);
}

#[test]
fn lookup_returns_defy_wireless_descriptor() {
    let descriptor = ProtocolDatabase::lookup(DYGMA_VENDOR_ID, PID_DEFY_WIRELESS)
        .expect("Dygma Defy Wireless descriptor should exist");

    assert_eq!(descriptor.name, "Dygma Defy Wireless");
    assert_eq!(descriptor.family, DeviceFamily::Dygma);
    assert_eq!(descriptor.protocol.id, "dygma/defy-wireless");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbSerial { baud_rate: 115_200 }
    );
}

#[test]
fn lookup_returns_icue_link_system_hub_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_ICUE_LINK_SYSTEM_HUB)
        .expect("Corsair iCUE LINK System Hub descriptor should exist");

    assert_eq!(descriptor.name, "Corsair iCUE LINK System Hub");
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
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
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
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
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
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
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
    assert_eq!(descriptor.protocol.id, "corsair/commander-pro");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });
}

#[test]
fn lookup_returns_elite_capellix_lcd_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_ELITE_CAPELLIX_LCD)
        .expect("Corsair Elite Capellix LCD descriptor should exist");

    assert_eq!(descriptor.name, "Corsair Elite Capellix LCD");
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
    assert_eq!(descriptor.protocol.id, "corsair/elite-capellix-lcd");
    assert_eq!(
        descriptor.transport,
        TransportType::UsbBulk {
            interface: 0,
            report_id: 0x03,
        }
    );

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
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
    assert_eq!(descriptor.protocol.id, "corsair/icue-link-lcd");
}

#[test]
fn lookup_returns_xd6_elite_lcd_descriptor() {
    let descriptor = ProtocolDatabase::lookup(CORSAIR_VID, PID_XD6_ELITE_LCD)
        .expect("Corsair XD6 Elite LCD descriptor should exist");

    assert_eq!(descriptor.name, "Corsair XD6 Elite LCD");
    assert_eq!(descriptor.family, DeviceFamily::Corsair);
    assert_eq!(descriptor.protocol.id, "corsair/xd6-elite-lcd");

    let protocol = (descriptor.protocol.build)();
    assert!(protocol.capabilities().has_display);
    assert_eq!(protocol.zones().len(), 1);
}

#[test]
fn lookup_returns_nollie_8_v2_descriptor() {
    let descriptor = ProtocolDatabase::lookup(NOLLIE_VENDOR_ID, PID_NOLLIE_8_V2)
        .expect("Nollie 8 v2 descriptor should exist");

    assert_eq!(descriptor.name, "Nollie 8 v2");
    assert_eq!(descriptor.family, DeviceFamily::PrismRgb);
    assert_eq!(descriptor.protocol.id, "prismrgb/nollie-8-v2");
    assert_eq!(descriptor.transport, TransportType::UsbHid { interface: 0 });
}

#[test]
fn lookup_returns_prism_s_descriptor() {
    let descriptor = ProtocolDatabase::lookup(PRISM_GCS_VENDOR_ID, PID_PRISM_S)
        .expect("Prism S descriptor should exist");

    assert_eq!(descriptor.name, "PrismRGB Prism S");
    assert_eq!(descriptor.family, DeviceFamily::PrismRgb);
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
    assert_eq!(descriptor.family, DeviceFamily::PrismRgb);
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
    assert_eq!(descriptor.family, DeviceFamily::Razer);
    assert_eq!(descriptor.protocol.id, "razer/huntsman-v2");

    let protocol = (descriptor.protocol.build)();
    assert_eq!(protocol.name(), "Razer Extended");
    assert_eq!(protocol.total_leds(), 132);
}

#[test]
fn lookup_returns_basilisk_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BASILISK_V3)
        .expect("Basilisk descriptor should exist");

    assert_eq!(descriptor.name, "Razer Basilisk V3");
    assert_eq!(descriptor.family, DeviceFamily::Razer);
}

#[test]
fn lookup_returns_blade_15_late_2021_advanced_descriptor() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_BLADE_15_LATE_2021_ADVANCED)
        .expect("Blade descriptor should exist");

    assert_eq!(descriptor.name, "Razer Blade 15 (Late 2021 Advanced)");
    assert_eq!(descriptor.family, DeviceFamily::Razer);
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
fn lookup_returns_seiren_emote_with_8x8_zone_topology() {
    let descriptor = ProtocolDatabase::lookup(RAZER_VENDOR_ID, PID_SEIREN_EMOTE)
        .expect("Seiren Emote descriptor should exist");

    assert_eq!(descriptor.name, "Razer Seiren Emote");
    assert_eq!(descriptor.family, DeviceFamily::Razer);

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
    assert!(pairs.contains(&(CORSAIR_VID, PID_ELITE_CAPELLIX_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ELITE_CAPELLIX_LCD_ALT)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ICUE_LINK_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_NAUTILUS_RS_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_XD6_ELITE_LCD)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_ICUE_LINK_SYSTEM_HUB)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_LIGHTING_NODE_CORE)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_LIGHTING_NODE_PRO)));
    assert!(pairs.contains(&(CORSAIR_VID, PID_COMMANDER_PRO)));
    assert!(pairs.contains(&(DYGMA_VENDOR_ID, PID_DEFY_WIRED)));
    assert!(pairs.contains(&(DYGMA_VENDOR_ID, PID_DEFY_WIRELESS)));
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
fn count_matches_static_descriptor_count() {
    assert_eq!(ProtocolDatabase::count(), ProtocolDatabase::all().len());
    assert!(ProtocolDatabase::count() >= 26);
}
