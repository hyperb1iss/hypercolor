//! Contract tests for joining HID top-level collections onto USB
//! observations.

use hypercolor_core::device::{HidUsageIndex, UsbObservation};
use hypercolor_hal::transport::hidapi::HidCollectionInfo;

const CONSUMER: u16 = 0x000C;
const GENERIC_DESKTOP: u16 = 0x0001;

fn unit(product_id: u16, serial: Option<&str>, bus_path: &str) -> UsbObservation {
    UsbObservation {
        vendor_id: 0x1532,
        product_id,
        manufacturer: None,
        product: None,
        serial: serial.map(ToOwned::to_owned),
        bus_path: Some(bus_path.to_owned()),
        device_class: 0,
        interface_classes: vec![0x03],
        descriptor_driver_id: None,
        hid_usage_pages: Vec::new(),
    }
}

fn collection(product_id: u16, interface: Option<u8>, usage_page: u16) -> HidCollectionInfo {
    HidCollectionInfo {
        vendor_id: 0x1532,
        product_id,
        serial: None,
        usb_path: None,
        interface_number: interface,
        usage_page,
        usage: 0x0001,
    }
}

fn at_path(mut collection: HidCollectionInfo, usb_path: &str) -> HidCollectionInfo {
    collection.usb_path = Some(usb_path.to_owned());
    collection
}

fn with_serial(mut collection: HidCollectionInfo, serial: &str) -> HidCollectionInfo {
    collection.serial = Some(serial.to_owned());
    collection
}

#[test]
fn a_usb_path_separates_identical_units_without_serials() {
    let index = HidUsageIndex::new(vec![
        at_path(collection(0x48F0, Some(0), CONSUMER), "1-1.2"),
        at_path(collection(0x48F0, Some(0), GENERIC_DESKTOP), "1-1.3"),
    ]);

    assert_eq!(
        index.usage_pages_for(&unit(0x48F0, None, "001-1.2"), &[0], true),
        vec![CONSUMER],
        "nusb pads the bus number; the path still matches"
    );
    assert_eq!(
        index.usage_pages_for(&unit(0x48F0, None, "001-1.3"), &[0], true),
        vec![GENERIC_DESKTOP]
    );
}

#[test]
fn identical_units_without_serials_or_paths_stay_unknown() {
    // Windows and macOS resolve no USB path on the HID side, so nothing
    // says which collection belongs to which unit.
    let index = HidUsageIndex::new(vec![
        collection(0x48F0, Some(0), CONSUMER),
        collection(0x48F0, Some(0), CONSUMER),
    ]);

    assert!(
        index
            .usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], true)
            .is_empty()
    );
}

#[test]
fn a_lone_unit_without_a_serial_joins_on_vendor_and_product() {
    // Windows hidapi invents a serial from the instance ID when the device
    // has no serial descriptor; nusb reports none.
    let index = HidUsageIndex::new(vec![with_serial(
        collection(0x48F0, Some(0), CONSUMER),
        "8&25D173E0&0&3",
    )]);

    assert_eq!(
        index.usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], false),
        vec![CONSUMER]
    );
}

#[test]
fn a_serial_picks_out_its_own_unit() {
    let index = HidUsageIndex::new(vec![
        with_serial(collection(0x48F0, Some(0), CONSUMER), "A1 \0"),
        with_serial(collection(0x48F0, Some(0), GENERIC_DESKTOP), "B2"),
    ]);

    assert_eq!(
        index.usage_pages_for(&unit(0x48F0, Some("A1"), "1-1.2"), &[0], false),
        vec![CONSUMER],
        "padding in the HID-side serial does not break the match"
    );
    assert_eq!(
        index.usage_pages_for(&unit(0x48F0, Some("B2"), "1-1.3"), &[0], false),
        vec![GENERIC_DESKTOP]
    );
    assert!(
        index
            .usage_pages_for(&unit(0x48F0, Some("C3"), "1-1.4"), &[0], false)
            .is_empty(),
        "a serial nobody reports joins nothing"
    );
}

#[test]
fn a_shared_serial_without_a_path_stays_unknown() {
    let index = HidUsageIndex::new(vec![
        with_serial(collection(0x48F0, Some(0), CONSUMER), "SAME"),
        with_serial(collection(0x48F0, Some(0), CONSUMER), "SAME"),
    ]);

    assert!(
        index
            .usage_pages_for(&unit(0x48F0, Some("SAME"), "1-1.2"), &[0], true)
            .is_empty()
    );
}

#[test]
fn a_path_resolved_elsewhere_never_falls_back_to_vendor_and_product() {
    // Linux resolved this model's only collections to another port, so
    // they belong to some other unit even though no twin was enumerated.
    let index = HidUsageIndex::new(vec![at_path(
        collection(0x48F0, Some(0), CONSUMER),
        "1-1.3",
    )]);

    assert!(
        index
            .usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], false)
            .is_empty()
    );
}

#[test]
fn every_hid_interface_must_be_covered() {
    let partial = HidUsageIndex::new(vec![collection(0x0099, Some(1), CONSUMER)]);
    assert!(
        partial
            .usage_pages_for(&unit(0x0099, None, "1-1.2"), &[0, 1], false)
            .is_empty(),
        "an interface the HID stack has not bound yet leaves the answer unknown"
    );

    let complete = HidUsageIndex::new(vec![
        collection(0x0099, Some(1), CONSUMER),
        collection(0x0099, Some(0), GENERIC_DESKTOP),
        collection(0x0099, Some(1), CONSUMER),
    ]);
    assert_eq!(
        complete.usage_pages_for(&unit(0x0099, None, "1-1.2"), &[0, 1], false),
        vec![GENERIC_DESKTOP, CONSUMER],
        "pages come back sorted and deduplicated"
    );
}

#[test]
fn an_unnamed_interface_resolves_only_when_there_is_one_hid_interface() {
    let index = HidUsageIndex::new(vec![collection(0x48F0, None, CONSUMER)]);

    assert_eq!(
        index.usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], false),
        vec![CONSUMER]
    );
    assert!(
        index
            .usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0, 2], false)
            .is_empty()
    );
}

#[test]
fn a_collection_on_an_interface_usb_does_not_call_hid_spoils_the_join() {
    let index = HidUsageIndex::new(vec![
        collection(0x48F0, Some(0), CONSUMER),
        collection(0x48F0, Some(3), GENERIC_DESKTOP),
    ]);

    assert!(
        index
            .usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], false)
            .is_empty()
    );
}

#[test]
fn nothing_is_known_without_collections_or_hid_interfaces() {
    assert!(
        HidUsageIndex::default()
            .usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], false)
            .is_empty()
    );

    let index = HidUsageIndex::new(vec![collection(0x48F0, Some(0), CONSUMER)]);
    assert!(
        index
            .usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[], false)
            .is_empty()
    );
}

#[test]
fn enumeration_never_fails_the_caller() {
    // Whatever the host exposes, a join against it returns pages or nothing.
    let index = HidUsageIndex::enumerate();
    let _ = index.usage_pages_for(&unit(0x48F0, None, "1-1.2"), &[0], false);
}

#[test]
fn hid_twins_share_vendor_product_and_any_serial_the_unit_reports() {
    let anonymous = unit(0x48F0, None, "1-1.2");
    assert!(anonymous.is_hid_twin_of(0x1532, 0x48F0, None));
    assert!(
        anonymous.is_hid_twin_of(0x1532, 0x48F0, Some("A1")),
        "a unit without a serial cannot rule out any sibling"
    );
    assert!(!anonymous.is_hid_twin_of(0x1532, 0x48F1, None));
    assert!(!anonymous.is_hid_twin_of(0x1B1C, 0x48F0, None));

    let serialled = unit(0x48F0, Some("A1"), "1-1.2");
    assert!(serialled.is_hid_twin_of(0x1532, 0x48F0, Some("A1")));
    assert!(!serialled.is_hid_twin_of(0x1532, 0x48F0, Some("B2")));
    assert!(!serialled.is_hid_twin_of(0x1532, 0x48F0, None));
}
