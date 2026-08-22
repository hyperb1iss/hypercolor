use hypercolor_types::config_registry::{self, ConfigKeyDescriptor, KeyPattern, Redaction};

fn redacted_marker() -> serde_json::Value {
    serde_json::json!({ "redacted": true })
}

pub(super) fn redact_document(mut document: serde_json::Value) -> serde_json::Value {
    let Some(root) = document.as_object_mut() else {
        return document;
    };

    for (key, value) in root.iter_mut() {
        let descriptor = config_registry::descriptor_for(key);
        if matches!(descriptor.redaction, Redaction::Secret) {
            *value = mask_section(descriptor, std::mem::take(value));
        }
    }
    document
}

pub(super) fn redact_key(key: &str, value: serde_json::Value) -> serde_json::Value {
    let descriptor = config_registry::descriptor_for(key);
    if !matches!(descriptor.redaction, Redaction::Secret) {
        return value;
    }

    if key == descriptor.pattern.root() {
        mask_section(descriptor, value)
    } else {
        redacted_marker()
    }
}

fn mask_section(descriptor: &ConfigKeyDescriptor, value: serde_json::Value) -> serde_json::Value {
    if let KeyPattern::Namespace(_) = descriptor.pattern
        && let Some(entries) = value.as_object()
    {
        return serde_json::Value::Object(
            entries
                .keys()
                .map(|entry| (entry.clone(), redacted_marker()))
                .collect(),
        );
    }
    redacted_marker()
}
