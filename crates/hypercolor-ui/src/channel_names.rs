//! Shared channel/slot display-name overrides stored in LocalStorage.

fn ls_channel_key(device_id: &str, slot_id: &str) -> String {
    format!("hc-channel-name-{device_id}-{slot_id}")
}

pub fn load_channel_name(device_id: &str, slot_id: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(&ls_channel_key(device_id, slot_id)).ok())
        .flatten()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

pub fn effective_channel_name(device_id: &str, slot_id: &str, default_name: &str) -> String {
    load_channel_name(device_id, slot_id).unwrap_or_else(|| default_name.to_owned())
}

pub fn save_channel_name(device_id: &str, slot_id: &str, default_name: &str, name: &str) {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let key = ls_channel_key(device_id, slot_id);
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == default_name {
        let _ = storage.remove_item(&key);
    } else {
        let _ = storage.set_item(&key, trimmed);
    }
}
