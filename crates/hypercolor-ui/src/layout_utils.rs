//! Shared layout output helpers: display names, attachment slot matching,
//! and identify targets for the outputs in a zone layout.

use std::collections::{HashMap, HashSet};

use crate::api;
use crate::channel_names;
use crate::layout_geometry;
pub use hypercolor_types::attachment::{
    channel_name_matches_slot_alias, slot_id_matches_channel_name,
};
use hypercolor_types::spatial::{Output, SpatialLayout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputIdentifyTarget {
    Segment {
        device_id: String,
        segment: String,
    },
    Attachment {
        device_id: String,
        slot_id: String,
        binding_index: Option<usize>,
        instance: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveOutputDisplay {
    pub label: String,
    pub default_label: String,
    pub identify_target: Option<OutputIdentifyTarget>,
}

pub fn prefixed_channel_display_name(device_name: &str, channel_name: &str) -> String {
    if channel_name.eq_ignore_ascii_case(device_name) {
        device_name.to_owned()
    } else {
        format!("{device_name} · {channel_name}")
    }
}

pub fn effective_zone_display(
    zone: &Output,
    devices: &[api::DeviceSummary],
    attachment_profiles: &HashMap<String, api::DeviceComponentsResponse>,
) -> EffectiveOutputDisplay {
    let Some(device) = devices
        .iter()
        .find(|device| device.layout_device_id == zone.device_id)
    else {
        return EffectiveOutputDisplay {
            label: zone.name.clone(),
            default_label: zone.name.clone(),
            identify_target: None,
        };
    };

    if let Some(attachment) = zone.attachment.as_ref() {
        return effective_attachment_zone_display(zone, device, attachment, attachment_profiles);
    }

    effective_device_output_display(zone, device)
}

pub fn effective_device_name(
    layout_device_id: &str,
    devices: &[api::DeviceSummary],
) -> Option<String> {
    devices
        .iter()
        .find(|device| device.layout_device_id == layout_device_id)
        .map(|device| device.name.clone())
}

pub fn effective_slot_name(
    layout_device_id: &str,
    slot_alias: &str,
    devices: &[api::DeviceSummary],
) -> Option<String> {
    let device = devices
        .iter()
        .find(|device| device.layout_device_id == layout_device_id)?;
    let zone = resolve_device_segment_summary(device, Some(slot_alias))?;
    Some(channel_names::effective_channel_name(
        &device.id, &zone.id, &zone.name,
    ))
}

pub fn sync_device_display_name_in_layout(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    previous_name: &str,
    new_name: &str,
) -> bool {
    if previous_name == new_name {
        return false;
    }

    let previous_prefix = format!("{previous_name} · ");
    let mut changed = false;

    for zone in &mut layout.zones {
        if zone.device_id != layout_device_id || zone.attachment.is_some() {
            continue;
        }

        if zone.name == previous_name {
            zone.name.clone_from(&new_name.to_owned());
            changed = true;
            continue;
        }

        if let Some(suffix) = zone.name.strip_prefix(&previous_prefix) {
            zone.name = format!("{new_name} · {suffix}");
            changed = true;
        }
    }

    changed
}

pub fn apply_slot_display_names_to_seeded_attachment_layout(
    seeded: &mut layout_geometry::SeededAttachmentLayout,
    _device_name: &str,
    slot_display_names: &std::collections::HashMap<String, String>,
) {
    if slot_display_names.is_empty() {
        return;
    }

    for zone in &mut seeded.zones {
        let slot_id = zone
            .attachment
            .as_ref()
            .map(|attachment| attachment.slot_id.as_str())
            .or(zone.zone_name.as_deref());
        if let Some(display_name) = slot_id.and_then(|id| slot_display_names.get(id)) {
            zone.name.clone_from(display_name);
        }
    }
}

pub fn sync_channel_display_name_in_layout(
    layout: &mut SpatialLayout,
    layout_device_id: &str,
    device_name: &str,
    slot_id: &str,
    default_display_name: &str,
    previous_display_name: &str,
    new_display_name: &str,
) -> bool {
    let expected_old_names = [
        prefixed_channel_display_name(device_name, default_display_name),
        prefixed_channel_display_name(device_name, previous_display_name),
    ]
    .into_iter()
    .collect::<HashSet<_>>();
    let new_layout_name = prefixed_channel_display_name(device_name, new_display_name);

    let mut changed = false;

    for zone in &mut layout.zones {
        if zone.device_id != layout_device_id || zone.attachment.is_some() {
            continue;
        }
        if !channel_name_matches_slot_alias(zone.zone_name.as_deref(), Some(slot_id)) {
            continue;
        }
        if expected_old_names.contains(zone.name.as_str()) {
            zone.name.clone_from(&new_layout_name);
            changed = true;
        }
    }

    changed
}

fn effective_device_output_display(
    zone: &Output,
    device: &api::DeviceSummary,
) -> EffectiveOutputDisplay {
    let matched_zone = resolve_device_segment_summary(device, zone.zone_name.as_deref());
    let identify_target = matched_zone.map(|matched_zone| OutputIdentifyTarget::Segment {
        device_id: device.id.clone(),
        segment: matched_zone.id.clone(),
    });

    let Some(slot_alias) = zone.zone_name.as_deref() else {
        let label = if zone.name == device.name {
            device.name.clone()
        } else {
            zone.name.clone()
        };
        return EffectiveOutputDisplay {
            default_label: device.name.clone(),
            label,
            identify_target,
        };
    };

    let channel_label = matched_zone.map_or_else(
        || slot_alias.to_owned(),
        |matched_zone| {
            channel_names::effective_channel_name(&device.id, &matched_zone.id, &matched_zone.name)
        },
    );
    let raw_channel_label = matched_zone.map_or_else(
        || slot_alias.to_owned(),
        |matched_zone| matched_zone.name.clone(),
    );
    let default_label = prefixed_channel_display_name(&device.name, &channel_label);
    let raw_default_label = prefixed_channel_display_name(&device.name, &raw_channel_label);
    let old_suffix_matches = zone.name.rsplit_once(" · ").is_some_and(|(_, suffix)| {
        suffix.eq_ignore_ascii_case(&channel_label)
            || suffix.eq_ignore_ascii_case(&raw_channel_label)
    });
    let label =
        if zone.name == default_label || zone.name == raw_default_label || old_suffix_matches {
            default_label.clone()
        } else {
            zone.name.clone()
        };

    EffectiveOutputDisplay {
        label,
        default_label,
        identify_target,
    }
}

fn effective_attachment_zone_display(
    zone: &Output,
    device: &api::DeviceSummary,
    attachment: &hypercolor_types::spatial::OutputComponent,
    attachment_profiles: &HashMap<String, api::DeviceComponentsResponse>,
) -> EffectiveOutputDisplay {
    let binding = attachment_profiles
        .get(&zone.device_id)
        .and_then(|profile| resolve_attachment_binding(profile, attachment));
    let label = binding.as_ref().and_then(|(binding_index, _)| {
        attachment_profiles
            .get(&zone.device_id)
            .and_then(|profile| {
                attachment_binding_display_name(profile, *binding_index, attachment.instance)
            })
    });
    let identify_target = Some(OutputIdentifyTarget::Attachment {
        device_id: device.id.clone(),
        slot_id: attachment.slot_id.clone(),
        binding_index: binding.as_ref().map(|(binding_index, _)| *binding_index),
        instance: Some(attachment.instance),
    });

    EffectiveOutputDisplay {
        label: label.clone().unwrap_or_else(|| zone.name.clone()),
        default_label: label.unwrap_or_else(|| zone.name.clone()),
        identify_target,
    }
}

fn resolve_device_segment_summary<'a>(
    device: &'a api::DeviceSummary,
    slot_alias: Option<&str>,
) -> Option<&'a api::SegmentSummary> {
    let slot_alias = slot_alias?;
    device.segments.iter().find(|zone| {
        zone.id == slot_alias || channel_name_matches_slot_alias(Some(slot_alias), Some(&zone.name))
    })
}

fn resolve_attachment_binding<'a>(
    profile: &'a api::DeviceComponentsResponse,
    attachment: &hypercolor_types::spatial::OutputComponent,
) -> Option<(usize, &'a api::ComponentBindingSummary)> {
    let slot = profile
        .slots
        .iter()
        .find(|slot| slot.id == attachment.slot_id)?;

    let enabled_bindings = profile
        .bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.slot_id == attachment.slot_id && binding.enabled)
        .collect::<Vec<_>>();

    let exact_led_match = attachment.led_start.and_then(|target_led_start| {
        enabled_bindings.iter().copied().find(|(_, binding)| {
            if binding.template_id != attachment.template_id {
                return false;
            }
            let instances = binding.instances.max(1);
            let template_led_count = binding.effective_led_count / instances;
            if template_led_count == 0 {
                return false;
            }

            let expected_led_start = slot
                .led_start
                .saturating_add(binding.led_offset)
                .saturating_add(attachment.instance.saturating_mul(template_led_count));
            let led_count_matches = attachment
                .led_count
                .is_none_or(|led_count| led_count == template_led_count);

            expected_led_start == target_led_start && led_count_matches
        })
    });
    if exact_led_match.is_some() {
        return exact_led_match;
    }

    let template_matches = enabled_bindings
        .iter()
        .copied()
        .filter(|(_, binding)| binding.template_id == attachment.template_id)
        .collect::<Vec<_>>();
    if template_matches.len() == 1 {
        return template_matches.into_iter().next();
    }

    enabled_bindings.into_iter().next()
}

fn attachment_binding_display_name(
    profile: &api::DeviceComponentsResponse,
    binding_index: usize,
    instance: u32,
) -> Option<String> {
    let slot_id = profile.bindings.get(binding_index)?.slot_id.clone();
    let mut slot_labels = profile
        .bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.slot_id == slot_id && binding.enabled)
        .flat_map(|(current_index, binding)| {
            (0..binding.instances.max(1)).map(move |current_instance| {
                (
                    current_index,
                    current_instance,
                    attachment_binding_base_name(binding, current_instance),
                )
            })
        })
        .collect::<Vec<_>>();

    let mut totals = HashMap::<String, usize>::new();
    for (_, _, label) in &slot_labels {
        *totals.entry(label.clone()).or_insert(0) += 1;
    }

    let mut seen = HashMap::<String, usize>::new();
    for (current_index, current_instance, label) in &mut slot_labels {
        if totals.get(label).copied().unwrap_or_default() > 1 {
            let next = seen.entry(label.clone()).or_insert(0);
            *next += 1;
            *label = format!("{label} {next}");
        }

        if *current_index == binding_index && *current_instance == instance {
            return Some(label.clone());
        }
    }

    None
}

fn attachment_binding_base_name(binding: &api::ComponentBindingSummary, instance: u32) -> String {
    match binding.name.as_deref() {
        Some(name) if binding.instances > 1 => {
            format!("{name} - {} {}", binding.template_name, instance + 1)
        }
        Some(name) => name.to_owned(),
        None if binding.instances > 1 => format!("{} {}", binding.template_name, instance + 1),
        None => binding.template_name.clone(),
    }
}

pub fn replace_attachment_layout(
    layout: &mut SpatialLayout,
    device_id: &str,
    seeded: layout_geometry::SeededAttachmentLayout,
) {
    layout
        .zones
        .retain(|zone| !(zone.device_id == device_id && zone.attachment.is_some()));

    layout.zones.extend(seeded.zones);
}

pub fn attachment_binding_matches_slot_alias(
    binding_slot_id: &str,
    zone_id: Option<&str>,
    zone_name: Option<&str>,
    display_name: &str,
) -> bool {
    zone_id_matches_attachment_slot(binding_slot_id, zone_id)
        || zone_id_matches_attachment_slot(binding_slot_id, zone_name)
        || slot_id_matches_channel_name(binding_slot_id, display_name)
}

fn zone_id_matches_attachment_slot(binding_slot_id: &str, candidate: Option<&str>) -> bool {
    candidate.is_some_and(|candidate| {
        binding_slot_id.eq_ignore_ascii_case(candidate)
            || slot_id_matches_channel_name(binding_slot_id, candidate)
    })
}

fn representative_zone_id_with_filter(
    layout: &SpatialLayout,
    mut filter: impl FnMut(&hypercolor_types::spatial::Output) -> bool,
) -> Option<String> {
    layout
        .zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| filter(zone))
        .min_by(|(left_index, left_zone), (right_index, right_zone)| {
            left_zone
                .display_order
                .cmp(&right_zone.display_order)
                .then_with(|| left_index.cmp(right_index))
        })
        .map(|(_, zone)| zone.id.clone())
}

pub fn representative_zone_id_for_device(
    layout: &SpatialLayout,
    device_id: &str,
) -> Option<String> {
    let suppressed = suppressed_attachment_source_zone_ids(layout);
    representative_zone_id_with_filter(layout, |zone| {
        zone.device_id == device_id && !suppressed.contains(&zone.id)
    })
    .or_else(|| representative_zone_id_with_filter(layout, |zone| zone.device_id == device_id))
}

pub fn representative_zone_id_for_device_slot(
    layout: &SpatialLayout,
    device_id: &str,
    zone_name: Option<&str>,
) -> Option<String> {
    let suppressed = suppressed_attachment_source_zone_ids(layout);
    representative_zone_id_with_filter(layout, |zone| {
        zone.device_id == device_id
            && channel_name_matches_slot_alias(zone.zone_name.as_deref(), zone_name)
            && !suppressed.contains(&zone.id)
    })
    .or_else(|| {
        representative_zone_id_with_filter(layout, |zone| {
            zone.device_id == device_id
                && channel_name_matches_slot_alias(zone.zone_name.as_deref(), zone_name)
        })
    })
}

pub fn selected_zone_matches_device_slot(
    layout: &SpatialLayout,
    selected_zone_id: &str,
    device_id: &str,
    zone_name: Option<&str>,
) -> bool {
    layout
        .zones
        .iter()
        .find(|zone| zone.id == selected_zone_id)
        .is_some_and(|zone| {
            zone.device_id == device_id
                && channel_name_matches_slot_alias(zone.zone_name.as_deref(), zone_name)
        })
}

pub fn suppressed_attachment_source_zone_ids(layout: &SpatialLayout) -> HashSet<String> {
    let attached_slots = layout
        .zones
        .iter()
        .filter_map(|zone| {
            let attachment = zone.attachment.as_ref()?;
            Some((zone.device_id.as_str(), attachment.slot_id.as_str()))
        })
        .collect::<Vec<_>>();

    layout
        .zones
        .iter()
        .filter_map(|zone| {
            let zone_name = zone.zone_name.as_deref()?;
            (zone.attachment.is_none()
                && attached_slots.iter().any(|(device_id, slot_id)| {
                    zone.device_id == *device_id && slot_id_matches_channel_name(slot_id, zone_name)
                }))
            .then(|| zone.id.clone())
        })
        .collect()
}
