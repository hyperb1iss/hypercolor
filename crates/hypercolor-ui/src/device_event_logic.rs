pub fn should_refetch_devices_for_event<T: AsRef<str>>(
    event_type: &str,
    device_id: Option<&str>,
    found_count: Option<usize>,
    current_device_ids: &[T],
) -> bool {
    let is_known = device_id.is_some_and(|id| {
        current_device_ids
            .iter()
            .any(|current| current.as_ref() == id)
    });

    match event_type {
        "device_connected" | "device_discovered" => !is_known,
        "device_disconnected" | "device_state_changed" => is_known,
        "device_discovery_completed" => {
            current_device_ids.is_empty() && found_count.is_some_and(|count| count > 0)
        }
        _ => false,
    }
}
