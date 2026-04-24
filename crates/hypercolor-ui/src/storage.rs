//! Thin helpers over browser localStorage.

use hypercolor_leptos_ext::prelude;

/// Read a string from `localStorage`.
pub fn get(key: &str) -> Option<String> {
    prelude::storage_get(key)
}

/// Read and parse a typed value from `localStorage`.
pub fn get_parsed<T: std::str::FromStr>(key: &str) -> Option<T> {
    prelude::storage_get_parsed(key)
}

/// Read a typed value, clamped to `[min, max]`, with a fallback default.
pub fn get_clamped(key: &str, default: f64, min: f64, max: f64) -> f64 {
    prelude::storage_get_clamped(key, default, min, max)
}

/// Write a string to `localStorage`. Returns `true` on success.
pub fn set(key: &str, value: &str) -> bool {
    prelude::storage_set(key, value)
}

pub fn try_set(key: &str, value: &str) -> Result<(), wasm_bindgen::JsValue> {
    prelude::storage_try_set(key, value)
}

/// Remove a key from `localStorage`. Returns `true` on success.
pub fn remove(key: &str) -> bool {
    prelude::storage_remove(key)
}
