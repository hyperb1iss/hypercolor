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
            .output_queues
            .iter()
            .filter_map(|((backend_id, device_id), queue)| {
                let error = queue.last_error()?;

                Some(AsyncWriteFailure {
                    backend_id: backend_id.clone(),
                    device_id: *device_id,
                    error,
                })
            })
            .collect::<Vec<_>>();

        failures.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
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

        let mut queues = Vec::with_capacity(self.output_queues.len());
        for ((backend_id, device_id), queue) in &self.output_queues {
            let mapped_layout_ids = layout_ids_by_key
                .get(&(backend_id.clone(), *device_id))
                .cloned()
                .unwrap_or_default();
            queues.push(queue.statistics(backend_id, *device_id, mapped_layout_ids));
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
