use std::collections::BTreeSet;
use std::sync::Arc;

use hypercolor_types::event::HypercolorEvent;

use super::*;
use crate::bus::HypercolorBus;

fn observation(vendor_id: u16, product_id: u16) -> UsbObservation {
    UsbObservation {
        vendor_id,
        product_id,
        manufacturer: Some("Fixture".to_owned()),
        product: Some("USB controller".to_owned()),
        serial: Some("fixture-1".to_owned()),
        bus_path: Some("1-2.3".to_owned()),
        interface_classes: vec![3],
        descriptor_driver_id: ProtocolDatabase::lookup(vendor_id, product_id)
            .map(|descriptor| descriptor.driver_id().to_string()),
    }
}

#[test]
fn unsupported_hotplug_updates_shared_inventory_and_publishes_without_scan() {
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let store = UnclaimedDeviceStore::new().with_event_bus(bus);
    let monitor = UsbHotplugMonitor::new(8).with_unclaimed_store(store.clone());
    let mut claimed_events = monitor.subscribe();
    let arrival = observation(0xffff, 0xffff);
    assert!(ProtocolDatabase::lookup(arrival.vendor_id, arrival.product_id).is_none());
    let seen = SeenDevice {
        vendor_id: arrival.vendor_id,
        product_id: arrival.product_id,
        claimed: false,
        observation_key: arrival.key(),
    };

    monitor.record_arrival(arrival, None);
    assert_eq!(store.snapshot()[0].serial.as_deref(), Some("fixture-1"));
    assert!(matches!(
        events.try_recv().expect("arrival event").event,
        HypercolorEvent::UnclaimedDevicesChanged { count: 1 }
    ));
    assert!(claimed_events.try_recv().is_err());

    monitor.record_removal(&seen);
    assert!(store.is_empty());
    assert!(matches!(
        events.try_recv().expect("removal event").event,
        HypercolorEvent::UnclaimedDevicesChanged { count: 0 }
    ));
    assert!(claimed_events.try_recv().is_err());
}

#[test]
fn disabled_native_hotplug_remains_claimable_and_keeps_hal_notifications() {
    let store = UnclaimedDeviceStore::new();
    store.set_enabled_driver_ids(Some(BTreeSet::new()));
    let monitor = UsbHotplugMonitor::new(8).with_unclaimed_store(store.clone());
    let mut claimed_events = monitor.subscribe();
    let arrival = observation(0x1532, 0x0226);
    let descriptor = ProtocolDatabase::lookup(arrival.vendor_id, arrival.product_id)
        .expect("known Razer keyboard");
    let seen = SeenDevice {
        vendor_id: arrival.vendor_id,
        product_id: arrival.product_id,
        claimed: true,
        observation_key: arrival.key(),
    };

    monitor.record_arrival(arrival, Some(descriptor));
    assert_eq!(store.snapshot()[0].claimable_by.as_deref(), Some("razer"));
    assert!(matches!(
        claimed_events.try_recv().expect("HAL arrival"),
        UsbHotplugEvent::Arrived { .. }
    ));

    store.set_enabled_driver_ids(Some(BTreeSet::from(["razer".to_owned()])));
    assert!(store.is_empty(), "enabling the owner reclaims the device");
    monitor.record_removal(&seen);
    assert!(matches!(
        claimed_events.try_recv().expect("HAL removal"),
        UsbHotplugEvent::Removed { .. }
    ));
    store.set_enabled_driver_ids(Some(BTreeSet::new()));
    assert!(store.is_empty(), "removed devices must not reappear");
}
