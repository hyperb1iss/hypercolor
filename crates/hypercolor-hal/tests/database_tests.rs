use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{
    PID_BASILISK_V3, PID_BLADE_15_LATE_2021_ADVANCED, PID_HUNTSMAN_V2, PID_SEIREN_EMOTE,
    RAZER_VENDOR_ID,
};
use hypercolor_hal::registry::TransportType;
use hypercolor_types::device::{DeviceFamily, DeviceTopologyHint};

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
        TransportType::UsbHidRaw {
            interface: 2,
            report_id: 0x00
        }
    );

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
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_HUNTSMAN_V2)));
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_BASILISK_V3)));
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
    assert!(ProtocolDatabase::count() >= 4);
}
