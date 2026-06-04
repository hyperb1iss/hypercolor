use std::collections::HashMap;

use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;
use tracing::{debug, trace, warn};

use super::output_color::{
    apply_zone_brightness, prepare_output_for_led_ranges, remap_zone_colors, write_segment_colors,
};
use super::routing::{PlannedZoneRoute, attachment_segment_for_zone};
use super::{BackendManager, FrameWriteStats, UNMAPPED_LAYOUT_WARN_INTERVAL};

impl BackendManager {
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
        self.output.begin_staging_frame();
        let plan = self.routing_plan(layout);
        self.warnings
            .retain_active_layout_devices(&plan.active_layout_device_ids);

        let mut stats = FrameWriteStats::default();

        let newly_inactive = self.output.newly_inactive_devices(&plan.inactive_devices);

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
        self.output.replace_inactive_devices(&plan.inactive_devices);

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

        let (active_staging_keys, active_staging_len) = self.output.take_active_staging_keys();

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
                    .output
                    .staging_mut(key)
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

            let backend = self.backends.get(backend_id.as_str()).cloned();
            let led_count = values.len();
            if !self.output.push_staged_frame(key, backend, values) {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            }

            stats.devices_written += 1;
            stats.total_leds += led_count;
        }

        self.output.restore_active_staging_keys(active_staging_keys);

        stats
    }

    fn route_zone_colors(&mut self, zone_id: &str, colors: &[[u8; 3]], route: &PlannedZoneRoute) {
        let PlannedZoneRoute::Mapped(route) = route else {
            let PlannedZoneRoute::Unmapped { layout_device_id } = route else {
                unreachable!("only mapped or unmapped zone routes are compiled");
            };
            if self
                .warnings
                .mark_unmapped_layout_device(layout_device_id.clone())
            {
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %layout_device_id,
                    "zone skipped because the target layout device is not mapped to a connected backend device"
                );
            }
            return;
        };

        self.warnings
            .clear_unmapped_layout_device(route.layout_device_id.as_str());

        let segment = attachment_segment_for_zone(
            zone_id,
            route.segment,
            route.attachment.as_ref(),
            colors.len(),
        );
        let mismatch = {
            let staging = self.output.staging_buffer(&route.target_key);
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
            if self
                .warnings
                .should_warn_segment_mismatch(warn_key, UNMAPPED_LAYOUT_WARN_INTERVAL)
            {
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %route.layout_device_id,
                    segment_start,
                    expected,
                    received,
                    "zone color count does not match mapped segment length"
                );
            }
        }
    }
}
