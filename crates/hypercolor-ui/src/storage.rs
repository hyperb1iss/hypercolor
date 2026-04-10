//! Thin helpers over `web_sys::Storage`, eliminating the
//! `window().and_then(|w| w.local_storage().ok().flatten())` chain that
//! was duplicated ~13 times across the codebase.

use web_sys::Storage;

fn local_storage() -> Option<Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Read a string from `localStorage`.
pub fn get(key: &str) -> Option<String> {
    local_storage()?.get_item(key).ok().flatten()
}

/// Read and parse a typed value from `localStorage`.
pub fn get_parsed<T: std::str::FromStr>(key: &str) -> Option<T> {
    get(key)?.parse().ok()
}

/// Read a typed value, clamped to `[min, max]`, with a fallback default.
pub fn get_clamped(key: &str, default: f64, min: f64, max: f64) -> f64 {
    get_parsed::<f64>(key)
        .map(|v| v.clamp(min, max))
        .unwrap_or(default)
}

/// Write a string to `localStorage`. Returns `true` on success.
pub fn set(key: &str, value: &str) -> bool {
    local_storage().is_some_and(|s| s.set_item(key, value).is_ok())
}

/// Remove a key from `localStorage`. Returns `true` on success.
pub fn remove(key: &str) -> bool {
    local_storage().is_some_and(|s| s.remove_item(key).is_ok())
}
