use std::collections::HashMap;

use super::{
    AsyncWriteFailure, BackendDeviceKey, BackendManager, BackendManagerDebugSnapshot,
    DeviceOutputStatistics,
};

impl BackendManager {
    /// Snapshot async write failures currently retained by output queues.
    #[must_use]
    pub fn async_write_failures(&self) -> Vec<AsyncWriteFailure> {
        let mut failures = self
            .output
            .queues()
            .filter_map(|((backend_id, device_id), queue)| {
                queue.async_write_failure(backend_id.clone(), *device_id)
            })
            .collect::<Vec<_>>();
        failures.extend(self.output.display_delivery_authority().pending_failures());

        failures.sort_by(|left, right| {
            failure_priority(&left.error)
                .cmp(&failure_priority(&right.error))
                .then(left.backend_id.cmp(&right.backend_id))
                .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
                .then_with(|| {
                    right
                        .is_from_retired_generation()
                        .cmp(&left.is_from_retired_generation())
                })
                .then(
                    left.delivery_id
                        .queue_generation
                        .cmp(&right.delivery_id.queue_generation),
                )
                .then(left.delivery_id.sequence.cmp(&right.delivery_id.sequence))
        });
        failures
    }

    /// Build a typed per-device output telemetry snapshot for collector tasks.
    #[must_use]
    pub fn device_output_statistics(&self) -> Vec<DeviceOutputStatistics> {
        let mut layout_ids_by_key: HashMap<BackendDeviceKey, Vec<String>> = HashMap::new();
        for (layout_device_id, mapping) in &self.device_map {
            layout_ids_by_key
                .entry((mapping.backend_id.clone(), mapping.device_id))
                .or_default()
                .push(layout_device_id.clone());
        }

        for ids in layout_ids_by_key.values_mut() {
            ids.sort_unstable();
        }

        let mut queues = Vec::with_capacity(self.output.queue_count());
        let mut queue_index_by_key = HashMap::new();
        for ((backend_id, device_id), queue) in self.output.queues() {
            let mapped_layout_ids = layout_ids_by_key
                .get(&(backend_id.clone(), *device_id))
                .cloned()
                .unwrap_or_default();
            queue_index_by_key.insert((backend_id.clone(), *device_id), queues.len());
            queues.push(queue.statistics(backend_id, *device_id, mapped_layout_ids));
        }

        for ((backend_id, device_id), lane) in self.output.display_lanes() {
            let display = lane.statistics();
            if let Some(index) = queue_index_by_key.get(&(backend_id.clone(), *device_id)) {
                queues[*index].record_display_statistics(
                    display.queue_generation,
                    display.transport_started,
                    display.transport_completed,
                    display.transport_failed,
                );
                continue;
            }

            queues.push(DeviceOutputStatistics::display_only(
                backend_id.clone(),
                *device_id,
                layout_ids_by_key
                    .get(&(backend_id.clone(), *device_id))
                    .cloned()
                    .unwrap_or_default(),
                display.queue_generation,
                display.transport_started,
                display.transport_completed,
                display.transport_failed,
            ));
        }

        queues.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
        });
        queues
    }

    /// Build a debug snapshot of queue and routing internals.
    #[must_use]
    pub fn debug_snapshot(&self) -> BackendManagerDebugSnapshot {
        let queues = self
            .device_output_statistics()
            .into_iter()
            .map(DeviceOutputStatistics::into_debug_snapshot)
            .collect::<Vec<_>>();

        BackendManagerDebugSnapshot {
            queue_count: queues.len(),
            mapped_device_count: self.device_map.len(),
            queues,
        }
    }
}

const fn failure_priority(error: &hypercolor_types::device::DeviceError) -> u8 {
    use hypercolor_types::device::ErrorRecoverability;

    match error.recoverability() {
        ErrorRecoverability::Permanent => 0,
        ErrorRecoverability::Reconnect => 1,
        ErrorRecoverability::Retry => 2,
    }
}
