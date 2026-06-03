use std::collections::HashMap;
use std::time::Instant;

use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;
use tracing::{debug, trace, warn};

use super::output_color::{
    apply_zone_brightness, prepare_output_for_led_ranges, remap_zone_colors, write_segment_colors,
};
use super::routing::{PlannedZoneRoute, attachment_segment_for_zone};
use super::{
    BackendDeviceKey, BackendManager, DeviceStagingBuffer, FrameWriteStats, OutputQueue,
    UNMAPPED_LAYOUT_WARN_INTERVAL,
};

impl BackendManager {
    fn begin_staging_frame(&mut self) {
        self.staging_generation = self.staging_generation.saturating_add(1);
        self.active_staging_len = 0;
    }

    fn staging_buffer(&mut self, key: &BackendDeviceKey) -> &mut DeviceStagingBuffer {
        let generation = self.staging_generation;
        let mut became_active = false;

        if let Some(staging) = self.device_staging.get_mut(key) {
            if staging.frame_generation != generation {
                staging.output.clear();
                staging.required_len = 0;
                staging.written_ranges.clear();
                staging.has_segmented_write = false;
                staging.frame_generation = generation;
                became_active = true;
            }
        } else {
            let staging = self.device_staging.entry(key.clone()).or_default();
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

        self.device_staging
            .get_mut(key)
            .expect("staging buffer must exist after entry initialization")
    }

    /// Push frame color data to all mapped devices.
    ///
    /// For each zone in `zone_colors`, looks up the target device via the
    /// spatial layout's zone-to-device mapping, groups colors by device,
    /// and enqueues one payload per device. Errors are
    /// collected but do not halt processing — every mapped device gets
    /// its data.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::too_many_lines,
        reason = "frame routing keeps mapping, remap, segmented writes, and queue dispatch together so the hot-path ordering stays readable"
    )]
    pub async fn write_frame(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
    ) -> FrameWriteStats {
        self.write_frame_with_brightness(zone_colors, layout, 1.0, None)
            .await
    }

    /// Push frame color data to all mapped devices with optional per-device
    /// output brightness scalars.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::too_many_lines,
        reason = "frame routing keeps mapping, remap, segmented writes, queue dispatch together so the hot-path ordering stays readable"
    )]
    pub async fn write_frame_with_brightness(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
        global_brightness: f32,
        device_brightness: Option<&HashMap<DeviceId, f32>>,
    ) -> FrameWriteStats {
        self.begin_staging_frame();
        let plan = self.routing_plan(layout);
        self.warned_unmapped_layout_devices
            .retain(|layout_device_id| plan.active_layout_device_ids.contains(layout_device_id));

        let mut stats = FrameWriteStats::default();

        let newly_inactive = plan
            .inactive_devices
            .iter()
            .filter(|key| !self.warned_inactive_layout_devices.contains(*key))
            .cloned()
            .collect::<Vec<_>>();

        if !newly_inactive.is_empty() {
            let devices = newly_inactive
                .iter()
                .take(8)
                .map(|(backend_id, device_id)| format!("{backend_id}:{device_id}"))
                .collect::<Vec<_>>();
            let mapped_layout_ids_by_device = newly_inactive
                .iter()
                .take(8)
                .map(|(backend_id, device_id)| {
                    let aliases = plan
                        .mapped_layout_ids_by_device
                        .get(&(backend_id.clone(), *device_id))
                        .cloned()
                        .unwrap_or_default();
                    format!("{backend_id}:{device_id} => [{}]", aliases.join(", "))
                })
                .collect::<Vec<_>>();
            let inactive_device_count = newly_inactive.len();
            let omitted_device_count = inactive_device_count.saturating_sub(devices.len());
            if layout.zones.is_empty() {
                debug!(
                    inactive_device_count,
                    sample_devices = ?devices,
                    omitted_device_count,
                    layout_zone_count = layout.zones.len(),
                    "connected devices are not in the empty active layout; frames will not be sent"
                );
            } else {
                warn!(
                    inactive_device_count,
                    sample_devices = ?devices,
                    omitted_device_count,
                    layout_zone_count = layout.zones.len(),
                    sample_mapped_layout_ids = ?mapped_layout_ids_by_device,
                    "connected devices have no active layout zones; frames will not be sent"
                );
            }
        }
        self.warned_inactive_layout_devices.clear();
        self.warned_inactive_layout_devices
            .extend(plan.inactive_devices.iter().cloned());

        if zone_colors.len() == plan.ordered_zone_routes.len()
            && zone_colors
                .iter()
                .zip(&plan.ordered_zone_routes)
                .all(|(zone_colors, ordered)| zone_colors.zone_id == ordered.zone_id)
        {
            for (zone_colors, ordered) in zone_colors.iter().zip(&plan.ordered_zone_routes) {
                self.route_zone_colors(
                    zone_colors.zone_id.as_str(),
                    &zone_colors.colors,
                    &ordered.route,
                );
            }
        } else {
            for zc in zone_colors {
                let Some(route) = plan.zone_routes.get(zc.zone_id.as_str()) else {
                    warn!(zone_id = %zc.zone_id, "zone not found in spatial layout");
                    continue;
                };
                self.route_zone_colors(&zc.zone_id, &zc.colors, route);
            }
        }

        let active_staging_len = self.active_staging_len;
        let mut active_staging_keys = Vec::new();
        std::mem::swap(&mut active_staging_keys, &mut self.active_staging_keys);
        self.active_staging_len = 0;

        for key in active_staging_keys.iter().take(active_staging_len) {
            let (backend_id, device_id) = key;

            if self.is_direct_control_active_key(key) {
                trace!(
                    backend_id = %backend_id,
                    device_id = %device_id,
                    "skipping queued device frame while direct control is active"
                );
                continue;
            }

            if !self.backends.contains_key(backend_id.as_str()) {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            }

            let device_output_brightness = self.device_output_brightness(*device_id);
            let per_frame_brightness = device_brightness
                .and_then(|settings| settings.get(device_id).copied())
                .unwrap_or(1.0);
            let brightness = (global_brightness * per_frame_brightness * device_output_brightness)
                .clamp(0.0, 1.0);

            let values = {
                let staging = self
                    .device_staging
                    .get_mut(key)
                    .expect("active staging key should resolve to a staging buffer");
                if staging.output.len() < staging.required_len {
                    staging.output.resize(staging.required_len, [0, 0, 0]);
                }

                prepare_output_for_led_ranges(
                    &mut staging.output,
                    &staging.written_ranges,
                    brightness,
                );

                let mut values = Vec::new();
                std::mem::swap(&mut values, &mut staging.output);
                values
            };

            let Some(queue) = self.ensure_output_queue_for_key(key) else {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                if let Some(staging) = self.device_staging.get_mut(key) {
                    staging.output = values;
                }
                continue;
            };

            stats.devices_written += 1;
            stats.total_leds += values.len();
            let recycled = queue.push(values);
            if let (Some(staging), Some(recycled)) = (self.device_staging.get_mut(key), recycled) {
                staging.output = recycled;
            }
        }

        self.active_staging_keys = active_staging_keys;

        stats
    }

    fn route_zone_colors(&mut self, zone_id: &str, colors: &[[u8; 3]], route: &PlannedZoneRoute) {
        let PlannedZoneRoute::Mapped(route) = route else {
            let PlannedZoneRoute::Unmapped { layout_device_id } = route else {
                unreachable!("only mapped or unmapped zone routes are compiled");
            };
            if !self.unmapped_layout_warnings_enabled {
                return;
            }
            if self
                .warned_unmapped_layout_devices
                .insert(layout_device_id.clone())
            {
                self.unmapped_layout_warning_count =
                    self.unmapped_layout_warning_count.saturating_add(1);
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %layout_device_id,
                    "zone skipped because the target layout device is not mapped to a connected backend device"
                );
            }
            return;
        };

        self.warned_unmapped_layout_devices
            .remove(route.layout_device_id.as_str());

        let segment = attachment_segment_for_zone(
            zone_id,
            route.segment,
            route.attachment.as_ref(),
            colors.len(),
        );
        let mismatch = {
            let staging = self.staging_buffer(&route.target_key);
            staging.required_len = staging
                .required_len
                .max(route.physical_led_count.unwrap_or_default());
            let remapped_colors = remap_zone_colors(
                zone_id,
                colors,
                route.led_mapping.as_deref(),
                &mut staging.remap_scratch,
            );
            let remapped_len = remapped_colors.len();

            if let Some(segment) = segment {
                staging.has_segmented_write = true;
                let segment_end = segment.end();
                if staging.output.len() < segment_end {
                    staging.output.resize(segment_end, [0, 0, 0]);
                }

                let wrote_segment =
                    write_segment_colors(&mut staging.output, segment, remapped_colors);
                if wrote_segment {
                    let start = segment.start;
                    let end = segment.end();
                    apply_zone_brightness(&mut staging.output[start..end], route.zone_brightness);
                    staging.mark_written_range(start, end);
                }

                (!wrote_segment && segment.length > 0).then_some((
                    segment.start,
                    segment.length,
                    remapped_len,
                ))
            } else {
                if staging.has_segmented_write {
                    warn!(
                        zone_id = %zone_id,
                        "mixed segmented and non-segmented mappings for the same physical device"
                    );
                }
                let start = staging.output.len();
                staging.output.extend_from_slice(remapped_colors);
                let end = staging.output.len();
                apply_zone_brightness(&mut staging.output[start..end], route.zone_brightness);
                staging.mark_written_range(start, end);
                None
            }
        };

        if let Some((segment_start, expected, received)) = mismatch {
            let warn_key = format!("{}:{zone_id}", route.layout_device_id);
            let should_warn = self
                .last_segment_mismatch_warn_at
                .get(&warn_key)
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= UNMAPPED_LAYOUT_WARN_INTERVAL);

            if should_warn {
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %route.layout_device_id,
                    segment_start,
                    expected,
                    received,
                    "zone color count does not match mapped segment length"
                );
                self.last_segment_mismatch_warn_at
                    .insert(warn_key, Instant::now());
            }
        }
    }

    fn ensure_output_queue_for_key(&mut self, key: &BackendDeviceKey) -> Option<&mut OutputQueue> {
        let frame_sink = self.device_frame_sinks.get(key).cloned();
        let should_replace_queue = self
            .output_queues
            .get(key)
            .is_some_and(|queue| queue.uses_frame_sink() != frame_sink.is_some());

        if should_replace_queue {
            self.output_queues.remove(key);
        }

        if !self.output_queues.contains_key(key) {
            let backend = self.backends.get(key.0.as_str())?.clone();
            let target_fps = self.device_fps_cache.get(key).copied().unwrap_or(60);
            let queue = OutputQueue::spawn(key.0.clone(), key.1, backend, frame_sink, target_fps);
            self.output_queues.insert(key.clone(), queue);
        }

        self.output_queues.get_mut(key)
    }
}
