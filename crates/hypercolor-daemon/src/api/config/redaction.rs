//! Read-surface masking for config documents and single keys.
//!
//! The rule itself lives in [`config_registry::redact_value`], shared
//! with the config manager's change stream, so a credential renders the
//! same way on a GET, a key read, and a `ConfigChanged` event.

use hypercolor_types::config_registry;

pub(super) fn redact_document(mut document: serde_json::Value) -> serde_json::Value {
    let Some(root) = document.as_object_mut() else {
        return document;
    };

    for (key, value) in root.iter_mut() {
        *value = config_registry::redact_value(key, std::mem::take(value));
    }
    document
}

pub(super) fn redact_key(key: &str, value: serde_json::Value) -> serde_json::Value {
    config_registry::redact_value(key, value)
}
