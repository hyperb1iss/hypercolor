use std::collections::HashMap;

use hypercolor_types::device::DeviceId;

use super::BackendManager;

#[derive(Default)]
pub(super) struct DeviceOutputBrightness {
    overrides: HashMap<DeviceId, f32>,
    generation: u64,
}

impl DeviceOutputBrightness {
    pub(super) fn set(&mut self, device_id: DeviceId, brightness: f32) {
        let normalized = brightness.clamp(0.0, 1.0);
        let changed = if normalized >= 0.999 {
            self.overrides.remove(&device_id).is_some()
        } else {
            self.overrides
                .insert(device_id, normalized)
                .is_none_or(|previous| previous.to_bits() != normalized.to_bits())
        };
        if changed {
            self.generation = self.generation.saturating_add(1);
        }
    }

    pub(super) fn value(&self, device_id: DeviceId) -> f32 {
        self.overrides.get(&device_id).copied().unwrap_or(1.0)
    }

    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }
}

impl BackendManager {
    /// Configure software output brightness for a physical device.
    pub fn set_device_output_brightness(&mut self, device_id: DeviceId, brightness: f32) {
        self.output_brightness.set(device_id, brightness);
    }

    /// Read the configured software output brightness for a physical device.
    #[must_use]
    pub fn device_output_brightness(&self, device_id: DeviceId) -> f32 {
        self.output_brightness.value(device_id)
    }

    /// Monotonic generation for software output-brightness state changes.
    #[must_use]
    pub const fn output_brightness_generation(&self) -> u64 {
        self.output_brightness.generation()
    }
}
