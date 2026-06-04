use std::collections::HashMap;

use hypercolor_types::device::{DeviceId, DeviceInfo};
use tracing::debug;

use super::routing::{DeviceMapping, device_output_len, zone_segments_from_device_info};
use super::{BackendManager, SegmentRange};

impl BackendManager {
    /// Map a spatial layout `device_id` to a `(backend_id, DeviceId)` pair.
    ///
    /// Call this after device discovery to link a zone's device reference
    /// to an actual connected device.
    pub fn map_device(
        &mut self,
        layout_device_id: impl Into<String>,
        backend_id: impl Into<String>,
        device_id: DeviceId,
    ) {
        self.map_device_with_segment(layout_device_id, backend_id, device_id, None);
    }

    /// Map a spatial layout `device_id` with an explicit physical LED range.
    ///
    /// When `segment` is `Some`, zone colors targeting this mapping are
    /// written into that slice of the physical output buffer.
    pub fn map_device_with_segment(
        &mut self,
        layout_device_id: impl Into<String>,
        backend_id: impl Into<String>,
        device_id: DeviceId,
        segment: Option<SegmentRange>,
    ) {
        let layout_id = layout_device_id.into();
        let backend = backend_id.into();
        debug!(
            layout_device_id = %layout_id,
            backend_id = %backend,
            %device_id,
            segment_start = segment.map(|s| s.start),
            segment_length = segment.map(|s| s.length),
            "mapped device"
        );
        self.warnings.clear_unmapped_layout_device(&layout_id);
        self.device_map.insert(
            layout_id,
            DeviceMapping {
                backend_id: backend,
                device_id,
                segment,
                zone_segments: HashMap::new(),
                physical_led_count: None,
            },
        );
        self.invalidate_routing_plan();
    }

    /// Attach hardware zone segment metadata to an existing layout-device mapping.
    ///
    /// This lets spatial zones that share one `device_id` but differ by
    /// `zone_name` target the correct physical LED ranges on multi-zone devices.
    #[must_use]
    pub fn set_device_zone_segments(
        &mut self,
        layout_device_id: &str,
        device_info: &DeviceInfo,
    ) -> bool {
        {
            let Some(mapping) = self.device_map.get_mut(layout_device_id) else {
                return false;
            };

            mapping.zone_segments = zone_segments_from_device_info(device_info);
            mapping.physical_led_count = device_output_len(device_info);
        }
        self.invalidate_routing_plan();
        true
    }

    /// Remove a device mapping.
    pub fn unmap_device(&mut self, layout_device_id: &str) -> bool {
        let Some(mapping) = self.device_map.remove(layout_device_id) else {
            return false;
        };

        let still_used = self.device_map.values().any(|candidate| {
            candidate.backend_id == mapping.backend_id && candidate.device_id == mapping.device_id
        });

        if !still_used {
            let key = (mapping.backend_id, mapping.device_id);
            self.output.remove_target_state(&key);
        }

        self.invalidate_routing_plan();
        true
    }

    /// Remove all mappings for a physical target and drop its queue.
    pub fn remove_device_mappings_for_physical(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> usize {
        let before = self.device_map.len();
        self.device_map.retain(|_, mapping| {
            !(mapping.backend_id == backend_id && mapping.device_id == device_id)
        });
        let removed = before.saturating_sub(self.device_map.len());

        if !self
            .device_map
            .values()
            .any(|mapping| mapping.backend_id == backend_id && mapping.device_id == device_id)
        {
            let key = (backend_id.to_owned(), device_id);
            self.output.remove_target_state(&key);
        }

        if removed > 0 {
            self.invalidate_routing_plan();
        }
        removed
    }

    /// Remove layout mappings for a connected physical target while keeping
    /// its output queue, frame sink, FPS cache, and direct-control state.
    pub fn clear_device_mappings_for_physical(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> usize {
        let before = self.device_map.len();
        self.device_map.retain(|_, mapping| {
            !(mapping.backend_id == backend_id && mapping.device_id == device_id)
        });
        let removed = before.saturating_sub(self.device_map.len());

        if removed > 0 {
            self.invalidate_routing_plan();
        }
        removed
    }

    /// Number of mapped devices.
    #[must_use]
    pub fn mapped_device_count(&self) -> usize {
        self.device_map.len()
    }
}
