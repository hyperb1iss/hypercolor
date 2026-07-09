use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use hypercolor_types::device::DeviceId;

use crate::device::output_queue::{DeviceStagingBuffer, OutputLane, OutputQueue};
use crate::device::traits::OutputCadence;

use super::{BackendDeviceKey, BackendHandle, DeviceFrameSinkHandle};

#[derive(Debug, Default)]
struct DirectControlRegistry {
    counts: StdMutex<HashMap<BackendDeviceKey, usize>>,
}

impl DirectControlRegistry {
    fn acquire(self: &Arc<Self>, key: BackendDeviceKey) -> DirectControlGuard {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let count = counts.entry(key.clone()).or_insert(0);
        *count = count.saturating_add(1);
        drop(counts);
        DirectControlGuard {
            registry: Arc::clone(self),
            key,
        }
    }

    fn release(&self, key: &BackendDeviceKey) {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(count) = counts.get_mut(key) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(key);
        }
    }

    fn is_active(&self, key: &BackendDeviceKey) -> bool {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .is_some_and(|count| *count > 0)
    }
}

/// Owned lease that suppresses queued writes while direct device control is active.
#[must_use = "dropping the guard releases direct-control suppression"]
pub struct DirectControlGuard {
    registry: Arc<DirectControlRegistry>,
    key: BackendDeviceKey,
}

impl Drop for DirectControlGuard {
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

#[derive(Default)]
pub(super) struct DeviceOutputCoordinator {
    queues: HashMap<BackendDeviceKey, OutputQueue>,
    frame_sinks: HashMap<BackendDeviceKey, DeviceFrameSinkHandle>,
    staging: HashMap<BackendDeviceKey, DeviceStagingBuffer>,
    active_staging_keys: Vec<BackendDeviceKey>,
    active_staging_len: usize,
    staging_generation: u64,
    output_cadence: HashMap<BackendDeviceKey, OutputCadence>,
    direct_control: Arc<DirectControlRegistry>,
    warned_inactive_layout_devices: HashSet<BackendDeviceKey>,
}

impl DeviceOutputCoordinator {
    pub(super) fn remove_backend_state(&mut self, backend_id: &str) {
        self.queues
            .retain(|(queued_backend_id, _), _| queued_backend_id != backend_id);
        self.frame_sinks
            .retain(|(sink_backend_id, _), _| sink_backend_id != backend_id);
        self.staging
            .retain(|(staged_backend_id, _), _| staged_backend_id != backend_id);
        self.output_cadence
            .retain(|(cached_backend_id, _), _| cached_backend_id != backend_id);
        self.warned_inactive_layout_devices
            .retain(|(warn_backend_id, _)| warn_backend_id != backend_id);
    }

    pub(super) fn remove_target_state(&mut self, key: &BackendDeviceKey) {
        self.queues.remove(key);
        self.frame_sinks.remove(key);
        self.staging.remove(key);
        self.output_cadence.remove(key);
        self.warned_inactive_layout_devices.remove(key);
    }

    pub(super) fn set_frame_sink(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        frame_sink: Option<DeviceFrameSinkHandle>,
    ) {
        let key = (backend_id.to_owned(), device_id);
        self.queues.remove(&key);
        if let Some(frame_sink) = frame_sink {
            self.frame_sinks.insert(key, frame_sink);
        } else {
            self.frame_sinks.remove(&key);
        }
    }

    pub(super) fn begin_direct_control(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> DirectControlGuard {
        let key = (backend_id.to_owned(), device_id);
        self.direct_control.acquire(key)
    }

    pub(super) fn is_direct_control_active_key(&self, key: &BackendDeviceKey) -> bool {
        self.direct_control.is_active(key)
    }

    pub(super) fn set_target_fps(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        target_fps: u32,
    ) {
        self.set_output_cadence(backend_id, device_id, OutputCadence::from_fps(target_fps));
    }

    pub(super) fn set_output_cadence(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        output_cadence: OutputCadence,
    ) {
        self.output_cadence
            .insert((backend_id.to_owned(), device_id), output_cadence);
    }

    pub(super) fn target_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.output_cadence
            .get(&(backend_id.to_owned(), device_id))
            .map(|cadence| cadence.target_fps())
    }

    pub(super) fn output_cadence(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Option<OutputCadence> {
        self.output_cadence
            .get(&(backend_id.to_owned(), device_id))
            .copied()
    }

    pub(super) fn begin_staging_frame(&mut self) {
        self.staging_generation = self.staging_generation.saturating_add(1);
        self.active_staging_len = 0;
    }

    pub(super) fn staging_buffer(&mut self, key: &BackendDeviceKey) -> &mut DeviceStagingBuffer {
        let generation = self.staging_generation;
        let mut became_active = false;

        if let Some(staging) = self.staging.get_mut(key) {
            if staging.frame_generation != generation {
                staging.output.clear();
                staging.required_len = 0;
                staging.written_ranges.clear();
                staging.has_segmented_write = false;
                staging.frame_generation = generation;
                became_active = true;
            }
        } else {
            let staging = self.staging.entry(key.clone()).or_default();
            staging.output.clear();
            staging.required_len = 0;
            staging.written_ranges.clear();
            staging.has_segmented_write = false;
            staging.frame_generation = generation;
            became_active = true;
        }

        if became_active {
            if self.active_staging_len < self.active_staging_keys.len() {
                self.active_staging_keys[self.active_staging_len].clone_from(key);
            } else {
                self.active_staging_keys.push(key.clone());
            }
            self.active_staging_len += 1;
        }

        self.staging
            .get_mut(key)
            .expect("staging buffer must exist after entry initialization")
    }

    pub(super) fn take_active_staging_keys(&mut self) -> (Vec<BackendDeviceKey>, usize) {
        let active_staging_len = self.active_staging_len;
        let active_staging_keys = std::mem::take(&mut self.active_staging_keys);
        self.active_staging_len = 0;
        (active_staging_keys, active_staging_len)
    }

    pub(super) fn restore_active_staging_keys(
        &mut self,
        active_staging_keys: Vec<BackendDeviceKey>,
    ) {
        self.active_staging_keys = active_staging_keys;
    }

    pub(super) fn staging_mut(
        &mut self,
        key: &BackendDeviceKey,
    ) -> Option<&mut DeviceStagingBuffer> {
        self.staging.get_mut(key)
    }

    pub(super) fn newly_inactive_devices(
        &self,
        inactive_devices: &[BackendDeviceKey],
    ) -> Vec<BackendDeviceKey> {
        inactive_devices
            .iter()
            .filter(|key| !self.warned_inactive_layout_devices.contains(*key))
            .cloned()
            .collect()
    }

    pub(super) fn replace_inactive_devices(&mut self, inactive_devices: &[BackendDeviceKey]) {
        self.warned_inactive_layout_devices.clear();
        self.warned_inactive_layout_devices
            .extend(inactive_devices.iter().cloned());
    }

    pub(super) fn has_queue(&self, key: &BackendDeviceKey) -> bool {
        self.queues
            .get(key)
            .is_some_and(|queue| !queue.worker_finished())
    }

    pub(super) fn queue_mut(&mut self, key: &BackendDeviceKey) -> Option<&mut OutputQueue> {
        self.queues.get_mut(key)
    }

    pub(super) fn ensure_queue_for_key(
        &mut self,
        key: &BackendDeviceKey,
        backend: Option<BackendHandle>,
        values: Vec<[u8; 3]>,
    ) -> bool {
        let frame_sink = self.frame_sinks.get(key).cloned();
        let lane_changed = self
            .queues
            .get(key)
            .is_some_and(|queue| queue.uses_frame_sink() != frame_sink.is_some());
        let worker_finished = self
            .queues
            .get(key)
            .is_some_and(OutputQueue::worker_finished);

        if worker_finished {
            let recycled = self
                .queues
                .get_mut(key)
                .expect("finished output queue should still exist")
                .push(values);
            let cadence = self.output_cadence.get(key).copied().unwrap_or_default();
            let lane = if let Some(frame_sink) = frame_sink {
                OutputLane::frame_sink(frame_sink)
            } else {
                let Some(backend) = backend else {
                    if let (Some(staging), Some(recycled)) = (self.staging.get_mut(key), recycled) {
                        staging.output = recycled;
                    }
                    return false;
                };
                OutputLane::backend(backend, key.1)
            };
            let previous = self
                .queues
                .remove(key)
                .expect("finished output queue should still exist");
            let queue = previous.recover(key.0.clone(), key.1, lane, cadence);
            self.queues.insert(key.clone(), queue);
            if let (Some(staging), Some(recycled)) = (self.staging.get_mut(key), recycled) {
                staging.output = recycled;
            }
            return true;
        }

        if !lane_changed && let Some(queue) = self.queues.get_mut(key) {
            let recycled = queue.push(values);
            if let (Some(staging), Some(recycled)) = (self.staging.get_mut(key), recycled) {
                staging.output = recycled;
            }
            return true;
        }

        let cadence = self.output_cadence.get(key).copied().unwrap_or_default();
        let lane = if let Some(frame_sink) = frame_sink {
            OutputLane::frame_sink(frame_sink)
        } else {
            let Some(backend) = backend else {
                if let Some(staging) = self.staging.get_mut(key) {
                    staging.output = values;
                }
                return false;
            };
            OutputLane::backend(backend, key.1)
        };
        self.queues.remove(key);
        let mut queue = OutputQueue::spawn(key.0.clone(), key.1, lane, cadence);
        let recycled = queue.push(values);
        self.queues.insert(key.clone(), queue);

        if let (Some(staging), Some(recycled)) = (self.staging.get_mut(key), recycled) {
            staging.output = recycled;
        }
        true
    }

    pub(super) fn push_staged_frame(
        &mut self,
        key: &BackendDeviceKey,
        backend: Option<BackendHandle>,
        values: Vec<[u8; 3]>,
    ) -> bool {
        self.ensure_queue_for_key(key, backend, values)
    }

    pub(super) fn queues(&self) -> impl Iterator<Item = (&BackendDeviceKey, &OutputQueue)> + '_ {
        self.queues.iter()
    }

    pub(super) fn queue_keys(&self) -> impl Iterator<Item = &BackendDeviceKey> + '_ {
        self.queues.keys()
    }

    pub(super) fn queue_count(&self) -> usize {
        self.queues.len()
    }
}
