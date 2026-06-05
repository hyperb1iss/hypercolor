use std::collections::HashMap;

use hypercolor_types::effect::{ControlDefinition, ControlValue};
use leptos::prelude::*;

use crate::control_value_json::json_to_control_value;

pub(crate) type ControlValueMap = HashMap<String, ControlValue>;
pub(crate) type RawControlUpdates = HashMap<String, serde_json::Value>;

#[derive(Clone, Copy)]
pub(crate) struct OptimisticControlSession {
    pending: StoredValue<RawControlUpdates>,
}

impl OptimisticControlSession {
    pub(crate) fn new() -> Self {
        Self {
            pending: StoredValue::new(HashMap::new()),
        }
    }

    pub(crate) fn apply_raw_update_to(
        self,
        set_values: WriteSignal<ControlValueMap>,
        controls: &[ControlDefinition],
        name: &str,
        value: &serde_json::Value,
    ) {
        set_values.update(|values| {
            apply_raw_control_update(values, controls, name, value);
        });
    }

    pub(crate) fn apply_raw_updates_to(
        self,
        set_values: WriteSignal<ControlValueMap>,
        controls: &[ControlDefinition],
        updates: &[(String, serde_json::Value)],
    ) {
        set_values.update(|values| {
            apply_raw_control_updates(values, controls, updates);
        });
    }

    pub(crate) fn apply_values_to(
        self,
        set_values: WriteSignal<ControlValueMap>,
        next_values: &ControlValueMap,
    ) {
        set_values.update(|values| {
            merge_control_values(values, next_values);
        });
    }

    pub(crate) fn queue_raw_update(self, name: String, value: serde_json::Value) {
        self.pending.update_value(|pending| {
            pending.insert(name, value);
        });
    }

    pub(crate) fn queue_raw_updates(self, updates: &[(String, serde_json::Value)]) {
        self.pending.update_value(|pending| {
            for (name, value) in updates {
                pending.insert(name.clone(), value.clone());
            }
        });
    }

    pub(crate) fn take_pending(self) -> RawControlUpdates {
        self.pending
            .try_update_value(std::mem::take)
            .unwrap_or_default()
    }

    pub(crate) fn clear_pending(self) {
        let _ = self.pending.try_update_value(std::mem::take);
    }

    pub(crate) fn has_pending(self) -> bool {
        self.pending.with_value(|pending| !pending.is_empty())
    }
}

impl Default for OptimisticControlSession {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn apply_raw_control_update(
    values: &mut ControlValueMap,
    controls: &[ControlDefinition],
    name: &str,
    value: &serde_json::Value,
) {
    if let Some(control_value) = json_to_control_value(name, controls, value) {
        values.insert(name.to_owned(), control_value);
    }
}

pub(crate) fn apply_raw_control_updates(
    values: &mut ControlValueMap,
    controls: &[ControlDefinition],
    updates: &[(String, serde_json::Value)],
) {
    for (name, value) in updates {
        apply_raw_control_update(values, controls, name, value);
    }
}

pub(crate) fn merge_control_values(values: &mut ControlValueMap, next_values: &ControlValueMap) {
    for (name, value) in next_values {
        values.insert(name.clone(), value.clone());
    }
}

pub(crate) fn raw_control_updates_payload(updates: RawControlUpdates) -> serde_json::Value {
    serde_json::Value::Object(updates.into_iter().collect::<serde_json::Map<_, _>>())
}
