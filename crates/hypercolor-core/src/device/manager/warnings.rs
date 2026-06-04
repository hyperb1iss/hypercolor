use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::BackendManager;

#[derive(Default)]
pub(super) struct DeviceOutputWarnings {
    warned_unmapped_layout_devices: HashSet<String>,
    unmapped_layout_warnings_enabled: bool,
    unmapped_layout_warning_count: u64,
    last_segment_mismatch_warn_at: HashMap<String, Instant>,
}

impl DeviceOutputWarnings {
    pub(super) fn retain_active_layout_devices(
        &mut self,
        active_layout_device_ids: &HashSet<String>,
    ) {
        self.warned_unmapped_layout_devices
            .retain(|layout_device_id| active_layout_device_ids.contains(layout_device_id));
    }

    pub(super) fn clear_unmapped_layout_device(&mut self, layout_device_id: &str) {
        self.warned_unmapped_layout_devices.remove(layout_device_id);
    }

    pub(super) fn mark_unmapped_layout_device(&mut self, layout_device_id: String) -> bool {
        if !self.unmapped_layout_warnings_enabled {
            return false;
        }
        if !self.warned_unmapped_layout_devices.insert(layout_device_id) {
            return false;
        }
        self.unmapped_layout_warning_count = self.unmapped_layout_warning_count.saturating_add(1);
        true
    }

    pub(super) fn should_warn_segment_mismatch(
        &mut self,
        warn_key: String,
        interval: Duration,
    ) -> bool {
        if self
            .last_segment_mismatch_warn_at
            .get(&warn_key)
            .is_some_and(|last_warn_at| last_warn_at.elapsed() < interval)
        {
            return false;
        }

        self.last_segment_mismatch_warn_at
            .insert(warn_key, Instant::now());
        true
    }

    pub(super) const fn unmapped_layout_warning_count(&self) -> u64 {
        self.unmapped_layout_warning_count
    }

    pub(super) fn enable_unmapped_layout_warnings(&mut self) {
        self.unmapped_layout_warnings_enabled = true;
    }
}

impl BackendManager {
    #[doc(hidden)]
    #[must_use]
    pub const fn unmapped_layout_warning_count(&self) -> u64 {
        self.warnings.unmapped_layout_warning_count()
    }

    /// Enable warnings for layout targets that still lack a connected device mapping.
    pub fn enable_unmapped_layout_warnings(&mut self) {
        self.warnings.enable_unmapped_layout_warnings();
    }
}
