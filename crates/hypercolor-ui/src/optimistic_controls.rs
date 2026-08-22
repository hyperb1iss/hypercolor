use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use leptos::prelude::*;

use crate::control_value_json::json_to_control_value;

pub type ControlValueMap = HashMap<String, ControlValue>;

#[derive(Clone, Copy)]
pub(crate) struct OptimisticControlSession {
    queue: StoredValue<ControlMutationQueue>,
}

#[derive(Default)]
struct ControlMutationQueue {
    pending: ControlValueMap,
    in_flight: bool,
}

impl ControlMutationQueue {
    fn insert(&mut self, name: String, value: ControlValue) {
        self.pending.insert(name, value);
    }

    fn merge(&mut self, values: &ControlValueMap) {
        merge_control_values(&mut self.pending, values);
    }

    fn start_flush(&mut self) -> Option<ControlValueMap> {
        if self.in_flight || self.pending.is_empty() {
            return None;
        }
        self.in_flight = true;
        Some(std::mem::take(&mut self.pending))
    }

    fn complete_flush(&mut self) -> Option<ControlValueMap> {
        if self.pending.is_empty() {
            self.in_flight = false;
            None
        } else {
            Some(std::mem::take(&mut self.pending))
        }
    }

    fn take_pending_for_retry(&mut self) -> ControlValueMap {
        std::mem::take(&mut self.pending)
    }

    fn fail_flush(&mut self) {
        self.pending.clear();
        self.in_flight = false;
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
    }
}

impl OptimisticControlSession {
    pub(crate) fn new() -> Self {
        Self {
            queue: StoredValue::new(ControlMutationQueue::default()),
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
        self.queue.update_value(|queue| {
            queue.insert(name.to_owned(), value);
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
        self.queue.update_value(|queue| {
            queue.merge(&admitted);
        });
    }

    pub(crate) fn start_flush(self) -> Option<ControlValueMap> {
        self.queue
            .try_update_value(ControlMutationQueue::start_flush)
            .flatten()
    }

    pub(crate) fn complete_flush(self) -> Option<ControlValueMap> {
        self.queue
            .try_update_value(ControlMutationQueue::complete_flush)
            .flatten()
    }

    pub(crate) fn take_pending_for_retry(self) -> ControlValueMap {
        self.queue
            .try_update_value(ControlMutationQueue::take_pending_for_retry)
            .unwrap_or_default()
    }

    pub(crate) fn fail_flush(self) {
        let _ = self
            .queue
            .try_update_value(ControlMutationQueue::fail_flush);
    }

    pub(crate) fn clear_pending(self) {
        let _ = self
            .queue
            .try_update_value(ControlMutationQueue::clear_pending);
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

#[cfg(test)]
mod tests {
    use hypercolor_types::control::ControlValue;

    use super::ControlMutationQueue;

    #[test]
    fn in_flight_flush_drains_distinct_newer_keys_in_a_second_batch() {
        let mut queue = ControlMutationQueue::default();
        queue.insert("speed".to_owned(), ControlValue::Float(0.5));

        let first = queue.start_flush().expect("first batch should start");
        queue.insert("hue".to_owned(), ControlValue::Int(120));
        assert!(queue.start_flush().is_none(), "only one batch may run");

        let second = queue
            .complete_flush()
            .expect("edits queued in flight must drain next");
        assert_eq!(first.get("speed"), Some(&ControlValue::Float(0.5)));
        assert_eq!(second.get("hue"), Some(&ControlValue::Int(120)));
        assert!(queue.complete_flush().is_none());
    }

    #[test]
    fn in_flight_edits_still_coalesce_last_write_per_key() {
        let mut queue = ControlMutationQueue::default();
        queue.insert("speed".to_owned(), ControlValue::Float(0.25));
        let _ = queue.start_flush().expect("first batch should start");

        queue.insert("speed".to_owned(), ControlValue::Float(0.5));
        queue.insert("speed".to_owned(), ControlValue::Float(0.75));

        let second = queue.complete_flush().expect("newest edit should drain");
        assert_eq!(second.len(), 1);
        assert_eq!(second.get("speed"), Some(&ControlValue::Float(0.75)));
    }

    #[test]
    fn failed_flush_discards_every_unconfirmed_optimistic_batch() {
        let mut queue = ControlMutationQueue::default();
        queue.insert("speed".to_owned(), ControlValue::Float(0.25));
        let _ = queue.start_flush().expect("first batch should start");
        queue.insert("hue".to_owned(), ControlValue::Int(120));

        queue.fail_flush();

        assert!(queue.start_flush().is_none());
        assert!(queue.pending.is_empty());
    }
}
