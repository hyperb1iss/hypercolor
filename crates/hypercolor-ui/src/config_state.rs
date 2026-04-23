use hypercolor_types::config::HypercolorConfig;
use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct ConfigContext {
    pub config: ReadSignal<Option<HypercolorConfig>>,
    pub set_config: WriteSignal<Option<HypercolorConfig>>,
    pub refresh: Callback<()>,
    pub audio_enabled: Memo<bool>,
}

pub fn apply_config_key(config: &mut HypercolorConfig, key: &str, value: &serde_json::Value) {
    let Ok(mut root) = serde_json::to_value(&*config) else {
        return;
    };

    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut cursor = &mut root;
    for &part in parents {
        let Some(obj) = cursor.as_object_mut() else {
            return;
        };
        cursor = obj
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    if let Some(obj) = cursor.as_object_mut() {
        obj.insert(leaf[0].to_owned(), value.clone());
    }

    if let Ok(updated) = serde_json::from_value(root) {
        *config = updated;
    }
}
