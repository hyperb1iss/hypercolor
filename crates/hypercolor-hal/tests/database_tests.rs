use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::{PID_BASILISK_V3, PID_HUNTSMAN_V2, RAZER_VENDOR_ID};
use hypercolor_types::device::DeviceFamily;

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
fn known_vid_pid_contains_razer_entries() {
    let pairs = ProtocolDatabase::known_vid_pids();
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_HUNTSMAN_V2)));
    assert!(pairs.contains(&(RAZER_VENDOR_ID, PID_BASILISK_V3)));
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
    assert!(ProtocolDatabase::count() >= 3);
}
