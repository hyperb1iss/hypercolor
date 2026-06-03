use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use hypercolor_types::attachment::zone_name_matches_slot_alias;
use hypercolor_types::device::{DeviceId, DeviceInfo, ZoneInfo};
use hypercolor_types::spatial::{
    LedTopology, NormalizedPosition, Output, OutputComponent, SpatialLayout, StripDirection,
};
use tracing::{debug, warn};

use crate::spatial::is_led_sampled_zone;

use super::{BackendDeviceKey, SegmentRange};

/// Internal mapping from a layout device identifier to a backend + device.
#[derive(Debug, Clone)]
pub(super) struct DeviceMapping {
    pub(super) backend_id: String,
    pub(super) device_id: DeviceId,
    pub(super) segment: Option<SegmentRange>,
    pub(super) zone_segments: HashMap<String, SegmentRange>,
    pub(super) physical_led_count: Option<usize>,
}

#[derive(Debug)]
pub(super) struct RoutingPlan {
    pub(super) layout_signature: u64,
    pub(super) mapping_generation: u64,
    pub(super) active_layout_device_ids: HashSet<String>,
    pub(super) active_target_keys: Vec<BackendDeviceKey>,
    pub(super) zone_routes: HashMap<String, PlannedZoneRoute>,
    pub(super) ordered_zone_routes: Vec<OrderedZoneRoute>,
    pub(super) inactive_devices: Vec<BackendDeviceKey>,
    pub(super) mapped_layout_ids_by_device: HashMap<BackendDeviceKey, Vec<String>>,
}

#[derive(Debug, Clone)]
pub(super) enum PlannedZoneRoute {
    Mapped(CompiledZoneRoute),
    Unmapped { layout_device_id: String },
}

#[derive(Debug, Clone)]
pub(super) struct CompiledZoneRoute {
    pub(super) layout_device_id: String,
    pub(super) target_key: BackendDeviceKey,
    pub(super) led_mapping: Option<Box<[u32]>>,
    pub(super) segment: Option<SegmentRange>,
    pub(super) attachment: Option<OutputComponent>,
    pub(super) physical_led_count: Option<usize>,
    pub(super) zone_brightness: f32,
}

#[derive(Debug, Clone)]
pub(super) struct OrderedZoneRoute {
    pub(super) zone_id: String,
    pub(super) route: PlannedZoneRoute,
}

#[derive(Debug, Default)]
pub(super) struct LayoutOutputCoverage {
    covers_whole_device: bool,
    pub(super) zone_names: HashSet<String>,
}

impl LayoutOutputCoverage {
    pub(super) const fn covers_whole_device(&self) -> bool {
        self.covers_whole_device
    }
}

pub(super) fn layout_output_coverage(
    layout: &SpatialLayout,
) -> HashMap<&str, LayoutOutputCoverage> {
    let mut coverage = HashMap::new();
    for zone in layout.zones.iter().filter(|zone| is_led_sampled_zone(zone)) {
        let entry = coverage
            .entry(zone.device_id.as_str())
            .or_insert_with(LayoutOutputCoverage::default);
        if let Some(zone_name) = zone.zone_name.as_ref() {
            entry.zone_names.insert(zone_name.clone());
        } else {
            entry.covers_whole_device = true;
        }
    }
    coverage
}

pub(super) fn zone_name_covered_by_layout(
    assigned_zone_names: &HashSet<String>,
    zone_name: &str,
) -> bool {
    assigned_zone_names
        .iter()
        .any(|assigned| zone_name_matches_slot_alias(Some(assigned.as_str()), Some(zone_name)))
}

pub(super) fn unassigned_output_zone(
    layout_device_id: &str,
    zone_name: Option<&str>,
    led_count: usize,
) -> Output {
    let led_count = u32::try_from(led_count).unwrap_or(u32::MAX);
    Output {
        id: unassigned_output_zone_id(layout_device_id, zone_name),
        name: zone_name.map_or_else(
            || format!("{layout_device_id} unassigned"),
            |zone_name| format!("{layout_device_id} {zone_name} unassigned"),
        ),
        device_id: layout_device_id.to_owned(),
        zone_name: zone_name.map(str::to_owned),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        display_order: i32::MAX,
        orientation: None,
        topology: LedTopology::Strip {
            count: led_count,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn unassigned_output_zone_id(layout_device_id: &str, zone_name: Option<&str>) -> String {
    zone_name.map_or_else(
        || format!("__unassigned:{layout_device_id}"),
        |zone_name| format!("__unassigned:{layout_device_id}:{zone_name}"),
    )
}

pub(super) fn should_use_ordered_routing(zone: &Output) -> bool {
    is_led_sampled_zone(zone)
}

pub(super) fn normalized_led_mapping(led_mapping: Option<&[u32]>) -> Option<Box<[u32]>> {
    let led_mapping = led_mapping?;

    if led_mapping
        .iter()
        .enumerate()
        .all(|(index, &physical_index)| u32::try_from(index).ok() == Some(physical_index))
    {
        return None;
    }

    Some(led_mapping.to_vec().into_boxed_slice())
}

pub(super) fn zone_segments_from_device_info(
    device_info: &DeviceInfo,
) -> HashMap<String, SegmentRange> {
    let mut next_start = 0_usize;
    let mut segments = HashMap::with_capacity(device_info.zones.len());

    for zone in &device_info.zones {
        let Some(segment) = next_zone_segment(zone, next_start) else {
            continue;
        };
        next_start = segment.end();
        segments.insert(zone.name.clone(), segment);
    }

    segments
}

pub(super) fn device_output_len(device_info: &DeviceInfo) -> Option<usize> {
    let total_leds = device_info
        .total_led_count()
        .max(device_info.capabilities.led_count);
    let Ok(total_leds) = usize::try_from(total_leds) else {
        warn!(
            device = %device_info.name,
            device_led_count = total_leds,
            "ignoring device output length because led_count does not fit in usize"
        );
        return None;
    };

    Some(total_leds)
}

fn next_zone_segment(zone: &ZoneInfo, start: usize) -> Option<SegmentRange> {
    let Ok(length) = usize::try_from(zone.led_count) else {
        warn!(
            zone_name = %zone.name,
            zone_led_count = zone.led_count,
            "ignoring device zone segment because led_count does not fit in usize"
        );
        return None;
    };

    Some(SegmentRange::new(start, length))
}

pub(super) fn mapped_segment_for_zone_name(
    zone_id: &str,
    zone_name: Option<&str>,
    mapping: &DeviceMapping,
) -> Option<SegmentRange> {
    let Some(zone_name) = zone_name else {
        return mapping.segment;
    };
    let Some(zone_segment) = zone_segment_for_name(&mapping.zone_segments, zone_name) else {
        return mapping.segment;
    };

    let Some(base_segment) = mapping.segment else {
        return Some(zone_segment);
    };

    if zone_segment.start >= base_segment.start && zone_segment.end() <= base_segment.end() {
        return Some(zone_segment);
    }

    if base_segment.start >= zone_segment.start && base_segment.end() <= zone_segment.end() {
        return Some(base_segment);
    }

    let overlap_start = base_segment.start.max(zone_segment.start);
    let overlap_end = base_segment.end().min(zone_segment.end());
    if overlap_start < overlap_end {
        warn!(
            zone_id = %zone_id,
            zone_name = %zone_name,
            base_segment_start = base_segment.start,
            base_segment_length = base_segment.length,
            zone_segment_start = zone_segment.start,
            zone_segment_length = zone_segment.length,
            "using the overlap between the logical device segment and the hardware zone segment"
        );
        return Some(SegmentRange::new(
            overlap_start,
            overlap_end.saturating_sub(overlap_start),
        ));
    }

    warn!(
        zone_id = %zone_id,
        zone_name = %zone_name,
        base_segment_start = base_segment.start,
        base_segment_length = base_segment.length,
        zone_segment_start = zone_segment.start,
        zone_segment_length = zone_segment.length,
        "ignoring hardware zone segment because it does not overlap the mapped logical segment"
    );
    Some(base_segment)
}

fn zone_segment_for_name(
    zone_segments: &HashMap<String, SegmentRange>,
    zone_name: &str,
) -> Option<SegmentRange> {
    zone_segments.get(zone_name).copied().or_else(|| {
        zone_segments.iter().find_map(|(candidate, segment)| {
            zone_name_matches_slot_alias(Some(zone_name), Some(candidate)).then_some(*segment)
        })
    })
}

pub(super) fn layout_routing_signature(layout: &SpatialLayout) -> u64 {
    let mut hasher = DefaultHasher::new();
    layout.id.hash(&mut hasher);
    layout.zones.len().hash(&mut hasher);

    for zone in &layout.zones {
        zone.id.hash(&mut hasher);
        zone.device_id.hash(&mut hasher);
        zone.zone_name.hash(&mut hasher);
        zone.led_mapping.hash(&mut hasher);
        normalized_zone_brightness(zone.brightness)
            .to_bits()
            .hash(&mut hasher);
        hash_attachment(zone.attachment.as_ref(), &mut hasher);
    }

    hasher.finish()
}

pub(super) fn normalized_zone_brightness(brightness: Option<f32>) -> f32 {
    brightness.unwrap_or(1.0).clamp(0.0, 1.0)
}

fn hash_attachment(attachment: Option<&OutputComponent>, hasher: &mut DefaultHasher) {
    let Some(attachment) = attachment else {
        0_u8.hash(hasher);
        return;
    };

    1_u8.hash(hasher);
    attachment.template_id.hash(hasher);
    attachment.slot_id.hash(hasher);
    attachment.instance.hash(hasher);
    attachment.led_start.hash(hasher);
    attachment.led_count.hash(hasher);
    attachment.led_mapping.hash(hasher);
}

pub(super) fn attachment_segment_for_zone(
    zone_id: &str,
    base_segment: Option<SegmentRange>,
    attachment: Option<&OutputComponent>,
    sampled_led_count: usize,
) -> Option<SegmentRange> {
    let Some(attachment) = attachment else {
        return base_segment;
    };
    let (Some(led_start), Some(led_count)) = (attachment.led_start, attachment.led_count) else {
        return base_segment;
    };

    let Ok(led_start) = usize::try_from(led_start) else {
        warn!(
            zone_id = %zone_id,
            attachment_led_start = led_start,
            "ignoring attachment segment override because led_start does not fit in usize"
        );
        return base_segment;
    };
    let Ok(led_count) = usize::try_from(led_count) else {
        warn!(
            zone_id = %zone_id,
            attachment_led_count = led_count,
            "ignoring attachment segment override because led_count does not fit in usize"
        );
        return base_segment;
    };
    let resolved_led_count = if sampled_led_count > 0 && sampled_led_count != led_count {
        debug!(
            zone_id = %zone_id,
            attachment_led_count = led_count,
            sampled_led_count,
            "attachment segment length differs from sampled zone length; using sampled LED count"
        );
        sampled_led_count
    } else {
        led_count
    };
    let attachment_end = led_start.saturating_add(resolved_led_count);

    if let Some(base_segment) = base_segment {
        if led_start >= base_segment.start && attachment_end <= base_segment.end() {
            return Some(SegmentRange::new(led_start, resolved_led_count));
        }

        if attachment_end <= base_segment.length {
            return Some(SegmentRange::new(
                base_segment.start.saturating_add(led_start),
                resolved_led_count,
            ));
        }

        if resolved_led_count == base_segment.length {
            return Some(base_segment);
        }

        warn!(
            zone_id = %zone_id,
            attachment_led_start = led_start,
            attachment_led_count = led_count,
            resolved_led_count,
            base_segment_start = base_segment.start,
            base_segment_length = base_segment.length,
            "ignoring attachment segment override because it exceeds the mapped segment"
        );
        return Some(base_segment);
    }

    Some(SegmentRange::new(led_start, resolved_led_count))
}
