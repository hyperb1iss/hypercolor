use hypercolor_ui::device_event_logic::should_refetch_devices_for_event;

#[test]
fn connected_device_refetches_unless_the_row_already_reads_connected() {
    let current = [("device-1", "connected"), ("device-2", "active")];

    assert!(!should_refetch_devices_for_event(
        "device_connected",
        Some("device-1"),
        None,
        &current,
    ));
    assert!(!should_refetch_devices_for_event(
        "device_connected",
        Some("device-2"),
        None,
        &current,
    ));
    assert!(should_refetch_devices_for_event(
        "device_connected",
        Some("device-3"),
        None,
        &current,
    ));
}

#[test]
fn replugged_device_refetches_from_a_stale_row() {
    let reconnecting = [("device-1", "reconnecting")];
    let known = [("device-1", "known")];

    assert!(should_refetch_devices_for_event(
        "device_connected",
        Some("device-1"),
        None,
        &reconnecting,
    ));
    assert!(should_refetch_devices_for_event(
        "device_discovered",
        Some("device-1"),
        None,
        &reconnecting,
    ));
    assert!(should_refetch_devices_for_event(
        "device_connected",
        Some("device-1"),
        None,
        &known,
    ));
}

#[test]
fn status_comparison_ignores_case() {
    let current = [("device-1", "Connected")];

    assert!(!should_refetch_devices_for_event(
        "device_connected",
        Some("device-1"),
        None,
        &current,
    ));
}

#[test]
fn state_changes_refetch_only_for_known_devices() {
    let current = [("device-1", "connected")];

    assert!(should_refetch_devices_for_event(
        "device_state_changed",
        Some("device-1"),
        None,
        &current,
    ));
    assert!(should_refetch_devices_for_event(
        "device_disconnected",
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
    let empty: [(&str, &str); 0] = [];
    let current = [("device-1", "connected")];

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
