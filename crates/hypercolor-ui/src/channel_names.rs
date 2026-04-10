//! Shared channel/slot display-name overrides stored in localStorage.

use crate::storage;

fn ls_channel_key(device_id: &str, slot_id: &str) -> String {
    format!("hc-channel-name-{device_id}-{slot_id}")
}

pub fn load_channel_name(device_id: &str, slot_id: &str) -> Option<String> {
    storage::get(&ls_channel_key(device_id, slot_id))
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

pub fn effective_channel_name(device_id: &str, slot_id: &str, default_name: &str) -> String {
    load_channel_name(device_id, slot_id).unwrap_or_else(|| default_name.to_owned())
}

pub fn save_channel_name(device_id: &str, slot_id: &str, default_name: &str, name: &str) {
    let key = ls_channel_key(device_id, slot_id);
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == default_name {
        storage::remove(&key);
    } else {
        storage::set(&key, trimmed);
    }
}
