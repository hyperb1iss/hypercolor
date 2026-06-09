//! Thin helpers over browser localStorage.
//!
//! On non-wasm32 targets there is no browser, so a thread-local in-memory map
//! stands in for localStorage. Native integration tests exercise code paths
//! that read or write persisted values (for example channel display-name
//! overrides) through the same API.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static STORE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    }

    pub(super) fn get(key: &str) -> Option<String> {
        STORE.with(|store| store.borrow().get(key).cloned())
    }

    pub(super) fn set(key: &str, value: &str) {
        STORE.with(|store| {
            store.borrow_mut().insert(key.to_owned(), value.to_owned());
        });
    }

    pub(super) fn remove(key: &str) {
        STORE.with(|store| {
            store.borrow_mut().remove(key);
        });
    }
}

#[cfg(target_arch = "wasm32")]
use hypercolor_leptos_ext::prelude;

/// Read a string from `localStorage`.
pub fn get(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        prelude::storage_get(key)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::get(key)
    }
}

/// Read and parse a typed value from `localStorage`.
pub fn get_parsed<T: std::str::FromStr>(key: &str) -> Option<T> {
    get(key)?.parse().ok()
}

/// Read a typed value, clamped to `[min, max]`, with a fallback default.
pub fn get_clamped(key: &str, default: f64, min: f64, max: f64) -> f64 {
    get_parsed::<f64>(key)
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

/// Write a string to `localStorage`. Returns `true` on success.
pub fn set(key: &str, value: &str) -> bool {
    try_set(key, value).is_ok()
}

pub fn try_set(key: &str, value: &str) -> Result<(), wasm_bindgen::JsValue> {
    #[cfg(target_arch = "wasm32")]
    {
        prelude::storage_try_set(key, value)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::set(key, value);
        Ok(())
    }
}

/// Remove a key from `localStorage`. Returns `true` on success.
pub fn remove(key: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        prelude::storage_remove(key)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::remove(key);
        true
    }
}
