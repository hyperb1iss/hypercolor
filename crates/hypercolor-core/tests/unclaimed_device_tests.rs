//! Contract tests for the unclaimed USB inventory the scanner and hotplug
//! watcher feed.

use std::collections::BTreeSet;
use std::sync::Arc;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{UnclaimedDeviceStore, UsbObservation};
use hypercolor_types::event::HypercolorEvent;

fn observation(vendor_id: u16, product_id: u16, bus_path: &str) -> UsbObservation {
    UsbObservation {
        vendor_id,
        product_id,
        manufacturer: Some("Acme".to_owned()),
        product: Some("Widget".to_owned()),
        serial: None,
        bus_path: Some(bus_path.to_owned()),
        device_class: 0,
        interface_classes: vec![3, 3, 255],
        descriptor_driver_id: None,
    }
}

fn claimed_by(mut observation: UsbObservation, driver_id: &str) -> UsbObservation {
    observation.descriptor_driver_id = Some(driver_id.to_owned());
    observation
}

fn drain_counts(
    events: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<usize> {
    let mut counts = Vec::new();
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::UnclaimedDevicesChanged { count } = timestamped.event {
            counts.push(count);
        }
    }
    counts
}

#[test]
fn a_scan_snapshot_records_devices_no_descriptor_matches() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([
        observation(0x1234, 0x0001, "1-1.2"),
        claimed_by(observation(0x1532, 0x0226, "1-1.3"), "razer"),
    ]);

    let snapshot = store.snapshot();
    assert_eq!(
        snapshot.len(),
        1,
        "descriptor-backed devices are claimed by default"
    );
    let device = &snapshot[0];
    assert_eq!((device.vendor_id, device.product_id), (0x1234, 0x0001));
    assert_eq!(device.bus_path.as_deref(), Some("1-1.2"));
    assert_eq!(device.manufacturer.as_deref(), Some("Acme"));
    assert_eq!(device.product.as_deref(), Some("Widget"));
    assert_eq!(
        device.interface_classes,
        vec![3, 255],
        "interface classes are sorted and deduplicated"
    );
    assert_eq!(device.claimable_by, None);
}

#[test]
fn a_disabled_driver_makes_its_devices_claimable() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([
        claimed_by(observation(0x1532, 0x0226, "1-1.3"), "razer"),
        claimed_by(observation(0x0CF2, 0x7750, "1-1.4"), "lian_li"),
    ]);
    assert!(
        store.is_empty(),
        "no enabled set means every descriptor claims"
    );

    store.set_enabled_driver_ids(Some(BTreeSet::from(["lian_li".to_owned()])));
    let snapshot = store.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].claimable_by.as_deref(), Some("razer"));

    store.set_enabled_driver_ids(Some(BTreeSet::from([
        "lian_li".to_owned(),
        "razer".to_owned(),
    ])));
    assert!(
        store.is_empty(),
        "enabling the driver claims the device again"
    );
    assert_eq!(
        store.enabled_driver_ids(),
        Some(BTreeSet::from(["lian_li".to_owned(), "razer".to_owned()]))
    );
}

#[test]
fn hotplug_patches_add_and_remove_single_devices() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([observation(0x1234, 0x0001, "1-1.2")]);

    let arrival = observation(0x5678, 0x0002, "1-1.5");
    let key = arrival.key();
    store.upsert(arrival);
    assert_eq!(store.len(), 2);

    store.remove(&key);
    let snapshot = store.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].vendor_id, 0x1234);

    store.remove("never-seen");
    assert_eq!(store.len(), 1, "removing an unknown key is a no-op");
}

#[test]
fn a_replaced_snapshot_forgets_devices_that_left() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([
        observation(0x1234, 0x0001, "1-1.2"),
        observation(0x5678, 0x0002, "1-1.5"),
    ]);
    store.replace_snapshot([observation(0x5678, 0x0002, "1-1.5")]);

    let snapshot = store.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].vendor_id, 0x5678);
}

#[test]
fn observation_keys_separate_identical_units_on_different_ports() {
    let left = observation(0x1234, 0x0001, "1-1.2");
    let right = observation(0x1234, 0x0001, "1-1.3");
    assert_ne!(left.key(), right.key());
    assert_eq!(left.key(), observation(0x1234, 0x0001, "1-1.2").key());
}

#[test]
fn the_store_publishes_a_count_only_when_the_view_changes() {
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let store = UnclaimedDeviceStore::new().with_event_bus(Arc::clone(&bus));

    store.replace_snapshot([observation(0x1234, 0x0001, "1-1.2")]);
    store.replace_snapshot([observation(0x1234, 0x0001, "1-1.2")]);
    assert_eq!(
        drain_counts(&mut events),
        vec![1],
        "an identical rescan is silent"
    );

    store.upsert(observation(0x5678, 0x0002, "1-1.5"));
    store.set_enabled_driver_ids(Some(BTreeSet::new()));
    store.set_enabled_driver_ids(Some(BTreeSet::new()));
    assert_eq!(
        drain_counts(&mut events),
        vec![2],
        "an enabled-set change that alters nothing is silent"
    );

    store.upsert(claimed_by(observation(0x1532, 0x0226, "1-1.3"), "razer"));
    assert_eq!(
        drain_counts(&mut events),
        vec![3],
        "empty enabled set makes it claimable"
    );

    store.replace_snapshot([]);
    assert_eq!(drain_counts(&mut events), vec![0]);
}

#[test]
fn a_missing_manufacturer_string_falls_back_to_the_vid_owner() {
    let store = UnclaimedDeviceStore::new();
    let mut razer = observation(0x1532, 0x0527, "1-1.2");
    razer.manufacturer = None;
    let mut blank = observation(0x1B1C, 0x0C99, "1-1.3");
    blank.manufacturer = Some("  ".to_owned());
    let mut shared = observation(0x1CBE, 0xA088, "1-1.4");
    shared.manufacturer = None;
    store.replace_snapshot([razer, blank, shared]);

    let manufacturers: Vec<_> = store
        .snapshot()
        .into_iter()
        .map(|device| (device.vendor_id, device.manufacturer))
        .collect();
    assert_eq!(
        manufacturers,
        vec![
            (0x1532, Some("Razer".to_owned())),
            (0x1B1C, Some("Corsair".to_owned())),
            (0x1CBE, None),
        ],
        "a shared VID names nobody rather than guessing a brand"
    );
}

#[test]
fn a_reported_manufacturer_string_wins_over_the_vid_owner() {
    let store = UnclaimedDeviceStore::new();
    let mut razer = observation(0x1532, 0x0527, "1-1.2");
    razer.manufacturer = Some("Razer Inc.".to_owned());
    store.replace_snapshot([razer]);

    assert_eq!(
        store.snapshot()[0].manufacturer.as_deref(),
        Some("Razer Inc.")
    );
}

fn with_classes(
    mut observation: UsbObservation,
    device_class: u8,
    interface_classes: &[u8],
) -> UsbObservation {
    observation.device_class = device_class;
    observation.interface_classes = interface_classes.to_vec();
    observation
}

#[test]
fn hubs_and_audio_only_functions_cannot_be_lighting() {
    let base = || observation(0x1532, 0x48F0, "1-1.2");

    assert!(with_classes(base(), 0x09, &[0x09]).cannot_be_lighting());
    assert!(
        with_classes(base(), 0x09, &[]).cannot_be_lighting(),
        "the device class alone marks a hub whose interfaces went unread"
    );
    assert!(
        with_classes(base(), 0x00, &[0x09, 0x09]).cannot_be_lighting(),
        "every interface a hub interface is a hub"
    );
    assert!(with_classes(base(), 0x00, &[0x01, 0x01, 0x01]).cannot_be_lighting());
    assert!(with_classes(base(), 0xEF, &[0x01, 0x01]).cannot_be_lighting());
}

#[test]
fn anything_with_a_controllable_interface_may_be_lighting() {
    let base = || observation(0x1532, 0x0527, "1-1.2");

    assert!(
        !with_classes(base(), 0x00, &[0x01, 0x01, 0x01, 0x03]).cannot_be_lighting(),
        "an RGB headset exposes HID beside its audio interfaces"
    );
    assert!(!with_classes(base(), 0x00, &[0x01, 0xFF]).cannot_be_lighting());
    assert!(!with_classes(base(), 0x00, &[0x03]).cannot_be_lighting());
    assert!(!with_classes(base(), 0xEF, &[0x02, 0x0A]).cannot_be_lighting());
    assert!(
        !with_classes(base(), 0x00, &[]).cannot_be_lighting(),
        "no interfaces reported proves nothing"
    );
}

#[test]
fn a_mass_storage_drive_cannot_be_lighting() {
    let drive = with_classes(observation(0x0BC2, 0x3322, "1-1.2"), 0x00, &[0x08]);
    assert!(drive.cannot_be_lighting());
}

#[test]
fn a_bluetooth_radio_cannot_be_lighting() {
    let radio = with_classes(observation(0x0489, 0xE116, "1-14"), 0xEF, &[0xE0, 0xE0]);
    assert!(radio.cannot_be_lighting());
}

#[test]
fn a_webcam_with_a_microphone_cannot_be_lighting() {
    let webcam = with_classes(
        observation(0x1532, 0x0E06, "1-1.3"),
        0xEF,
        &[0x0E, 0x0E, 0x01, 0x01],
    );
    assert!(webcam.cannot_be_lighting());
}

#[test]
fn an_audio_interface_with_a_firmware_update_function_cannot_be_lighting() {
    let interface = with_classes(
        observation(0x0763, 0x400E, "1-10.4.1"),
        0xEF,
        &[0x01, 0x01, 0x01, 0x01, 0x01, 0xFE],
    );
    assert!(
        interface.cannot_be_lighting(),
        "DFU is not a lighting interface"
    );
}

#[test]
fn a_vendor_specific_device_may_be_lighting() {
    let panel = with_classes(observation(0x1CBE, 0xA088, "1-1.4"), 0xFF, &[0xFF]);
    assert!(!panel.cannot_be_lighting());
}

#[test]
fn a_cdc_serial_device_may_be_lighting() {
    let serial = with_classes(observation(0x1A86, 0x55D3, "1-1.5"), 0x02, &[0x02, 0x0A]);
    assert!(!serial.cannot_be_lighting());
}

#[test]
fn the_unclaimed_view_keeps_only_interfaces_that_can_carry_lighting() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([
        with_classes(observation(0x0BC2, 0x3322, "1-1"), 0x00, &[0x08]),
        with_classes(observation(0x0489, 0xE116, "1-2"), 0xEF, &[0xE0, 0xE0]),
        with_classes(
            observation(0x1532, 0x0E06, "1-3"),
            0xEF,
            &[0x0E, 0x0E, 0x01, 0x01],
        ),
        with_classes(observation(0x1CBE, 0xA088, "1-4"), 0xFF, &[0xFF]),
        with_classes(observation(0x1A86, 0x55D3, "1-5"), 0x02, &[0x02, 0x0A]),
    ]);

    let listed: Vec<_> = store
        .snapshot()
        .into_iter()
        .map(|device| (device.vendor_id, device.product_id))
        .collect();
    assert_eq!(listed, vec![(0x1A86, 0x55D3), (0x1CBE, 0xA088)]);
}

#[test]
fn the_unclaimed_view_hides_hubs_and_audio_only_functions() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([
        with_classes(observation(0x1532, 0x48F0, "1-1"), 0x09, &[0x09]),
        with_classes(observation(0x1532, 0x0527, "1-1.1"), 0x00, &[0x01, 0x03]),
        with_classes(observation(0x1234, 0x0002, "1-1.2"), 0x00, &[0x01, 0x01]),
        with_classes(observation(0x1CBE, 0xA088, "1-1.3"), 0xEF, &[0x02, 0x0A]),
    ]);

    let listed: Vec<_> = store
        .snapshot()
        .into_iter()
        .map(|device| (device.vendor_id, device.product_id))
        .collect();
    assert_eq!(listed, vec![(0x1532, 0x0527), (0x1CBE, 0xA088)]);
}

#[test]
fn a_matching_descriptor_outranks_the_class_filter() {
    let store = UnclaimedDeviceStore::new();
    store.replace_snapshot([claimed_by(
        with_classes(observation(0x1532, 0x0F20, "1-1.4"), 0x09, &[0x09]),
        "razer",
    )]);
    store.set_enabled_driver_ids(Some(BTreeSet::new()));

    let snapshot = store.snapshot();
    assert_eq!(
        snapshot.len(),
        1,
        "a disabled driver's device stays claimable whatever its classes say"
    );
    assert_eq!(snapshot[0].claimable_by.as_deref(), Some("razer"));
}
