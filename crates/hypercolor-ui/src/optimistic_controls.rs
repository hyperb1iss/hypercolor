use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use leptos::prelude::*;

use crate::control_value_json::json_to_control_value;

pub type ControlValueMap = HashMap<String, ControlValue>;

#[derive(Clone, Copy)]
pub(crate) struct OptimisticControlSession {
    pending: StoredValue<ControlValueMap>,
}

impl OptimisticControlSession {
    pub(crate) fn new() -> Self {
        Self {
            pending: StoredValue::new(HashMap::new()),
        }
    }

    pub(crate) fn admit_raw_update_to(
        self,
        set_values: WriteSignal<ControlValueMap>,
        controls: &[ControlDefinition],
        name: &str,
        value: &serde_json::Value,
    ) {
        let Some(value) = json_to_control_value(name, controls, value) else {
            return;
        };
        set_values.update(|values| {
            values.insert(name.to_owned(), value.clone());
        });
        self.pending.update_value(|pending| {
            pending.insert(name.to_owned(), value);
        });
    }

    pub(crate) fn admit_raw_updates_to(
        self,
        set_values: WriteSignal<ControlValueMap>,
        controls: &[ControlDefinition],
        updates: &[(String, serde_json::Value)],
    ) {
        let admitted = normalize_raw_control_updates(controls, updates);
        set_values.update(|values| merge_control_values(values, &admitted));
        self.pending.update_value(|pending| {
            merge_control_values(pending, &admitted);
        });
    }

    pub(crate) fn take_pending(self) -> ControlValueMap {
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

pub fn apply_raw_control_updates(
    values: &mut ControlValueMap,
    controls: &[ControlDefinition],
    updates: &[(String, serde_json::Value)],
) {
    for (name, value) in updates {
        apply_raw_control_update(values, controls, name, value);
    }
}

/// Normalize raw edits once at the schema boundary for optimistic state and
/// daemon delivery. Rejected values never enter either destination.
#[must_use]
pub fn normalize_raw_control_updates(
    controls: &[ControlDefinition],
    updates: &[(String, serde_json::Value)],
) -> ControlValueMap {
    updates
        .iter()
        .filter_map(|(name, value)| {
            json_to_control_value(name, controls, value).map(|value| (name.clone(), value))
        })
        .collect()
}

pub fn merge_control_values(values: &mut ControlValueMap, next_values: &ControlValueMap) {
    for (name, value) in next_values {
        values.insert(name.clone(), value.clone());
    }
}
