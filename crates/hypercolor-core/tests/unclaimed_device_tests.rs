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
