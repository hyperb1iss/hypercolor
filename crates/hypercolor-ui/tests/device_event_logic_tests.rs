#![allow(dead_code)]

#[path = "../src/device_event_logic.rs"]
mod device_event_logic;

use device_event_logic::should_refetch_devices_for_event;

#[test]
fn connected_device_refetches_only_when_unknown() {
    let current = ["device-1"];

    assert!(!should_refetch_devices_for_event(
        "device_connected",
        Some("device-1"),
        None,
        &current,
    ));
    assert!(should_refetch_devices_for_event(
        "device_connected",
        Some("device-2"),
        None,
        &current,
    ));
}

#[test]
fn state_changes_refetch_only_for_known_devices() {
    let current = ["device-1"];

    assert!(should_refetch_devices_for_event(
        "device_state_changed",
        Some("device-1"),
        None,
        &current,
    ));
    assert!(!should_refetch_devices_for_event(
        "device_state_changed",
        Some("device-2"),
        None,
        &current,
    ));
}

#[test]
fn discovery_completed_only_refetches_when_new_devices_were_found_and_list_is_empty() {
    let empty: [&str; 0] = [];
    let current = ["device-1"];

    assert!(should_refetch_devices_for_event(
        "device_discovery_completed",
        None,
        Some(2),
        &empty,
    ));
    assert!(!should_refetch_devices_for_event(
        "device_discovery_completed",
        None,
        Some(0),
        &empty,
    ));
    assert!(!should_refetch_devices_for_event(
        "device_discovery_completed",
        None,
        Some(2),
        &current,
    ));
}
