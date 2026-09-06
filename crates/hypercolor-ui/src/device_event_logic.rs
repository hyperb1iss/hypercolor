/// Statuses under which a device row already reflects a live connection,
/// so a fresh `device_connected` carries nothing the list does not show.
const SETTLED_STATUSES: &[&str] = &["connected", "active"];

/// Decide whether a device lifecycle event should refetch the device list.
///
/// `current_devices` carries `(id, status)` pairs from the last fetched
/// list. The registry keeps unplugged devices around in a `reconnecting`
/// state, so a known id is not proof that nothing changed: a connect on a
/// known row is a status transition and needs a refetch unless the row
/// already reads as connected.
pub fn should_refetch_devices_for_event<I, S>(
    event_type: &str,
    device_id: Option<&str>,
    found_count: Option<usize>,
    current_devices: &[(I, S)],
) -> bool
where
    I: AsRef<str>,
    S: AsRef<str>,
{
    let known_status = device_id.and_then(|id| {
        current_devices
            .iter()
            .find(|(current_id, _)| current_id.as_ref() == id)
            .map(|(_, status)| status.as_ref())
    });
    let is_known = known_status.is_some();
    let is_settled = known_status.is_some_and(|status| {
        SETTLED_STATUSES
            .iter()
            .any(|settled| status.eq_ignore_ascii_case(settled))
    });

    match event_type {
        "device_connected" | "device_discovered" => !is_settled,
        "device_disconnected" | "device_state_changed" => is_known,
        "device_discovery_completed" => {
            current_devices.is_empty() && found_count.is_some_and(|count| count > 0)
        }
        _ => false,
    }
}
